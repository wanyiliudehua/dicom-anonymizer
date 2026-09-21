//! Run summary, exit-code selection, and the run completion manifest.

use crate::audit::Status;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
