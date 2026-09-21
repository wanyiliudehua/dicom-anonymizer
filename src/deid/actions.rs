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

/// Apply the effective rules to `obj`, recursing into sequences.
/// Returns the number of elements changed.
pub fn apply(obj: &mut InMemDicomObject, ctx: &DeidContext) -> Result<usize> {
    let rules: std::collections::HashMap<Tag, Action> =
        effective_rules(ctx.cfg)?.into_iter().collect();
    let patient_id = ds::get_string(obj, Tag(0x0010, 0x0020)).unwrap_or_default();
    let offset = ctx.dates.offset_for(&patient_id);
    let mut changed = 0usize;

    for tag in ds::tag_list(obj) {
        let is_seq = obj
            .element(tag)
            .map(|e| e.vr() == VR::SQ)
            .unwrap_or(false);

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
                    if let Some(s) = shifted {
                        ds::set_string(obj, tag, vr, &s);
                        changed += 1;
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
                let value = dummy_for(vr, "ANONYMOUS");
                ds::set_string(obj, tag, vr, &value);
                changed += 1;
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
        DeidContext { cfg, uids: u, dates: d }
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
    fn date_shift_applied_and_malformed_left_alone() {
        let cfg =
            Config::from_toml_str("[profile]\nretain_longitudinal_temporal = true\n").unwrap();
        let u = UidMapper::new("2.25.", b"s");
        let d = DateShifter::new(b"s", 10);
        let c = ctx(&cfg, &u, &d);
        let mut o = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0008, 0x0020), VR::DA, "20240101"),
            DataElement::new(Tag(0x0008, 0x0021), VR::DA, "2024"),
        ]);
        apply(&mut o, &c).unwrap();
        let shifted = crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0020)).unwrap();
        assert_ne!(shifted, "20240101");
        assert_eq!(
            crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0021)).as_deref(),
            Some("2024")
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
    fn private_tags_removed_by_default_and_kept_with_option() {        let mk = |retain: bool| {
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
}
