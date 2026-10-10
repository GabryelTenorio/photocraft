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
}

impl Status {
    fn done(self) -> bool {
        matches!(self, Status::Succeeded | Status::Failed | Status::NoTests | Status::Cancelled)
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
    head: Option<String>,
    dirty: bool,
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
    exit: Option<i32>,
    zero_tests: bool,
    head: Option<String>,
    dirty: bool,
    log_path: Option<PathBuf>,
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
    pub exit: Option<i32>,
    pub zero_tests: bool,
    pub kind: String,
    pub tests: TestCounts,
    pub diagnostics: usize,
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
                    program: manifest.command,
                    args: Vec::new(),
                    status: manifest.status,
                    log: Arc::new(Mutex::new(String::new())),
                    cancel: Arc::new(AtomicBool::new(false)),
                    pid: None,
                    started_ms: manifest.started_ms,
                    duration_ms: manifest.duration_ms,
                    exit: manifest.exit,
                    zero_tests: manifest.zero_tests,
                    head: manifest.head,
                    dirty: manifest.dirty,
                    log_path: Some(log_path),
                    diagnostics: manifest.diagnostics,
                },
            );
        }
        self.next.store(max_id + 1, Ordering::Relaxed);
    }

    /// Queue a job and return its id. It runs as soon as the serialization gate frees up.
    pub fn start(self: &Arc<Self>, name: String, kind: String, reason: String, program: String, args: Vec<String>, cwd: PathBuf) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let fingerprint = crate::context::fingerprint(&cwd);
        let log = Arc::new(Mutex::new(String::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.inner).jobs.insert(
            id,
            Job {
                id,
                name,
                kind,
                reason,
                program: program.clone(),
                args: args.clone(),
                status: Status::Queued,
                log: Arc::clone(&log),
                cancel: Arc::clone(&cancel),
                pid: None,
                started_ms: None,
                duration_ms: None,
                exit: None,
                zero_tests: false,
                head: fingerprint.head,
                dirty: fingerprint.dirty,
                log_path: None,
                diagnostics: Diagnostics::default(),
            },
        );
        let me = Arc::clone(self);
        std::thread::spawn(move || me.run(id, program, args, cwd, log, cancel));
        id
    }

    fn run(self: Arc<Self>, id: u64, program: String, args: Vec<String>, cwd: PathBuf, log: Arc<Mutex<String>>, cancel: Arc<AtomicBool>) {
        let _serial = lock(&self.gate);
        if cancel.load(Ordering::Relaxed) {
            self.finish(id, Status::Cancelled, None, false);
            return;
        }
        self.mark_running(id);
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
                append(&log, &format!("forge: failed to spawn `{program}`: {e}\n"));
                self.finish(id, Status::Failed, None, false);
                return;
            }
        };
        self.set_pid(id, child.id());
        let out = child.stdout.take().map(|s| spawn_reader(s, Arc::clone(&log)));
        let err = child.stderr.take().map(|s| spawn_reader(s, Arc::clone(&log)));
        let status = child.wait();
        if let Some(handle) = out {
            let _ = handle.join();
        }
        if let Some(handle) = err {
            let _ = handle.join();
        }
        if cancel.load(Ordering::Relaxed) {
            self.finish(id, Status::Cancelled, None, false);
            return;
        }
        let code = status.as_ref().ok().and_then(std::process::ExitStatus::code);
        let zero = detect_zero_tests(lock(&log).as_str());
        let status = match status {
            Ok(s) if s.success() && zero => Status::NoTests,
            Ok(s) if s.success() => Status::Succeeded,
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
            }
        }
        self.persist(id);
    }

    /// Write the durable manifest + log for a completed job.
    fn persist(&self, id: u64) {
        let Some(dir) = &self.evidence else {
            return;
        };
        let (manifest, log_text, log_path) = {
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
                command: command_string(&job.program, &job.args),
                status: job.status,
                exit: job.exit,
                zero_tests: job.zero_tests,
                started_ms: job.started_ms,
                duration_ms: job.duration_ms,
                head: job.head.clone(),
                dirty: job.dirty,
                diagnostics: job.diagnostics.clone(),
            };
            (manifest, lock(&job.log).clone(), dir.join(format!("{id}.log")))
        };
        if let Ok(text) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(dir.join(format!("{id}.json")), text);
        }
        let _ = std::fs::write(&log_path, log_text);
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.log_path = Some(log_path);
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
            exit: job.exit,
            zero_tests: job.zero_tests,
            kind: job.kind.clone(),
            tests: job.diagnostics.tests.clone(),
            diagnostics: job.diagnostics.entries.len(),
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

fn append(log: &Arc<Mutex<String>>, text: &str) {
    let mut g = lock(log);
    if g.len() < LOG_CAP {
        g.push_str(text);
    }
}

fn spawn_reader(mut reader: impl Read + Send + 'static, log: Arc<Mutex<String>>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => append(&log, &String::from_utf8_lossy(&buf[..n])),
            }
        }
    })
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // The child leads its own process group (process_group(0)), so `-pid` signals the tree.
    let _ = Command::new("kill").args(["-TERM", &format!("-{pid}")]).status();
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
        let id = mgr.start("echo".into(), "test".into(), "unit".into(), "sh".into(), vec!["-c".into(), "echo forge-hello".into()], std::env::temp_dir());
        assert_eq!(wait_done(&mgr, id), Status::Succeeded);
        let (_, log, _) = mgr.logs(id, 0).expect("logs");
        assert!(log.contains("forge-hello"), "{log}");
    }

    #[test]
    fn evidence_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!("forge-evidence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mgr = Arc::new(JobManager::new(Some(dir.clone())));
        let id =
            mgr.start("echo".into(), "test".into(), "evidence".into(), "sh".into(), vec!["-c".into(), "echo persisted-hello".into()], std::env::temp_dir());
        assert_eq!(wait_done(&mgr, id), Status::Succeeded);

        // A fresh manager over the same directory must see the persisted job and its log.
        let reloaded = JobManager::new(Some(dir.clone()));
        let views = reloaded.views(None);
        assert!(views.iter().any(|v| v.id == id && v.status == Status::Succeeded), "{views:?}");
        let (_, log, _) = reloaded.logs(id, 0).expect("reloaded logs");
        assert!(log.contains("persisted-hello"), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
