//! Long-running commands as jobs: each registers a cancel channel under its id while it runs,
//! and reports progress to the UI as `job-progress` events.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::watch;

use crate::engines::proc::{Cancel, Result};

#[derive(Default)]
pub struct Jobs(Mutex<HashMap<String, watch::Sender<bool>>>);

impl Jobs {
    /// Registers a cancellable job. Ids are unique per running job; a duplicate would
    /// otherwise replace (and orphan) the first job's cancel channel.
    pub fn start(&self, id: &str) -> Result<Cancel> {
        let mut jobs = self.0.lock().unwrap();
        if jobs.contains_key(id) {
            return Err("Already in progress".into());
        }
        let (tx, rx) = watch::channel(false);
        jobs.insert(id.to_string(), tx);
        Ok(rx)
    }

    pub fn finish(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }

    pub fn cancel(&self, id: &str) {
        if let Some(tx) = self.0.lock().unwrap().get(id) {
            let _ = tx.send(true);
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobProgress<'a> {
    job_id: &'a str,
    percent: f64,
}

/// Emits `job-progress` for `job_id`, at most once per whole percent.
pub fn progress_emitter<'a>(app: &'a AppHandle, job_id: &'a str) -> impl Fn(f64) + 'a {
    // f64 bits of the last emitted value; an atomic keeps the command future `Send`.
    let last = AtomicU64::new(u64::MAX);
    move |percent: f64| {
        let percent = if percent < 0.0 { -1.0 } else { percent.floor().min(100.0) };
        if last.swap(percent.to_bits(), Ordering::Relaxed) != percent.to_bits() {
            let _ = app.emit("job-progress", JobProgress { job_id, percent });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_job_ids_are_rejected_until_finished() {
        let jobs = Jobs::default();
        assert!(jobs.start("1-0").is_ok());
        assert!(jobs.start("1-0").is_err());
        jobs.finish("1-0");
        assert!(jobs.start("1-0").is_ok());
    }
}
