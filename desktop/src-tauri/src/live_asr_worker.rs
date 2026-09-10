//! Shared lifecycle utilities for the native CoreML and headless ONNX live-ASR workers.
//!
//! Decoding remains backend-specific. Startup readiness, liveness, bounded waits,
//! and trace metadata should behave identically so a capture is never reported as
//! transcription-ready while its worker is still loading or warming models.

use serde_json::Value;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const AUDIO_QUEUE_CHUNKS: usize = 2_000;
pub(crate) const AUDIO_QUEUE_MAX_SAMPLES: u64 = 16_000 * 60 * 2;
pub(crate) const WORKER_LOOP_POLL: Duration = Duration::from_millis(20);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
pub(crate) const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const WORKER_HEALTH_POLL: Duration = Duration::from_millis(250);
pub(crate) const WORKER_STOPPED: &str = "Live transcript worker stopped";
pub(crate) const WORKER_WARMING: &str = "Live transcript worker warming";

#[derive(Clone)]
pub(crate) struct WorkerHealth {
    alive: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl WorkerHealth {
    pub(crate) fn new() -> Self {
        Self {
            alive: Arc::new(AtomicBool::new(true)),
            ready: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub(crate) fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    pub(crate) fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_shutdown_requested(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub(crate) fn guard(&self) -> WorkerAliveGuard {
        WorkerAliveGuard {
            alive: Arc::clone(&self.alive),
        }
    }

    pub(crate) fn recv<T>(
        &self,
        rx: mpsc::Receiver<T>,
        timeout: Duration,
        timeout_message: &str,
    ) -> Result<T, String> {
        recv_while_alive(rx, &self.alive, timeout, timeout_message)
    }
}

pub(crate) struct WorkerAliveGuard {
    alive: Arc<AtomicBool>,
}

impl Drop for WorkerAliveGuard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn recv_while_alive<T>(
    rx: mpsc::Receiver<T>,
    alive: &AtomicBool,
    timeout: Duration,
    timeout_message: &str,
) -> Result<T, String> {
    let started = Instant::now();
    loop {
        match rx.try_recv() {
            Ok(value) => return Ok(value),
            Err(mpsc::TryRecvError::Disconnected) => return Err(WORKER_STOPPED.to_string()),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if !alive.load(Ordering::SeqCst) {
            return Err(WORKER_STOPPED.to_string());
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(timeout_message.to_string());
        }
        match rx.recv_timeout(timeout.saturating_sub(elapsed).min(WORKER_HEALTH_POLL)) {
            Ok(value) => return Ok(value),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(WORKER_STOPPED.to_string());
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct LiveAsrTrace {
    margins_dir: PathBuf,
    session_name: String,
    backend: &'static str,
}

impl LiveAsrTrace {
    pub(crate) fn new(margins_dir: PathBuf, session_name: String, backend: &'static str) -> Self {
        Self {
            margins_dir,
            session_name,
            backend,
        }
    }

    pub(crate) fn append(&self, mut event: Value) {
        let Some(object) = event.as_object_mut() else {
            return;
        };
        object
            .entry("backend")
            .or_insert_with(|| Value::String(self.backend.to_string()));
        object
            .entry("unix_ms")
            .or_insert_with(|| Value::from(unix_ms() as u64));

        let _ = std::fs::create_dir_all(&self.margins_dir);
        let path = self
            .margins_dir
            .join(format!("{}_backchannel_trace.jsonl", self.session_name));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{event}");
        }
    }
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_returns_while_worker_is_alive() {
        let health = WorkerHealth::new();
        let (tx, rx) = mpsc::channel();
        tx.send(42).unwrap();
        assert_eq!(
            health
                .recv(rx, Duration::from_millis(20), "timed out")
                .unwrap(),
            42
        );
    }

    #[test]
    fn response_fails_when_worker_guard_drops() {
        let health = WorkerHealth::new();
        let guard = health.guard();
        let (_tx, rx) = mpsc::channel::<()>();
        drop(guard);
        assert_eq!(
            health
                .recv(rx, Duration::from_secs(1), "timed out")
                .unwrap_err(),
            WORKER_STOPPED
        );
    }

    #[test]
    fn response_timeout_is_bounded() {
        let health = WorkerHealth::new();
        let (_tx, rx) = mpsc::channel::<()>();
        assert_eq!(
            health
                .recv(rx, Duration::from_millis(1), "bounded timeout")
                .unwrap_err(),
            "bounded timeout"
        );
    }
}
