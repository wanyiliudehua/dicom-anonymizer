//! PS3.15 rule assembly.
//!
//! The base table is the **full** PS3.15 Table E.1-1 Basic Application Level
//! Confidentiality Profile in `e1_1.rs` (generated from the standard by
//! `scripts/gen_e1_1.py`). `OVERRIDES` adds the few attributes the standard
//! table does not carry and pins operationally-required tags.

use crate::config::{parse_tag, Action, Config, Strategy};
use crate::deid::e1_1::E11;
use crate::error::Result;
use dicom_core::Tag;

/// Applied after the E.1-1 table (a later entry for the same tag wins).
const OVERRIDES: &[(&str, Action)] = &[
    ("0008,0016", Action::K), // SOP Class UID — identifies the IOD, always kept
    ("0008,0070", Action::X), // Manufacturer (not listed in E.1-1)
    ("0008,1090", Action::X), // Manufacturer Model Name (not listed in E.1-1)
    ("0018,1020", Action::X), // Software Versions (not listed in E.1-1)
    ("0028,0301", Action::K), // Burned In Annotation — needed for masking
    ("7FE0,0010", Action::K), // Pixel Data — never a tag action
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

/// Whether a PS3.15 option (E.1-1 column key) is enabled in the config.
fn option_enabled(cfg: &Config, key: &str) -> bool {
    match key {
        "rtn_safe_priv" => cfg.profile.retain_safe_private,
        "rtn_uids" => cfg.profile.retain_uids,
        "rtn_dev_id" => cfg.profile.retain_device_identity,
        "rtn_inst_id" => cfg.profile.retain_institution_identity,
        "rtn_pat_chars" => cfg.profile.retain_patient_characteristics,
        "rtn_long_full_dates" => cfg.profile.retain_longitudinal_full_dates,
        "rtn_long_modif_dates" => cfg.profile.retain_longitudinal_temporal,
        "clean_desc" => cfg.profile.clean_descriptors,
        "clean_struct" => cfg.profile.clean_structured_content,
        "clean_graph" => cfg.profile.clean_graphics,
        _ => false,
    }
}

/// Resolve one attribute's action: the E.1-1 Basic action, overridden by every
/// enabled option column (in the table's column order; later wins).
fn resolve(basic: Action, opts: &[(&str, Action)], cfg: &Config) -> Action {
    let mut action = basic;
    for (key, value) in opts {
        if option_enabled(cfg, key) {
            action = *value;
        }
    }
    action
}

/// The full E.1-1 table (with option resolution) plus the pinned overrides.
pub fn base_rules(cfg: &Config) -> Result<Vec<(Tag, Action)>> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut map: BTreeMap<Tag, Action> = BTreeMap::new();
    let conservative = cfg.profile.strategy == Strategy::Conservative;
    let mut retain: BTreeSet<Tag> = BTreeSet::new();
    if conservative {
        for t in CONSERVATIVE_RETAIN {
            retain.insert(parse_tag(t)?);
        }
    }
    for (tag, basic, opts) in E11 {
        let t = parse_tag(tag)?;
        // The strategy adjusts the *basic* action rather than overwriting the resolved one,
        // so an explicitly enabled option still wins over it. Overwriting afterwards meant a
        // user who asked for shifted dates silently got verbatim dates instead.
        let base = if conservative && retain.contains(&t) {
            Action::K
        } else {
            *basic
        };
        map.insert(t, resolve(base, opts, cfg));
    }
    for (t, a) in OVERRIDES {
        map.insert(parse_tag(t)?, *a);
    }
    Ok(map.into_iter().collect())
}

/// Base rules with the user `[tags]` overrides applied.
pub fn effective_rules(cfg: &Config) -> Result<Vec<(Tag, Action)>> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<Tag, Action> = base_rules(cfg)?.into_iter().collect();
    // User overrides win over everything.
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
    fn e1_1_table_is_ported_and_has_anchors() {
        assert!(
            E11.len() >= 600,
            "E.1-1 table looks truncated: {} rows",
            E11.len()
        );
        let m = rule_map(&Config::default());
        use crate::config::parse_tag;
        assert_eq!(m[&parse_tag("0010,0010").unwrap()], Action::Z); // Patient's Name
        assert_eq!(m[&parse_tag("0020,000D").unwrap()], Action::U); // Study Instance UID
        assert_eq!(m[&parse_tag("0020,000E").unwrap()], Action::U); // Series Instance UID
        assert_eq!(m[&parse_tag("0008,0018").unwrap()], Action::U); // SOP Instance UID
        assert_eq!(m[&parse_tag("0008,0020").unwrap()], Action::Z); // Study Date
        assert_eq!(m[&parse_tag("0008,0050").unwrap()], Action::Z); // Accession Number
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
        assert_eq!(
            m[&crate::config::parse_tag("0010,0010").unwrap()],
            Action::K
        );
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
        assert_eq!(
            m[&crate::config::parse_tag("0008,0018").unwrap()],
            Action::U
        );
        assert_eq!(
            m[&crate::config::parse_tag("0008,1030").unwrap()],
            Action::X
        );
        assert_eq!(
            m[&crate::config::parse_tag("0008,103E").unwrap()],
            Action::X
        );
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
            ("0008,0031", Action::D), // Series Time [X/D]
            ("0008,0032", Action::Z), // Acquisition Time
            ("0008,0033", Action::Z), // Content Time
            ("0040,0245", Action::X), // Performed Procedure Step Start Time
            // P1 — device / institution identity
            ("0008,1010", Action::Z), // Station Name [X/Z/D]
            ("0008,0055", Action::X), // Station AE Title
            ("0018,1020", Action::X), // Software Versions (override)
            ("0008,1090", Action::X), // Manufacturer Model Name (override)
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
            ("0008,1110", Action::Z), // Referenced Study Sequence
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
            assert_eq!(
                m[&parse_tag(t).unwrap()],
                Action::K,
                "{t} should be retained"
            );
        }
        // direct identifiers still cleared
        assert_eq!(m[&parse_tag("0010,0010").unwrap()], Action::Z); // Name
        assert_eq!(m[&parse_tag("0010,0020").unwrap()], Action::Z); // ID
        assert_eq!(m[&parse_tag("0010,0030").unwrap()], Action::Z); // Birth Date
        assert_eq!(m[&parse_tag("0008,0050").unwrap()], Action::Z); // Accession
        assert_eq!(m[&parse_tag("0008,0080").unwrap()], Action::Z); // Institution
        assert_eq!(m[&parse_tag("0008,1010").unwrap()], Action::Z); // Station Name
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

    #[test]
    fn an_explicit_date_option_beats_the_conservative_default() {
        // The strategy supplies the *default* action, so an explicitly enabled option must
        // still win. Otherwise a user who asks for shifted dates silently gets verbatim
        // dates - the less protective of the two choices - just because the strategy also
        // has something to say about dates.
        use crate::config::parse_tag;
        let both = Config::from_toml_str(
            "[profile]\nstrategy = \"conservative\"\nretain_longitudinal_temporal = true\n",
        )
        .unwrap();
        assert_eq!(
            rule_map(&both)[&parse_tag("0008,0020").unwrap()],
            Action::C,
            "the Modified Dates option must still shift"
        );

        // The conservative default is unchanged when no option speaks.
        let alone = Config::from_toml_str("[profile]\nstrategy = \"conservative\"\n").unwrap();
        assert_eq!(
            rule_map(&alone)[&parse_tag("0008,0020").unwrap()],
            Action::K
        );
        assert_eq!(
            rule_map(&alone)[&parse_tag("0010,0040").unwrap()],
            Action::K,
            "demographics are still retained"
        );
    }

    #[test]
    fn options_override_basic_actions() {
        use crate::config::parse_tag;
        // (flag, tag, basic action, action with the option enabled)
        let cases = [
            (
                "retain_institution_identity",
                "0008,0080",
                Action::Z,
                Action::K,
            ),
            ("retain_uids", "0020,000D", Action::U, Action::K),
            ("retain_device_identity", "0018,1000", Action::Z, Action::K),
            (
                "retain_patient_characteristics",
                "0010,0040",
                Action::Z,
                Action::K,
            ),
            (
                "retain_longitudinal_full_dates",
                "0008,0020",
                Action::Z,
                Action::K,
            ),
            (
                "retain_longitudinal_temporal",
                "0008,0020",
                Action::Z,
                Action::C,
            ),
            ("clean_descriptors", "0008,1030", Action::X, Action::C),
        ];
        for (flag, tag, basic, with_option) in cases {
            let t = parse_tag(tag).unwrap();
            assert_eq!(rule_map(&Config::default())[&t], basic, "basic for {tag}");
            let cfg = Config::from_toml_str(&format!("[profile]\n{flag} = true\n")).unwrap();
            assert_eq!(rule_map(&cfg)[&t], with_option, "{flag} on {tag}");
        }
    }
}
