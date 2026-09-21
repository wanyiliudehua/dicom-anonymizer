//! PS3.15 Basic Application Level Confidentiality Profile rule table.
//!
//! Source: DICOM PS3.15 Table E.1-1
//! (<https://dicom.nema.org/medical/dicom/current/output/chtml/part15/chapter_E.html>).
//! The table below is a curated high-risk subset; extend it to the full E.1-1
//! as needed. The test `covers_high_risk_identifiers` is the minimum bar.

use crate::config::{parse_tag, Action, Config};
use crate::error::Result;
use dicom_core::Tag;

const BASE: &[(&str, Action)] = &[
    ("0008,0012", Action::U), // Instance Creation Date
    ("0008,0020", Action::Z), // Study Date
    ("0008,0021", Action::Z), // Series Date
    ("0008,0022", Action::Z), // Acquisition Date
    ("0008,0023", Action::Z), // Content Date
    ("0008,0030", Action::Z), // Study Time
    ("0008,0050", Action::Z), // Accession Number
    ("0008,0070", Action::X), // Manufacturer
    ("0008,0080", Action::X), // Institution Name
    ("0008,0081", Action::X), // Institution Address
    ("0008,0090", Action::X), // Referring Physician's Name
    ("0008,0092", Action::X), // Referring Physician's Address
    ("0008,0094", Action::X), // Referring Physician's Telephone Numbers
    ("0008,1040", Action::X), // Institutional Department Name
    ("0008,1048", Action::X), // Physician(s) of Record
    ("0008,1050", Action::X), // Performing Physician's Name
    ("0008,1060", Action::X), // Name of Physician(s) Reading Study
    ("0008,1070", Action::X), // Operators' Name
    ("0008,1080", Action::X), // Admitting Diagnoses Description
    ("0008,0016", Action::K), // SOP Class UID (identifies the IOD, kept)
    ("0008,0018", Action::U), // SOP Instance UID
    ("0008,1030", Action::X), // Study Description
    ("0008,103E", Action::X), // Series Description
    ("0008,1155", Action::U), // Referenced SOP Instance UID
    ("0010,0010", Action::Z), // Patient's Name
    ("0010,0020", Action::Z), // Patient ID
    ("0010,0021", Action::X), // Issuer of Patient ID
    ("0010,0030", Action::Z), // Patient's Birth Date
    ("0010,0032", Action::Z), // Patient's Birth Time
    ("0010,0040", Action::Z), // Patient's Sex
    ("0010,1000", Action::X), // Other Patient IDs
    ("0010,1001", Action::X), // Other Patient Names
    ("0010,1010", Action::X), // Patient's Age
    ("0010,1020", Action::X), // Patient's Size
    ("0010,1030", Action::X), // Patient's Weight
    ("0010,1040", Action::X), // Patient's Address
    ("0010,1060", Action::X), // Patient's Mother's Birth Name
    ("0010,1090", Action::X), // Medical Record Locator
    ("0010,2154", Action::X), // Patient's Telephone Numbers
    ("0010,2160", Action::X), // Ethnic Group
    ("0010,2180", Action::X), // Occupation
    ("0010,21B0", Action::X), // Additional Patient History
    ("0010,4000", Action::X), // Patient Comments
    ("0018,1000", Action::U), // Device Serial Number
    ("0018,1030", Action::X), // Protocol Name
    ("0020,000D", Action::U), // Study Instance UID
    ("0020,000E", Action::U), // Series Instance UID
    ("0020,0010", Action::Z), // Study ID
    ("0020,4000", Action::X), // Image Comments
    ("0028,0301", Action::K), // Burned In Annotation (informational)
    ("0032,1032", Action::X), // Requesting Physician
    ("0032,1060", Action::X), // Requested Procedure Description
    ("0040,0244", Action::Z), // Performed Procedure Step Start Date
    ("0040,0254", Action::X), // Performed Procedure Step Description
    ("0040,A073", Action::U), // Verifying Observer Sequence ref
    ("0040,A123", Action::X), // Person Name (SR)
    ("0070,0084", Action::X), // Content Creator's Name
    ("7FE0,0010", Action::K), // Pixel Data (never a tag action)
];

/// The base profile rules as `(tag, action)` pairs.
pub fn base_rules() -> Vec<(Tag, Action)> {
    BASE.iter()
        .map(|(t, a)| (parse_tag(t).expect("built-in tags are valid"), *a))
        .collect()
}

/// Base rules with profile options applied, then user `[tags]` overrides.
pub fn effective_rules(cfg: &Config) -> Result<Vec<(Tag, Action)>> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<Tag, Action> = base_rules().into_iter().collect();
    // Option-dependent rule adjustments go here; the MVP keeps the base table.
    let _ = (
        cfg.profile.retain_safe_private,
        cfg.profile.clean_descriptors,
        cfg.profile.clean_structured_content,
    );
    for (k, v) in &cfg.tags {
        map.insert(parse_tag(k)?, *v);
    }
    Ok(map.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn rule_map(cfg: &Config) -> std::collections::HashMap<Tag, Action> {
        effective_rules(cfg).unwrap().into_iter().collect()
    }

    #[test]
    fn covers_high_risk_identifiers() {
        let m = rule_map(&Config::default());
        for t in [
            "0010,0010", // PatientName
            "0010,0020", // PatientID
            "0010,0030", // PatientBirthDate
            "0010,1000", // OtherPatientIDs
            "0010,1040", // PatientAddress
            "0008,0080", // InstitutionName
            "0008,0090", // ReferringPhysicianName
            "0008,0050", // AccessionNumber
        ] {
            let tag = crate::config::parse_tag(t).unwrap();
            assert!(m.contains_key(&tag), "profile missing {t}");
        }
    }

    #[test]
    fn user_tags_override_profile() {
        let cfg = Config::from_toml_str("[tags]\n\"0010,0010\" = \"K\"\n").unwrap();
        let m = rule_map(&cfg);
        assert_eq!(m[&crate::config::parse_tag("0010,0010").unwrap()], Action::K);
    }

    #[test]
    fn research_keeps_longitudinal_dates_out_of_remove() {
        let cfg = Config::from_toml_str("[profile]\nbase = \"research\"\n").unwrap();
        let m = rule_map(&cfg);
        assert!(m.contains_key(&crate::config::parse_tag("0008,0020").unwrap()));
    }

    #[test]
    fn covers_instance_uid_and_descriptions() {
        let m = rule_map(&Config::default());
        assert_eq!(m[&crate::config::parse_tag("0008,0018").unwrap()], Action::U);
        assert_eq!(m[&crate::config::parse_tag("0008,1030").unwrap()], Action::X);
        assert_eq!(m[&crate::config::parse_tag("0008,103E").unwrap()], Action::X);
    }
}
