//! A small, serialized job manager for `validation_start` and the `job_*` tools.
//!
//! Jobs run `cargo`/`xtask` commands as child processes. Execution is **serialized** (one at a time)
//! so Cargo jobs sharing `target/`, the package cache and the registry cache do not contend. Logs are
//! captured live and read cursor-wise. A `cargo test` that exits 0 after matching **zero tests** is
//! reported as [`Status::NoTests`] — invalid evidence, not a pass.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const LOG_CAP: usize = 4 << 20;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Job lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
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

struct Job {
    id: u64,
    name: String,
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
}

/// A serializable view of a job.
#[derive(Serialize)]
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
}

struct Inner {
    jobs: BTreeMap<u64, Job>,
}

/// Serialized, in-memory job manager (process lifetime).
pub struct JobManager {
    inner: Mutex<Inner>,
    gate: Mutex<()>,
    next: AtomicU64,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobManager {
    pub fn new() -> Self {
        Self { inner: Mutex::new(Inner { jobs: BTreeMap::new() }), gate: Mutex::new(()), next: AtomicU64::new(1) }
    }

    /// Queue a job and return its id. It runs as soon as the serialization gate frees up.
    pub fn start(self: &Arc<Self>, name: String, program: String, args: Vec<String>, cwd: PathBuf) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let log = Arc::new(Mutex::new(String::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.inner).jobs.insert(
            id,
            Job {
                id,
                name,
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
        if let Some(job) = lock(&self.inner).jobs.get_mut(&id) {
            job.status = status;
            job.exit = exit;
            job.zero_tests = zero_tests;
            if let Some(started) = job.started_ms {
                job.duration_ms = Some(now_ms().saturating_sub(started));
            }
        }
    }

    fn view(job: &Job) -> JobView {
        JobView {
            id: job.id,
            name: job.name.clone(),
            status: job.status,
            command: format!("{} {}", job.program, job.args.join(" ")).trim_end().to_string(),
            pid: job.pid,
            started_ms: job.started_ms,
            duration_ms: job.duration_ms,
            exit: job.exit,
            zero_tests: job.zero_tests,
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
        let log = lock(&job.log);
        let cursor = cursor.min(log.len());
        // Slice on a char boundary to stay UTF-8 safe.
        let start = (cursor..=log.len()).find(|i| log.is_char_boundary(*i)).unwrap_or(log.len());
        Some((job.status, log[start..].to_string(), log.len()))
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
        let mgr = Arc::new(JobManager::new());
        let id = mgr.start("echo".into(), "sh".into(), vec!["-c".into(), "echo forge-hello".into()], std::env::temp_dir());
        let mut status = Status::Queued;
        for _ in 0..200 {
            status = mgr.views(Some(id))[0].status;
            if status.done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(status, Status::Succeeded, "status: {status:?}");
        let (_, log, _) = mgr.logs(id, 0).expect("logs");
        assert!(log.contains("forge-hello"), "{log}");
    }
}
