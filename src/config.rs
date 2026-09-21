use crate::error::{AnonError, Result};
use dicom_core::Tag;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Action {
    X,
    Z,
    D,
    U,
    C,
    K,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateStrategy {
    Shift,
    Zero,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UidStrategy {
    Remap,
    Zero,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PixelMode {
    Mask,
    Keep,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMissing {
    #[default]
    Warn,
    Skip,
    MaskAll,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    #[default]
    Rect,
    Polygon,
}

/// How aggressively to remove information.
///
/// * `aggressive` (default) — clear every potentially identifying attribute,
///   keeping only the image's own clinical/technical parameters.
/// * `conservative` — clear the direct identifiers (name, ID, birth date,
///   accession number, institution, device serial, ...) but retain patient
///   demographics (sex, age, size, weight) and all dates/times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Conservative,
    #[default]
    Aggressive,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilenameMode {
    SopInstanceUid,
    Preserve,
}

/// How output files are placed under the output root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Mirror the input tree's relative paths (default).
    #[default]
    Mirror,
    /// Flatten every file into a directory named after its (remapped) series UID.
    Series,
}

/// What to do with files that are not DICOM.
///
/// Copying them is deliberately not offered. A de-identification tool must not put an
/// un-de-identified file into the tree it is certifying as clean, so the only choice is
/// whether dropping the file is silent or loud. De-identify a sidecar separately and
/// `--verify` the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonDicom {
    /// Log it as `skipped` and leave it out of the output tree (default).
    #[default]
    Skip,
    /// Record a per-file `error`, so a run that drops files cannot look successful.
    Error,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Output {
    pub filename: FilenameMode,
    pub layout: Layout,
    pub non_dicom: NonDicom,
}

impl Default for Output {
    fn default() -> Self {
        Output {
            filename: FilenameMode::SopInstanceUid,
            layout: Layout::Mirror,
            non_dicom: NonDicom::Skip,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum FillValue {
    Num(u16),
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Frames {
    #[default]
    All,
    List(Vec<u32>),
}

impl<'de> Deserialize<'de> for Frames {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Frames;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("\"all\" or a list of frame indices")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> std::result::Result<Frames, E> {
                if s == "all" {
                    Ok(Frames::All)
                } else {
                    Err(E::custom("expected \"all\""))
                }
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Frames, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = seq.next_element::<u32>()? {
                    v.push(x);
                }
                Ok(Frames::List(v))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Profile {
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default)]
    pub strategy: Strategy,
    #[serde(default)]
    pub retain_longitudinal_temporal: bool,
    #[serde(default)]
    pub retain_safe_private: bool,
    #[serde(default)]
    pub clean_descriptors: bool,
    #[serde(default)]
    pub clean_structured_content: bool,
    /// PS3.15 option: Retain UIDs.
    #[serde(default)]
    pub retain_uids: bool,
    /// PS3.15 option: Retain Device Identity.
    #[serde(default)]
    pub retain_device_identity: bool,
    /// PS3.15 option: Retain Institution Identity.
    #[serde(default)]
    pub retain_institution_identity: bool,
    /// PS3.15 option: Retain Patient Characteristics.
    #[serde(default)]
    pub retain_patient_characteristics: bool,
    /// PS3.15 option: Retain Longitudinal Temporal Information (Full Dates).
    #[serde(default)]
    pub retain_longitudinal_full_dates: bool,
    /// PS3.15 option: Clean Graphics.
    #[serde(default)]
    pub clean_graphics: bool,
}

fn default_base() -> String {
    "ps3.15-basic".to_string()
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            base: default_base(),
            strategy: Strategy::Aggressive,
            retain_longitudinal_temporal: false,
            retain_safe_private: false,
            clean_descriptors: false,
            clean_structured_content: false,
            retain_uids: false,
            retain_device_identity: false,
            retain_institution_identity: false,
            retain_patient_characteristics: false,
            retain_longitudinal_full_dates: false,
            clean_graphics: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Dates {
    pub strategy: DateStrategy,
    pub shift_salt: String,
    pub range_days: i64,
}

/// Upper bound for `[dates] range_days`.
///
/// `DateShifter::offset_for` computes `2 * range_days + 1`; without a bound an absurd
/// value overflows (panic in debug, silent wrap in release) instead of being a config
/// error. 36500 days is a century, which is already far beyond any real date range.
pub const MAX_RANGE_DAYS: i64 = 36500;

impl Default for Dates {
    fn default() -> Self {
        Dates {
            strategy: DateStrategy::Shift,
            shift_salt: String::new(),
            range_days: 3650,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Uids {
    pub strategy: UidStrategy,
    pub root: String,
    pub salt: String,
}

impl Default for Uids {
    fn default() -> Self {
        Uids {
            strategy: UidStrategy::Remap,
            root: "2.25.".to_string(),
            salt: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Region {
    pub modality: Option<String>,
    pub sop_class_uid: Option<String>,
    pub shape: Shape,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub points: Vec<[i64; 2]>,
    pub frames: Frames,
}

impl Default for Region {
    fn default() -> Self {
        Region {
            modality: None,
            sop_class_uid: None,
            shape: Shape::Rect,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            points: Vec::new(),
            frames: Frames::All,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Pixels {
    pub mode: PixelMode,
    pub fill: FillValue,
    pub on_missing_burnedin: OnMissing,
    pub regions: Vec<Region>,
}

impl Default for Pixels {
    fn default() -> Self {
        Pixels {
            mode: PixelMode::Mask,
            fill: FillValue::Num(0),
            on_missing_burnedin: OnMissing::Warn,
            regions: Vec::new(),
        }
    }
}

impl Default for FillValue {
    fn default() -> Self {
        FillValue::Num(0)
    }
}

/// Hard limits that keep a run from exhausting the machine.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Refuse any input larger than this many bytes and record a per-file error
    /// instead of processing it. `0` disables the guard.
    ///
    /// This is a per-file guard, not a total budget: peak memory is a multiple of the
    /// file size (the object, the transcoded pixels and the serialised output all live
    /// at once) multiplied by the number of jobs in flight.
    pub max_input_bytes: u64,
    /// Maximimum bytes of input allowed in flight at once. The job count is reduced so that
    /// `jobs x largest_input` stays within this. `0` disables the cap.
    pub max_parallel_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        // A single DICOM this large is already exceptional, and the failure it guards
        // against - the process being killed part way through a batch - is worse than
        // refusing one file loudly.
        Limits {
            max_input_bytes: 2 * 1024 * 1024 * 1024,
            max_parallel_bytes: 1024 * 1024 * 1024,
        }
    }
}

impl Limits {
    /// How many files may be in flight at once.
    ///
    /// Every file is held whole, so the peak scales with `jobs x largest input`. Capping
    /// the job count from the largest input we know about bounds that statically and
    /// predictably, unlike a runtime byte budget which would have to block workers.
    pub fn parallel_jobs(&self, requested: usize, largest: u64) -> usize {
        let requested = requested.max(1);
        if self.max_parallel_bytes == 0 || largest == 0 {
            return requested;
        }
        let by_budget = (self.max_parallel_bytes / largest).max(1);
        requested.min(by_budget.min(usize::MAX as u64) as usize)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub profile: Profile,
    pub dates: Dates,
    pub uids: Uids,
    pub pixels: Pixels,
    pub output: Output,
    pub limits: Limits,
    pub tags: BTreeMap<String, Action>,
    pub dummies: BTreeMap<String, String>,
}

const BASIC_PRESET: &str = r#"
[profile]
base = "ps3.15-basic"
"#;

const RESEARCH_PRESET: &str = r#"
[profile]
base = "research"
retain_longitudinal_temporal = true
retain_safe_private = true
"#;

fn preset_for(base: &str) -> Result<&'static str> {
    match base {
        "ps3.15-basic" => Ok(BASIC_PRESET),
        "research" => Ok(RESEARCH_PRESET),
        other => Err(AnonError::Config(format!("unknown base profile `{other}`"))),
    }
}

fn deep_merge(base: toml::Value, over: toml::Value) -> toml::Value {
    match (base, over) {
        (toml::Value::Table(mut b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.remove(&k) {
                    Some(existing) => {
                        b.insert(k, deep_merge(existing, v));
                    }
                    None => {
                        b.insert(k, v);
                    }
                }
            }
            toml::Value::Table(b)
        }
        (_, over) => over,
    }
}

pub fn parse_tag(s: &str) -> Result<Tag> {
    let s = s.trim().trim_matches(|c| c == '(' || c == ')');
    let (g, e) = s
        .split_once(',')
        .ok_or_else(|| AnonError::Config(format!("bad tag `{s}`")))?;
    let g = u16::from_str_radix(g.trim(), 16)
        .map_err(|_| AnonError::Config(format!("bad group in `{s}`")))?;
    let e = u16::from_str_radix(e.trim(), 16)
        .map_err(|_| AnonError::Config(format!("bad element in `{s}`")))?;
    Ok(Tag(g, e))
}

pub fn is_valid_uid_root(root: &str) -> bool {
    let r = root.trim_end_matches('.');
    if r.is_empty() {
        return false;
    }
    r.split('.').all(|c| {
        !c.is_empty()
            && c.chars().all(|ch| ch.is_ascii_digit())
            && (c.len() == 1 || !c.starts_with('0'))
    })
}

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Config> {
        let user: toml::Value = toml::from_str(s).map_err(|e| AnonError::Config(e.to_string()))?;
        let base = user
            .get("profile")
            .and_then(|p| p.get("base"))
            .and_then(|b| b.as_str())
            .unwrap_or("ps3.15-basic")
            .to_string();
        let preset: toml::Value =
            toml::from_str(preset_for(&base)?).map_err(|e| AnonError::Config(e.to_string()))?;
        let merged = deep_merge(preset, user);
        let cfg: Config = merged
            .try_into()
            .map_err(|e: toml::de::Error| AnonError::Config(e.to_string()))?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)?;
        let cfg = Config::from_toml_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.uids.strategy == UidStrategy::Remap && !is_valid_uid_root(&self.uids.root) {
            return Err(AnonError::Config(format!(
                "invalid uids.root `{}`",
                self.uids.root
            )));
        }
        if !(0..=MAX_RANGE_DAYS).contains(&self.dates.range_days) {
            return Err(AnonError::Config(format!(
                "invalid dates.range_days `{}` (expected 0..={MAX_RANGE_DAYS})",
                self.dates.range_days
            )));
        }
        if let FillValue::Named(n) = &self.pixels.fill {
            if n != "max" {
                return Err(AnonError::Config(format!("invalid pixels.fill `{n}`")));
            }
        }
        for (i, r) in self.pixels.regions.iter().enumerate() {
            match r.shape {
                Shape::Rect if r.width == 0 || r.height == 0 => {
                    return Err(AnonError::Config(format!(
                        "region {i}: rect needs width>0 and height>0"
                    )));
                }
                Shape::Polygon if r.points.len() < 3 => {
                    return Err(AnonError::Config(format!(
                        "region {i}: polygon needs >= 3 points"
                    )));
                }
                _ => {}
            }
        }
        for k in self.tags.keys() {
            parse_tag(k)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_config_empty() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.profile.base, "ps3.15-basic");
        assert_eq!(c.uids.strategy, UidStrategy::Remap);
        assert_eq!(c.pixels.mode, PixelMode::Mask);
    }

    #[test]
    fn research_preset_sets_longitudinal_and_safe_private() {
        let c = Config::from_toml_str("[profile]\nbase = \"research\"\n").unwrap();
        assert!(c.profile.retain_longitudinal_temporal);
        assert!(c.profile.retain_safe_private);
    }

    #[test]
    fn user_value_overrides_preset() {
        let c =
            Config::from_toml_str("[profile]\nbase = \"research\"\nretain_safe_private = false\n")
                .unwrap();
        assert!(c.profile.retain_longitudinal_temporal);
        assert!(!c.profile.retain_safe_private);
    }

    #[test]
    fn parses_frames_all_and_list() {
        let c = Config::from_toml_str(
            "[[pixels.regions]]\nshape = \"rect\"\nwidth = 1\nheight = 1\nframes = \"all\"\n\
             [[pixels.regions]]\nshape = \"rect\"\nwidth = 1\nheight = 1\nframes = [0, 2]\n",
        )
        .unwrap();
        assert_eq!(c.pixels.regions[0].frames, Frames::All);
        assert_eq!(c.pixels.regions[1].frames, Frames::List(vec![0, 2]));
    }

    #[test]
    fn rejects_zero_size_rect() {
        let c = Config::from_toml_str(
            "[pixels]\nmode = \"mask\"\n[[pixels.regions]]\nshape = \"rect\"\nwidth = 0\nheight = 5\n",
        )
        .unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_invalid_uid_root_when_remapping() {
        let c = Config::from_toml_str("[uids]\nstrategy = \"remap\"\nroot = \"abc\"\n").unwrap();
        assert!(c.validate().is_err());
    }

    #[test]
    fn rejects_a_date_range_that_would_overflow() {
        // offset_for computes `2 * range_days + 1`, so an absurd range used to panic
        // mid-run in a debug build and wrap silently in release - aborting a batch over a
        // config typo that should have been a clean exit 2.
        let huge = Config::from_toml_str("[dates]\nrange_days = 9223372036854775807\n").unwrap();
        assert!(huge.validate().is_err(), "i64::MAX must be rejected");
        assert!(
            Config::from_toml_str("[dates]\nrange_days = -5\n")
                .unwrap()
                .validate()
                .is_err(),
            "a negative range is meaningless"
        );
        assert!(
            Config::from_toml_str("[dates]\nrange_days = 36500\n")
                .unwrap()
                .validate()
                .is_ok(),
            "100 years is fine"
        );
        assert!(
            Config::from_toml_str("[dates]\nrange_days = 0\n")
                .unwrap()
                .validate()
                .is_ok(),
            "0 is clamped to 1, not an error"
        );
    }

    #[test]
    fn non_dicom_defaults_to_skip_and_can_error() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.output.non_dicom, NonDicom::Skip);
        let c2 = Config::from_toml_str("[output]\nnon_dicom = \"error\"\n").unwrap();
        assert_eq!(c2.output.non_dicom, NonDicom::Error);
        // and it does not disturb the neighbouring keys
        assert_eq!(c2.output.filename, FilenameMode::SopInstanceUid);
        assert_eq!(c2.output.layout, Layout::Mirror);
    }

    #[test]
    fn parallel_jobs_is_capped_by_the_largest_input() {
        let mut l = Limits {
            max_input_bytes: 0,
            max_parallel_bytes: 1024,
        };
        assert_eq!(l.parallel_jobs(8, 256), 4);
        assert_eq!(l.parallel_jobs(2, 256), 2, "never more than asked for");
        assert_eq!(
            l.parallel_jobs(8, 4096),
            1,
            "a file larger than the whole budget runs alone"
        );
        assert_eq!(l.parallel_jobs(8, 0), 8, "an unknown size does not cap");
        assert_eq!(l.parallel_jobs(0, 128), 1, "at least one job");

        l.max_parallel_bytes = 0;
        assert_eq!(l.parallel_jobs(8, 4096), 8, "0 disables the cap");
    }

    #[test]
    fn size_limit_defaults_to_two_gib_and_parses() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.limits.max_input_bytes, 2 * 1024 * 1024 * 1024);
        let c2 = Config::from_toml_str("[limits]\nmax_input_bytes = 1024\n").unwrap();
        assert_eq!(c2.limits.max_input_bytes, 1024);
        let c3 = Config::from_toml_str("[limits]\nmax_input_bytes = 0\n").unwrap();
        assert_eq!(c3.limits.max_input_bytes, 0, "0 means unlimited");
        assert_eq!(
            c.limits.max_parallel_bytes,
            1024 * 1024 * 1024,
            "parallel budget defaults to 1 GiB"
        );
        let c4 = Config::from_toml_str("[limits]\nmax_parallel_bytes = 0\n").unwrap();
        assert_eq!(c4.limits.max_parallel_bytes, 0);
    }

    #[test]
    fn output_filename_defaults_to_sop_instance_uid_and_can_preserve() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.output.filename, FilenameMode::SopInstanceUid);
        let c2 = Config::from_toml_str("[output]\nfilename = \"preserve\"\n").unwrap();
        assert_eq!(c2.output.filename, FilenameMode::Preserve);
    }

    #[test]
    fn output_layout_defaults_to_mirror_and_can_group_by_series() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.output.layout, Layout::Mirror);
        // `filename` still parses when `layout` is absent, and vice versa.
        let c2 = Config::from_toml_str("[output]\nfilename = \"preserve\"\n").unwrap();
        assert_eq!(c2.output.layout, Layout::Mirror);
        let c3 = Config::from_toml_str("[output]\nlayout = \"series\"\n").unwrap();
        assert_eq!(c3.output.layout, Layout::Series);
        assert_eq!(c3.output.filename, FilenameMode::SopInstanceUid);
    }

    #[test]
    fn strategy_defaults_to_aggressive_and_parses_conservative() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.profile.strategy, Strategy::Aggressive);
        let c2 = Config::from_toml_str("[profile]\nstrategy = \"conservative\"\n").unwrap();
        assert_eq!(c2.profile.strategy, Strategy::Conservative);
    }
}
