//! Post-run leak check: scan the output tree for identifiers harvested from the
//! input tree.
//!
//! The de-identification engine can only be trusted if its result can be
//! inspected, so `--check-leaks` re-reads the input tree to learn what was
//! identifying, then re-reads the output tree and fails the run if any of it
//! survived.

use crate::config::Config;
use crate::deid::actions::{collect_identifiers, Identifiers};
use crate::discovery;
use crate::error::Result;
use dicom_core::header::Header;
use dicom_core::{Tag, VR};
use dicom_object::InMemDicomObject;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    /// Output-relative path, `/`-separated.
    pub file: String,
    pub tag: Tag,
    pub value: String,
    pub identifier: String,
}

/// Harvest identifiers from every DICOM file under `input`.
pub fn harvest(input: &Path) -> Result<Identifiers> {
    let mut ids = Identifiers::default();
    for job in discovery::discover(input)? {
        if !job.dicom {
            continue;
        }
        // A file we cannot read simply contributes no identifiers.
        if let Ok(obj) = dicom_object::open_file(&job.input) {
            ids.merge(&collect_identifiers(&obj));
        }
    }
    Ok(ids)
}

/// The identifiers to check for, dropping categories the user explicitly asked
/// to retain (retaining institution identity is not a leak).
pub fn check_set(ids: &Identifiers, cfg: &Config) -> Vec<String> {
    let mut wanted = ids.clone();
    if cfg.profile.retain_institution_identity {
        wanted.orgs.clear();
    }
    wanted.all()
}

/// Scan every DICOM file under `root` for any of `ids` (case-insensitive).
pub fn scan(root: &Path, ids: &[String]) -> Result<Vec<Leak>> {
    let needles: Vec<(String, String)> = ids
        .iter()
        .map(|s| (s.to_ascii_lowercase(), s.clone()))
        .filter(|(lower, _)| !lower.is_empty())
        .collect();
    let mut leaks = Vec::new();
    if needles.is_empty() {
        return Ok(leaks);
    }

    for job in discovery::discover(root)? {
        if !job.dicom {
            continue;
        }
        let Ok(obj) = dicom_object::open_file(&job.input) else {
            continue;
        };
        let file = job.rel.to_string_lossy().replace('\\', "/");
        visit(&obj, &mut |tag, vr, text| {
            // UIDs are remapped, not scrubbed: digits inside them are expected.
            if vr == VR::UI || !text.is_ascii() {
                return;
            }
            let hay = text.to_ascii_lowercase();
            for (needle, original) in &needles {
                if hay.contains(needle.as_str()) {
                    leaks.push(Leak {
                        file: file.clone(),
                        tag,
                        value: text.to_string(),
                        identifier: original.clone(),
                    });
                }
            }
        });
    }
    Ok(leaks)
}

/// Depth-first visit of every element, including values nested in sequences.
fn visit(obj: &InMemDicomObject, f: &mut impl FnMut(Tag, VR, &str)) {
    for e in obj.iter() {
        if let Ok(s) = e.to_str() {
            f(e.tag(), e.vr(), &s);
        }
        if let Some(items) = e.value().items() {
            for item in items {
                visit(item, f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, VR};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};

    fn write_file(path: &std::path::Path, obj: InMemDicomObject) {
        let file = obj
            .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        file.write_to_file(path).unwrap();
    }

    /// Minimal SOP identity so `with_meta` succeeds.
    fn sop() -> [DataElement<InMemDicomObject>; 2] {
        [
            DataElement::new(
                dicom_dictionary_std::tags::SOP_CLASS_UID,
                VR::UI,
                "1.2.840.10008.5.1.4.1.1.7",
            ),
            DataElement::new(
                dicom_dictionary_std::tags::SOP_INSTANCE_UID,
                VR::UI,
                "1.2.3.4",
            ),
        ]
    }

    #[test]
    fn harvest_reads_identity_from_the_input_tree() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("a.dcm"),
            InMemDicomObject::from_element_iter([
                a,
                b,
                DataElement::new(dicom_dictionary_std::tags::PATIENT_NAME, VR::PN, "Doe^John"),
                DataElement::new(
                    dicom_dictionary_std::tags::INSTITUTION_NAME,
                    VR::LO,
                    "WanNan Medical College YiJiShan Hospital",
                ),
            ]),
        );

        let ids = harvest(d.path()).unwrap();
        // person name contributes its components
        assert!(ids.names.contains(&"Doe".to_string()), "{ids:?}");
        assert!(ids.names.contains(&"John".to_string()), "{ids:?}");
        // the institution contributes its distinctive words
        assert!(ids.orgs.contains(&"YiJiShan".to_string()), "{ids:?}");
        assert!(ids.orgs.contains(&"WanNan".to_string()), "{ids:?}");
    }

    #[test]
    fn check_set_drops_categories_the_user_retained() {
        let ids = Identifiers {
            patient: vec!["PID-1".to_string()],
            names: vec!["Doe".to_string()],
            orgs: vec!["YiJiShan".to_string()],
        };
        let mut cfg = Config::default();
        assert_eq!(
            check_set(&ids, &cfg),
            vec![
                "Doe".to_string(),
                "PID-1".to_string(),
                "YiJiShan".to_string()
            ]
        );
        cfg.profile.retain_institution_identity = true;
        assert_eq!(
            check_set(&ids, &cfg),
            vec!["Doe".to_string(), "PID-1".to_string()],
            "retaining institution identity is not a leak"
        );
    }

    #[test]
    fn scan_reports_an_identifier_left_in_the_output() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([
                a,
                b,
                DataElement::new(Tag(0x0008, 0x1030), VR::LO, "YiJiShan Hospital^l-spine"),
            ]),
        );

        let leaks = scan(d.path(), &["YiJiShan".to_string()]).unwrap();
        assert_eq!(leaks.len(), 1, "{leaks:?}");
        assert_eq!(leaks[0].tag, Tag(0x0008, 0x1030));
        assert_eq!(leaks[0].file, "x.dcm");
        assert!(leaks[0].value.contains("YiJiShan"));
    }

    #[test]
    fn scan_is_case_insensitive() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([
                a,
                b,
                DataElement::new(Tag(0x0008, 0x1030), VR::LO, "yijishan hospital"),
            ]),
        );
        assert_eq!(scan(d.path(), &["YiJiShan".to_string()]).unwrap().len(), 1);
    }

    #[test]
    fn scan_ignores_uid_values() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([a, b]),
        );
        // UIDs are remapped, not scrubbed; digits there are not a leak.
        let leaks = scan(d.path(), &["1.2.3.4".to_string(), "1.2.840".to_string()]).unwrap();
        assert!(leaks.is_empty(), "UID values must be ignored: {leaks:?}");
    }

    #[test]
    fn scan_sees_values_nested_in_sequences() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        let inner = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x1030),
            VR::LO,
            "YiJiShan Hospital",
        )]);
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([
                a,
                b,
                DataElement::new(
                    Tag(0x0040, 0x0275),
                    VR::SQ,
                    DataSetSequence::from(vec![inner]),
                ),
            ]),
        );
        assert_eq!(
            scan(d.path(), &["YiJiShan".to_string()]).unwrap().len(),
            1,
            "values inside sequences must be scanned too"
        );
    }

    #[test]
    fn scan_of_a_clean_tree_reports_nothing() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([
                a,
                b,
                DataElement::new(Tag(0x0008, 0x1030), VR::LO, "l-spine"),
            ]),
        );
        assert!(scan(d.path(), &["YiJiShan".to_string()])
            .unwrap()
            .is_empty());
    }
}
