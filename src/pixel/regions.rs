//! Select configured pixel regions that apply to a given image.

use crate::config::{Config, Frames, Region};

/// Regions whose modality / SOP Class filters match (unset filters match all).
pub fn select<'a>(
    cfg: &'a Config,
    modality: Option<&str>,
    sop_class_uid: Option<&str>,
) -> Vec<&'a Region> {
    cfg.pixels
        .regions
        .iter()
        .filter(|r| match (&r.modality, modality) {
            (Some(want), Some(have)) => want == have,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .filter(|r| match (&r.sop_class_uid, sop_class_uid) {
            (Some(want), Some(have)) => want == have,
            (Some(_), None) => false,
            (None, _) => true,
        })
        .collect()
}

/// Expand a region's frame selector to concrete, in-range frame indices.
pub fn frames_for(region: &Region, total: u32) -> Vec<u32> {
    match &region.frames {
        Frames::All => (0..total).collect(),
        Frames::List(v) => v.iter().copied().filter(|f| *f < total).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn cfg() -> Config {
        Config::from_toml_str(
            "[[pixels.regions]]\nshape=\"rect\"\nwidth=1\nheight=1\nmodality=\"US\"\n\
             [[pixels.regions]]\nshape=\"rect\"\nwidth=1\nheight=1\nsop_class_uid=\"1.2.3\"\n\
             [[pixels.regions]]\nshape=\"rect\"\nwidth=1\nheight=1\n",
        )
        .unwrap()
    }

    #[test]
    fn modality_filter_matches_only_us() {
        let c = cfg();
        let r = select(&c, Some("US"), None);
        assert_eq!(r.len(), 2); // US rule + unconditional rule
    }

    #[test]
    fn sop_class_filter_and_unconditional() {
        let c = cfg();
        let r = select(&c, Some("CT"), Some("1.2.3"));
        assert_eq!(r.len(), 2); // sop rule + unconditional rule
    }

    #[test]
    fn frames_all_expands_to_all_frames() {
        let c = cfg();
        assert_eq!(frames_for(&c.pixels.regions[0], 3), vec![0, 1, 2]);
    }

    #[test]
    fn frames_list_filters_out_of_range() {
        let mut c = cfg();
        c.pixels.regions[0].frames = crate::config::Frames::List(vec![0, 5]);
        assert_eq!(frames_for(&c.pixels.regions[0], 3), vec![0]);
    }
}
