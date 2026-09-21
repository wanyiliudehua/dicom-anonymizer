//! Run summary, exit-code selection, and the run completion manifest.

use crate::audit::Status;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub ok: usize,
    pub skipped: usize,
    pub unsupported: usize,
    pub error: usize,
}

impl Summary {
    pub fn count(&mut self, status: Status) {
        match status {
            Status::Ok => self.ok += 1,
            Status::Skipped | Status::SkippedDicomdir => self.skipped += 1,
            Status::Unsupported => self.unsupported += 1,
            Status::Error => self.error += 1,
        }
    }

    /// 0 when every file succeeded or was skipped, 1 on any error.
    pub fn exit_code(&self) -> i32 {
        if self.error > 0 {
            1
        } else {
            0
        }
    }
}

/// Name of the completion marker written into the output root.
pub const MANIFEST_FILE: &str = "run.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The run reached the end with every file accounted for.
    Complete,
    /// The run reached the end, but at least one file was not written.
    Partial,
}

/// Marker written at the end of a run.
///
/// Its presence is the only trustworthy evidence that a run finished. `audit.jsonl` is
/// created before processing starts, so a killed process leaves a partial audit and a
/// partial output tree that look no different from a finished product.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub tool: String,
    pub version: String,
    pub outcome: Outcome,
    pub total: usize,
    pub ok: usize,
    pub skipped: usize,
    pub unsupported: usize,
    pub error: usize,
    /// False when the audit sink failed, so records may be missing.
    pub audit_complete: bool,
}

impl Manifest {
    pub fn new(summary: &Summary, total: usize, audit_complete: bool) -> Manifest {
        Manifest {
            tool: "dicom-anonymizer".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            outcome: if summary.error == 0 {
                Outcome::Complete
            } else {
                Outcome::Partial
            },
            total,
            ok: summary.ok,
            skipped: summary.skipped,
            unsupported: summary.unsupported,
            error: summary.error,
            audit_complete,
        }
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| crate::error::AnonError::Config(e.to_string()))?;
        std::fs::write(dir.join(MANIFEST_FILE), json)?;
        Ok(())
    }

    /// Load the marker from a tree; `Ok(None)` when the tree carries none.
    pub fn load(dir: &Path) -> Result<Option<Manifest>> {
        let path = dir.join(MANIFEST_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        let m = serde_json::from_str(&text)
            .map_err(|e| crate::error::AnonError::Config(format!("{}: {e}", path.display())))?;
        Ok(Some(m))
    }
}

/// Periodic progress on stderr.
///
/// A batch can run for hours, so it should not be a black box. Whether to report is
/// the caller's decision (`--verbose`, or stderr being a terminal) so that redirected
/// output stays clean.
pub struct Progress {
    total: usize,
    done: AtomicUsize,
    started: Instant,
    last_report: Mutex<Instant>,
    interval: Duration,
    enabled: bool,
}

impl Progress {
    /// Report at most every two seconds, and always on the final file.
    pub fn new(total: usize, enabled: bool) -> Progress {
        Progress::with_interval(total, enabled, Duration::from_secs(2))
    }

    fn with_interval(total: usize, enabled: bool, interval: Duration) -> Progress {
        let now = Instant::now();
        Progress {
            total,
            done: AtomicUsize::new(0),
            started: now,
            last_report: Mutex::new(now),
            interval,
            enabled,
        }
    }

    /// Count one finished file, returning the line to report when one is due.
    ///
    /// Returning the line rather than printing it keeps the cadence testable instead
    /// of something that can only be observed by watching a real batch.
    pub fn tick(&self) -> Option<String> {
        let done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        if !self.enabled {
            return None;
        }
        let now = Instant::now();
        let due = {
            let mut last = self.last_report.lock().unwrap_or_else(|e| e.into_inner());
            if done >= self.total || now.duration_since(*last) >= self.interval {
                *last = now;
                true
            } else {
                false
            }
        };
        due.then(|| self.line(done, now.duration_since(self.started)))
    }

    fn line(&self, done: usize, elapsed: Duration) -> String {
        let secs = elapsed.as_secs_f64();
        let rate = if secs > 0.0 { done as f64 / secs } else { 0.0 };
        let remaining = self.total.saturating_sub(done) as f64;
        let eta = if rate > 0.0 {
            Duration::from_secs_f64(remaining / rate)
        } else {
            Duration::ZERO
        };
        format!(
            "progress: {done}/{} files ({:.0}%), {:.1} files/s, elapsed {}, eta {}",
            self.total,
            100.0 * done as f64 / self.total.max(1) as f64,
            rate,
            fmt_duration(elapsed),
            fmt_duration(eta),
        )
    }
}

/// Human-readable size: `874 B`, `514 KiB`, `1.5 MiB`, `2.00 GiB`.
pub fn fmt_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KiB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MiB", f / (K * K))
    } else {
        format!("{:.2} GiB", f / (K * K * K))
    }
}

/// Compact duration: `12s`, `3m05s`, `2h07m`.
fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Status;

    #[test]
    fn all_ok_is_exit_zero() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        assert_eq!(s.exit_code(), 0);
    }

    #[test]
    fn any_error_is_exit_one() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        s.count(Status::Error);
        assert_eq!(s.exit_code(), 1);
    }

    #[test]
    fn skipped_only_is_exit_zero() {
        let mut s = Summary::default();
        s.count(Status::Skipped);
        s.count(Status::SkippedDicomdir);
        assert_eq!(s.exit_code(), 0);
    }

    #[test]
    fn progress_line_reports_rate_and_eta() {
        let p = Progress::new(10, true);
        let line = p.line(5, Duration::from_secs(10));
        assert!(line.starts_with("progress: 5/10 files (50%)"), "{line}");
        assert!(line.contains("0.5 files/s"), "{line}");
        assert!(line.contains("elapsed 10s"), "{line}");
        assert!(line.contains("eta 10s"), "{line}");
    }

    #[test]
    fn progress_line_survives_zero_elapsed_time() {
        let p = Progress::new(4, true);
        let line = p.line(4, Duration::ZERO);
        assert!(line.contains("(100%)"), "{line}");
        assert!(!line.contains("inf"), "{line}");
        assert!(!line.contains("NaN"), "{line}");
    }

    #[test]
    fn durations_are_formatted_compactly() {
        assert_eq!(fmt_duration(Duration::from_secs(12)), "12s");
        assert_eq!(fmt_duration(Duration::from_secs(65)), "1m05s");
        assert_eq!(fmt_duration(Duration::from_secs(3600 + 420)), "1h07m");
    }

    #[test]
    fn sizes_are_formatted_with_a_unit_that_does_not_round_to_zero() {
        // Reporting a 526 KB file as "0 MiB" reads like an empty file.
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(874), "874 B");
        assert_eq!(fmt_bytes(526_446), "514 KiB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(fmt_bytes(2 * 1024 * 1024 * 1024), "2.00 GiB");
    }

    #[test]
    fn a_disabled_progress_counts_without_reporting() {
        let p = Progress::new(3, false);
        assert!(p.tick().is_none());
        assert!(p.tick().is_none());
        assert_eq!(p.done.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn the_final_file_always_reports() {
        // A long interval would otherwise silence a fast run completely, and the last
        // line is the one a caller most wants to see.
        let p = Progress::with_interval(2, true, Duration::from_secs(3600));
        assert!(p.tick().is_none(), "the first of two is not due yet");
        let last = p.tick().expect("the final file must always report");
        assert!(last.contains("progress: 2/2 files (100%)"), "{last}");
    }

    #[test]
    fn a_zero_interval_reports_every_file() {
        let p = Progress::with_interval(3, true, Duration::ZERO);
        for n in 1..=3 {
            let line = p.tick().unwrap_or_else(|| panic!("tick {n} should report"));
            assert!(line.contains(&format!("progress: {n}/3 files")), "{line}");
        }
    }

    #[test]
    fn manifest_is_complete_only_without_errors() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        s.count(Status::Skipped);
        let m = Manifest::new(&s, 3, true);
        assert_eq!(m.outcome, Outcome::Complete);
        assert_eq!(m.total, 3);
        assert!(m.audit_complete);

        s.count(Status::Error);
        let m = Manifest::new(&s, 3, false);
        assert_eq!(m.outcome, Outcome::Partial);
        assert_eq!(m.error, 1);
        assert!(!m.audit_complete);
    }

    #[test]
    fn manifest_round_trips_through_a_directory() {
        let d = tempfile::tempdir().unwrap();
        // absence is meaningful, so it must not be an error
        assert!(Manifest::load(d.path()).unwrap().is_none());

        let mut s = Summary::default();
        s.count(Status::Ok);
        Manifest::new(&s, 1, true).write(d.path()).unwrap();

        let back = Manifest::load(d.path()).unwrap().unwrap();
        assert_eq!(back.outcome, Outcome::Complete);
        assert_eq!(back.ok, 1);
        assert_eq!(back.version, env!("CARGO_PKG_VERSION"));
    }
}
