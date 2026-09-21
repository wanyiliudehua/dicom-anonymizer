//! Per-file pipeline: parse, de-identify tags, mask pixels, write, audit.

use crate::audit::{self, Record, Status};
use crate::config::Config;
use crate::deid::actions::{apply, DeidContext};
use crate::deid::dates::DateShifter;
use crate::deid::dataset as ds;
use crate::deid::uids::UidMapper;
use crate::discovery::Job;
use crate::error::Result;
use crate::pixel::stage;
use crate::writer;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct Shared {
    pub cfg: Config,
    pub uids: UidMapper,
    pub dates: DateShifter,
    pub out_root: PathBuf,
    pub dry_run: bool,
}

const DICOMDIR_SOP: &str = "1.2.840.10008.1.3.10";

fn rel_str(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

fn temp_path(target: &Path) -> PathBuf {
    target.with_extension(format!(
        "{}tmp",
        target
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{e}."))
            .unwrap_or_default()
    ))
}

fn emit(log: &mut impl Write, rec: &Record) {
    let _ = audit::write_jsonl(&mut *log, rec);
}

/// Process one file. Per-file errors are recorded, not propagated.
pub fn process_job(shared: &Shared, job: &Job, log: &mut impl Write) -> Result<Record> {
    let start = Instant::now();
    let input_str = rel_str(&job.rel);

    let mut obj = match dicom_object::open_file(&job.input) {
        Ok(o) => o,
        Err(e) => {
            let rec = Record::error(input_str, format!("open: {e}"));
            emit(log, &rec);
            return Ok(rec);
        }
    };

    let ts_in = obj.meta().transfer_syntax().to_string();

    let sop = ds::get_string(&obj, dicom_dictionary_std::tags::SOP_CLASS_UID);
    if sop.as_deref() == Some(DICOMDIR_SOP) {
        let rec = Record::skipped(input_str, Status::SkippedDicomdir);
        emit(log, &rec);
        return Ok(rec);
    }

    let ctx = DeidContext {
        cfg: &shared.cfg,
        uids: &shared.uids,
        dates: &shared.dates,
    };
    let tags_changed = match apply(&mut obj, &ctx) {
        Ok(n) => n,
        Err(e) => {
            let rec = Record::error(input_str, format!("deid: {e}"));
            emit(log, &rec);
            return Ok(rec);
        }
    };

    let regions_masked = match stage::process(&mut obj, &shared.cfg) {
        Ok(n) => n,
        Err(e) => {
            let rec = Record::error(input_str, format!("pixel: {e}"));
            emit(log, &rec);
            return Ok(rec);
        }
    };

    let mut rec = Record::ok(input_str, rel_str(&job.rel), tags_changed, regions_masked);
    rec.transfer_syntax_in = Some(ts_in);
    rec.transfer_syntax_out = Some(obj.meta().transfer_syntax().to_string());
    rec.duration_ms = start.elapsed().as_millis();

    if shared.dry_run {
        rec.output = None;
        emit(log, &rec);
        return Ok(rec);
    }

    let target = writer::output_path(&shared.out_root, &job.rel);
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let rec = Record::error(rel_str(&job.rel), format!("mkdir: {e}"));
            emit(log, &rec);
            return Ok(rec);
        }
    }

    // Write the whole DICOM file (preamble + file meta + dataset) via
    // `write_to_file`, to a sibling temp path, then rename over the target.
    let tmp = temp_path(&target);
    if let Err(e) = obj.write_to_file(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        let rec = Record::error(rel_str(&job.rel), format!("write: {e}"));
        emit(log, &rec);
        return Ok(rec);
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        let rec = Record::error(rel_str(&job.rel), format!("rename: {e}"));
        emit(log, &rec);
        return Ok(rec);
    }
    rec.output_sha256 = std::fs::read(&target).ok().map(|b| sha256_hex(&b));
    rec.duration_ms = start.elapsed().as_millis();
    emit(log, &rec);
    Ok(rec)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::Job;
    use dicom_core::{DataElement, Tag, VR};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

    fn make_dicom(path: &std::path::Path) {
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Doe^John"),
            DataElement::new(Tag(0x0020, 0x000D), VR::UI, "1.2.3"),
        ]);
        let file = obj
            .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap();
        file.write_to_file(path).unwrap();
    }

    #[test]
    fn anonymizes_and_writes_mirrored_output() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        make_dicom(&inpath);

        let cfg = crate::config::Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let shared = Shared {
            cfg,
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job { input: inpath.clone(), rel: "p.dcm".into() };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, crate::audit::Status::Ok);
        let written = out.path().join("p.dcm");
        assert!(written.exists());

        let o = dicom_object::open_file(&written).unwrap();
        let name = o
            .element(Tag(0x0010, 0x0010))
            .ok()
            .and_then(|e| e.to_str().ok().map(|s| s.to_string()));
        assert_ne!(name.as_deref(), Some("Doe^John"));
    }

    #[test]
    fn dry_run_writes_nothing_but_returns_record() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        make_dicom(&inpath);
        let shared = Shared {
            cfg: crate::config::Config::default(),
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: true,
        };
        let job = Job { input: inpath, rel: "p.dcm".into() };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, crate::audit::Status::Ok);
        assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
    }
}
