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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMissing {
    Warn,
    Skip,
    MaskAll,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Conservative,
    Aggressive,
}

impl Default for Strategy {
    fn default() -> Self {
        Strategy::Aggressive
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilenameMode {
    SopInstanceUid,
    Preserve,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Output {
    pub filename: FilenameMode,
}

impl Default for Output {
    fn default() -> Self {
        Output {
            filename: FilenameMode::SopInstanceUid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum FillValue {
    Num(u16),
    Named(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Frames {
    All,
    List(Vec<u32>),
}

impl Default for Frames {
    fn default() -> Self {
        Frames::All
    }
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

impl Default for OnMissing {
    fn default() -> Self {
        OnMissing::Warn
    }
}

impl Default for Shape {
    fn default() -> Self {
        Shape::Rect
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
        let user: toml::Value =
            toml::from_str(s).map_err(|e| AnonError::Config(e.to_string()))?;
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
        let c = Config::from_toml_str(
            "[profile]\nbase = \"research\"\nretain_safe_private = false\n",
        )
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
    fn output_filename_defaults_to_sop_instance_uid_and_can_preserve() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.output.filename, FilenameMode::SopInstanceUid);
        let c2 = Config::from_toml_str("[output]\nfilename = \"preserve\"\n").unwrap();
        assert_eq!(c2.output.filename, FilenameMode::Preserve);
    }

    #[test]
    fn strategy_defaults_to_aggressive_and_parses_conservative() {
        let c = Config::from_toml_str("").unwrap();
        assert_eq!(c.profile.strategy, Strategy::Aggressive);
        let c2 = Config::from_toml_str("[profile]\nstrategy = \"conservative\"\n").unwrap();
        assert_eq!(c2.profile.strategy, Strategy::Conservative);
    }
}
