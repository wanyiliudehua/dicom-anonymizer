# dicom-anonymizer

[![CI](https://github.com/OWNER/REPO/actions/workflows/ci.yml/badge.svg)](https://github.com/OWNER/REPO/actions/workflows/ci.yml)
<!-- Replace OWNER/REPO above with your GitHub slug once the remote is set. -->

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
      [--jobs N] [--dry-run] [--check-leaks] [--export-identifiers FILE] \
      [--fail-fast] [--verbose]

    # later, with no need for the originals:
    dicom-anonymizer --input ./out --verify fingerprint.json

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
- `--check-leaks`: after the run, verify the result by re-reading the input tree for
  identifying strings (person names and their components, patient / other-patient IDs,
  accession number, institution name/address/department words) and scanning every
  output value for them. Any hit is printed and the run exits `1`, so an attribute the
  profile was told to retain cannot silently carry a name through.
- `--export-identifiers FILE`: also write a **salted fingerprint** of the input tree's
  identifiers, so the result can be re-verified later (see below).
- `--verify FILE`: scan `--input` against a fingerprint *instead of* anonymizing. Needs
  neither `--config` nor `--output`; prints every hit with its file, tag and offending
  text, and exits `1` if anything survived.

### Verifying later

A run can record what was identifying, so a delivered tree can be audited afterwards
without the originals:

    dicom-anonymizer -i ./in  -o ./out --export-identifiers ids.json   # during the run
    dicom-anonymizer -i ./out --verify ids.json                        # later, anywhere

`ids.json` contains only salted SHA-256 hashes plus the identifier lengths — never a
name. A tool whose whole job is removing identifiers should not answer by writing a new
plaintext list of patients. The salt sits beside the hashes so they can be recomputed,
which makes the file a *derived* artefact that still deserves care (common names can be
brute-forced out of hashes). Hits are reported with the offending substring, so
`--verify` doubles as a triage tool on a tree you did not produce.

See `dicom-anonymizer.example.toml` for all options.

## Output

The output tree mirrors the input tree. By default, each output file is renamed
to its **remapped SOP Instance UID** (`[output] filename = "sop_instance_uid"`),
so the original instance UID cannot leak through the filename; set
`filename = "preserve"` to keep the original relative filenames.

Set `[output] layout = "series"` to flatten the tree instead: every file goes into a
directory named after its **remapped** Series Instance UID, which is usually what a
viewer or analysis pipeline wants. Directory names are the new UIDs, so no original
identifier leaks through the path, and a file with no series UID (or one that is not
safe to use as a directory name) falls back to the mirrored path rather than being
dropped. Note that with `layout = "series"` and `filename = "preserve"`, two files in
one series that share a file name would collide; prefer the default UID naming there.

`audit.jsonl` in
the output directory has one JSON object per file (status, changed tag count,
`regions_masked` and `samples_masked`, transfer syntaxes, output SHA-256).

At the end of a run the tool also writes `run.json`:

```json
{ "tool": "dicom-anonymizer", "version": "0.1.0", "outcome": "complete",
  "total": 70, "ok": 69, "skipped": 1, "unsupported": 0, "error": 0,
  "audit_complete": true }
```

`audit.jsonl` is created before processing starts, so a killed run leaves a partial audit
and a partial output tree that look no different from a finished product. **`run.json` is
the marker that separates the two: if it is absent, the run did not finish.** `outcome`
is `partial` when any file was left un-anonymised, and `audit_complete` is `false` when
the audit sink itself failed, so records may be missing. `--verify` prints the marker
when it finds one, and says so when it does not.
Non-DICOM files are logged with `status: skipped`; a file whose transfer syntax
cannot be decoded (e.g. JPEG-LS without `--features charls`) is logged with
`status: unsupported` and the batch continues. Exit codes: `0` success,
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

De-identified objects carry the PS3.15 markers `(0012,0062) Patient Identity
Removed = "YES"` and `(0012,0063) De-identification Method`.

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

`C` (“clean”) removes identifiers rather than recognizing arbitrary PHI: before any
attribute is changed, the file's own identifying strings are harvested — patient-name
components, patient / other-patient IDs and accession number, the institution
name/address/department (whole value plus its ≥4-character words), and
referring / performing / operator names. Those strings are then deleted,
case-insensitively, from the descriptor text marked `C`, so useful clinical text
survives:

```
InstitutionName    WanNan Medical College YiJiShan Hospital   -> (removed)
StudyDescription   YiJiShan Hospital^l-spine                 -> l-spine
```

Matching is Unicode-aware and case-insensitive, so non-ASCII names are removed too:
CJK text is cleaned, and the character-set attribute itself is never modified. Verified
by test for `ISO_IR 192`, `GB18030`, `GBK` and `ISO 2022 IR 58` — the Simplified Chinese
sets. (Careful: DICOM's `ISO 2022 IR 149` is the *Korean* set, not Chinese.) Because
matching is by substring, short ASCII tokens
are deliberately not harvested — scrubbing a 2-character component such as `Li` would
tear `Ligament` apart — while dense scripts get a lower floor, since two CJK characters
are already a whole name.

Values whose VR is not textual fall back to neutralization, and temporal attributes
under the *Modified Dates* option are shifted instead. This is heuristic, not NLP:
unrecognized free-text PHI is not detected. `clean_structured_content` /
`clean_graphics` are accepted but only exercised on the attributes E.1-1 marks.

## Limits

Irreversible by design. JPEG-LS requires `--features charls`. DICOMDIR files
are skipped. Burned-in masking uses configured regions only; there is no OCR.
With the default random salts, results are not reproducible across runs; set
`dates.shift_salt` and `uids.salt` for reproducibility.

A source file that carries non-ASCII text while declaring **no** `SpecificCharacterSet`
is not conformant, but it is handled rather than rejected: the bytes are decoded
single-byte and re-encoded the same way, so the text survives byte-for-byte (verified by
test) and cleaning runs on that same decoding. The caveat is that the encoding stays
ambiguous — declare the character set at the source if you control it.
