//! Per-file pipeline: parse, de-identify tags, mask pixels, write, audit.

use crate::audit::{self, Record, Status};
use crate::config::{Config, FilenameMode, Layout};
use crate::deid::actions::{apply, DeidContext};
use crate::deid::dataset as ds;
use crate::deid::dates::DateShifter;
use crate::deid::uids::UidMapper;
use crate::discovery::Job;
use crate::error::{AnonError, Result};
use crate::pixel::stage;
use crate::writer;
use dicom_core::{Tag, VR};
use dicom_object::InMemDicomObject;
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

/// DICOMDIR identifies itself in the file meta (0002,0002); fall back to the
/// dataset's SOP Class UID for non-standard files.
fn is_dicomdir(obj: &dicom_object::FileDicomObject<InMemDicomObject>) -> bool {
    let ds_sop = ds::get_string(obj, dicom_dictionary_std::tags::SOP_CLASS_UID);
    let meta_sop = obj.meta().media_storage_sop_class_uid().to_string();
    ds_sop.as_deref() == Some(DICOMDIR_SOP) || meta_sop == DICOMDIR_SOP
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

/// The series UID becomes a directory name, so keep it to UID-safe characters.
/// A malformed file cannot then steer writes outside the output root: separators
/// are dropped, and a name that does not start alphanumerically (`.`, `..`, a
/// hidden or absolute path) is refused and the caller falls back to mirroring.
fn series_dir_name(uid: &str) -> Option<String> {
    let cleaned: String = uid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    let starts_ok = cleaned
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric());
    starts_ok.then_some(cleaned)
}

fn emit(log: &mut impl Write, rec: &Record) {
    let _ = audit::write_jsonl(&mut *log, rec);
}

/// Process one file. Per-file errors are recorded, not propagated.
pub fn process_job(shared: &Shared, job: &Job, log: &mut impl Write) -> Result<Record> {
    let start = Instant::now();
    let input_str = rel_str(&job.rel);

    // Non-DICOM files are logged as skipped (spec §11).
    if !job.dicom {
        let rec = Record::skipped(input_str, Status::Skipped);
        emit(log, &rec);
        return Ok(rec);
    }

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
    let _ = sop;
    if is_dicomdir(&obj) {
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

    // Keep the file meta's Media Storage SOP Instance UID consistent with the
    // (remapped) dataset SOP Instance UID.
    if let Some(new_sop) = ds::get_string(&obj, dicom_dictionary_std::tags::SOP_INSTANCE_UID) {
        if !new_sop.is_empty() {
            obj.update_meta(|m| m.media_storage_sop_instance_uid = new_sop.clone());
        }
    }

    // PS3.15 de-identification markers.
    ds::set_string(&mut obj, Tag(0x0012, 0x0062), VR::CS, "YES");
    ds::set_string(
        &mut obj,
        Tag(0x0012, 0x0063),
        VR::LO,
        "PS3.15 Basic Application Level Confidentiality Profile",
    );

    let report = match stage::process(&mut obj, &shared.cfg) {
        Ok(r) => r,
        Err(AnonError::Unsupported(ts)) => {
            eprintln!(
                "info: {} uses transfer syntax {ts}, which is not supported; skipping",
                job.rel.display()
            );
            let rec = Record::skipped(input_str, Status::Unsupported);
            emit(log, &rec);
            return Ok(rec);
        }
        Err(e) => {
            let rec = Record::error(input_str, format!("pixel: {e}"));
            emit(log, &rec);
            return Ok(rec);
        }
    };

    let base_rel = match shared.cfg.output.filename {
        FilenameMode::Preserve => job.rel.clone(),
        FilenameMode::SopInstanceUid => {
            let new_sop = ds::get_string(&obj, dicom_dictionary_std::tags::SOP_INSTANCE_UID);
            match new_sop.filter(|s| !s.is_empty()) {
                Some(uid) => job.rel.with_file_name(format!("{uid}.dcm")),
                None => job.rel.clone(),
            }
        }
    };

    let out_rel = match shared.cfg.output.layout {
        Layout::Mirror => base_rel,
        // Group by the *new* (remapped) series UID, so directory names do not
        // leak the original, and fall back to mirroring when there is none.
        Layout::Series => {
            match ds::get_string(&obj, dicom_dictionary_std::tags::SERIES_INSTANCE_UID)
                .as_deref()
                .and_then(series_dir_name)
            {
                Some(series) => {
                    let name = base_rel
                        .file_name()
                        .map(|n| n.to_os_string())
                        .unwrap_or_else(|| base_rel.clone().into_os_string());
                    Path::new(&series).join(name)
                }
                None => base_rel,
            }
        }
    };

    let mut rec = Record::ok(
        input_str,
        rel_str(&out_rel),
        tags_changed,
        report.regions_masked,
    );
    rec.samples_masked = report.samples_masked;
    rec.transfer_syntax_in = Some(ts_in);
    rec.transfer_syntax_out = Some(obj.meta().transfer_syntax().to_string());
    rec.duration_ms = start.elapsed().as_millis();

    if shared.dry_run {
        rec.output = None;
        emit(log, &rec);
        return Ok(rec);
    }

    let target = writer::output_path(&shared.out_root, &out_rel);
    if let Some(parent) = target.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            let rec = Record::error(rel_str(&out_rel), format!("mkdir: {e}"));
            emit(log, &rec);
            return Ok(rec);
        }
    }

    // Write the whole DICOM file (preamble + file meta + dataset) via
    // `write_to_file`, to a sibling temp path, then rename over the target.
    let tmp = temp_path(&target);
    if let Err(e) = obj.write_to_file(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        let rec = Record::error(rel_str(&out_rel), format!("write: {e}"));
        emit(log, &rec);
        return Ok(rec);
    }
    if let Err(e) = std::fs::rename(&tmp, &target) {
        let _ = std::fs::remove_file(&tmp);
        let rec = Record::error(rel_str(&out_rel), format!("rename: {e}"));
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
            DataElement::new(Tag(0x0008, 0x0016), VR::UI, "1.2.840.10008.5.1.4.1.1.7"),
            DataElement::new(Tag(0x0008, 0x0018), VR::UI, "1.2.840.10008.1.2.3"),
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

        let cfg = crate::config::Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[output]\nfilename = \"preserve\"\n",
        )
        .unwrap();
        let shared = Shared {
            cfg,
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job {
            input: inpath.clone(),
            rel: "p.dcm".into(),
            dicom: true,
        };
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
    fn series_dir_name_refuses_to_escape_the_output_root() {
        assert_eq!(series_dir_name("1.2.840.9"), Some("1.2.840.9".to_string()));
        assert_eq!(series_dir_name(""), None);
        assert_eq!(series_dir_name(".."), None);
        assert_eq!(series_dir_name("../../etc"), None);
        assert_eq!(series_dir_name(".hidden"), None);
        // separators are dropped rather than followed
        assert_eq!(series_dir_name("a/b"), Some("ab".to_string()));
        assert_eq!(series_dir_name("/abs"), Some("abs".to_string()));
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
        let job = Job {
            input: inpath,
            rel: "p.dcm".into(),
            dicom: true,
        };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, crate::audit::Status::Ok);
        assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
    }

    #[test]
    fn dicomdir_detected_from_file_meta_sop_class() {
        use dicom_object::FileMetaTableBuilder;
        let file = InMemDicomObject::new_empty()
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax("1.2.840.10008.1.2.1")
                    .media_storage_sop_class_uid("1.2.840.10008.1.3.10"),
            )
            .unwrap();
        assert!(is_dicomdir(&file));

        let not_dir = InMemDicomObject::new_empty()
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax("1.2.840.10008.1.2.1")
                    .media_storage_sop_class_uid("1.2.840.10008.5.1.4.1.1.7"),
            )
            .unwrap();
        assert!(!is_dicomdir(&not_dir));
    }

    #[test]
    fn renames_output_to_new_sop_instance_uid_and_syncs_meta() {
        use crate::audit::Status;
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("orig-name.dcm");
        make_dicom(&inpath);
        let shared = Shared {
            cfg: crate::config::Config::default(),
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job {
            input: inpath,
            rel: "sub/orig-name.dcm".into(),
            dicom: true,
        };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, Status::Ok);

        let written = out.path().join(rec.output.clone().unwrap());
        assert!(written.exists(), "output {written:?} missing");
        // directory preserved
        assert!(written.parent().unwrap().ends_with("sub"));

        let o = dicom_object::open_file(&written).unwrap();
        let new_sop = o
            .element(Tag(0x0008, 0x0018))
            .ok()
            .and_then(|e| e.to_str().ok().map(|s| s.to_string()))
            .unwrap();
        assert_ne!(new_sop, "1.2.840.10008.1.2.3");
        assert!(new_sop.starts_with("2.25."));
        // file meta must match the dataset instance UID
        assert_eq!(o.meta().media_storage_sop_instance_uid(), new_sop);
        // filename is the new UID
        assert_eq!(
            written.file_name().unwrap().to_string_lossy(),
            format!("{new_sop}.dcm")
        );
    }

    #[test]
    fn writes_deidentification_markers() {
        use crate::audit::Status;
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        make_dicom(&inpath);
        let shared = Shared {
            cfg: crate::config::Config::default(),
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job {
            input: inpath,
            rel: "p.dcm".into(),
            dicom: true,
        };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, Status::Ok);
        let written = out.path().join(rec.output.unwrap());
        let o = dicom_object::open_file(&written).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0012, 0x0062)).as_deref(),
            Some("YES")
        );
        assert!(crate::deid::dataset::get_string(&o, Tag(0x0012, 0x0063)).is_some());
    }

    #[test]
    fn preserve_mode_keeps_relative_name() {
        use crate::audit::Status;
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("orig-name.dcm");
        make_dicom(&inpath);
        let cfg =
            crate::config::Config::from_toml_str("[output]\nfilename = \"preserve\"\n").unwrap();
        let shared = Shared {
            cfg,
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job {
            input: inpath,
            rel: "sub/orig-name.dcm".into(),
            dicom: true,
        };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, Status::Ok);
        assert!(out.path().join("sub/orig-name.dcm").exists());
        assert_eq!(rec.output.as_deref(), Some("sub/orig-name.dcm"));
    }

    fn make_pixel_dicom(path: &std::path::Path) {
        use dicom_core::PrimitiveValue;
        let u16v = |n: u16| PrimitiveValue::U16(vec![n].into());
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(
                dicom_dictionary_std::tags::SOP_CLASS_UID,
                VR::UI,
                "1.2.840.10008.5.1.4.1.1.7",
            ),
            DataElement::new(
                dicom_dictionary_std::tags::SOP_INSTANCE_UID,
                VR::UI,
                "1.2.840.10008.1.2.3",
            ),
            DataElement::new(dicom_dictionary_std::tags::ROWS, VR::US, u16v(2)),
            DataElement::new(dicom_dictionary_std::tags::COLUMNS, VR::US, u16v(2)),
            DataElement::new(dicom_dictionary_std::tags::BITS_ALLOCATED, VR::US, u16v(8)),
            DataElement::new(
                dicom_dictionary_std::tags::SAMPLES_PER_PIXEL,
                VR::US,
                u16v(1),
            ),
            DataElement::new(
                dicom_dictionary_std::tags::PIXEL_DATA,
                VR::OB,
                PrimitiveValue::U8(vec![0u8; 4].into()),
            ),
        ]);
        obj.with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap()
            .write_to_file(path)
            .unwrap();
    }

    #[test]
    fn out_of_bounds_region_is_per_file_error() {
        use crate::audit::Status;
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        make_pixel_dicom(&inpath);
        let cfg = crate::config::Config::from_toml_str(
            "[pixels]\nmode=\"mask\"\n[[pixels.regions]]\nshape=\"rect\"\nwidth=9\nheight=9\n",
        )
        .unwrap();
        let shared = Shared {
            cfg,
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job {
            input: inpath,
            rel: "p.dcm".into(),
            dicom: true,
        };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, Status::Error);
        assert!(rec.error.unwrap_or_default().contains("pixel"));
        // nothing was written for this file
        assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
    }

    #[test]
    fn sequence_phi_is_anonymized() {
        use crate::audit::Status;
        use dicom_core::value::DataSetSequence;
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        let inner = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0010, 0x0010),
            VR::PN,
            "Doe^John",
        )]);
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(
                dicom_dictionary_std::tags::SOP_CLASS_UID,
                VR::UI,
                "1.2.840.10008.5.1.4.1.1.7",
            ),
            DataElement::new(
                dicom_dictionary_std::tags::SOP_INSTANCE_UID,
                VR::UI,
                "1.2.840.10008.1.2.3",
            ),
            DataElement::new(
                Tag(0x0008, 0x1115),
                VR::SQ,
                DataSetSequence::from(vec![inner]),
            ),
        ]);
        obj.with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap()
            .write_to_file(&inpath)
            .unwrap();

        let shared = Shared {
            cfg: crate::config::Config::default(),
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job {
            input: inpath,
            rel: "p.dcm".into(),
            dicom: true,
        };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, Status::Ok);
        let written = out.path().join(rec.output.unwrap());
        let o = dicom_object::open_file(&written).unwrap();
        // unruled sequence is kept, but its nested PHI is gone
        assert!(o.element(Tag(0x0008, 0x1115)).is_ok());
        let bytes = std::fs::read(&written).unwrap();
        assert!(
            !bytes.windows(8).any(|w| w == b"Doe^John"),
            "nested PHI survived"
        );
    }
}
