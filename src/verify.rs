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
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
        .map(|s| (s.to_lowercase(), s.clone()))
        .filter(|(lower, _)| !lower.is_empty())
        .collect();
    let mut leaks = Vec::new();
    if needles.is_empty() {
        return Ok(leaks);
    }

    for_each_text_value(root, &mut |file, tag, vr, text| {
        // UIDs are remapped, not scrubbed: digits inside them are expected.
        if vr == VR::UI {
            return;
        }
        // Lowercase the whole value (Unicode-aware). Folding only ASCII bytes
        // would skip any value holding a non-ASCII byte, i.e. every CJK name.
        let hay = text.to_lowercase();
        for (needle, original) in &needles {
            if hay.contains(needle.as_str()) {
                leaks.push(Leak {
                    file: file.to_string(),
                    tag,
                    value: text.to_string(),
                    identifier: original.clone(),
                });
            }
        }
    })?;
    Ok(leaks)
}

/// Run `f` over every text value of every DICOM file under `root`.
fn for_each_text_value(root: &Path, f: &mut impl FnMut(&str, Tag, VR, &str)) -> Result<()> {
    for job in discovery::discover(root)? {
        if !job.dicom {
            continue;
        }
        let Ok(obj) = dicom_object::open_file(&job.input) else {
            continue;
        };
        let file = job.rel.to_string_lossy().replace('\\', "/");
        visit(&obj, &mut |tag, vr, text| f(&file, tag, vr, text));
    }
    Ok(())
}

/// A salted, hashed summary of what was identifying in an input tree.
///
/// Verification should not require the original data *and* should not require
/// writing the identifiers down again: a de-identification tool that emitted a
/// plaintext list of patient names would have created a new PHI artefact. So the
/// export keeps only salted hashes (plus the identifier lengths, to avoid
/// pointless work), and nothing in the file reveals a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub version: u32,
    /// Salt, stored alongside so hashes can be recomputed later. It only has to
    /// prevent casual disclosure (e.g. grepping the artefact), not brute force.
    pub salt: String,
    /// Distinct identifier lengths in characters, so matching can skip windows
    /// that cannot possibly match.
    pub lengths: Vec<usize>,
    /// Lowercase hex SHA-256 of `salt:lowercase(identifier)`, sorted and deduped.
    pub hashes: Vec<String>,
}

/// Hash the identifiers so the result can be written down safely.
pub fn fingerprint(ids: &[String], salt: &str) -> Fingerprint {
    let mut lengths = BTreeSet::new();
    let mut hashes = BTreeSet::new();
    for id in ids {
        let normalised = id.to_lowercase();
        let len = normalised.chars().count();
        if len == 0 {
            continue;
        }
        lengths.insert(len);
        hashes.insert(hash_hex(salt, &normalised));
    }
    Fingerprint {
        version: 1,
        salt: salt.to_string(),
        lengths: lengths.into_iter().collect(),
        hashes: hashes.into_iter().collect(),
    }
}

impl Fingerprint {
    /// Substrings of `text` that match a fingerprinted identifier. Matching is
    /// case-insensitive and works on characters, so it agrees with the scrub.
    pub fn hits(&self, text: &str) -> Vec<String> {
        let known: BTreeSet<&str> = self.hashes.iter().map(|h| h.as_str()).collect();
        let chars: Vec<char> = text.chars().collect();
        let mut found = BTreeSet::new();
        for &len in &self.lengths {
            if len == 0 || len > chars.len() {
                continue;
            }
            for i in 0..=chars.len() - len {
                let window: String = chars[i..i + len].iter().collect();
                let digest = hash_hex(&self.salt, &window.to_lowercase());
                if known.contains(digest.as_str()) {
                    found.insert(window);
                }
            }
        }
        found.into_iter().collect()
    }
}

/// A text value in a scanned tree that matched a fingerprinted identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintHit {
    pub file: String,
    pub tag: Tag,
    pub value: String,
    /// The offending substring exactly as it appears in the scanned file.
    pub matched: String,
}

/// Scan every DICOM file under `root` against a fingerprint.
pub fn scan_fingerprint(root: &Path, fp: &Fingerprint) -> Result<Vec<FingerprintHit>> {
    let mut hits = Vec::new();
    for_each_text_value(root, &mut |file, tag, vr, text| {
        // UIDs are remapped rather than scrubbed, so their digits are expected.
        if vr == VR::UI {
            return;
        }
        for matched in fp.hits(text) {
            hits.push(FingerprintHit {
                file: file.to_string(),
                tag,
                value: text.to_string(),
                matched,
            });
        }
    })?;
    Ok(hits)
}

/// Salted SHA-256 as lowercase hex.
fn hash_hex(salt: &str, value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(value.as_bytes());
    format!("{:x}", h.finalize())
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

    #[test]
    fn fingerprint_finds_an_identifier_hidden_in_text() {
        let fp = fingerprint(&["YiJiShan".to_string()], "pepper");
        assert_eq!(
            fp.hits("YiJiShan Hospital^l-spine"),
            vec!["YiJiShan".to_string()]
        );
    }

    #[test]
    fn fingerprint_matching_is_case_insensitive() {
        let fp = fingerprint(&["YiJiShan".to_string()], "pepper");
        assert_eq!(fp.hits("yijishan hospital").len(), 1);
    }

    #[test]
    fn fingerprint_handles_non_ascii_identifiers() {
        let fp = fingerprint(&["万楠医学院".to_string()], "pepper");
        assert_eq!(fp.hits("万楠医学院附属医院^腰椎").len(), 1);
        assert!(fp.hits("l-spine T2").is_empty());
    }

    #[test]
    fn fingerprint_ignores_unrelated_text() {
        let fp = fingerprint(&["YiJiShan".to_string()], "pepper");
        assert!(fp.hits("l-spine T2 w").is_empty());
    }

    #[test]
    fn fingerprint_artefact_holds_no_plaintext_identifier() {
        let fp = fingerprint(
            &["Cheng Yingping".to_string(), "YiJiShan".to_string()],
            "pepper",
        );
        let json = serde_json::to_string(&fp).unwrap();
        assert!(!json.contains("Cheng"), "{json}");
        assert!(!json.contains("Yingping"), "{json}");
        assert!(!json.contains("YiJiShan"), "{json}");
        // and it is still usable, i.e. the hashes are really in there
        assert_eq!(fp.hashes.len(), 2);
    }

    #[test]
    fn fingerprint_round_trips_through_json() {
        let fp = fingerprint(&["Doe".to_string()], "pepper");
        let json = serde_json::to_string(&fp).unwrap();
        let back: Fingerprint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.hits("Doe^John"), vec!["Doe".to_string()]);
    }

    #[test]
    fn a_different_salt_produces_different_hashes() {
        let a = fingerprint(&["Doe".to_string()], "one");
        let b = fingerprint(&["Doe".to_string()], "two");
        assert_ne!(a.hashes, b.hashes);
        assert_eq!(a.hits("Doe").len(), 1);
        assert_eq!(b.hits("Doe").len(), 1);
    }

    #[test]
    fn scan_finds_non_ascii_identifiers() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([
                a,
                b,
                // A UTF-8 character set must be declared, otherwise the writer
                // cannot encode CJK text at all (see the integration test for
                // what the pipeline does in that case).
                DataElement::new(
                    dicom_dictionary_std::tags::SPECIFIC_CHARACTER_SET,
                    VR::CS,
                    "ISO_IR 192",
                ),
                DataElement::new(Tag(0x0008, 0x1030), VR::LO, "万楠医学院附属医院^腰椎"),
            ]),
        );
        // This used to be skipped entirely: the scan bailed out on any value
        // containing a non-ASCII byte, so CJK leaks were invisible to it.
        let leaks = scan(d.path(), &["万楠医学院".to_string()]).unwrap();
        assert_eq!(leaks.len(), 1, "{leaks:?}");
        assert_eq!(leaks[0].tag, Tag(0x0008, 0x1030));
    }

    #[test]
    fn scan_fingerprint_finds_a_leak_in_a_tree() {
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
        let fp = fingerprint(&["YiJiShan".to_string()], "pepper");
        let hits = scan_fingerprint(d.path(), &fp).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].tag, Tag(0x0008, 0x1030));
        assert_eq!(hits[0].file, "x.dcm");
        assert_eq!(hits[0].matched, "YiJiShan");
    }

    #[test]
    fn scan_fingerprint_ignores_uid_values() {
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        write_file(
            &d.path().join("x.dcm"),
            InMemDicomObject::from_element_iter([a, b]),
        );
        let fp = fingerprint(&["1.2.3.4".to_string()], "pepper");
        assert!(scan_fingerprint(d.path(), &fp).unwrap().is_empty());
    }

    #[test]
    fn the_library_rejects_non_ascii_text_without_a_character_set() {
        // Constructing a value with real CJK characters and writing it under the
        // default (ASCII-equivalent) codec fails. Note this is NOT what reading a
        // file does: bytes read from disk decode to single-byte characters that the
        // same codec re-encodes byte-for-byte, so the pipeline never meets this. It
        // is kept to pin the bytes-versus-characters distinction that caused a
        // wrong inference once already.
        let d = tempfile::tempdir().unwrap();
        let [a, b] = sop();
        let obj = InMemDicomObject::from_element_iter([
            a,
            b,
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "万楠医学院"),
        ]);
        let file = obj
            .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap();
        assert!(
            file.write_to_file(d.path().join("x.dcm")).is_err(),
            "expected the default character set to reject CJK text"
        );
    }
}
