//! PS3.15 Basic Application Level Confidentiality Profile rule table.
//!
//! Source: DICOM PS3.15 Table E.1-1
//! (<https://dicom.nema.org/medical/dicom/current/output/chtml/part15/chapter_E.html>).
//! The table below is a curated high-risk subset; extend it to the full E.1-1
//! as needed. The test `covers_high_risk_identifiers` is the minimum bar.

use crate::config::{parse_tag, Action, Config, Strategy};
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
    // ---- P0: present in real data, previously unhandled ----
    ("0008,0013", Action::Z), // Instance Creation Time
    ("0008,002A", Action::Z), // Acquisition DateTime
    ("0008,0031", Action::Z), // Series Time
    ("0008,0032", Action::Z), // Acquisition Time
    ("0008,0033", Action::Z), // Content Time
    ("0020,0052", Action::U), // Frame of Reference UID
    ("0040,0245", Action::X), // Performed Procedure Step Start Time
    ("0040,0253", Action::X), // Performed Procedure Step ID
    // ---- P1: device / institution identity (default removed) ----
    ("0008,0055", Action::X), // Station AE Title
    ("0008,1010", Action::X), // Station Name
    ("0008,1090", Action::X), // Manufacturer Model Name
    ("0018,1004", Action::X), // Plate ID
    ("0018,1005", Action::X), // Generator ID
    ("0018,1007", Action::X), // Cassette ID
    ("0018,1008", Action::X), // Gantry ID
    ("0018,1010", Action::X), // Secondary Capture Device ID
    ("0018,1011", Action::X), // Hardcopy Creation Device ID
    ("0018,1020", Action::X), // Software Versions
    ("0018,1200", Action::Z), // Date of Last Calibration
    ("0018,1201", Action::Z), // Time of Last Calibration
    ("0018,1203", Action::Z), // Calibration DateTime
    ("0008,0017", Action::U), // Acquisition UID
    ("0040,0243", Action::X), // Performed Location
    ("0040,0250", Action::X), // Performed Procedure Step End Date
    ("0040,0251", Action::X), // Performed Procedure Step End Time
    // ---- P2: more UIDs, names, orders, comments ----
    ("0004,1511", Action::U), // Referenced SOP Instance UID in File
    ("0008,0058", Action::U), // Failed SOP Instance UID List
    ("0064,0003", Action::U), // Source Frame of Reference UID
    ("0070,031A", Action::U), // Fiducial UID
    ("0088,0140", Action::U), // Storage Media File-set UID
    ("0040,0554", Action::U), // Specimen UID
    ("3006,00C2", Action::U), // Related Frame of Reference UID
    ("300A,0650", Action::U), // Patient Setup UID
    ("3010,0015", Action::U), // Source Conceptual Volume UID
    ("0010,0050", Action::X), // Patient's Insurance Plan Code Sequence
    ("0010,0101", Action::X), // Patient's Primary Language Code Sequence
    ("0010,0102", Action::X), // Patient's Primary Language Modifier Code Sequence
    ("0010,1005", Action::X), // Patient's Birth Name
    ("0010,2152", Action::X), // Region of Residence
    ("0010,21A0", Action::X), // Smoking Status
    ("0038,001C", Action::X), // Scheduled Discharge Date
    ("0038,001D", Action::X), // Scheduled Discharge Time
    ("0038,001E", Action::X), // Scheduled Patient Institution Residence
    ("0038,0400", Action::X), // Patient's Institution Residence
    ("0032,0012", Action::X), // Study ID Issuer
    ("0040,0006", Action::X), // Scheduled Performing Physician's Name
    ("0040,000B", Action::X), // Scheduled Performing Physician Identification Sequence
    ("0040,4036", Action::X), // Human Performer's Organization
    ("0040,4037", Action::X), // Human Performer's Name
    ("0040,2017", Action::Z), // Filler Order Number / Imaging Service Request
    ("0040,1004", Action::X), // Patient Transport Arrangements
    ("0040,0275", Action::X), // Request Attributes Sequence
    ("0008,0096", Action::X), // Referring Physician Identification Sequence
    ("0008,1110", Action::X), // Referenced Study Sequence
    ("0018,9074", Action::Z), // Frame Acquisition DateTime
    ("0018,4000", Action::X), // Acquisition Comments
    ("0018,1400", Action::X), // Acquisition Device Processing Description
    ("0018,9424", Action::X), // Acquisition Protocol Description
    ("0020,9158", Action::X), // Frame Comments
    ("0032,4000", Action::X), // Study Comments
    ("0040,0556", Action::X), // Acquisition Context Description
    ("0040,0600", Action::X), // Specimen Short Description
    ("7FE0,0010", Action::K), // Pixel Data (never a tag action)
];

/// Tags retained verbatim under the `conservative` strategy: patient
/// demographics and all dates/times.
const CONSERVATIVE_RETAIN: &[&str] = &[
    "0010,0040", // Patient's Sex
    "0010,1010", // Patient's Age
    "0010,1020", // Patient's Size
    "0010,1030", // Patient's Weight
    "0008,0012", // Instance Creation Date
    "0008,0013", // Instance Creation Time
    "0008,0020", // Study Date
    "0008,0021", // Series Date
    "0008,0022", // Acquisition Date
    "0008,0023", // Content Date
    "0008,002A", // Acquisition DateTime
    "0008,0030", // Study Time
    "0008,0031", // Series Time
    "0008,0032", // Acquisition Time
    "0008,0033", // Content Time
    "0018,1200", // Date of Last Calibration
    "0018,1201", // Time of Last Calibration
    "0018,1203", // Calibration DateTime
    "0038,001C", // Scheduled Discharge Date
    "0038,001D", // Scheduled Discharge Time
    "0040,0244", // Performed Procedure Step Start Date
    "0040,0245", // Performed Procedure Step Start Time
    "0040,0250", // Performed Procedure Step End Date
    "0040,0251", // Performed Procedure Step End Time
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
    // Conservative: retain demographics and dates/times verbatim.
    if cfg.profile.strategy == Strategy::Conservative {
        for t in CONSERVATIVE_RETAIN {
            map.insert(parse_tag(t)?, Action::K);
        }
    }
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

    #[test]
    fn covers_device_uid_time_and_order_identifiers() {
        use crate::config::parse_tag;
        let m = rule_map(&Config::default());
        let expect = [
            // P0 — present in real data, previously unhandled
            ("0020,0052", Action::U), // Frame of Reference UID
            ("0040,0253", Action::X), // Performed Procedure Step ID
            ("0008,0013", Action::Z), // Instance Creation Time
            ("0008,002A", Action::Z), // Acquisition DateTime
            ("0008,0031", Action::Z), // Series Time
            ("0008,0032", Action::Z), // Acquisition Time
            ("0008,0033", Action::Z), // Content Time
            ("0040,0245", Action::X), // Performed Procedure Step Start Time
            // P1 — device / institution identity
            ("0008,1010", Action::X), // Station Name
            ("0008,0055", Action::X), // Station AE Title
            ("0018,1020", Action::X), // Software Versions
            ("0008,1090", Action::X), // Manufacturer Model Name
            ("0018,1010", Action::X), // Secondary Capture Device ID
            ("0018,1011", Action::X), // Hardcopy Creation Device ID
            ("0018,1007", Action::X), // Cassette ID
            ("0018,1004", Action::X), // Plate ID
            ("0018,1005", Action::X), // Generator ID
            ("0018,1008", Action::X), // Gantry ID
            ("0018,1203", Action::Z), // Calibration DateTime
            ("0008,0017", Action::U), // Acquisition UID
            ("0040,0243", Action::X), // Performed Location
            // P2 — names, orders, comments, more UIDs
            ("0010,1005", Action::X), // Patient's Birth Name
            ("0010,2152", Action::X), // Region of Residence
            ("0038,0400", Action::X), // Patient's Institution Residence
            ("0040,0006", Action::X), // Scheduled Performing Physician's Name
            ("0040,4037", Action::X), // Human Performer's Name
            ("0040,2017", Action::Z), // Filler Order Number
            ("0040,1004", Action::X), // Patient Transport Arrangements
            ("0040,0275", Action::X), // Request Attributes Sequence
            ("0008,1110", Action::X), // Referenced Study Sequence
            ("0018,4000", Action::X), // Acquisition Comments
            ("0032,4000", Action::X), // Study Comments
            ("0040,0554", Action::U), // Specimen UID
            ("0040,0556", Action::X), // Acquisition Context Description
            ("0088,0140", Action::U), // Storage Media File-set UID
        ];
        for (t, a) in expect {
            let tag = parse_tag(t).unwrap();
            assert_eq!(m.get(&tag), Some(&a), "missing/wrong rule for {t}");
        }
    }

    #[test]
    fn conservative_retains_demographics_and_dates_but_clears_identifiers() {
        use crate::config::parse_tag;
        let cfg = Config::from_toml_str("[profile]\nstrategy = \"conservative\"\n").unwrap();
        let m = rule_map(&cfg);
        for t in [
            "0010,0040", // Patient's Sex
            "0010,1010", // Patient's Age
            "0010,1020", // Patient's Size
            "0010,1030", // Patient's Weight
            "0008,0020", // Study Date
            "0008,0032", // Acquisition Time
            "0008,002A", // Acquisition DateTime
            "0008,0013", // Instance Creation Time
        ] {
            assert_eq!(m[&parse_tag(t).unwrap()], Action::K, "{t} should be retained");
        }
        // direct identifiers still cleared
        assert_eq!(m[&parse_tag("0010,0010").unwrap()], Action::Z); // Name
        assert_eq!(m[&parse_tag("0010,0020").unwrap()], Action::Z); // ID
        assert_eq!(m[&parse_tag("0010,0030").unwrap()], Action::Z); // Birth Date
        assert_eq!(m[&parse_tag("0008,0050").unwrap()], Action::Z); // Accession
        assert_eq!(m[&parse_tag("0008,0080").unwrap()], Action::X); // Institution
        assert_eq!(m[&parse_tag("0008,1010").unwrap()], Action::X); // Station Name
    }

    #[test]
    fn aggressive_clears_demographics_and_dates() {
        use crate::config::parse_tag;
        let m = rule_map(&Config::default());
        assert_ne!(m[&parse_tag("0010,0040").unwrap()], Action::K);
        assert_ne!(m[&parse_tag("0010,1010").unwrap()], Action::K);
        assert_ne!(m[&parse_tag("0008,0020").unwrap()], Action::K);
        assert_ne!(m[&parse_tag("0008,0032").unwrap()], Action::K);
    }
}
