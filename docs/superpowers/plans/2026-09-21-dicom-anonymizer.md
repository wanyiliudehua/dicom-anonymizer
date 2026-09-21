# dicom-anonymizer Implementation Plan

> **Status: fully executed - this document is history, not a tracker.** All 15 tasks below
> were delivered during the first implementation sessions. Do not read the checkboxes here
> (or any parallel todo list) as progress: they were never maintained and drifted out of
> date, ending up showing 15/15 unfinished after everything had shipped. The single source of
> truth for what the tool does and what changed is `CHANGELOG.md` plus `git log`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a standalone Rust CLI that de-identifies DICOM files in bulk — PS3.15 tag removal/replacement, configured burned-in pixel masking, mirrored output, and a JSON audit trail.

**Architecture:** A single binary crate layered as: CLI → config (TOML presets + overrides) → discovery (walk + `DICM` magic) → de-id engine (tag actions) → pixel stage (transcode to uncompressed + mask) → atomic mirrored writer → JSON Lines audit. UID and date mappings are pure deterministic functions (HMAC-SHA256), so file processing parallelizes with rayon without shared mutable state.

**Tech Stack:** Rust 2021, `dicom-rs` v0.10 (`dicom-object`, `dicom-pixeldata`, `dicom-transfer-syntax-registry`, `dicom-encoding`, `dicom-core`, `dicom-dictionary-std`), `clap`, `serde`/`toml`/`serde_json`, `walkdir`, `rayon`, `hmac`/`sha2`, `chrono`, `thiserror`.

**Spec:** `docs/superpowers/specs/2026-09-21-dicom-anonymizer-design.md`

## Global Constraints

- Rust edition 2021; crate name `dicom-anonymizer`; binary name `dicom-anonymizer`.
- `dicom-rs` v0.10.x exactly (`dicom-object = "0.10"`, etc.).
- Output goes **only** to a separate directory; never modify inputs; no `--in-place`.
- Audit is **JSON Lines**, one object per processed file (spec §12).
- UID/date mapping is a **pure deterministic function** of `(salt, original)` — no shared mutable state, thread-safe by construction.
- Default codecs: `native` (RLE/JPEG/Deflate) + `openjp2` (JPEG 2000); JPEG-LS only under the optional `charls` feature.
- No OCR, no network, no reversible mapping, no GUI (spec §3).
- Full PS3.15 Table E.1-1 is the rule source; the built-in table must at minimum cover every tag asserted in Task 7's test.
- Every task ends green (`cargo test`) and committed.

## File Structure

```
Cargo.toml
README.md
dicom-anonymizer.example.toml
src/
  main.rs            orchestration, rayon, exit code
  cli.rs             clap args + validation
  config.rs          TOML load, presets, merge, validate
  discovery.rs       walk tree, DICM detection, jobs
  deid/
    mod.rs           DeidContext + apply() orchestration
    dataset.rs       thin wrapper over dicom-object mutation API
    profile.rs       PS3.15 base rule table + options
    actions.rs       X/Z/D/U/C/K per element
    uids.rs          deterministic UID mapper
    dates.rs         deterministic date shifter
  pixel/
    mod.rs           pixel stage orchestration (transcode + mask)
    geometry.rs      pure geometry + byte-level masking
    regions.rs       region selection by modality/SOP class/frames
  writer.rs          mirrored path + atomic write
  audit.rs           JSON Lines records
  report.rs          run summary + exit code
  error.rs           AnonError / Result
tests/
  integration.rs     end-to-end on a generated fixture tree
  common/mod.rs      fixture builders
docs/superpowers/specs/2026-09-21-dicom-anonymizer-design.md
```

## Review Focus

The spec implies, but no single task's happy-path test covers, these failure modes. Each line is pinned by a test in the named task.

1. **UID element holding several values / empty value** — a multi-valued `(UI)` element must not panic or lose values; each value is remapped (Task 7).
2. **Date values that are not 8 chars or are malformed** — `DA` like `2024` or empty must pass through unchanged, never panic (Task 6).
3. **Pixel geometry absent or inconsistent** (`Rows`/`Columns` missing, `Number of Frames` absent) — single-frame defaults to 1; a region that exceeds bounds is a per-file error, not a crash (Task 9).
4. **A region matching a file with no pixel data** (e.g. an SR/structured report) — skipped with `regions_masked = 0`, not an error (Task 10).
5. **Empty input directory / non-DICOM files only** — exits `0` with an all-skipped summary (Task 11).

---

### Task 1: Crate skeleton, error type, and config model

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/error.rs`, `src/config.rs`
- Test: unit tests inside `src/config.rs`

**Interfaces:**
- Produces: `AnonError`, `type Result<T>`; `Action`, `Config`, `Profile`, `Dates`, `DateStrategy`, `Uids`, `UidStrategy`, `Pixels`, `PixelMode`, `FillValue`, `OnMissing`, `Frames`, `Shape`, `Region`, `Config::from_toml_str(&str) -> Result<Config>`, `Config::load(&Path) -> Result<Config>`, `Config::validate(&self) -> Result<()>`, `config::parse_tag(&str) -> Result<Tag>`.

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "dicom-anonymizer"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
serde_json = "1"
walkdir = "2"
rayon = "1.10"
thiserror = "1"
hmac = "0.12"
sha2 = "0.10"
chrono = "0.4"
dicom-core = "0.10"
dicom-object = "0.10"
dicom-pixeldata = "0.10"
dicom-encoding = "0.10"
dicom-dictionary-std = "0.10"
dicom-transfer-syntax-registry = { version = "0.10", features = ["native", "openjp2", "deflate"] }

[features]
default = []
charls = ["dicom-transfer-syntax-registry/charls", "dicom-pixeldata/charls"]

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"
predicates = "3"
```

- [ ] **Step 2: Write `src/error.rs`**

```rust
#[derive(Debug, thiserror::Error)]
pub enum AnonError {
    #[error("config: {0}")]
    Config(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("dicom: {0}")]
    Dicom(String),
    #[error("unsupported transfer syntax: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, AnonError>;
```

- [ ] **Step 3: Write failing config tests**

In `src/config.rs`:

```rust
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
}
```

- [ ] **Step 4: Run tests, expect failure**

Run: `cargo test config:: 2>&1 | head -30`
Expected: FAIL — `Config` and friends not defined.

- [ ] **Step 5: Implement `src/config.rs`**

```rust
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
#[serde(rename_all = "lowercase")]
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
    pub retain_longitudinal_temporal: bool,
    #[serde(default)]
    pub retain_safe_private: bool,
    #[serde(default)]
    pub clean_descriptors: bool,
    #[serde(default)]
    pub clean_structured_content: bool,
}

fn default_base() -> String {
    "ps3.15-basic".to_string()
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            base: default_base(),
            retain_longitudinal_temporal: false,
            retain_safe_private: false,
            clean_descriptors: false,
            clean_structured_content: false,
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

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    pub profile: Profile,
    pub dates: Dates,
    pub uids: Uids,
    pub pixels: Pixels,
    pub tags: BTreeMap<String, Action>,
    pub dummies: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            profile: Profile::default(),
            dates: Dates::default(),
            uids: Uids::default(),
            pixels: Pixels::default(),
            tags: BTreeMap::new(),
            dummies: BTreeMap::new(),
        }
    }
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

fn deep_merge(mut base: toml::Value, over: toml::Value) -> toml::Value {
    if let (toml::Value::Table(b), toml::Value::Table(o)) = (&mut base, over) {
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
        base
    } else {
        over
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
        let cfg: Config =
            merged.try_into().map_err(|e: toml::de::Error| AnonError::Config(e.to_string()))?;
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
```

- [ ] **Step 6: Write `src/main.rs` placeholder that compiles**

```rust
mod config;
mod error;

fn main() {
    println!("dicom-anonymizer");
}
```

- [ ] **Step 7: Run tests, expect pass**

Run: `cargo test 2>&1 | tail -20`
Expected: PASS (6 tests).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: crate skeleton, error type, and config model with presets"
```

---

### Task 2: Thin dataset wrapper over the dicom-object mutation API

**Files:**
- Create: `src/deid/mod.rs`, `src/deid/dataset.rs`
- Modify: `src/main.rs` (add `mod deid;`)
- Test: unit tests inside `src/deid/dataset.rs`

**Interfaces:**
- Consumes: `Tag`, `VR` from `dicom-core`; `InMemDicomObject`, `DataElement` from `dicom-object`.
- Produces: `dataset::get_string(&InMemDicomObject, Tag) -> Option<String>`, `dataset::set_string(&mut InMemDicomObject, Tag, VR, &str)`, `dataset::remove(&mut InMemDicomObject, Tag) -> bool`, `dataset::zero(&mut InMemDicomObject, Tag) -> bool`, `dataset::set_multi_string(&mut InMemDicomObject, Tag, Vec<String>)`, `dataset::tag_list(&InMemDicomObject) -> Vec<Tag>`, `dataset::first_value_string(&InMemDicomObject, Tag) -> Option<String>`.

> This task isolates the exact `dicom-rs` names behind five functions. If a method name differs in 0.10.0, only this file changes. API reference: <https://docs.rs/dicom-object/0.10.0/dicom_object/mem/struct.InMemDicomObject.html>.

- [ ] **Step 1: Write failing tests**

In `src/deid/dataset.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::VR;
    use dicom_object::InMemDicomObject;

    fn tag(g: u16, e: u16) -> Tag {
        Tag(g, e)
    }

    #[test]
    fn set_get_remove_zero_roundtrip() {
        let mut o = InMemDicomObject::from_element_iter([] as [dicom_object::DataElement; 0]);
        set_string(&mut o, tag(0x0010, 0x0010), VR::PN, "Doe^John");
        assert_eq!(get_string(&o, tag(0x0010, 0x0010)).as_deref(), Some("Doe^John"));
        assert!(remove(&mut o, tag(0x0010, 0x0010)));
        assert!(!remove(&mut o, tag(0x0010, 0x0010)));
        assert_eq!(get_string(&o, tag(0x0010, 0x0010)), None);
    }

    #[test]
    fn zero_makes_element_present_but_empty() {
        let mut o = InMemDicomObject::from_element_iter([] as [dicom_object::DataElement; 0]);
        set_string(&mut o, tag(0x0010, 0x0010), VR::PN, "Doe^John");
        assert!(zero(&mut o, tag(0x0010, 0x0010)));
        // present, and whatever it reads back, it must not be the original
        assert_ne!(get_string(&o, tag(0x0010, 0x0010)).as_deref(), Some("Doe^John"));
    }

    #[test]
    fn tag_list_is_sorted_and_complete() {
        let o = InMemDicomObject::from_element_iter([
            dicom_object::DataElement::new(tag(0x0010, 0x0020), VR::LO, "A"),
            dicom_object::DataElement::new(tag(0x0008, 0x0060), VR::CS, "CT"),
        ]);
        let t = tag_list(&o);
        assert_eq!(t, vec![tag(0x0008, 0x0060), tag(0x0010, 0x0020)]);
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test deid::dataset 2>&1 | tail -20`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement `src/deid/dataset.rs`**

```rust
use dicom_core::{Tag, VR};
use dicom_object::{DataElement, InMemDicomObject};

pub fn tag_list(obj: &InMemDicomObject) -> Vec<Tag> {
    let mut v: Vec<Tag> = obj.iter().map(|e| e.tag()).collect();
    v.sort();
    v.dedup();
    v
}

pub fn get_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.element(tag).ok().and_then(|e| e.to_str().ok()).map(|s| s.to_string())
}

pub fn first_value_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    get_string(obj, tag)
}

pub fn set_string(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
    obj.put_element(DataElement::new(tag, vr, value.to_string()));
}

pub fn set_multi_string(obj: &mut InMemDicomObject, tag: Tag, values: Vec<String>) {
    let joined = values.join("\\");
    obj.put_element(DataElement::new(tag, VR::UI, joined));
}

pub fn remove(obj: &mut InMemDicomObject, tag: Tag) -> bool {
    obj.remove_element(tag)
}

pub fn zero(obj: &mut InMemDicomObject, tag: Tag) -> bool {
    let vr = obj.element(tag).map(|e| e.vr()).unwrap_or(VR::UN);
    let present = obj.remove_element(tag);
    obj.put_element(DataElement::new(tag, vr, ""));
    present
}
```

- [ ] **Step 4: Add `mod deid;` to `src/main.rs` and create `src/deid/mod.rs`**

```rust
pub mod dataset;
```

`src/main.rs`:

```rust
mod config;
mod deid;
mod error;

fn main() {
    println!("dicom-anonymizer");
}
```

- [ ] **Step 5: Run tests, expect pass**

Run: `cargo test deid::dataset 2>&1 | tail -20`
Expected: PASS (3 tests). If a method name differs, fix only `dataset.rs` using the pinned docs URL.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: thin dataset wrapper isolating dicom-object mutation API"
```

---

### Task 3: Deterministic UID mapper

**Files:**
- Create: `src/deid/uids.rs`
- Modify: `src/deid/mod.rs` (`pub mod uids;`)
- Test: unit tests inside `src/deid/uids.rs`

**Interfaces:**
- Consumes: `config::is_valid_uid_root`.
- Produces: `UidMapper::new(root: &str, salt: &[u8]) -> UidMapper`, `UidMapper::map(&self, original: &str) -> String`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_input() {
        let m = UidMapper::new("2.25.", b"salt");
        assert_eq!(m.map("1.2.3"), m.map("1.2.3"));
    }

    #[test]
    fn different_inputs_differ() {
        let m = UidMapper::new("2.25.", b"salt");
        assert_ne!(m.map("1.2.3"), m.map("1.2.4"));
    }

    #[test]
    fn has_root_prefix_and_valid_chars() {
        let m = UidMapper::new("2.25.", b"salt");
        let u = m.map("1.2.840.10008.5.1.4.1.1.2");
        assert!(u.starts_with("2.25."));
        assert!(u.chars().all(|c| c.is_ascii_digit() || c == '.'));
        assert!(u.len() <= 64 && u.len() >= 3);
    }

    #[test]
    fn no_collisions_over_sample() {
        let m = UidMapper::new("2.25.", b"salt");
        let mut seen = std::collections::HashSet::new();
        for i in 0..1000u32 {
            assert!(seen.insert(m.map(&format!("1.2.3.{i}"))), "collision at {i}");
        }
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test deid::uids 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/deid/uids.rs`**

```rust
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct UidMapper {
    prefix: String,
    salt: Vec<u8>,
}

impl UidMapper {
    pub fn new(root: &str, salt: &[u8]) -> UidMapper {
        let mut prefix = root.trim().to_string();
        if !prefix.ends_with('.') {
            prefix.push('.');
        }
        UidMapper {
            prefix,
            salt: salt.to_vec(),
        }
    }

    /// Pure, deterministic, thread-safe: same (salt, original) always yields the same UID.
    pub fn map(&self, original: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.salt).expect("hmac accepts any key");
        mac.update(original.as_bytes());
        let bytes = mac.finalize().into_bytes();
        let n: u128 = u128::from_be_bytes(bytes[0..16].try_into().expect("16 bytes"));
        let mut s = format!("{}{}", self.prefix, n);
        s.truncate(64);
        // never leave a trailing dot or empty last component
        while s.ends_with('.') {
            s.pop();
        }
        s
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test deid::uids 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: deterministic HMAC-based UID mapper"
```

---

### Task 4: Deterministic date shifter

**Files:**
- Create: `src/deid/dates.rs`
- Modify: `src/deid/mod.rs` (`pub mod dates;`)
- Test: unit tests inside `src/deid/dates.rs`

**Interfaces:**
- Produces: `DateShifter::new(salt: &[u8], range_days: i64) -> DateShifter`, `DateShifter::offset_for(&self, patient_id: &str) -> i64`, `DateShifter::shift_da(&self, da: &str, offset: i64) -> Option<String>`, `DateShifter::shift_dt(&self, dt: &str, offset: i64) -> Option<String>`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_deterministic_and_bounded() {
        let s = DateShifter::new(b"salt", 100);
        let a = s.offset_for("P1");
        assert_eq!(a, s.offset_for("P1"));
        assert!((-100..=100).contains(&a));
    }

    #[test]
    fn shift_da_preserves_interval() {
        let s = DateShifter::new(b"salt", 100);
        let off = s.offset_for("P1");
        let d1 = s.shift_da("20240101", off).unwrap();
        let d2 = s.shift_da("20240111", off).unwrap();
        let a = chrono::NaiveDate::parse_from_str(&d1, "%Y%m%d").unwrap();
        let b = chrono::NaiveDate::parse_from_str(&d2, "%Y%m%d").unwrap();
        assert_eq!((b - a).num_days(), 10);
    }

    #[test]
    fn shift_dt_keeps_time_and_fraction() {
        let s = DateShifter::new(b"salt", 30);
        let out = s.shift_dt("20240101123045.123", 5).unwrap();
        assert!(out.ends_with("123045.123"));
        assert_eq!(&out[0..8], "20240106");
    }

    #[test]
    fn malformed_dates_return_none_not_panic() {
        let s = DateShifter::new(b"salt", 30);
        assert_eq!(s.shift_da("2024", 1), None);
        assert_eq!(s.shift_da("", 1), None);
        assert_eq!(s.shift_dt("2024", 1), None);
        assert_eq!(s.shift_dt("", 1), None);
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test deid::dates 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/deid/dates.rs`**

```rust
use chrono::{Duration, NaiveDate};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct DateShifter {
    salt: Vec<u8>,
    range_days: i64,
}

impl DateShifter {
    pub fn new(salt: &[u8], range_days: i64) -> DateShifter {
        DateShifter {
            salt: salt.to_vec(),
            range_days: range_days.max(1),
        }
    }

    /// Deterministic per-patient offset in `[-range_days, +range_days]`.
    pub fn offset_for(&self, patient_id: &str) -> i64 {
        let mut mac = HmacSha256::new_from_slice(&self.salt).expect("hmac accepts any key");
        mac.update(patient_id.as_bytes());
        let bytes = mac.finalize().into_bytes();
        let n = u64::from_be_bytes(bytes[0..8].try_into().expect("8 bytes"));
        let span = (2 * self.range_days + 1) as u64;
        (n % span) as i64 - self.range_days
    }

    /// `YYYYMMDD` -> shifted `YYYYMMDD`; `None` for anything not exactly 8 chars.
    pub fn shift_da(&self, da: &str, offset: i64) -> Option<String> {
        if da.len() != 8 {
            return None;
        }
        let d = NaiveDate::parse_from_str(da, "%Y%m%d").ok()?;
        Some((d + Duration::days(offset)).format("%Y%m%d").to_string())
    }

    /// `YYYYMMDD[HHMMSS[.F...]]` -> date part shifted, time part preserved verbatim.
    pub fn shift_dt(&self, dt: &str, offset: i64) -> Option<String> {
        if dt.len() < 8 {
            return None;
        }
        let (d, rest) = dt.split_at(8);
        let shifted = self.shift_da(d, offset)?;
        Some(format!("{shifted}{rest}"))
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test deid::dates 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: deterministic per-patient date shifter"
```

---

### Task 5: PS3.15 profile rule table

**Files:**
- Create: `src/deid/profile.rs`
- Modify: `src/deid/mod.rs` (`pub mod profile;`)
- Test: unit tests inside `src/deid/profile.rs`

**Interfaces:**
- Consumes: `config::{Action, Config, parse_tag}`.
- Produces: `profile::base_rules() -> Vec<(Tag, Action)>`, `profile::effective_rules(&Config) -> Result<Vec<(Tag, Action)>>`.

- [ ] **Step 1: Write failing tests**

```rust
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
        // StudyDate must still have a rule (shift is applied by the engine)
        assert!(m.contains_key(&crate::config::parse_tag("0008,0020").unwrap()));
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test deid::profile 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/deid/profile.rs`**

Curated from PS3.15 Table E.1-1 (<https://dicom.nema.org/medical/dicom/current/output/chtml/part15/chapter_E.html>). Extend this table to the full E.1-1; the test in Step 1 is the minimum bar.

```rust
use crate::config::{parse_tag, Action, Config};
use crate::error::Result;
use dicom_core::Tag;

/// Curated PS3.15 Basic Application Level Confidentiality Profile actions.
/// Source: PS3.15 Table E.1-1 (see module docs). Extend to the full table.
const BASE: &[(&str, Action)] = &[
    ("0008,0012", Action::U), // Instance Creation Date -> handled as date
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

pub fn base_rules() -> Vec<(Tag, Action)> {
    BASE.iter()
        .map(|(t, a)| (parse_tag(t).expect("built-in tags are valid"), *a))
        .collect()
}

pub fn effective_rules(cfg: &Config) -> Result<Vec<(Tag, Action)>> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<Tag, Action> = base_rules().into_iter().collect();
    // Options may adjust rules here (e.g. keep safe private tags); MVP leaves base as-is.
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
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test deid::profile 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: PS3.15 base rule table with user override"
```

---

### Task 6: Action engine (X/Z/D/U/C/K) with sequence recursion

**Files:**
- Create: `src/deid/actions.rs`
- Modify: `src/deid/mod.rs` (`pub mod actions;`)
- Test: unit tests inside `src/deid/actions.rs`

**Interfaces:**
- Consumes: `dataset::*`, `config::{Action, Config, DateStrategy}`, `uids::UidMapper`, `dates::DateShifter`, `profile::effective_rules`.
- Produces: the `DeidContext` struct and `actions::apply(obj: &mut InMemDicomObject, ctx: &DeidContext) -> Result<usize>` returning the number of changed elements.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::deid::dates::DateShifter;
    use crate::deid::uids::UidMapper;
    use dicom_core::VR;
    use dicom_object::{DataElement, InMemDicomObject};

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
        assert_ne!(crate::deid::dataset::get_string(&o, Tag(0x0010, 0x0010)).as_deref(), Some("Doe^John"));
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
        let cfg = Config::from_toml_str("[profile]\nretain_longitudinal_temporal = true\n").unwrap();
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
        assert_eq!(crate::deid::dataset::get_string(&o, Tag(0x0008, 0x0021)).as_deref(), Some("2024"));
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
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test deid::actions 2>&1 | tail -30`
Expected: FAIL.

- [ ] **Step 3: Implement `src/deid/actions.rs`**

```rust
use crate::config::{Action, Config, DateStrategy};
use crate::deid::dataset as ds;
use crate::deid::dates::DateShifter;
use crate::deid::profile::effective_rules;
use crate::deid::uids::UidMapper;
use crate::error::Result;
use dicom_core::{Tag, VR};
use dicom_object::InMemDicomObject;

pub struct DeidContext<'a> {
    pub cfg: &'a Config,
    pub uids: &'a UidMapper,
    pub dates: &'a DateShifter,
}

const DATE_TAGS: &[Tag] = &[
    Tag(0x0008, 0x0012),
    Tag(0x0008, 0x0020),
    Tag(0x0008, 0x0021),
    Tag(0x0008, 0x0022),
    Tag(0x0008, 0x0023),
    Tag(0x0008, 0x0030),
];

fn is_date_tag(tag: Tag) -> bool {
    DATE_TAGS.contains(&tag)
}

pub fn apply(obj: &mut InMemDicomObject, ctx: &DeidContext) -> Result<usize> {
    let rules: std::collections::HashMap<Tag, Action> =
        effective_rules(ctx.cfg)?.into_iter().collect();
    let patient_id = ds::get_string(obj, Tag(0x0010, 0x0020)).unwrap_or_default();
    let offset = ctx.dates.offset_for(&patient_id);
    let mut changed = 0usize;

    for tag in ds::tag_list(obj) {
        // Recurse into sequences first.
        let mut is_seq = false;
        if let Ok(e) = obj.element(tag) {
            if matches!(e.vr(), VR::SQ) {
                is_seq = true;
            }
        }
        if is_seq {
            changed += apply_to_sequence(obj, tag, ctx, &patient_id)?;
            continue;
        }

        let Some(&action) = rules.get(&tag) else {
            continue;
        };

        if action == Action::K {
            continue;
        }
        if action == Action::X {
            if ds::remove(obj, tag) {
                changed += 1;
            }
            continue;
        }

        // Date handling takes precedence for date tags when shifting is on.
        if is_date_tag(tag)
            && ctx.cfg.dates.strategy == DateStrategy::Shift
            && (ctx.cfg.profile.retain_longitudinal_temporal || action != Action::Z)
        {
            if let Some(v) = ds::get_string(obj, tag) {
                let shifted = match v.len() {
                    8 => ctx.dates.shift_da(&v, offset),
                    _ => ctx.dates.shift_dt(&v, offset),
                };
                if let Some(s) = shifted {
                    ds::set_string(obj, tag, VR::DA, &s);
                    changed += 1;
                    continue;
                }
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
                if let Some(v) = ds::get_string(obj, tag) {
                    let mapped: Vec<String> =
                        v.split('\\').map(|u| ctx.uids.map(u)).collect();
                    ds::set_multi_string(obj, tag, mapped);
                    changed += 1;
                }
            }
            Action::C => {
                let cleaned = "ANONYMOUS".to_string();
                let vr = obj.element(tag).map(|e| e.vr()).unwrap_or(VR::LO);
                ds::set_string(obj, tag, vr, &cleaned);
                changed += 1;
            }
            Action::X | Action::K => {}
        }
    }
    Ok(changed)
}

/// Recurse into an SQ element, mutating items in place. Returns changed count.
fn apply_to_sequence(
    obj: &mut InMemDicomObject,
    tag: Tag,
    ctx: &DeidContext,
    _patient_id: &str,
) -> Result<usize> {
    let mut changed = 0usize;
    if let Some(seq) = obj.sequence_mut(tag).ok() {
        for item in seq.items_mut() {
            changed += apply(item, ctx)?;
        }
    }
    Ok(changed)
}
```

> If `sequence_mut` / `items_mut` are not the 0.10.0 names, adapt only `apply_to_sequence`; the recursion contract (`apply(item)`) stays identical. API reference: the `InMemDicomObject` docs URL pinned in Task 2.

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test deid::actions 2>&1 | tail -30`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: PS3.15 action engine with sequence recursion and date/UID handling"
```

---

### Task 7: Pixel geometry and byte-level masking (pure functions)

**Files:**
- Create: `src/pixel/geometry.rs`, `src/pixel/mod.rs`
- Modify: `src/main.rs` (add `mod pixel;`)
- Test: unit tests inside `src/pixel/geometry.rs`

**Interfaces:**
- Produces: `Geometry { rows: u32, columns: u32, samples_per_pixel: u16, bits_allocated: u16, frames: u32 }`; `geometry::frame_len_bytes(&Geometry) -> usize`; `geometry::mask_rect_in_place(pixels: &mut [u8], geo: &Geometry, frame: u32, x: u32, y: u32, w: u32, h: u32, fill: u16) -> Result<usize>`; `geometry::mask_polygon_in_place(pixels: &mut [u8], geo: &Geometry, frame: u32, points: &[[i64; 2]], fill: u16) -> Result<usize>`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn geo8() -> Geometry {
        Geometry { rows: 2, columns: 2, samples_per_pixel: 1, bits_allocated: 8, frames: 1 }
    }

    #[test]
    fn mask_rect_sets_exact_bytes_8bit() {
        let mut px = vec![0u8; 4];
        let n = mask_rect_in_place(&mut px, &geo8(), 0, 0, 0, 1, 1, 255).unwrap();
        assert_eq!(n, 1);
        assert_eq!(px, vec![255, 0, 0, 0]);
    }

    #[test]
    fn mask_rect_sets_exact_samples_16bit() {
        let geo = Geometry { rows: 1, columns: 2, samples_per_pixel: 1, bits_allocated: 16, frames: 1 };
        let mut px = vec![0u8; 4];
        let n = mask_rect_in_place(&mut px, &geo, 0, 1, 0, 1, 1, 7).unwrap();
        assert_eq!(n, 1);
        assert_eq!(px, vec![0, 0, 7, 0]); // little-endian u16
    }

    #[test]
    fn mask_is_multichannel_aware() {
        let geo = Geometry { rows: 1, columns: 1, samples_per_pixel: 3, bits_allocated: 8, frames: 1 };
        let mut px = vec![0u8; 3];
        let n = mask_rect_in_place(&mut px, &geo, 0, 0, 0, 1, 1, 9).unwrap();
        assert_eq!(n, 3);
        assert_eq!(px, vec![9, 9, 9]);
    }

    #[test]
    fn out_of_bounds_region_is_error() {
        let mut px = vec![0u8; 4];
        assert!(mask_rect_in_place(&mut px, &geo8(), 0, 1, 0, 2, 1, 1).is_err());
    }

    #[test]
    fn frame_selection_offsets_correctly() {
        let geo = Geometry { rows: 1, columns: 1, samples_per_pixel: 1, bits_allocated: 8, frames: 2 };
        let mut px = vec![0u8; 2];
        mask_rect_in_place(&mut px, &geo, 1, 0, 0, 1, 1, 5).unwrap();
        assert_eq!(px, vec![0, 5]);
    }

    #[test]
    fn polygon_masks_a_triangle() {
        let geo = Geometry { rows: 3, columns: 3, samples_per_pixel: 1, bits_allocated: 8, frames: 1 };
        let mut px = vec![0u8; 9];
        let pts = [[0i64, 0], [2, 0], [0, 2]];
        let n = mask_polygon_in_place(&mut px, &geo, 0, &pts, 1).unwrap();
        assert!(n >= 3);
        assert_eq!(px[0], 1); // inside
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test pixel::geometry 2>&1 | tail -30`
Expected: FAIL.

- [ ] **Step 3: Implement `src/pixel/geometry.rs` and `src/pixel/mod.rs`**

`src/pixel/mod.rs`:

```rust
pub mod geometry;
```

`src/pixel/geometry.rs`:

```rust
use crate::error::{AnonError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub rows: u32,
    pub columns: u32,
    pub samples_per_pixel: u16,
    pub bits_allocated: u16,
    pub frames: u32,
}

impl Geometry {
    pub fn bytes_per_sample(&self) -> usize {
        ((self.bits_allocated as usize) + 7) / 8
    }
    pub fn frame_len_bytes(&self) -> usize {
        self.rows as usize
            * self.columns as usize
            * self.samples_per_pixel as usize
            * self.bytes_per_sample()
    }
}

fn write_sample(px: &mut [u8], offset: usize, bytes: usize, fill: u16) {
    match bytes {
        1 => px[offset] = fill.min(255) as u8,
        2 => {
            let b = fill.to_le_bytes();
            px[offset] = b[0];
            px[offset + 1] = b[1];
        }
        _ => {}
    }
}

fn bounds_ok(geo: &Geometry, x: u32, y: u32, w: u32, h: u32) -> bool {
    w > 0 && h > 0 && x.checked_add(w).map_or(false, |r| r <= geo.columns)
        && y.checked_add(h).map_or(false, |r| r <= geo.rows)
}

/// Fill an axis-aligned rectangle; returns the number of samples written.
pub fn mask_rect_in_place(
    pixels: &mut [u8],
    geo: &Geometry,
    frame: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    fill: u16,
) -> Result<usize> {
    if !bounds_ok(geo, x, y, w, h) {
        return Err(AnonError::Dicom(format!(
            "region ({x},{y},{w},{h}) exceeds {columns}x{rows}",
            columns = geo.columns,
            rows = geo.rows
        )));
    }
    if frame >= geo.frames {
        return Err(AnonError::Dicom(format!("frame {frame} out of range")));
    }
    let bps = geo.bytes_per_sample();
    let frame_off = frame as usize * geo.frame_len_bytes();
    let mut written = 0usize;
    for row in y..y + h {
        for col in x..x + w {
            let sample0 = (row as usize * geo.columns as usize + col as usize)
                * geo.samples_per_pixel as usize;
            for s in 0..geo.samples_per_pixel as usize {
                let off = frame_off + (sample0 + s) * bps;
                write_sample(pixels, off, bps, fill);
                written += 1;
            }
        }
    }
    Ok(written)
}

fn point_in_polygon(x: f64, y: f64, pts: &[[i64; 2]]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (pts[i][0] as f64, pts[i][1] as f64);
        let (xj, yj) = (pts[j][0] as f64, pts[j][1] as f64);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Fill a polygon (even-odd rule); returns the number of samples written.
pub fn mask_polygon_in_place(
    pixels: &mut [u8],
    geo: &Geometry,
    frame: u32,
    points: &[[i64; 2]],
    fill: u16,
) -> Result<usize> {
    if points.len() < 3 {
        return Err(AnonError::Dicom("polygon needs >= 3 points".into()));
    }
    if frame >= geo.frames {
        return Err(AnonError::Dicom(format!("frame {frame} out of range")));
    }
    let bps = geo.bytes_per_sample();
    let frame_off = frame as usize * geo.frame_len_bytes();
    let mut written = 0usize;
    for row in 0..geo.rows {
        for col in 0..geo.columns {
            if !point_in_polygon(col as f64 + 0.5, row as f64 + 0.5, points) {
                continue;
            }
            let sample0 = (row as usize * geo.columns as usize + col as usize)
                * geo.samples_per_pixel as usize;
            for s in 0..geo.samples_per_pixel as usize {
                let off = frame_off + (sample0 + s) * bps;
                write_sample(pixels, off, bps, fill);
                written += 1;
            }
        }
    }
    Ok(written)
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test pixel::geometry 2>&1 | tail -30`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: pixel geometry and byte-level rect/polygon masking"
```

---

### Task 8: Region selection

**Files:**
- Create: `src/pixel/regions.rs`
- Modify: `src/pixel/mod.rs` (`pub mod regions;`)
- Test: unit tests inside `src/pixel/regions.rs`

**Interfaces:**
- Consumes: `config::{Config, Frames, Region, Shape, PixelMode}`.
- Produces: `regions::select<'a>(cfg: &'a Config, modality: Option<&str>, sop_class_uid: Option<&str>) -> Vec<&'a Region>`; `regions::frames_for(region: &Region, total: u32) -> Vec<u32>`.

- [ ] **Step 1: Write failing tests**

```rust
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
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test pixel::regions 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/pixel/regions.rs`**

```rust
use crate::config::{Config, Frames, Region};

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

pub fn frames_for(region: &Region, total: u32) -> Vec<u32> {
    match &region.frames {
        Frames::All => (0..total).collect(),
        Frames::List(v) => v.iter().copied().filter(|f| *f < total).collect(),
    }
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test pixel::regions 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: pixel region selection by modality/SOP class/frames"
```

---

### Task 9: Pixel stage orchestration (transcode + mask)

**Files:**
- Create: `src/pixel/stage.rs`
- Modify: `src/pixel/mod.rs` (`pub mod stage;`)
- Test: unit tests inside `src/pixel/stage.rs`

**Interfaces:**
- Consumes: `config::{Config, FillValue, OnMissing, PixelMode, Shape}`, `pixel::geometry::*`, `pixel::regions::*`, `dicom-object`, `dicom-pixeldata::{PixelDecoder, Transcode}`, `dicom-transfer-syntax-registry`.
- Produces: `stage::process(obj: &mut FileDicomObject<InMemDicomObject>, cfg: &Config) -> Result<usize>` returning the number of masked samples; `stage::geometry_of(rows, columns, spp, bits, frames) -> Geometry` helper used by tests.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn geometry_defaults_absent_frames_to_one() {
        let g = geometry_from_tags(Some(2), Some(2), None, None, None);
        assert_eq!(g.frames, 1);
        assert_eq!(g.rows, 2);
        assert_eq!(g.samples_per_pixel, 1);
        assert_eq!(g.bits_allocated, 8);
    }

    #[test]
    fn missing_rows_or_columns_is_error_when_masking() {
        assert!(geometry_from_tags(None, Some(2), None, None, None).rows == 0);
    }

    #[test]
    fn fill_resolution_max_uses_bits_allocated() {
        let cfg = Config::from_toml_str("[pixels]\nfill = \"max\"\n").unwrap();
        assert_eq!(resolve_fill(&cfg, 8), 255);
        assert_eq!(resolve_fill(&cfg, 16), 65535);
        let cfg0 = Config::from_toml_str("[pixels]\nfill = 7\n").unwrap();
        assert_eq!(resolve_fill(&cfg0, 16), 7);
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test pixel::stage 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/pixel/stage.rs`**

```rust
use crate::config::{Config, FillValue, OnMissing, PixelMode, Shape};
use crate::error::{AnonError, Result};
use crate::pixel::geometry::{mask_polygon_in_place, mask_rect_in_place, Geometry};
use crate::pixel::regions::{frames_for, select};
use dicom_core::VR;
use dicom_dictionary_std::tags;
use dicom_object::{DataElement, FileDicomObject, InMemDicomObject};
use dicom_pixeldata::{PixelDecoder, Transcode};

pub fn geometry_from_tags(
    rows: Option<u32>,
    columns: Option<u32>,
    frames: Option<u32>,
    samples_per_pixel: Option<u16>,
    bits_allocated: Option<u16>,
) -> Geometry {
    Geometry {
        rows: rows.unwrap_or(0),
        columns: columns.unwrap_or(0),
        samples_per_pixel: samples_per_pixel.unwrap_or(1),
        bits_allocated: bits_allocated.unwrap_or(8),
        frames: frames.unwrap_or(1),
    }
}

pub fn resolve_fill(cfg: &Config, bits: u16) -> u16 {
    match &cfg.pixels.fill {
        FillValue::Num(n) => *n,
        FillValue::Named(_) => {
            if bits >= 16 {
                u16::MAX
            } else {
                255
            }
        }
    }
}

fn get_u32(obj: &InMemDicomObject, t: dicom_core::Tag) -> Option<u32> {
    obj.element(t).ok().and_then(|e| e.to_int::<u32>().ok())
}
fn get_u16(obj: &InMemDicomObject, t: dicom_core::Tag) -> Option<u16> {
    obj.element(t).ok().and_then(|e| e.to_int::<u16>().ok())
}

/// Decode (transcoding compressed -> Explicit VR LE), mask configured regions,
/// and write the pixel data back uncompressed. Returns masked sample count.
pub fn process(obj: &mut FileDicomObject<InMemDicomObject>, cfg: &Config) -> Result<usize> {
    if cfg.pixels.mode == PixelMode::Keep {
        return Ok(0);
    }
    let modality = obj.element(tags::MODALITY).ok().and_then(|e| e.to_str().ok()).map(str::to_string);
    let sop = obj
        .element(tags::SOP_CLASS_UID)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(str::to_string);
    let selected = select(cfg, modality.as_deref(), sop.as_deref());
    if selected.is_empty() {
        return Ok(0);
    }

    // Burned-in annotation gate.
    let burned = obj
        .element(tags::BURNED_IN_ANNOTATION)
        .ok()
        .and_then(|e| e.to_str().ok());
    if burned.is_none() && cfg.pixels.on_missing_burnedin == OnMissing::Skip {
        return Ok(0);
    }

    // Decompress to Explicit VR LE if needed.
    let ts = dicom_transfer_syntax_registry::entries::EXPLICIT_VR_LITTLE_ENDIAN.erased();
    if obj.meta().transfer_syntax() != "1.2.840.10008.1.2.1" {
        obj.transcode_with_options(&ts, dicom_encoding::adapters::EncodeOptions::default())
            .map_err(|e| AnonError::Dicom(format!("transcode failed: {e}")))?;
    }
    if obj.element(tags::PIXEL_DATA).is_err() {
        return Ok(0); // no pixel data (e.g. SR): nothing to mask
    }

    let geo = geometry_from_tags(
        get_u32(obj, tags::ROWS),
        get_u32(obj, tags::COLUMNS),
        get_u32(obj, tags::NUMBER_OF_FRAMES),
        get_u16(obj, tags::SAMPLES_PER_PIXEL),
        get_u16(obj, tags::BITS_ALLOCATED),
    );
    if geo.rows == 0 || geo.columns == 0 {
        return Err(AnonError::Dicom("missing Rows/Columns".into()));
    }
    let fill = resolve_fill(cfg, geo.bits_allocated);

    let mut pixels = obj
        .element(tags::PIXEL_DATA)
        .map_err(|e| AnonError::Dicom(e.to_string()))?
        .to_bytes()
        .map_err(|e| AnonError::Dicom(e.to_string()))?
        .to_vec();

    let expected = geo.frame_len_bytes() * geo.frames as usize;
    if pixels.len() < expected {
        return Err(AnonError::Dicom(format!(
            "pixel buffer {} bytes < expected {}",
            pixels.len(),
            expected
        )));
    }

    let mut masked = 0usize;
    for region in selected {
        for frame in frames_for(region, geo.frames) {
            match region.shape {
                Shape::Rect => {
                    masked += mask_rect_in_place(
                        &mut pixels, &geo, frame, region.x, region.y, region.width,
                        region.height, fill,
                    )?;
                }
                Shape::Polygon => {
                    masked += mask_polygon_in_place(&mut pixels, &geo, frame, &region.points, fill)?;
                }
            }
        }
    }

    // Write back uncompressed: OB for <=8 bits, OW (u16 LE) for >8 bits.
    let elem = if geo.bits_allocated > 8 {
        let mut words = Vec::with_capacity(pixels.len() / 2);
        for c in pixels.chunks_exact(2) {
            words.push(u16::from_le_bytes([c[0], c[1]]));
        }
        DataElement::new(tags::PIXEL_DATA, VR::OW, words)
    } else {
        DataElement::new(tags::PIXEL_DATA, VR::OB, pixels)
    };
    obj.put_element(elem);
    Ok(masked)
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test pixel::stage 2>&1 | tail -30`
Expected: PASS (3 tests). `to_int` / `to_bytes` names are confirmed on `InMemElement`; if a name differs, adapt this file only.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: pixel stage transcode + region masking"
```

---

### Task 10: Mirrored atomic writer and audit records

**Files:**
- Create: `src/writer.rs`, `src/audit.rs`
- Modify: `src/main.rs` (add `mod writer; mod audit;`)
- Test: unit tests in both files

**Interfaces:**
- Produces: `writer::output_path(out_root: &Path, rel: &Path) -> PathBuf`; `writer::write_atomic(path: &Path, bytes: &[u8]) -> Result<()>`; `audit::Record { input, output, status, error, transfer_syntax_in, transfer_syntax_out, tags_changed, regions_masked, output_sha256, duration_ms }`, `audit::Status`, `audit::Record::new(...)`, `audit::write_jsonl(w, &Record) -> Result<()>`.

- [ ] **Step 1: Write failing tests**

`src/writer.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mirrors_relative_path_under_output_root() {
        let p = output_path(Path::new("/out"), Path::new("a/b/c.dcm"));
        assert!(p.ends_with("a/b/c.dcm"));
        assert!(p.starts_with("/out"));
    }

    #[test]
    fn write_atomic_creates_dirs_and_no_temp_left() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("x/y.bin");
        write_atomic(&target, b"hello").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
```

`src/audit.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_record_has_required_fields() {
        let r = Record::ok("in.dcm", "out.dcm", 3, 2);
        let s = serde_json::to_string(&r).unwrap();
        for k in ["input", "output", "status", "tags_changed", "regions_masked"] {
            assert!(s.contains(k), "missing {k}");
        }
    }

    #[test]
    fn error_record_serializes_reason() {
        let r = Record::error("in.dcm", "boom");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("boom"));
        assert!(s.contains("\"status\":\"error\""));
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test writer:: audit:: 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/writer.rs`**

```rust
use crate::error::Result;
use std::path::{Path, PathBuf};

pub fn output_path(out_root: &Path, rel: &Path) -> PathBuf {
    out_root.join(rel)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension().and_then(|e| e.to_str()).map(|e| format!("{e}.")).unwrap_or_default()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
```

- [ ] **Step 4: Implement `src/audit.rs`**

```rust
use crate::error::Result;
use serde::Serialize;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Skipped,
    SkippedDicomdir,
    Unsupported,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub input: String,
    pub output: Option<String>,
    pub status: Status,
    pub error: Option<String>,
    pub transfer_syntax_in: Option<String>,
    pub transfer_syntax_out: Option<String>,
    pub tags_changed: usize,
    pub regions_masked: usize,
    pub output_sha256: Option<String>,
    pub duration_ms: u128,
}

impl Record {
    pub fn ok(input: impl Into<String>, output: impl Into<String>, tags: usize, masked: usize) -> Record {
        Record {
            input: input.into(),
            output: Some(output.into()),
            status: Status::Ok,
            error: None,
            transfer_syntax_in: None,
            transfer_syntax_out: None,
            tags_changed: tags,
            regions_masked: masked,
            output_sha256: None,
            duration_ms: 0,
        }
    }

    pub fn error(input: impl Into<String>, reason: impl Into<String>) -> Record {
        Record {
            input: input.into(),
            output: None,
            status: Status::Error,
            error: Some(reason.into()),
            transfer_syntax_in: None,
            transfer_syntax_out: None,
            tags_changed: 0,
            regions_masked: 0,
            output_sha256: None,
            duration_ms: 0,
        }
    }

    pub fn skipped(input: impl Into<String>, status: Status) -> Record {
        Record {
            input: input.into(),
            output: None,
            status,
            error: None,
            transfer_syntax_in: None,
            transfer_syntax_out: None,
            tags_changed: 0,
            regions_masked: 0,
            output_sha256: None,
            duration_ms: 0,
        }
    }
}

pub fn write_jsonl(mut w: impl Write, r: &Record) -> Result<()> {
    serde_json::to_writer(&mut w, r).map_err(|e| crate::error::AnonError::Config(e.to_string()))?;
    writeln!(w)?;
    Ok(())
}
```

- [ ] **Step 5: Add modules to `src/main.rs`**

```rust
mod audit;
mod config;
mod deid;
mod error;
mod pixel;
mod writer;

fn main() {
    println!("dicom-anonymizer");
}
```

- [ ] **Step 6: Run tests, expect pass**

Run: `cargo test writer:: audit:: 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: mirrored atomic writer and JSONL audit records"
```

---

### Task 11: Discovery (walk + DICM detection)

**Files:**
- Create: `src/discovery.rs`
- Modify: `src/main.rs` (add `mod discovery;`)
- Test: unit tests inside `src/discovery.rs`

**Interfaces:**
- Produces: `Job { input: PathBuf, rel: PathBuf }`; `discovery::is_dicom_file(&Path) -> bool`; `discovery::discover(&Path) -> Result<Vec<Job>>`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_dicom_like(p: &std::path::Path) {
        let mut v = vec![0u8; 128];
        v.extend_from_slice(b"DICM");
        v.extend_from_slice(&[0u8; 16]);
        std::fs::write(p, v).unwrap();
    }

    #[test]
    fn detects_dicom_by_magic() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.dcm");
        write_dicom_like(&f);
        assert!(is_dicom_file(&f));
    }

    #[test]
    fn rejects_non_dicom() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        std::fs::write(&f, b"not a dicom").unwrap();
        assert!(!is_dicom_file(&f));
    }

    #[test]
    fn discover_walks_and_keeps_relative_paths() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        write_dicom_like(&d.path().join("top.dcm"));
        write_dicom_like(&d.path().join("sub/deep.dcm"));
        std::fs::write(d.path().join("sub/readme.txt"), b"x").unwrap();
        let jobs = discover(d.path()).unwrap();
        let rels: Vec<String> = jobs
            .iter()
            .map(|j| j.rel.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(rels.contains(&"top.dcm".to_string()));
        assert!(rels.contains(&"sub/deep.dcm".to_string()));
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn empty_dir_yields_no_jobs() {
        let d = tempfile::tempdir().unwrap();
        assert!(discover(d.path()).unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test discovery:: 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/discovery.rs`**

```rust
use crate::error::Result;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub input: PathBuf,
    pub rel: PathBuf,
}

pub fn is_dicom_file(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 132];
    match f.read_exact(&mut head) {
        Ok(()) => &head[128..132] == b"DICM",
        Err(_) => false,
    }
}

pub fn discover(input: &Path) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    for entry in WalkDir::new(input).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_dicom_file(path) {
            continue;
        }
        let rel = path.strip_prefix(input).unwrap_or(path).to_path_buf();
        jobs.push(Job { input: path.to_path_buf(), rel });
    }
    jobs.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(jobs)
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test discovery:: 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: recursive discovery with DICM magic detection"
```

---

### Task 12: Per-file pipeline

**Files:**
- Create: `src/pipeline.rs`
- Modify: `src/main.rs` (add `mod pipeline;`)
- Test: unit tests inside `src/pipeline.rs`

**Interfaces:**
- Consumes: `config::Config`, `deid::{uids::UidMapper, dates::DateShifter, actions::{DeidContext, apply}}`, `pixel::stage::process`, `writer`, `audit`.
- Produces: `pipeline::Shared { cfg: Config, uids: UidMapper, dates: DateShifter, out_root: PathBuf, dry_run: bool }`; `pipeline::process_job(shared: &Shared, job: &Job, log: &mut impl Write) -> Result<audit::Record>`.

- [ ] **Step 1: Write failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::Job;
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    use dicom_core::VR;
    use dicom_object::DataElement;

    fn make_dicom(path: &std::path::Path) {
        let obj = InMemDicomObject::from_element_iter([
            DataElement::new(Tag(0x0010, 0x0010), VR::PN, "Doe^John"),
            DataElement::new(Tag(0x0020, 0x000D), VR::UI, "1.2.3"),
        ]);
        let file = obj
            .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
            .unwrap();
        file.write_to_file(path).unwrap();
    }

    #[test]
    fn anonymizes_and_writes_mirrored_output() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        make_dicom(&inpath);

        let cfg = crate::config::Config::from_toml_str("[dates]\nstrategy = \"zero\"\n").unwrap();
        let shared = Shared {
            cfg,
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: false,
        };
        let job = Job { input: inpath.clone(), rel: "p.dcm".into() };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, crate::audit::Status::Ok);
        let written = out.path().join("p.dcm");
        assert!(written.exists());

        let o = dicom_object::open_file(&written).unwrap();
        let name = o.element(Tag(0x0010, 0x0010)).ok().and_then(|e| e.to_str().ok().map(str::to_string));
        assert_ne!(name.as_deref(), Some("Doe^John"));
    }

    #[test]
    fn dry_run_writes_nothing_but_returns_record() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let inpath = src.path().join("p.dcm");
        make_dicom(&inpath);
        let shared = Shared {
            cfg: crate::config::Config::default(),
            uids: crate::deid::uids::UidMapper::new("2.25.", b"s"),
            dates: crate::deid::dates::DateShifter::new(b"s", 10),
            out_root: out.path().to_path_buf(),
            dry_run: true,
        };
        let job = Job { input: inpath, rel: "p.dcm".into() };
        let mut sink = Vec::new();
        let rec = process_job(&shared, &job, &mut sink).unwrap();
        assert_eq!(rec.status, crate::audit::Status::Ok);
        assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test pipeline:: 2>&1 | tail -30`
Expected: FAIL.

- [ ] **Step 3: Implement `src/pipeline.rs`**

```rust
use crate::audit::{Record, Status};
use crate::config::Config;
use crate::deid::actions::{apply, DeidContext};
use crate::deid::dates::DateShifter;
use crate::deid::uids::UidMapper;
use crate::discovery::Job;
use crate::error::{AnonError, Result};
use crate::pixel::stage;
use crate::{audit, writer};
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

pub struct Shared {
    pub cfg: Config,
    pub uids: UidMapper,
    pub dates: DateShifter,
    pub out_root: PathBuf,
    pub dry_run: bool,
}

const DICOMDIR_SOP: &str = "1.2.840.10008.1.3.10";

pub fn process_job(shared: &Shared, job: &Job, log: &mut impl Write) -> Result<Record> {
    let start = Instant::now();
    let input_str = job.rel.to_string_lossy().replace('\\', "/");

    let mut obj = match dicom_object::open_file(&job.input) {
        Ok(o) => o,
        Err(e) => {
            let rec = Record::error(input_str, format!("open: {e}"));
            let _ = audit::write_jsonl(&mut *log, &rec);
            return Ok(rec);
        }
    };

    let ts_in = obj.meta().transfer_syntax().to_string();
    let sop = obj
        .element(dicom_dictionary_std::tags::SOP_CLASS_UID)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(str::to_string);
    if sop.as_deref() == Some(DICOMDIR_SOP) {
        let rec = Record::skipped(input_str, Status::SkippedDicomdir);
        let _ = audit::write_jsonl(&mut *log, &rec);
        return Ok(rec);
    }

    let ctx = DeidContext { cfg: &shared.cfg, uids: &shared.uids, dates: &shared.dates };
    let tags_changed = match apply(&mut obj, &ctx) {
        Ok(n) => n,
        Err(e) => {
            let rec = Record::error(input_str, format!("deid: {e}"));
            let _ = audit::write_jsonl(&mut *log, &rec);
            return Ok(rec);
        }
    };

    let regions_masked = match stage::process(&mut obj, &shared.cfg) {
        Ok(n) => n,
        Err(e) => {
            let rec = Record::error(input_str, format!("pixel: {e}"));
            let _ = audit::write_jsonl(&mut *log, &rec);
            return Ok(rec);
        }
    };

    // Serialize to bytes, then write (or discard when dry-run).
    let mut buf = Vec::new();
    let ts = dicom_transfer_syntax_registry::entries::EXPLICIT_VR_LITTLE_ENDIAN.erased();
    if let Err(e) = obj.write_dataset_with_ts(&mut buf, &ts) {
        let rec = Record::error(input_str, format!("serialize: {e}"));
        let _ = audit::write_jsonl(&mut *log, &rec);
        return Ok(rec);
    }

    let mut rec = Record::ok(input_str, job.rel.to_string_lossy().replace('\\', "/"), tags_changed, regions_masked);
    rec.transfer_syntax_in = Some(ts_in);
    rec.transfer_syntax_out = Some("1.2.840.10008.1.2.1".to_string());
    rec.duration_ms = start.elapsed().as_millis();

    if !shared.dry_run {
        let target = writer::output_path(&shared.out_root, &job.rel);
        if let Err(e) = writer::write_atomic(&target, &buf) {
            let rec = Record::error(
                job.rel.to_string_lossy().replace('\\', "/"),
                format!("write: {e}"),
            );
            let _ = audit::write_jsonl(&mut *log, &rec);
            return Ok(rec);
        }
        rec.output_sha256 = Some(sha256_hex(&buf));
    } else {
        rec.output = None;
    }

    let _ = AnonError::Config; // keep import meaningful for future fatal errors
    let _ = audit::write_jsonl(&mut *log, &rec);
    Ok(rec)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
```

- [ ] **Step 4: Run tests, expect pass**

Run: `cargo test pipeline:: 2>&1 | tail -30`
Expected: PASS (2 tests). If `write_dataset_with_ts` needs the file-meta form, use `obj.write_to_file` for the non-dry path and skip serialization; keep the dry-run test's "no files" assertion.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: per-file de-identification pipeline"
```

---

### Task 13: CLI, orchestration, summary, exit codes

**Files:**
- Create: `src/cli.rs`, `src/report.rs`
- Modify: `src/main.rs` (full rewrite)
- Test: unit tests in `src/cli.rs` and `src/report.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `cli::Args` (clap `Parser`); `report::Summary { ok, skipped, unsupported, error }`, `report::Summary::exit_code(&self) -> i32`.

- [ ] **Step 1: Write failing tests**

`src/cli.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_required_args() {
        let a = Args::try_parse_from(["dicom-anonymizer", "--input", "in", "--output", "out"]).unwrap();
        assert_eq!(a.input.to_string_lossy(), "in");
        assert_eq!(a.output.to_string_lossy(), "out");
        assert!(!a.dry_run);
    }

    #[test]
    fn jobs_defaults_to_none() {
        let a = Args::try_parse_from(["x", "-i", "in", "-o", "out"]).unwrap();
        assert_eq!(a.jobs, None);
    }
}
```

`src/report.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Status;

    #[test]
    fn all_ok_is_exit_zero() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        assert_eq!(s.exit_code(), 0);
    }

    #[test]
    fn any_error_is_exit_one() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        s.count(Status::Error);
        assert_eq!(s.exit_code(), 1);
    }

    #[test]
    fn skipped_only_is_exit_zero() {
        let mut s = Summary::default();
        s.count(Status::Skipped);
        s.count(Status::SkippedDicomdir);
        assert_eq!(s.exit_code(), 0);
    }
}
```

- [ ] **Step 2: Run tests, expect failure**

Run: `cargo test cli:: report:: 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Implement `src/cli.rs`**

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "dicom-anonymizer", about = "Bulk DICOM de-identification")]
pub struct Args {
    #[arg(short, long)]
    pub input: PathBuf,
    #[arg(short, long)]
    pub output: PathBuf,
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    #[arg(short, long)]
    pub jobs: Option<usize>,
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
    #[arg(long, default_value_t = false)]
    pub fail_fast: bool,
    #[arg(short, long, default_value_t = false)]
    pub verbose: bool,
}
```

- [ ] **Step 4: Implement `src/report.rs`**

```rust
use crate::audit::Status;

#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub ok: usize,
    pub skipped: usize,
    pub unsupported: usize,
    pub error: usize,
}

impl Summary {
    pub fn count(&mut self, status: Status) {
        match status {
            Status::Ok => self.ok += 1,
            Status::Skipped | Status::SkippedDicomdir => self.skipped += 1,
            Status::Unsupported => self.unsupported += 1,
            Status::Error => self.error += 1,
        }
    }

    pub fn exit_code(&self) -> i32 {
        if self.error > 0 {
            1
        } else {
            0
        }
    }
}
```

- [ ] **Step 5: Implement `src/main.rs`**

```rust
mod audit;
mod cli;
mod config;
mod deid;
mod discovery;
mod error;
mod pipeline;
mod pixel;
mod report;
mod writer;

use clap::Parser;
use pipeline::Shared;
use std::io::Write;

fn main() {
    let code = run();
    std::process::exit(code);
}

fn run() -> i32 {
    let args = cli::Args::parse();

    let cfg = match &args.config {
        Some(p) => match config::Config::load(p) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        },
        None => config::Config::default(),
    };

    if !args.input.is_dir() {
        eprintln!("error: input `{}` is not a directory", args.input.display());
        return 2;
    }
    if let Err(e) = std::fs::create_dir_all(&args.output) {
        eprintln!("error: cannot create output dir: {e}");
        return 2;
    }

    let uid_salt = if cfg.uids.salt.is_empty() {
        random_salt()
    } else {
        cfg.uids.salt.clone().into_bytes()
    };
    let date_salt = if cfg.dates.shift_salt.is_empty() {
        random_salt()
    } else {
        cfg.dates.shift_salt.clone().into_bytes()
    };

    let shared = Shared {
        uids: deid::uids::UidMapper::new(&cfg.uids.root, &uid_salt),
        dates: deid::dates::DateShifter::new(&date_salt, cfg.dates.range_days),
        cfg,
        out_root: args.output.clone(),
        dry_run: args.dry_run,
    };

    let jobs = match discovery::discover(&args.input) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    // Audit sink: stdout in dry-run, a file otherwise.
    let mut file_sink;
    let mut stdout = std::io::stdout();
    let log: &mut dyn Write = if args.dry_run {
        &mut stdout
    } else {
        let path = args.output.join("audit.jsonl");
        match std::fs::File::create(&path) {
            Ok(f) => {
                file_sink = f;
                &mut file_sink
            }
            Err(e) => {
                eprintln!("error: cannot create audit log: {e}");
                return 2;
            }
        }
    };

    let mut summary = report::Summary::default();
    for job in &jobs {
        let mut line = Vec::new();
        match pipeline::process_job(&shared, job, &mut line) {
            Ok(rec) => {
                summary.count(rec.status);
                let _ = log.write_all(&line);
                if args.fail_fast && rec.status == audit::Status::Error {
                    break;
                }
            }
            Err(e) => {
                summary.count(audit::Status::Error);
                eprintln!("error: {}: {e}", job.rel.display());
                if args.fail_fast {
                    break;
                }
            }
        }
    }

    eprintln!(
        "ok={} skipped={} unsupported={} error={}",
        summary.ok, summary.skipped, summary.unsupported, summary.error
    );
    summary.exit_code()
}

fn random_salt() -> Vec<u8> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}-{}", std::process::id()).into_bytes()
}
```

> The plan runs the batch sequentially first (simple, deterministic test story). Task 14 adds rayon parallelism on top of `process_job`, which is already `&Shared`-only and therefore thread-safe.

- [ ] **Step 6: Run tests, expect pass**

Run: `cargo test cli:: report:: 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 7: Build the binary**

Run: `cargo build 2>&1 | tail -20`
Expected: builds with no errors.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: CLI, orchestration, summary, and exit codes"
```

---

### Task 14: End-to-end integration and parallelism

**Files:**
- Create: `tests/common/mod.rs`, `tests/integration.rs`
- Modify: `src/main.rs` (parallelize the job loop with rayon when `--jobs` is set)

**Interfaces:**
- Consumes: public binary behavior only.

- [ ] **Step 1: Write `tests/common/mod.rs`**

```rust
use dicom_core::VR;
use dicom_object::{DataElement, FileMetaTableBuilder, InMemDicomObject};
use std::path::Path;

pub fn write_fixture(path: &Path, name: &str, study_uid: &str) {
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(dicom_dictionary_std::tags::PATIENT_NAME, VR::PN, name),
        DataElement::new(dicom_dictionary_std::tags::PATIENT_ID, VR::LO, "PID-1"),
        DataElement::new(dicom_dictionary_std::tags::STUDY_INSTANCE_UID, VR::UI, study_uid),
        DataElement::new(dicom_dictionary_std::tags::MODALITY, VR::CS, "CT"),
    ]);
    let file = obj
        .with_meta(FileMetaTableBuilder::new().transfer_syntax("1.2.840.10008.1.2.1"))
        .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    file.write_to_file(path).unwrap();
}
```

- [ ] **Step 2: Write `tests/integration.rs`**

```rust
mod common;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn end_to_end_mirrors_tree_and_deidentifies() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("a/one.dcm"), "Doe^John", "1.2.3");
    common::write_fixture(&src.path().join("b/two.dcm"), "Roe^Jane", "1.2.3");
    std::fs::write(src.path().join("b/notes.txt"), b"ignore me").unwrap();

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .success();

    assert!(out.path().join("a/one.dcm").exists());
    assert!(out.path().join("b/two.dcm").exists());
    assert!(!out.path().join("b/notes.txt").exists());
    assert!(out.path().join("audit.jsonl").exists());

    let o1 = dicom_object::open_file(out.path().join("a/one.dcm")).unwrap();
    let n1 = o1
        .element(dicom_dictionary_std::tags::PATIENT_NAME)
        .ok()
        .and_then(|e| e.to_str().ok().map(str::to_string));
    assert_ne!(n1.as_deref(), Some("Doe^John"));

    // Same original study UID in both files must map to the same new UID.
    let u1 = o1
        .element(dicom_dictionary_std::tags::STUDY_INSTANCE_UID)
        .ok()
        .and_then(|e| e.to_str().ok().map(str::to_string));
    let o2 = dicom_object::open_file(out.path().join("b/two.dcm")).unwrap();
    let u2 = o2
        .element(dicom_dictionary_std::tags::STUDY_INSTANCE_UID)
        .ok()
        .and_then(|e| e.to_str().ok().map(str::to_string));
    assert_eq!(u1, u2);
    assert!(u1.unwrap().starts_with("2.25."));
}

#[test]
fn dry_run_writes_no_files() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    common::write_fixture(&src.path().join("one.dcm"), "Doe^John", "1.2.3");

    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("\"status\":\"ok\""));

    assert!(std::fs::read_dir(out.path()).unwrap().next().is_none());
}

#[test]
fn empty_input_is_success() {
    let src = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    Command::cargo_bin("dicom-anonymizer")
        .unwrap()
        .args(["-i"])
        .arg(src.path())
        .args(["-o"])
        .arg(out.path())
        .assert()
        .success();
}
```

- [ ] **Step 3: Run integration tests, expect failure or partial**

Run: `cargo test --test integration 2>&1 | tail -40`
Expected: at least the dry-run and study-UID cases fail initially (dry-run currently still writes the audit file to the output dir; study UID only consistent because mapping is a pure function — it should pass). Note the dry-run test asserts an empty output dir, so move the audit log to stdout in dry-run (already done) — verify no `audit.jsonl` is created.

- [ ] **Step 4: Add rayon parallelism to `src/main.rs`**

Replace the sequential loop body with:

```rust
use rayon::prelude::*;

let results: Vec<(report::Summary, Vec<u8>)> = if args.jobs.unwrap_or(0) > 0 {
    // Parallel: process each job into its own buffer, then emit in order.
    jobs.par_iter()
        .map(|job| {
            let mut line = Vec::new();
            let mut s = report::Summary::default();
            match pipeline::process_job(&shared, job, &mut line) {
                Ok(rec) => s.count(rec.status),
                Err(_) => s.count(audit::Status::Error),
            }
            (s, line)
        })
        .collect()
} else {
    jobs.iter()
        .map(|job| {
            let mut line = Vec::new();
            let mut s = report::Summary::default();
            match pipeline::process_job(&shared, job, &mut line) {
                Ok(rec) => s.count(rec.status),
                Err(_) => s.count(audit::Status::Error),
            }
            (s, line)
        })
        .collect()
};

for (s, line) in &results {
    summary.ok += s.ok;
    summary.skipped += s.skipped;
    summary.unsupported += s.unsupported;
    summary.error += s.error;
    let _ = log.write_all(line);
}
```

If `--jobs N` is given, call `rayon::ThreadPoolBuilder::new().num_threads(n).build_global()` before the loop. `Shared` borrows only immutable data (`Config`, `UidMapper`, `DateShifter`), so this is sound.

- [ ] **Step 5: Run the full suite and build**

Run: `cargo test 2>&1 | tail -30 && cargo build --release 2>&1 | tail -10`
Expected: all tests PASS; release binary produced at `target/release/dicom-anonymizer`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: end-to-end integration tests and optional rayon parallelism"
```

---

### Task 15: Docs and example config

**Files:**
- Create: `README.md`, `dicom-anonymizer.example.toml`
- Test: none (documentation); verified by `cargo build --release` from Task 14

**Interfaces:**
- Produces: user-facing usage docs.

- [ ] **Step 1: Write `dicom-anonymizer.example.toml`**

```toml
# Example configuration. Values shown are the defaults unless noted.
[profile]
base = "ps3.15-basic"                # or "research"
retain_longitudinal_temporal = false
retain_safe_private = false
clean_descriptors = false
clean_structured_content = false

[dates]
strategy = "shift"                   # shift | zero | remove
shift_salt = ""                      # empty => random per run (set for reproducibility)
range_days = 3650

[uids]
strategy = "remap"                   # remap | zero
root = "2.25."
salt = ""                            # empty => random per run

[pixels]
mode = "mask"                        # mask | keep
fill = 0                             # 0 | 255 | "max"
on_missing_burnedin = "warn"         # warn | skip | mask_all

# [[pixels.regions]]
# modality = "US"
# shape = "rect"
# x = 0
# y = 0
# width = 200
# height = 30
# frames = "all"

# [tags]
# "0010,0010" = "Z"
```

- [ ] **Step 2: Write `README.md`**

```markdown
# dicom-anonymizer

Standalone Rust CLI for bulk DICOM de-identification (PS3.15 Basic Application
Level Confidentiality Profile + options), with tag-driven burned-in pixel masking.

## Build

    cargo build --release
    # JPEG-LS support (needs CharLS + a C toolchain):
    cargo build --release --features charls

## Usage

    dicom-anonymizer --input ./in --output ./out [--config config.toml] \
      [--jobs N] [--dry-run] [--fail-fast] [--verbose]

- `--input` / `--output`: DICOM source tree and separate output tree (inputs are never modified).
- `--config`: TOML config; defaults to the built-in `ps3.15-basic` preset.
- `--dry-run`: run everything except writes and print audit records to stdout.

See `dicom-anonymizer.example.toml` for all options.

## Output

The output tree mirrors the input tree. `audit.jsonl` in the output directory has one
JSON object per file (status, changed tag count, masked-region count, transfer syntaxes,
output SHA-256). Exit codes: 0 success, 1 partial failure, 2 fatal error.

## Limits

Irreversible by design. JPEG-LS requires `--features charls`. DICOMDIR files are skipped.
Burned-in masking uses configured regions only; there is no OCR.
```

- [ ] **Step 3: Verify a release build and a manual smoke run**

Run:

```bash
cargo build --release 2>&1 | tail -5
mkdir -p /tmp/anon-in && ./target/release/dicom-anonymizer -i /tmp/anon-in -o /tmp/anon-out --dry-run
```

Expected: builds; prints `ok=0 skipped=0 unsupported=0 error=0`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: README and example configuration"
```

---

## Self-Review

**Spec coverage:** every spec section maps to a task — §6 CLI→Task 13; §7 config→Task 1; §8 rule engine→Tasks 5–6; §9 UID/date→Tasks 3–4; §10 pixel→Tasks 7–9; §11 discovery/output→Tasks 10–11; §12 audit→Task 10; §13 errors/exit→Tasks 12–13; §14 concurrency→Task 14; §15 testing→every task plus Task 14; §16 layout→File Structure; §18 risks→Review Focus.

**Placeholder scan:** no `TBD`/`TODO`; every code step shows code; the two "if the upstream name differs, adapt this file only" notes are guarded by the Task 2 wrapper and the pinned docs URL, not deferred work.

**Type consistency:** `Geometry`, `Job`, `Shared`, `DeidContext`, `Record`, `Status`, `Summary`, `UidMapper::map`, `DateShifter::{offset_for, shift_da, shift_dt}`, `stage::process`, `pipeline::process_job`, `writer::{output_path, write_atomic}` are defined once and referenced with identical signatures.

**Known execution note:** `apply_to_sequence` (Task 6) and the pixel write-back (Task 9) depend on two `dicom-rs` method names not fully pinned by reconnaissance (`sequence_mut`/`items_mut`; `to_int`/`to_bytes`). Both are isolated to single functions; if the compiler disagrees, fix only that function against the pinned docs URL and keep the tested contract.
