//! Heartbeat file for supervisor monitoring (issue #3661).
//!
//! An external supervisor can monitor this file to distinguish an idle app from a dead or
//! wedged process. While the event loop runs, the file is touched at an interval (default 1s),
//! updating its modified time and recording the process ID and timestamp.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(1000);

pub struct Heartbeat {
    path: PathBuf,
    last: Instant,
    interval: Duration,
}

impl Heartbeat {
    pub fn new(path: PathBuf) -> Self {
        Self::with_interval(path, DEFAULT_INTERVAL)
    }

    pub fn with_interval(path: PathBuf, interval: Duration) -> Self {
        let mut hb = Self {
            path,
            last: Instant::now() - interval,
            interval,
        };
        hb.touch();
        hb
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.last.elapsed() >= self.interval {
            self.touch();
        }
        ctx.request_repaint_after(self.interval);
    }

    pub fn touch(&mut self) {
        self.last = Instant::now();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let content = format!("pid={}\ntime={:.3}\n", std::process::id(), now);
        if let Err(err) = std::fs::write(&self.path, content) {
            log::warn!("could not write heartbeat file {}: {err}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_file_written_on_creation_and_touch() {
        let dir = std::env::temp_dir().join(format!("photocraft_test_hb_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("heartbeat");

        let mut hb = Heartbeat::new(path.clone());
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(&format!("pid={}", std::process::id())));
        assert!(content.contains("time="));

        // Subsequent touch updates file
        hb.touch();
        let content2 = std::fs::read_to_string(&path).unwrap();
        assert!(content2.contains(&format!("pid={}", std::process::id())));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
