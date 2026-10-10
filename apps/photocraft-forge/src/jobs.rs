//! A small, serialized job manager for `validation_start` and the `job_*` tools.
//!
//! Jobs run `cargo`/`xtask` commands as child processes. Execution is **serialized** (one at a time)
//! so Cargo jobs sharing `target/`, the package cache and the registry cache do not contend. Logs are
//! captured live and read cursor-wise. A `cargo test` that exits 0 after matching **zero tests** is
//! reported as [`Status::NoTests`] — invalid evidence, not a pass.
//!
//! Completed jobs are **persisted** to an evidence directory (`<repo>/target/forge/jobs/<id>.json`
//! plus `<id>.log`), stamped with the source fingerprint at start, so evidence survives a restart
//! and another model can resume from it.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact::Artifact;
use crate::diagnostics::{self, Diagnostics, TestCounts};

const LOG_CAP: usize = 4 << 20;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Job lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Running,
    Succeeded,
    Failed,
    /// Exited 0 but matched zero tests — not a pass.
    NoTests,
    Cancelled,
    /// Exceeded its idle or total time limit — distinct from a plain failure (research §8).
    TimedOut,
    /// Was still queued/running when Forge last exited — interrupted, not failed (research §8).
    Interrupted,
}

impl Status {
    fn done(self) -> bool {
        matches!(self, Status::Succeeded | Status::Failed | Status::NoTests | Status::Cancelled | Status::TimedOut | Status::Interrupted)
    }

    /// Whether this outcome blocks the change (it is neither a pass nor a non-verdict).
    fn blocking(self) -> bool {
        matches!(self, Status::Failed | Status::NoTests | Status::TimedOut)
    }
}

/// Which time limit a job hit (research §8), and whether the log suggests a hang.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimeoutHit {
    /// `idle` (no output for the idle window) or `total` (the absolute cap).
    pub kind: String,
    /// True when the **idle** window fired — no recent output, so a hang is suspected. False when
    /// the job was still printing and merely outran the absolute cap (build progress).
    pub hang_suspected: bool,
}

/// Per-job time limits (research §8). The idle window is generous so a slow-but-printing build is
/// not killed; a hung test prints nothing and is caught by the idle window; the absolute cap
/// catches a runaway.
#[derive(Clone, Copy, Debug)]
pub struct Timeout {
    pub idle_ms: u64,
    pub total_ms: u64,
}

impl Default for Timeout {
    fn default() -> Self {
        Self { idle_ms: 15 * 60_000, total_ms: 60 * 60_000 }
    }
}

impl Timeout {
    /// Limits from optional seconds; each defaults to [`Timeout::default`].
    pub fn from_secs(idle_secs: Option<u64>, total_secs: Option<u64>) -> Self {
        let d = Self::default();
        Self { idle_ms: idle_secs.map_or(d.idle_ms, |s| s.saturating_mul(1000)), total_ms: total_secs.map_or(d.total_ms, |s| s.saturating_mul(1000)) }
    }
}

/// The highest evidence tier a job reached (research §4). Ordered weakest → strongest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Source/history/capability inspection, or a compile-only check.
    #[default]
    Static,
    /// Focused tests or benchmarks.
    Unit,
    /// A headless engine session.
    Headless,
    /// Offscreen rendering / snapshot.
    Offscreen,
    /// The native desktop runtime / application MCP.
    Native,
    /// A platform or hardware environment (macOS, Windows, FreeBSD, a device).
    Platform,
    /// An external oracle (Photoshop, another tool).
    Oracle,
}

/// Which evidence tier a gate exercises. Unknown gates default to `static`.
pub fn tier_for_gate(id: &str) -> Tier {
    match id {
        "ci" | "panic_hunt" | "test-corpus" | "perf" => Tier::Unit,
        _ => Tier::Static,
    }
}

/// Parameters for `validation_start`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationParams {
    /// Paths of the change to validate. When empty, the diff against `base` is used.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Git ref to diff against (default: merge-base with origin/main, else HEAD).
    #[serde(default)]
    pub base: Option<String>,
    /// Also run the recommended (advisory) gates.
    #[serde(default)]
    pub include_recommended: bool,
    /// Kill a job that produces no output for this many seconds (default 900).
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// Absolute per-job cap in seconds (default 3600).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// A job to run.
pub struct JobSpec {
    pub name: String,
    pub kind: String,
    pub reason: String,
    pub tier: Tier,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Timeout,
}

/// A persisted, immutable manifest of a completed job (fields per research §8).
#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    schema_version: u32,
    id: u64,
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    reason: String,
    command: String,
    status: Status,
    #[serde(default)]
    exit: Option<i32>,
    zero_tests: bool,
    #[serde(default)]
    started_ms: Option<u128>,
    #[serde(default)]
    duration_ms: Option<u128>,
    #[serde(default)]
    finished_ms: Option<u128>,
    #[serde(default)]
    head: Option<String>,
    dirty: bool,
    #[serde(default)]
    tier: Tier,
    /// Whether this job blocks the change (it failed or matched zero tests).
    #[serde(default)]
    blocking: bool,
    /// Why the job could not produce evidence (e.g. a missing tool), when that happened.
    #[serde(default)]
    unavailable: Option<String>,
    /// Which time limit the job hit, if any.
    #[serde(default)]
    timeout: Option<TimeoutHit>,
    /// Outputs of the job (its log, and any produced files) with media types + content hashes.
    #[serde(default)]
    artifacts: Vec<Artifact>,
    #[serde(default)]
    diagnostics: Diagnostics,
}

struct Job {
    id: u64,
    name: String,
    kind: String,
    reason: String,
    program: String,
    args: Vec<String>,
    status: Status,
    log: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
    pid: Option<u32>,
    started_ms: Option<u128>,
    duration_ms: Option<u128>,
    finished_ms: Option<u128>,
    exit: Option<i32>,
    zero_tests: bool,
    head: Option<String>,
    dirty: bool,
    log_path: Option<PathBuf>,
    tier: Tier,
    unavailable: Option<String>,
    timeout: Option<TimeoutHit>,
    artifacts: Vec<Artifact>,
    diagnostics: Diagnostics,
}

/// A serializable view of a job.
#[derive(Debug, Serialize)]
pub struct JobView {
    pub id: u64,
    pub name: String,
    pub status: Status,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    pub zero_tests: bool,
    pub kind: String,
    pub tests: TestCounts,
    pub diagnostics: usize,
    pub tier: Tier,
    /// Whether this job blocks the change (failed or matched zero tests).
    pub blocking: bool,
    /// Why the job could not produce evidence, when that happened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    /// Which time limit the job hit, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<TimeoutHit>,
    /// Outputs with media types + content hashes (research §8).
    pub artifacts: Vec<Artifact>,
    /// Source fingerprint (HEAD) the job ran against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    pub dirty: bool,
    /// Where the durable manifest/log live, if persisted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

struct Inner {
    jobs: BTreeMap<u64, Job>,
}

/// Serialized, evidence-persisting job manager.
pub struct JobManager {
    inner: Mutex<Inner>,
    gate: Mutex<()>,
    next: AtomicU64,
    evidence: Option<PathBuf>,
}

impl JobManager {
    /// Create a manager. When `evidence` is set, completed jobs are persisted there and any existing
    /// manifests are loaded so evidence survives a restart.
    pub fn new(evidence: Option<PathBuf>) -> Self {
        if let Some(dir) = &evidence {
            let _ = std::fs::create_dir_all(dir);
        }
        let manager = Self { inner: Mutex::new(Inner { jobs: BTreeMap::new() }), gate: Mutex::new(()), next: AtomicU64::new(1), evidence };
        manager.load();
        manager
    }

    fn load(&self) {
        let Some(dir) = &self.evidence else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut max_id = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(manifest) = serde_json::from_str::<Manifest>(&text) else {
                continue;
            };
            max_id = max_id.max(manifest.id);
            let log_path = dir.join(format!("{}.log", manifest.id));
            lock(&self.inner).jobs.insert(
                manifest.id,
                Job {
                    id: manifest.id,
                    name: manifest.name,
                    kind: manifest.kind,
                    reason: manifest.reason,
                    tier: manifest.tier,
                    program: manifest.command,
                    args: Vec::new(),
                    status: match manifest.status {
                        Status::Queued | Status::Running => Status::Interrupted,
                        other => other,
                    },
                    log: Arc::new(Mutex::new(String::new())),
                    cancel: Arc::new(AtomicBool::new(false)),
                    pid: None,
                    started_ms: manifest.started_ms,
                    duration_ms: manifest.duration_ms,
                    finished_ms: manifest.finished_ms,
                    exit: manifest.exit,
                    zero_tests: manifest.zero_tests,
                    head: manifest.head,
                    dirty: manifest.dirty,
                    log_path: Some(log_path),
                    unavailable: manifest.unavailable,
                    timeout: manifest.timeout,
                    artifacts: manifest.artifacts,
                    diagnostics: manifest.diagnostics,
                },
            );
        }
        self.next.store(max_id + 1, Ordering::Relaxed);
    }

    /// Queue a job and return its id. It runs as soon as the serialization gate frees up.
    pub fn start(self: &Arc<Self>, spec: JobSpec) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let fingerprint = crate::context::fingerprint(&spec.cwd);
        let log = Arc::new(Mutex::new(String::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.inner).jobs.insert(
            id,
            Job {
                id,
                name: spec.name.clone(),
                kind: spec.kind.clone(),
                reason: spec.reason.clone(),
                tier: spec.tier,
                program: spec.program.clone(),
                args: spec.args.clone(),
                status: Status::Queued,
                log: Arc::clone(&log),
                cancel: Arc::clone(&cancel),
                pid: None,
                started_ms: None,
                duration_ms: None,
                finished_ms: None,
                exit: None,
                zero_tests: false,
                head: fingerprint.head,
                dirty: fingerprint.dirty,
                log_path: None,
                unavailable: None,
                timeout: None,
                artifacts: Vec::new(),
                diagnostics: Diagnostics::default(),
            },
        );
        // Persist immediately so a crash mid-run leaves a non-terminal manifest, which `load` marks
        // `interrupted` rather than silently `failed` (research §8).
        self.persist(id);
        let me = Arc::clone(self);
        std::thread::spawn(move || me.run(id, spec, log, cancel));
        id
    }

    fn run(self: Arc<Self>, id: u64, spec: JobSpec, log: Arc<Mutex<String>>, cancel: Arc<AtomicBool>) {
        let JobSpec { program, args, cwd, timeout, .. } = spec;
        let _serial = lock(&self.gate);
        if cancel.load(Ordering::Relaxed) {
            self.finish(id, Status::Cancelled, None, false);
            return;
        }
        self.mark_running(id);
        let last_output = Arc::new(AtomicU64::new(now_ms_u64()));
        let done = Arc::new(AtomicBool::new(false));
        let mut command = Command::new(&program);
        command.args(&args).current_dir(&cwd).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                let reason = format!("failed to spawn `{program}`: {e}");
                append(&log, &last_output, &format!("forge: {reason}\n"));
                self.mark_unavailable(id, reason);
                self.finish(id, Status::Failed, None, false);
                return;
            }
        };
        let pid = child.id();
        self.set_pid(id, pid);
        // A watchdog kills the process tree when output stalls (a hang) or the absolute cap is hit
        // (a runaway), so a universal timeout never misfires on a slow build (research §8).
        let timed_out = Arc::new(AtomicBool::new(false));
        spawn_watchdog(id, pid, now_ms(), Arc::clone(&last_output), Arc::clone(&done), Arc::clone(&timed_out), timeout, Arc::clone(&log), Arc::clone(&self));
        let out = child.stdout.take().map(|s| spawn_reader(s, Arc::clone(&log), Arc::clone(&last_output)));
        let err = child.stderr.take().map(|s| spawn_reader(s, Arc::clone(&log), Arc::clone(&last_output)));
        // Poll so a timeout is not held hostage by a `wait` the watchdog cannot interrupt.
        let mut exit = None;
        loop {
            match child.try_wait() {
                Ok(Some(code)) => {
                    exit = Some(code);
                    break;
                }
                Ok(None) => {
                    if cancel.load(Ordering::Relaxed) || timed_out.load(Ordering::Relaxed) {
                        kill_group(pid);
                        let _ = child.kill();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => break,
            }
        }
        done.store(true, Ordering::Relaxed);
        // The direct child is gone. Join the readers so the log tail (test counts, diagnostics) is
        // captured; on a timeout/cancel a surviving grandchild may hold the pipe, so don't block on
        // it — the direct child is already terminated.
        if !(timed_out.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed)) {
            if let Some(handle) = out {
                let _ = handle.join();
            }
            if let Some(handle) = err {
                let _ = handle.join();
            }
        }
        if cancel.load(Ordering::Relaxed) {
            self.finish(id, Status::Cancelled, None, false);
            return;
        }
        if timed_out.load(Ordering::Relaxed) {
            self.finish(id, Status::TimedOut, None, false);
            return;
        }
        let code = exit.as_ref().and_then(std::process::ExitStatus::code);
        let zero = detect_zero_tests(lock(&log).as_str());
        let status = match exit {
            Some(s) if s.success() && zero => Status::NoTests,
            Some(s) if s.success() => Status::Succeeded,
            _ => Status::Failed,
        };
        self.finish(id, status, code, zero);
    }

    fn mark_running(&self, id: u64) {
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.status = Status::Running;
            job.started_ms = Some(now_ms());
        }
    }

    fn set_pid(&self, id: u64, pid: u32) {
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.pid = Some(pid);
        }
    }

    /// Record that a job could not produce evidence (e.g. its tool is missing).
    fn mark_unavailable(&self, id: u64, reason: String) {
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.unavailable = Some(reason);
        }
    }

    /// Record which time limit a job hit (the watchdog calls this before terminating it).
    fn mark_timeout(&self, id: u64, hit: TimeoutHit) {
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.timeout = Some(hit);
        }
    }

    fn finish(&self, id: u64, status: Status, exit: Option<i32>, zero_tests: bool) {
        let parsed = {
            let inner = lock(&self.inner);
            inner.jobs.get(&id).map(|job| diagnostics::parse(lock(&job.log).as_str())).unwrap_or_default()
        };
        {
            let mut inner = lock(&self.inner);
            if let Some(job) = inner.jobs.get_mut(&id) {
                job.status = status;
                job.exit = exit;
                job.zero_tests = zero_tests;
                job.diagnostics = parsed;
                if let Some(started) = job.started_ms {
                    job.duration_ms = Some(now_ms().saturating_sub(started));
                }
                job.finished_ms = Some(now_ms());
            }
        }
        self.persist(id);
    }

    /// Write the durable manifest + log for a completed job.
    fn persist(&self, id: u64) {
        let Some(dir) = &self.evidence else {
            return;
        };
        let (mut manifest, log_text, log_path) = {
            let inner = lock(&self.inner);
            let Some(job) = inner.jobs.get(&id) else {
                return;
            };
            let manifest = Manifest {
                schema_version: 1,
                id: job.id,
                name: job.name.clone(),
                kind: job.kind.clone(),
                reason: job.reason.clone(),
                tier: job.tier,
                command: command_string(&job.program, &job.args),
                status: job.status,
                exit: job.exit,
                zero_tests: job.zero_tests,
                started_ms: job.started_ms,
                duration_ms: job.duration_ms,
                finished_ms: job.finished_ms,
                head: job.head.clone(),
                dirty: job.dirty,
                blocking: job.status.blocking(),
                unavailable: job.unavailable.clone(),
                timeout: job.timeout.clone(),
                artifacts: Vec::new(),
                diagnostics: job.diagnostics.clone(),
            };
            (manifest, lock(&job.log).clone(), dir.join(format!("{id}.log")))
        };
        manifest.artifacts = vec![Artifact::from_bytes(format!("{id}.log"), "text/plain", log_text.as_bytes())];
        if let Ok(text) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(dir.join(format!("{id}.json")), text);
        }
        let _ = std::fs::write(&log_path, log_text);
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.log_path = Some(log_path);
            job.artifacts = manifest.artifacts.clone();
        }
    }

    fn view(job: &Job) -> JobView {
        JobView {
            id: job.id,
            name: job.name.clone(),
            status: job.status,
            command: command_string(&job.program, &job.args),
            pid: job.pid,
            started_ms: job.started_ms,
            duration_ms: job.duration_ms,
            finished_ms: job.finished_ms,
            exit: job.exit,
            zero_tests: job.zero_tests,
            kind: job.kind.clone(),
            tests: job.diagnostics.tests.clone(),
            diagnostics: job.diagnostics.entries.len(),
            tier: job.tier,
            blocking: job.status.blocking(),
            unavailable: job.unavailable.clone(),
            timeout: job.timeout.clone(),
            artifacts: job.artifacts.clone(),
            head: job.head.clone(),
            dirty: job.dirty,
            evidence: job.log_path.as_ref().and_then(|p| p.parent()).map(|p| p.display().to_string()),
        }
    }

    /// One job, or all jobs (running first, then most recent first).
    pub fn views(&self, id: Option<u64>) -> Vec<JobView> {
        let inner = lock(&self.inner);
        if let Some(id) = id {
            return inner.jobs.get(&id).map(JobManager::view).into_iter().collect();
        }
        let mut all: Vec<&Job> = inner.jobs.values().collect();
        all.sort_by_key(|j| (j.status.done(), std::cmp::Reverse(j.id)));
        all.into_iter().map(JobManager::view).collect()
    }

    /// A run-level summary (research §8): what **blocks** the change, what never ran, and where
    /// evidence was **unavailable**.
    pub fn summary(&self) -> serde_json::Value {
        let views = self.views(None);
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut blocking = Vec::new();
        let mut unrun = Vec::new();
        let mut unavailable = Vec::new();
        for v in &views {
            *counts.entry(format!("{:?}", v.status).to_lowercase()).or_default() += 1;
            let entry = serde_json::json!({"job": v.id, "name": v.name, "status": v.status});
            if v.status.blocking() {
                blocking.push(entry);
            } else if v.status == Status::Queued {
                unrun.push(entry);
            }
            if let Some(reason) = &v.unavailable {
                unavailable.push(serde_json::json!({"job": v.id, "name": v.name, "reason": reason}));
            }
        }
        serde_json::json!({
            "total": views.len(),
            "counts": counts,
            "blocking": blocking,
            "unrun": unrun,
            "unavailable": unavailable,
        })
    }

    /// `(status, log slice from cursor, next cursor)`, or `None` for an unknown id.
    pub fn logs(&self, id: u64, cursor: usize) -> Option<(Status, String, usize)> {
        let inner = lock(&self.inner);
        let job = inner.jobs.get(&id)?;
        let text: String = {
            let in_memory = lock(&job.log);
            if in_memory.is_empty() { job.log_path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default() } else { in_memory.clone() }
        };
        let cursor = cursor.min(text.len());
        let start = (cursor..=text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
        Some((job.status, text[start..].to_string(), text.len()))
    }

    /// Request cancellation; returns whether the id existed.
    pub fn cancel(&self, id: u64) -> bool {
        let (exists, pid, cancel) = {
            let inner = lock(&self.inner);
            match inner.jobs.get(&id) {
                Some(job) => (true, job.pid, Arc::clone(&job.cancel)),
                None => (false, None, Arc::new(AtomicBool::new(false))),
            }
        };
        if !exists {
            return false;
        }
        cancel.store(true, Ordering::Relaxed);
        if let Some(pid) = pid {
            kill_group(pid);
        }
        true
    }
}

fn command_string(program: &str, args: &[String]) -> String {
    format!("{program} {}", args.join(" ")).trim_end().to_string()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// `now_ms` narrowed to `u64` for [`AtomicU64`] (milliseconds since the epoch fit for ~584M years).
fn now_ms_u64() -> u64 {
    u64::try_from(now_ms()).unwrap_or(u64::MAX)
}

fn append(log: &Arc<Mutex<String>>, last_output: &Arc<AtomicU64>, text: &str) {
    {
        let mut g = lock(log);
        if g.len() < LOG_CAP {
            g.push_str(text);
        }
    }
    // Any output counts as progress, even past the log cap: the watchdog must not call it a hang.
    last_output.store(now_ms_u64(), Ordering::Relaxed);
}

fn spawn_reader(mut reader: impl Read + Send + 'static, log: Arc<Mutex<String>>, last_output: Arc<AtomicU64>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => append(&log, &last_output, &String::from_utf8_lossy(&buf[..n])),
            }
        }
    })
}

/// Watch a running job: terminate the process tree when output stalls (a hang) or the absolute cap
/// is hit (a runaway). On the stall path it records `hang_suspected`; on the cap path it does not,
/// because output was still arriving (build progress).
#[allow(clippy::too_many_arguments, reason = "one watch per job; a small, explicit set of handles")]
fn spawn_watchdog(
    id: u64,
    pid: u32,
    started: u128,
    last_output: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    timed_out: Arc<AtomicBool>,
    timeout: Timeout,
    log: Arc<Mutex<String>>,
    manager: Arc<JobManager>,
) {
    std::thread::spawn(move || {
        loop {
            if done.load(Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            if done.load(Ordering::Relaxed) {
                return;
            }
            let now = now_ms();
            let idle = now.saturating_sub(u128::from(last_output.load(Ordering::Relaxed)));
            let hit = if idle >= u128::from(timeout.idle_ms) {
                Some(TimeoutHit { kind: "idle".to_string(), hang_suspected: true })
            } else if now.saturating_sub(started) >= u128::from(timeout.total_ms) {
                Some(TimeoutHit { kind: "total".to_string(), hang_suspected: false })
            } else {
                None
            };
            if let Some(hit) = hit {
                append(&log, &last_output, &format!("forge: exceeded the {} time limit; terminating the job tree\n", hit.kind));
                manager.mark_timeout(id, hit);
                timed_out.store(true, Ordering::Relaxed);
                kill_group(pid);
                return;
            }
        }
    });
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // The child leads its own process group (process_group(0)), so `-- -pid` signals the tree.
    // `--` stops option parsing so the negative pgid is read as a target, not a flag.
    let pgid = format!("-{pid}");
    let _ = Command::new("kill").args(["-s", "TERM", "--", &pgid]).status();
    let _ = Command::new("kill").args(["-s", "KILL", "--", &pgid]).status();
}

#[cfg(windows)]
fn kill_group(pid: u32) {
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).status();
}

/// Did a test run match zero tests? (`cargo test` prints `running 0 tests` / `0 passed`.)
pub fn detect_zero_tests(log: &str) -> bool {
    let mut zero = false;
    let mut positive = false;
    for line in log.lines() {
        match running_count(line) {
            Some(0) => zero = true,
            Some(_) => positive = true,
            None => {}
        }
        match passed_count(line) {
            Some(0) => zero = true,
            Some(_) => positive = true,
            None => {}
        }
    }
    zero && !positive
}

fn running_count(line: &str) -> Option<u64> {
    let t = line.trim();
    let rest = t.strip_prefix("running ")?;
    let n = rest.strip_suffix(" tests")?;
    n.trim().parse().ok()
}

fn passed_count(line: &str) -> Option<u64> {
    let i = line.find(" passed")?;
    line[..i].rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_done(mgr: &Arc<JobManager>, id: u64) -> Status {
        let mut status = Status::Queued;
        for _ in 0..300 {
            status = mgr.views(Some(id))[0].status;
            if status.done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        status
    }

    #[test]
    fn zero_matched_tests_is_not_a_pass() {
        assert!(detect_zero_tests("running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored\n"));
        assert!(detect_zero_tests("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"));
    }

    #[test]
    fn real_tests_are_a_pass() {
        assert!(!detect_zero_tests("running 3 tests\n\ntest result: ok. 3 passed; 0 failed; 0 ignored\n"));
        assert!(!detect_zero_tests("running 0 tests\n\ntest result: ok. 0 passed; 0 failed\n\nrunning 5 tests\n\ntest result: ok. 5 passed; 0 failed\n"));
    }

    #[test]
    fn manager_runs_a_job_and_captures_logs() {
        let mgr = Arc::new(JobManager::new(None));
        let spec = JobSpec {
            name: "echo".into(),
            kind: "test".into(),
            reason: "unit".into(),
            tier: Tier::Unit,
            program: "sh".into(),
            args: vec!["-c".into(), "echo forge-hello".into()],
            cwd: std::env::temp_dir(),
            timeout: Timeout::default(),
        };
        let id = mgr.start(spec);
        assert_eq!(wait_done(&mgr, id), Status::Succeeded);
        let (_, log, _) = mgr.logs(id, 0).expect("logs");
        assert!(log.contains("forge-hello"), "{log}");
    }

    #[test]
    fn evidence_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("forge-evidence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mgr = Arc::new(JobManager::new(Some(dir.clone())));
        let spec = JobSpec {
            name: "echo".into(),
            kind: "test".into(),
            reason: "evidence".into(),
            tier: Tier::Unit,
            program: "sh".into(),
            args: vec!["-c".into(), "echo persisted-hello".into()],
            cwd: std::env::temp_dir(),
            timeout: Timeout::default(),
        };
        let id = mgr.start(spec);
        assert_eq!(wait_done(&mgr, id), Status::Succeeded);

        // A fresh manager over the same directory must see the persisted job and its log.
        let reloaded = JobManager::new(Some(dir.clone()));
        let views = reloaded.views(None);
        assert!(views.iter().any(|v| v.id == id && v.status == Status::Succeeded), "{views:?}");
        let (_, log, _) = reloaded.logs(id, 0).expect("reloaded logs");
        assert!(log.contains("persisted-hello"), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gates_map_to_evidence_tiers() {
        assert_eq!(tier_for_gate("ci"), Tier::Unit);
        assert_eq!(tier_for_gate("test-corpus"), Tier::Unit);
        assert_eq!(tier_for_gate("layers"), Tier::Static);
        assert_eq!(tier_for_gate("wasm"), Tier::Static);
    }

    #[test]
    fn a_persisted_job_records_its_log_artifact_with_a_hash() {
        let dir = std::env::temp_dir().join(format!("forge-artifact-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mgr = Arc::new(JobManager::new(Some(dir.clone())));
        let spec = JobSpec {
            name: "echo".into(),
            kind: "test".into(),
            reason: "artifact".into(),
            tier: Tier::Unit,
            program: "sh".into(),
            args: vec!["-c".into(), "echo artifact-hello".into()],
            cwd: std::env::temp_dir(),
            timeout: Timeout::default(),
        };
        let id = mgr.start(spec);
        assert_eq!(wait_done(&mgr, id), Status::Succeeded);
        let view = mgr.views(Some(id));
        let art = view[0].artifacts.iter().find(|a| a.name.ends_with(".log")).expect("log artifact");
        assert_eq!(art.media_type, "text/plain");
        assert!(art.hash.starts_with("blake3:"), "{art:?}");
        assert!(art.bytes > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_reports_blocking_and_unavailable_evidence() {
        let dir = std::env::temp_dir().join(format!("forge-summary-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mgr = Arc::new(JobManager::new(Some(dir.clone())));
        let fail = mgr.start(JobSpec {
            name: "ci".into(),
            kind: "validation".into(),
            reason: "test".into(),
            tier: Tier::Unit,
            program: "sh".into(),
            args: vec!["-c".into(), "exit 3".into()],
            cwd: std::env::temp_dir(),
            timeout: Timeout::default(),
        });
        assert_eq!(wait_done(&mgr, fail), Status::Failed);
        let missing = mgr.start(JobSpec {
            name: "tool".into(),
            kind: "validation".into(),
            reason: "test".into(),
            tier: Tier::Unit,
            program: "forge-no-such-tool-xyz".into(),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            timeout: Timeout::default(),
        });
        assert_eq!(wait_done(&mgr, missing), Status::Failed);
        let s = mgr.summary();
        assert_eq!(s["total"], 2, "{s}");
        assert_eq!(s["counts"]["failed"], 2, "{s}");
        assert!(s["blocking"].as_array().is_some_and(|a| a.len() == 2), "{s}");
        assert!(s["unavailable"].as_array().is_some_and(|a| a.iter().any(|u| u["job"] == missing)), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stalled_job_times_out_and_is_flagged_as_a_hang() {
        let mgr = Arc::new(JobManager::new(None));
        let spec = JobSpec {
            name: "hang".into(),
            kind: "test".into(),
            reason: "timeout".into(),
            tier: Tier::Unit,
            program: "sh".into(),
            args: vec!["-c".into(), "echo starting; sleep 30".into()],
            cwd: std::env::temp_dir(),
            timeout: Timeout { idle_ms: 250, total_ms: 60_000 },
        };
        let id = mgr.start(spec);
        assert_eq!(wait_done(&mgr, id), Status::TimedOut);
        let view = &mgr.views(Some(id))[0];
        let hit = view.timeout.as_ref().expect("timeout hit");
        assert_eq!(hit.kind, "idle");
        assert!(hit.hang_suspected, "{hit:?}");
        assert!(view.blocking);
    }

    #[test]
    fn a_progressing_job_hits_the_total_cap_not_a_hang() {
        let mgr = Arc::new(JobManager::new(None));
        let spec = JobSpec {
            name: "long".into(),
            kind: "test".into(),
            reason: "timeout".into(),
            tier: Tier::Unit,
            program: "sh".into(),
            args: vec!["-c".into(), "i=0; while [ $i -lt 60 ]; do echo tick; i=$((i+1)); sleep 0.1; done".into()],
            cwd: std::env::temp_dir(),
            timeout: Timeout { idle_ms: 60_000, total_ms: 350 },
        };
        let id = mgr.start(spec);
        assert_eq!(wait_done(&mgr, id), Status::TimedOut);
        let view = &mgr.views(Some(id))[0];
        let hit = view.timeout.as_ref().expect("timeout hit");
        assert_eq!(hit.kind, "total");
        assert!(!hit.hang_suspected, "{hit:?}");
    }

    #[test]
    fn a_non_terminal_manifest_loads_as_interrupted() {
        let dir = std::env::temp_dir().join(format!("forge-interrupted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        let manifest = serde_json::json!({
            "id": 7, "name": "ci", "command": "cargo xtask ci",
            "status": "running", "zero_tests": false, "dirty": false
        });
        std::fs::write(dir.join("7.json"), manifest.to_string()).expect("write");
        let mgr = JobManager::new(Some(dir.clone()));
        let views = mgr.views(Some(7));
        assert_eq!(views.first().map(|v| v.status), Some(Status::Interrupted), "{views:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
