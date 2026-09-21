//! JSON Lines audit records: one object per processed file.

use crate::error::Result;
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Skipped,
    SkippedDicomdir,
    Unsupported,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub input: String,
    pub output: Option<String>,
    pub status: Status,
    pub error: Option<String>,
    pub transfer_syntax_in: Option<String>,
    pub transfer_syntax_out: Option<String>,
    pub tags_changed: usize,
    pub regions_masked: usize,
    pub output_sha256: Option<String>,
    pub duration_ms: u128,
}

impl Record {
    pub fn ok(
        input: impl Into<String>,
        output: impl Into<String>,
        tags: usize,
        masked: usize,
    ) -> Record {
        Record {
            input: input.into(),
            output: Some(output.into()),
            status: Status::Ok,
            error: None,
            transfer_syntax_in: None,
            transfer_syntax_out: None,
            tags_changed: tags,
            regions_masked: masked,
            output_sha256: None,
            duration_ms: 0,
        }
    }

    pub fn error(input: impl Into<String>, reason: impl Into<String>) -> Record {
        Record {
            input: input.into(),
            output: None,
            status: Status::Error,
            error: Some(reason.into()),
            transfer_syntax_in: None,
            transfer_syntax_out: None,
            tags_changed: 0,
            regions_masked: 0,
            output_sha256: None,
            duration_ms: 0,
        }
    }

    pub fn skipped(input: impl Into<String>, status: Status) -> Record {
        Record {
            input: input.into(),
            output: None,
            status,
            error: None,
            transfer_syntax_in: None,
            transfer_syntax_out: None,
            tags_changed: 0,
            regions_masked: 0,
            output_sha256: None,
            duration_ms: 0,
        }
    }
}

/// Serialize one record as a JSON line.
pub fn write_jsonl(mut w: impl Write, r: &Record) -> Result<()> {
    serde_json::to_writer(&mut w, r).map_err(|e| crate::error::AnonError::Config(e.to_string()))?;
    writeln!(w)?;
    Ok(())
}

/// Write a pre-rendered audit line to the sink, surfacing IO errors so the
/// caller can fail the run instead of silently losing records.
pub fn write_raw(w: &mut impl Write, line: &[u8]) -> Result<()> {
    w.write_all(line)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_record_has_required_fields() {
        let r = Record::ok("in.dcm", "out.dcm", 3, 2);
        let s = serde_json::to_string(&r).unwrap();
        for k in ["input", "output", "status", "tags_changed", "regions_masked"] {
            assert!(s.contains(k), "missing {k}");
        }
    }

    #[test]
    fn error_record_serializes_reason() {
        let r = Record::error("in.dcm", "boom");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("boom"));
        assert!(s.contains("\"status\":\"error\""));
    }

    struct FailWriter;
    impl std::io::Write for FailWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_raw_reports_io_failure() {
        let mut w = FailWriter;
        assert!(write_raw(&mut w, b"x").is_err());
    }
}
