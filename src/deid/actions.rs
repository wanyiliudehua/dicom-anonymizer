//! PS3.15 action engine: apply X/Z/D/U/C/K to a dataset, recursing sequences.

use crate::config::{Action, Config, DateStrategy, UidStrategy};
use crate::deid::dataset as ds;
use crate::deid::dates::DateShifter;
use crate::deid::profile::effective_rules;
use crate::deid::uids::UidMapper;
use crate::error::{AnonError, Result};
use dicom_core::value::Value;
use dicom_core::{Tag, VR};
use dicom_object::InMemDicomObject;

pub struct DeidContext<'a> {
    pub cfg: &'a Config,
    pub uids: &'a UidMapper,
    pub dates: &'a DateShifter,
}

/// A dummy value that is valid for common value representations (E.1-1 `D`).
fn dummy_for(vr: VR, fallback: &str) -> String {
    match vr {
        VR::DA => "19000101".to_string(),
        VR::DT => "19000101000000".to_string(),
        VR::TM => "000000".to_string(),
        _ => fallback.to_string(),
    }
}

/// E.1-1 repeating-group attributes that cannot be a single tag:
/// Curve Data `(50xx,xxxx)` and Overlay Data/Comments `(60xx,3000|4000)`.
fn repeating_group_action(tag: Tag) -> Option<Action> {
    let g = tag.0;
    if (0x5000..=0x50FF).contains(&g) {
        return Some(Action::X);
    }
    if (0x6000..=0x60FF).contains(&g) && (tag.1 == 0x3000 || tag.1 == 0x4000) {
        return Some(Action::X);
    }
    None
}

/// Identifiers grouped by the kind of attribute they came from, so the leak
/// check can ignore categories the user explicitly chose to retain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identifiers {
    pub patient: Vec<String>,
    pub names: Vec<String>,
    pub orgs: Vec<String>,
}

impl Identifiers {
    /// Every identifier, sorted and de-duplicated (the scrub set).
    pub fn all(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .patient
            .iter()
            .chain(&self.names)
            .chain(&self.orgs)
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn merge(&mut self, other: &Identifiers) {
        self.patient.extend(other.patient.iter().cloned());
        self.names.extend(other.names.iter().cloned());
        self.orgs.extend(other.orgs.iter().cloned());
    }
}

/// Attribute groups whose values identify the subject, used for text scrubbing.
const ID_PLAIN: &[Tag] = &[
    Tag(0x0010, 0x0020), // Patient ID
    Tag(0x0010, 0x1000), // Other Patient IDs
    Tag(0x0008, 0x0050), // Accession Number
];
const ID_NAMES: &[Tag] = &[
    Tag(0x0010, 0x0010), // Patient's Name
    Tag(0x0010, 0x1001), // Other Patient Names
    Tag(0x0008, 0x0090), // Referring Physician's Name
    Tag(0x0008, 0x1050), // Performing Physician's Name
    Tag(0x0008, 0x1070), // Operators' Name
];
const ID_ORGS: &[Tag] = &[
    Tag(0x0008, 0x0080), // Institution Name
    Tag(0x0008, 0x0081), // Institution Address
    Tag(0x0008, 0x1040), // Institutional Department Name
];

/// Identifying strings from this object, gathered *before* it is anonymized and
/// used to scrub free-text descriptor fields (the E.1-1 "Clean" options).
///
/// Names contribute their components; institution-like values also contribute
/// their (length-filtered) words, because descriptors often carry a *substring*
/// of the institution rather than the whole string.
pub fn collect_identifiers(obj: &InMemDicomObject) -> Identifiers {
    let (mut patient, mut names, mut orgs) = (Vec::new(), Vec::new(), Vec::new());
    for tag in ID_PLAIN {
        if let Some(v) = ds::get_string(obj, *tag) {
            add_identifier(&mut patient, &v, 3, false);
        }
    }
    for tag in ID_NAMES {
        if let Some(v) = ds::get_string(obj, *tag) {
            add_identifier(&mut names, &v, 3, true);
        }
    }
    for tag in ID_ORGS {
        if let Some(v) = ds::get_string(obj, *tag) {
            add_identifier(&mut orgs, &v, 4, true);
        }
    }
    Identifiers {
        patient,
        names,
        orgs,
    }
}

fn add_identifier(ids: &mut Vec<String>, raw: &str, min: usize, split: bool) {
    let s = raw.trim();
    if s.is_empty() {
        return;
    }
    // Two CJK characters are a whole name, while two ASCII characters are
    // usually a fragment ("Li" occurs inside "Ligament") - and matching is by
    // substring. So the ASCII floor stays put and dense scripts get a lower one.
    let ok = |t: &str| {
        let floor = if t.is_ascii() { min } else { 2 };
        t.chars().count() >= floor
    };
    if ok(s) {
        ids.push(s.to_string());
    }
    if split {
        for tok in s.split(|c: char| {
            c.is_whitespace() || matches!(c, '^' | '=' | ',' | ';' | '(' | ')' | '/')
        }) {
            let t = tok.trim();
            if ok(t) {
                ids.push(t.to_string());
            }
        }
    }
}

/// Remove each identifier (case-insensitive) from a text value. A value with no
/// match is returned unchanged.
fn scrub(text: &str, ids: &[String]) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let mut removed = false;
    for id in ids {
        // Match case-insensitively but keep the needle at its original character
        // count: `to_lowercase` can change how many characters a codepoint has,
        // so a span measured on a lowercased copy would remove the wrong range.
        let needle: Vec<char> = id.chars().collect();
        if needle.is_empty() {
            continue;
        }
        while let Some(pos) = find_ci(&chars, &needle) {
            chars.drain(pos..pos + needle.len());
            removed = true;
        }
    }
    if !removed {
        // Nothing matched, so the text is untouched: hand it back byte for byte
        // rather than normalizing whitespace we were not asked to touch.
        return text.to_string();
    }
    // Tidy only the separators a removal can expose. Collapsing whitespace with
    // `split_whitespace` would rewrite every character Rust calls whitespace, and
    // that set includes NEL (U+0085) and NBSP - which is exactly what undeclared
    // single-byte text decodes to. Doing so silently turns those bytes into spaces
    // and corrupts text the clean action was never asked to touch.
    chars
        .iter()
        .collect::<String>()
        .trim_matches(|c| c == ' ' || c == '^' || c == '\t')
        .to_string()
}

/// Case-insensitive, Unicode-aware, character-level substring search.
fn find_ci(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| {
        needle
            .iter()
            .zip(&hay[i..i + needle.len()])
            .all(|(n, h)| n.to_lowercase().eq(h.to_lowercase()))
    })
}

/// Apply the effective rules to `obj`, recursing into sequences.
/// Returns the number of elements changed.
pub fn apply(obj: &mut InMemDicomObject, ctx: &DeidContext) -> Result<usize> {
    let rules: std::collections::HashMap<Tag, Action> =
        effective_rules(ctx.cfg)?.into_iter().collect();
    let patient_id = ds::get_string(obj, Tag(0x0010, 0x0020)).unwrap_or_default();
    let identifiers = collect_identifiers(obj).all();
    let offset = ctx.dates.offset_for(&patient_id);
    let mut changed = 0usize;

    for tag in ds::tag_list(obj) {
        let is_seq = obj.element(tag).map(|e| e.vr() == VR::SQ).unwrap_or(false);

        // Private (odd-group) tags are removed by default; retained only when
        // the safe-private option is on.
        if tag.0 % 2 == 1 {
            if !ctx.cfg.profile.retain_safe_private && ds::remove(obj, tag) {
                changed += 1;
            }
            continue;
        }

        let action = rules
            .get(&tag)
            .copied()
            .or_else(|| repeating_group_action(tag));

        // An explicit X removes the element as a whole — including a sequence.
        if action == Some(Action::X) {
            if ds::remove(obj, tag) {
                changed += 1;
            }
            continue;
        }

        // Unruled / non-X sequences are recursed into rather than removed.
        if is_seq {
            changed += apply_to_sequence(obj, tag, ctx)?;
            continue;
        }

        let Some(action) = action else {
            continue;
        };
        if action == Action::K {
            continue;
        }

        let vr = obj.element(tag).map(|e| e.vr()).unwrap_or(VR::LO);

        // Temporal attributes: honor `remove`, and the "modified dates" option
        // (which resolves to `C`) by shifting rather than replacing.
        if matches!(vr, VR::DA | VR::DT | VR::TM) {
            if ctx.cfg.dates.strategy == DateStrategy::Remove {
                if ds::remove(obj, tag) {
                    changed += 1;
                }
                continue;
            }
            if action == Action::C {
                if vr == VR::TM {
                    continue; // times are preserved; only dates are shifted
                }
                if let Some(v) = ds::get_string(obj, tag) {
                    let shifted = match v.len() {
                        8 => ctx.dates.shift_da(&v, offset),
                        _ => ctx.dates.shift_dt(&v, offset),
                    };
                    match shifted {
                        Some(s) => {
                            ds::set_string(obj, tag, vr, &s);
                            changed += 1;
                        }
                        // The option promises the original date is not recoverable, so a value
                        // this code cannot shift is emptied rather than passed through - keeping
                        // it would be a silent hole in that promise, and an uncounted one.
                        None => {
                            if ds::zero(obj, tag) {
                                changed += 1;
                            }
                        }
                    }
                }
                continue;
            }
        }

        match action {
            Action::Z => {
                if ds::zero(obj, tag) {
                    changed += 1;
                }
            }
            Action::D => {
                let value = if vr == VR::UI {
                    ctx.uids.map(&format!("{:04X}{:04X}", tag.0, tag.1))
                } else {
                    let fallback = ctx
                        .cfg
                        .dummies
                        .get(&format!("{:04X},{:04X}", tag.0, tag.1))
                        .cloned()
                        .unwrap_or_else(|| "ANONYMOUS".to_string());
                    dummy_for(vr, &fallback)
                };
                ds::set_string(obj, tag, vr, &value);
                changed += 1;
            }
            Action::U => {
                if ctx.cfg.uids.strategy == UidStrategy::Zero {
                    if ds::zero(obj, tag) {
                        changed += 1;
                    }
                } else if let Some(v) = ds::get_string(obj, tag) {
                    let mapped: Vec<String> = v.split('\\').map(|u| ctx.uids.map(u)).collect();
                    ds::set_multi_string(obj, tag, mapped);
                    changed += 1;
                }
            }
            Action::C => {
                match ds::get_string(obj, tag) {
                    // Clean: drop this object's own identifiers, keep the rest.
                    Some(text) => {
                        let cleaned = scrub(&text, &identifiers);
                        ds::set_string(obj, tag, vr, &cleaned);
                        changed += 1;
                    }
                    // Not a text value: neutralize instead.
                    None => {
                        let value = dummy_for(vr, "ANONYMOUS");
                        ds::set_string(obj, tag, vr, &value);
                        changed += 1;
                    }
                }
            }
            Action::X | Action::K => {}
        }
    }
    Ok(changed)
}

/// Recurse into an SQ element, mutating items in place. Returns changed count.
fn apply_to_sequence(obj: &mut InMemDicomObject, tag: Tag, ctx: &DeidContext) -> Result<usize> {
    let mut changed = 0usize;
    let mut failure: Option<AnonError> = None;
    obj.update_value_at(tag, |value| {
        if failure.is_some() {
            return;
        }
        if let Value::Sequence(seq) = value {
            for item in seq.items_mut() {
                match apply(item, ctx) {
                    Ok(n) => changed += n,
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
        }
    })
    .map_err(|e| AnonError::Dicom(format!("sequence {tag:?}: {e}")))?;
    if let Some(e) = failure {
        return Err(e);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::deid::dates::DateShifter;
    use crate::deid::uids::UidMapper;
    use dicom_core::{DataElement, Tag, VR};
    use dicom_object::InMemDicomObject;

    fn ctx<'a>(cfg: &'a Config, u: &'a UidMapper, d: &'a DateShifter) -> DeidContext<'a> {
        DeidContext {
            cfg,
            uids: u,
            dates: d,
        }
    }

    #[test]
    fn removes_and_zeroes_identifiers() {
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);

        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Doe^John"),
            DataElement::new(Tag(0x0008, 0x0080), VR::LO, "General Hospital"),
        ]);
        let n = apply(&mut o, &c).unwrap();
        assert!(n >= 2);
        assert_ne!(
            crate::deid::dataset::get_string(&o, Tag(0x0010, 0x0010)).as_deref(),
            Some("Doe^John")
        );
        assert_ne!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0080)).as_deref(),
            Some("General Hospital")
        );
    }

    #[test]
    fn remaps_uids_and_keeps_consistency() {
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0020, 0x000D), VR::UI, "1.2.3"),
            DataElement::new(Tag(0x0020, 0x000E), VR::UI, "1.2.3"),
        ]);
        apply(&mut o, &c).unwrap();
        let a = crate::deid::dataset::get_string(&o, Tag(0x0020, 0x000D)).unwrap();
        let b = crate::deid::dataset::get_string(&o, Tag(0x0020, 0x000E)).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("2.25."));
    }

    #[test]
    fn date_shift_applied_and_unshiftable_dates_cleared() {
        // Under the Modified Dates option a date must not survive as-is: the point of the
        // option is that the original date is not recoverable. "I could not parse it" used
        // to mean "keep the original", which was a silent hole in that promise - and because
        // nothing was written, tags_changed did not even record it.
        let cfg =
            Config::from_toml_str("[profile]\nretain_longitudinal_temporal = true\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0008, 0x0020), VR::DA, "20240101"),
            DataElement::new(Tag(0x0008, 0x0021), VR::DA, "2024"),
            DataElement::new(Tag(0x0008, 0x002A), VR::DT, "2024"),
            DataElement::new(Tag(0x0008, 0x0030), VR::TM, "120000"),
        ]);
        apply(&mut o, &c).unwrap();

        let shifted = crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0020)).unwrap();
        assert_ne!(shifted, "20240101");
        // unshiftable values are emptied rather than passed through
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0021)).as_deref(),
            Some("")
        );
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x002A)).as_deref(),
            Some("")
        );
        // times are retained by this option; only dates are shifted
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0030)).as_deref(),
            Some("120000")
        );
    }

    #[test]
    fn multi_valued_uid_is_remapped_without_loss() {
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x1155),
            VR::UI,
            "1.2.3\\1.2.4",
        )]);
        apply(&mut o, &c).unwrap();
        let v = crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1155)).unwrap();
        assert_eq!(v.split('\\').count(), 2);
        for part in v.split('\\') {
            assert!(part.starts_with("2.25."));
        }
    }

    #[test]
    fn uid_strategy_zero_empties_uids() {
        let cfg = Config::from_toml_str("[uids]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0020, 0x000D),
            VR::UI,
            "1.2.3",
        )]);
        apply(&mut o, &c).unwrap();
        let v = crate::deid::dataset::get_string(&o, Tag(0x0020, 0x000D));
        assert_ne!(v.as_deref(), Some("1.2.3"));
        assert!(!v.unwrap_or_default().starts_with("2.25."));
    }

    #[test]
    fn date_strategy_remove_deletes_date_tag() {
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"remove\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x0020),
            VR::DA,
            "20240101",
        )]);
        apply(&mut o, &c).unwrap();
        assert!(o.element(Tag(0x0008, 0x0020)).is_err());
    }

    #[test]
    fn private_tags_removed_by_default_and_kept_with_option() {
        let mk = |retain: bool| {
            Config::from_toml_str(&format!(
                "[dates]\nstrategy = \"zero\"\n[profile]\nretain_safe_private = {retain}\n"
            ))
            .unwrap()
        };
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);

        let cfg = mk(false);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0009, 0x0010),
            VR::LO,
            "VENDOR",
        )]);
        apply(&mut o, &c).unwrap();
        assert!(o.element(Tag(0x0009, 0x0010)).is_err());

        let cfg2 = mk(true);
        let c2 = ctx(&cfg2, &u, &d);
        let mut o2 = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0009, 0x0010),
            VR::LO,
            "VENDOR",
        )]);
        apply(&mut o2, &c2).unwrap();
        assert!(o2.element(Tag(0x0009, 0x0010)).is_ok());

        // No allow-list is consulted, so even an obviously identifying private element
        // survives. That is why the flag is documented as an approximation of the PS3.15
        // option rather than as an implementation of it, and why main warns when it is on.
        let mut o3 = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0009, 0x1001),
            VR::LO,
            "Doe^John",
        )]);
        apply(&mut o3, &c2).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o3, Tag(0x0009, 0x1001)).as_deref(),
            Some("Doe^John"),
            "the safe-private option is not a safety filter"
        );
    }

    #[test]
    fn sequence_with_x_rule_is_removed_whole() {
        use dicom_core::value::DataSetSequence;
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let inner = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0040, 0x0006),
            VR::PN,
            "Dr^Who",
        )]);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0040, 0x0275),
            VR::SQ,
            DataSetSequence::from(vec![inner]),
        )]);
        apply(&mut o, &c).unwrap();
        assert!(
            o.element(Tag(0x0040, 0x0275)).is_err(),
            "a sequence with an X rule must be removed as a whole"
        );
    }

    #[test]
    fn unruled_sequence_is_kept_and_recursed() {
        use dicom_core::value::DataSetSequence;
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let inner = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x1155),
            VR::UI,
            "1.2.3",
        )]);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x1115),
            VR::SQ,
            DataSetSequence::from(vec![inner]),
        )]);
        apply(&mut o, &c).unwrap();
        // unruled sequence is kept (recursed into, not removed)
        assert!(o.element(Tag(0x0008, 0x1115)).is_ok());
    }

    #[test]
    fn dummy_action_writes_vr_valid_value() {
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        // Series Date is `D` in E.1-1; a dummy DA must be a valid date.
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x0021),
            VR::DA,
            "20240101",
        )]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0021)).as_deref(),
            Some("19000101")
        );
    }

    #[test]
    fn repeating_group_overlay_data_is_removed() {
        use dicom_core::PrimitiveValue;
        let cfg = Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x6000, 0x3000),
            VR::OW,
            PrimitiveValue::U16(vec![1u16].into()),
        )]);
        apply(&mut o, &c).unwrap();
        assert!(o.element(Tag(0x6000, 0x3000)).is_err());
    }

    #[test]
    fn full_dates_option_keeps_dates_verbatim() {
        let cfg =
            Config::from_toml_str("[profile]\nretain_longitudinal_full_dates = true\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x0020),
            VR::DA,
            "20240101",
        )]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0020)).as_deref(),
            Some("20240101")
        );
    }

    #[test]
    fn clean_action_scrubs_identifiers_and_keeps_text() {
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        // Institution is the identifier to scrub; it is collected before removal.
        // The descriptor carries only a substring of it, so tokens must match too.
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(
                Tag(0x0008, 0x0080),
                VR::LO,
                "WanNan Medical College YiJiShan Hospital",
            ),
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "YiJiShan Hospital^l-spine"),
            DataElement::new(Tag(0x0008, 0x103E), VR::LO, "localizer"),
        ]);
        apply(&mut o, &c).unwrap();
        // identifier stripped, useful text kept
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("l-spine")
        );
        // no identifier present -> unchanged
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x103E)).as_deref(),
            Some("localizer")
        );
    }

    #[test]
    fn clean_leaves_unmatched_text_byte_for_byte() {
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        // No identifier occurs here, so the value must survive exactly as-is:
        // cleaning is not a licence to rewrite text it did not need to touch.
        let mut o = InMemDicomObject::from_element_iter([DataElement::new(
            Tag(0x0008, 0x1030),
            VR::LO,
            "l-spine  T2  w",
        )]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("l-spine  T2  w")
        );
    }

    #[test]
    fn clean_scrubs_non_ascii_identifiers() {
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "程英平"),
            DataElement::new(Tag(0x0008, 0x0080), VR::LO, "万楠医学院附属医院"),
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "万楠医学院附属医院^腰椎"),
        ]);
        apply(&mut o, &c).unwrap();
        // The CJK institution is stripped and the clinical text survives.
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("腰椎")
        );
    }

    #[test]
    fn clean_handles_short_cjk_names() {
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        // Two CJK characters are a whole name, so the ASCII length floor of 3
        // must not be applied to them.
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "李伟"),
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "李伟^腰椎"),
        ]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("腰椎")
        );
    }

    #[test]
    fn clean_is_case_insensitive_for_non_ascii() {
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Иванов"),
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "иванов И.И."),
        ]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("И.И.")
        );
    }

    #[test]
    fn short_ascii_tokens_are_not_scrubbed() {
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        // Matching is by substring, so a 2-character ASCII token such as "Li"
        // would tear "Ligament" apart. Keep the ASCII floor at 3.
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Li^Wei"),
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "Ligament tear"),
        ]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("Ligament tear")
        );
    }

    #[test]
    fn clean_does_not_rewrite_characters_rust_calls_whitespace() {
        // Text that declares no character set is decoded byte-wise, so byte 0x85
        // becomes NEL (U+0085) - which `char::is_whitespace` accepts. Collapsing
        // whitespace would rewrite that byte into a space, corrupting the value.
        let cfg = Config::from_toml_str(
            "[dates]\nstrategy = \"zero\"\n[profile]\nclean_descriptors = true\n",
        )
        .unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Doe"),
            DataElement::new(Tag(0x0008, 0x1030), VR::LO, "Doe\u{85}x\u{a0}y"),
        ]);
        apply(&mut o, &c).unwrap();
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x1030)).as_deref(),
            Some("\u{85}x\u{a0}y"),
            "NEL/NBSP must survive a clean untouched"
        );
    }
}
