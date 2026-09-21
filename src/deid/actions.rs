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

const DATE_TAGS: &[Tag] = &[
    Tag(0x0008, 0x0012), // Instance Creation Date
    Tag(0x0008, 0x0020), // Study Date
    Tag(0x0008, 0x0021), // Series Date
    Tag(0x0008, 0x0022), // Acquisition Date
    Tag(0x0008, 0x0023), // Content Date
    Tag(0x0008, 0x002A), // Acquisition DateTime
    Tag(0x0008, 0x0030), // Study Time
];

fn is_date_tag(tag: Tag) -> bool {
    DATE_TAGS.contains(&tag)
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

        let action = rules.get(&tag).copied();

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

        // Date handling takes precedence for date tags.
        if is_date_tag(tag) {
            match ctx.cfg.dates.strategy {
                DateStrategy::Remove => {
                    if ds::remove(obj, tag) {
                        changed += 1;
                    }
                    continue;
                }
                DateStrategy::Shift
                    if ctx.cfg.profile.retain_longitudinal_temporal
                        || action != Action::Z =>
                {
                    if let Some(v) = ds::get_string(obj, tag) {
                        let shifted = match v.len() {
                            8 => ctx.dates.shift_da(&v, offset),
                            _ => ctx.dates.shift_dt(&v, offset),
                        };
                        if let Some(s) = shifted {
                            ds::set_string(obj, tag, VR::DA, &s);
                            changed += 1;
                        }
                    }
                    // Under shifting a date tag is either shifted or left as-is;
                    // malformed dates pass through instead of falling through to zeroing.
                    continue;
                }
                _ => {}
            }
        }

        match action {
            Action::Z => {
                if ds::zero(obj, tag) {
                    changed += 1;
                }
            }
            Action::D => {
                let dummy = ctx
                    .cfg
                    .dummies
                    .get(&format!("{:04X},{:04X}", tag.0, tag.1))
                    .cloned()
                    .unwrap_or_else(|| "ANONYMOUS".to_string());
                let vr = obj.element(tag).map(|e| e.vr()).unwrap_or(VR::LO);
                ds::set_string(obj, tag, vr, &dummy);
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
                let vr = obj.element(tag).map(|e| e.vr()).unwrap_or(VR::LO);
                ds::set_string(obj, tag, vr, "ANONYMOUS");
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
    obj.update_value_at(tag, |value| {
        if let Value::Sequence(seq) = value {
            for item in seq.items_mut() {
                if let Ok(n) = apply(item, ctx) {
                    changed += n;
                }
            }
        }
    })
    .map_err(|e| AnonError::Dicom(format!("sequence {tag:?}: {e}")))?;
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
        assert_eq!(crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0080)), None);
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
            Tag(0x0008, 0x1140),
            VR::SQ,
            DataSetSequence::from(vec![inner]),
        )]);
        apply(&mut o, &c).unwrap();
        // unruled sequence is kept (recursed into, not removed)
        assert!(o.element(Tag(0x0008, 0x1140)).is_ok());
    }
}
