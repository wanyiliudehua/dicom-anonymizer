# dicom-anonymizer

Standalone Rust CLI for bulk DICOM de-identification (PS3.15 Basic Application
Level Confidentiality Profile + options), with tag-driven burned-in pixel masking.

## Build

    cargo build --release
    # JPEG-LS support (needs CharLS + a C toolchain):
    cargo build --release --features charls

The release binary is written to
`target/x86_64-pc-windows-gnullvm/release/dicom-anonymizer.exe`.

### Windows toolchain note

This repository's `.cargo/config.toml` targets `x86_64-pc-windows-gnullvm`
because the host has no working MinGW `dlltool` and no MSVC linker. It uses a
thin `rust-lld` wrapper that selects the correct CRT object per crate type,
reusing the GNU target's MinGW import libraries. Regenerate that wrapper with:

    scripts/setup-toolchain.sh

## Usage

    dicom-anonymizer --input ./in --output ./out [--config config.toml] \
      [--jobs N] [--dry-run] [--fail-fast] [--verbose]

- `--input` / `--output`: DICOM source tree and a separate output tree
  (inputs are never modified).
- `--config`: TOML config; defaults to the built-in `ps3.15-basic` preset.
- `--strategy conservative|aggressive`: de-identification level (overrides the
  config). **aggressive** (default) clears every potentially identifying
  attribute and keeps only the image's own clinical parameters.
  **conservative** clears the direct identifiers (name, ID, birth date,
  accession number, institution, device serial, physician names, ...) but
  retains patient demographics (sex, age, size, weight) and all dates/times.
- `--jobs N`: process files in parallel with N threads.
- `--dry-run`: run everything except writes and print audit records to stdout.

See `dicom-anonymizer.example.toml` for all options.

## Output

The output tree mirrors the input tree. By default, each output file is renamed
to its **remapped SOP Instance UID** (`[output] filename = "sop_instance_uid"`),
so the original instance UID cannot leak through the filename; set
`filename = "preserve"` to keep the original relative filenames. `audit.jsonl` in
the output directory has one JSON object per file (status, changed tag count,
masked-region count, transfer syntaxes, output SHA-256). Exit codes: `0` success,
`1` partial failure, `2` fatal error.

## De-identification table

The base rule table is the **full PS3.15 Table E.1-1** (Basic Application Level
Confidentiality Profile, ~650 attributes), generated from the standard by
`scripts/gen_e1_1.py` into `src/deid/e1_1.rs`. Ambiguous `Basic Prof.` cells
(e.g. `X/Z`, `X/Z/D`) are collapsed to one action, preferring `Z` (keeps the
element present, valid for Type 1/2 attributes). A few attributes not listed in
E.1-1 are added as overrides (Manufacturer, Manufacturer Model Name, Software
Versions); `SOP Class UID`, `Pixel Data` and `Burned In Annotation` are pinned
to keep. Repeating-group rows that cannot be a single tag — Curve Data
`(50xx,xxxx)`, Overlay Data/Comments `(60xx,3000|4000)` — are handled as ranges,
and private (odd-group) tags are always removed.

Date shifting is applied with `[profile] retain_longitudinal_temporal = true`
(the E.1-1 *Retain Longitudinal Temporal Information — Modified Dates* option:
dates are shifted, times kept); `retain_longitudinal_full_dates = true` keeps
dates verbatim. Otherwise each date/time attribute takes its E.1-1 action.

### PS3.15 options

All eleven E.1-1 option columns are implemented. Enabling an option sets the
action from that column for every attribute the standard marks, overriding the
Basic Profile action (options are resolved in table column order; later wins):

| config flag | option |
|---|---|
| `retain_safe_private` | Retain Safe Private |
| `retain_uids` | Retain UIDs |
| `retain_device_identity` | Retain Device Identity |
| `retain_institution_identity` | Retain Institution Identity |
| `retain_patient_characteristics` | Retain Patient Characteristics |
| `retain_longitudinal_full_dates` | Retain Longitudinal Temporal Information (Full Dates) |
| `retain_longitudinal_temporal` | Retain Longitudinal Temporal Information (Modified Dates) |
| `clean_descriptors` | Clean Descriptors |
| `clean_structured_content` | Clean Structured Content |
| `clean_graphics` | Clean Graphics |

`C` (“clean”) in the standard is a content-aware operation; with no NLP this
tool approximates it by neutralizing the value (and, for the *Modified Dates*
option, by shifting dates). `clean_structured_content` / `clean_graphics` are
accepted but only exercised on the attributes E.1-1 marks.

## Limits

Irreversible by design. JPEG-LS requires `--features charls`. DICOMDIR files
are skipped. Burned-in masking uses configured regions only; there is no OCR.
With the default random salts, results are not reproducible across runs; set
`dates.shift_salt` and `uids.salt` for reproducibility.
