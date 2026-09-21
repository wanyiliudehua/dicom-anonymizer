# Design: `dicom-anonymizer` — Rust DICOM De-identification Tool

- **Status:** Implemented (as-built) — supersedes the original "Approved for planning" revision
- **Date:** 2026-09-21 (as-built revision)
- **Path:** `C:\Users\YJSYY\projects\dicom-anonymizer`
- **Process:** architectural (brainstorming → spec → writing-plans → inline execution → review)

This document describes the tool **as built**. Where the implementation diverged from
the approved plan, the divergence is recorded in §20.

## 1. Purpose

A standalone, single-binary Rust command-line tool that de-identifies DICOM medical
images in bulk. It walks an input directory tree, applies the DICOM PS3.15
de-identification profile (and options) to metadata, masks burned-in pixel annotations
according to configuration, writes results to a separate output directory, and records a
JSON Lines audit trail.

Intended use: preparing image datasets for research sharing and downstream analysis
(e.g. AI training data) without leaking patient identity.

### Success criteria

1. A single self-contained executable processes a directory tree of DICOM files.
2. Every attribute covered by the selected PS3.15 profile/options is de-identified; no
   covered identifier survives in the output.
3. Configured burned-in pixel regions are filled for matching images.
4. Output is written to a separate directory; the input is never modified.
5. Every walked file produces exactly one JSON Lines audit record.
6. A failure on one file does not abort the batch (unless `--fail-fast`).
7. De-identified objects carry the PS3.15 markers `(0012,0062)` and `(0012,0063)`.

## 2. Decisions

| # | Decision | Choice |
|---|----------|--------|
| 1 | Scope | Full data pipeline (batch, audit, options) |
| 2 | Burned-in pixel handling | Tag-driven (`BurnedInAnnotation`) + configured regions; **no OCR** |
| 3 | De-id policy | Irreversible; full PS3.15 Table E.1-1 + all 11 option columns |
| 4 | Transfer syntaxes | Full set incl. JPEG-LS / JPEG 2000; masked pixels **transcoded to uncompressed** |
| 5 | Deliverable form | Pure CLI binary (single package) |
| 6 | Config format | TOML with built-in presets + user override |
| 7 | Output strategy | Separate output directory only (no in-place); files named by new SOP Instance UID by default |
| 8 | Audit log | JSON Lines |
| 9 | Strategy levels | `conservative` / `aggressive` selector |
| 10 | Implementation base | `dicom-rs` v0.10 (`native` + `openjp2`; `charls` optional) |

## 3. Non-goals (YAGNI)

- No OCR / text recognition in pixels.
- No network, PACS, DICOMweb, or DIMSE services.
- No reversible pseudonymization, persistent mapping table, or key management.
- No GUI.
- No in-place overwrite of source files.
- No DICOMDIR directory-record rewriting (DICOMDIR files are skipped).
- No NLP-grade cleaning; the `C`/Clean options remove this object's own known
  identifiers from text, not arbitrary unrecognized PHI (§10).

## 4. Architecture

Single package producing one binary, layered as:

```
CLI (clap)
  -> config (TOML: presets + strategy + options + overrides)
  -> discovery (walk tree, DICM detection, build jobs for every file)
  -> deid engine (PS3.15 E.1-1 actions + options on dataset)
  -> pixel stage (transcode to Explicit VR LE, mask regions -> PixelReport)
  -> writer (mirrored/renamed path) ; file written via `write_to_file` + rename
  -> audit (JSON Lines)
```

Data flow per file (parallelizable):

```
open file (or log skipped if not DICOM)
  -> skip if DICOMDIR
  -> resolve rules (E.1-1 + enabled options + strategy overlay + user [tags])
  -> determine original PatientID (for the date offset)
  -> apply actions (X/Z/D/U/C/K), recursing sequences (X removes an SQ whole)
  -> sync file meta Media Storage SOP Instance UID
  -> write PS3.15 de-identification markers
  -> if pixels.mode == mask: transcode -> mask -> PixelReport
  -> compute output path (preserve, or new SOP Instance UID + .dcm)
  -> write the whole file to a temp path, then rename
  -> emit one audit record
```

UID and date mapping are pure functions of `(salt, original)`, so files parallelize
without shared mutable state.

## 5. Components

| Module | Responsibility | Key deps |
|--------|----------------|----------|
| `main` | Orchestration, rayon pool, exit code | `rayon` |
| `cli` | Argument parsing | `clap` |
| `config` | TOML load, presets, strategy, options, merge, validation | `serde`, `toml` |
| `discovery` | Recursive walk, DICM detection, job list (all files) | `walkdir` |
| `deid::e1_1` | Generated PS3.15 Table E.1-1 (653 rows) + option columns | — |
| `deid::profile` | Option resolution, strategy overlay, overrides | — |
| `deid::actions` | Apply X/Z/D/U/C/K, sequence recursion, repeating groups | `dicom-object` |
| `deid::dataset` | Thin wrapper over the object mutation API | `dicom-object` |
| `deid::uids` | Deterministic UID remapping | `hmac`, `sha2` |
| `deid::dates` | Deterministic per-patient date shifting | `hmac`, `sha2`, `chrono` |
| `pixel::geometry` | Pure byte-level rect/polygon masking | — |
| `pixel::regions` | Match regions by modality/SOP Class/frames | — |
| `pixel::stage` | Transcode, mask, write back, `PixelReport` | `dicom-pixeldata`, `dicom-encoding` |
| `writer` | Output path mapping | std |
| `audit` | JSON Lines record emission | `serde_json` |
| `error` | Error types, per-file isolation | `thiserror` |
| `report` | Run summary and exit code selection | — |
| `verify` | Post-run leak check: harvest input identifiers, scan output values | `dicom-object` |

## 6. CLI

```
dicom-anonymizer \
  --input <DIR> --output <DIR> \
  [--config <FILE>] [--strategy conservative|aggressive] \
  [--jobs N] [--dry-run] [--check-leaks] [--fail-fast] [--verbose]
```

- `--input` / `--output`: required; output is created if needed.
- `--config`: optional; defaults to the built-in `ps3.15-basic` preset.
- `--strategy`: `conservative` | `aggressive`; overrides the config. Default `aggressive`.
- `--jobs`: worker threads; default = all logical CPUs. `--jobs 1` runs sequentially.
  `--fail-fast` forces the sequential path.
- `--dry-run`: full pipeline up to (not including) writes; audit records go to stdout
  and no files are produced.
- `--check-leaks`: post-run verification. The input tree is re-read for identifying
  strings (person names and their components, patient / other-patient IDs, accession
  number, institution name/address/department words — the same harvest the `C` clean
  action uses), then every text value in the output tree is scanned for them
  case-insensitively. UI values are skipped (UIDs are remapped, so their digits are
  expected), keeping the check free of false positives. Categories the user explicitly
  retained (`retain_institution_identity`) are excluded from the check set. Any hit is
  reported on stderr and forces exit `1`. Ignored under `--dry-run`, which has no output
  tree to scan.
- `--fail-fast`: abort on the first per-file error.
- `--verbose`: report progress, and extra detail on stderr. Progress is also shown when
  stderr is a terminal, so an interactive run is not a black box while a redirected one
  stays clean. A line is emitted at most every two seconds, and always for the final file.
- `--version` / `--help`: standard clap output.
- `--export-identifiers <FILE>`: additionally write a salted, hashed fingerprint of the
  input tree's identifiers (see `verify`).
- `--verify <FILE>`: a separate mode. Scan `--input` against a fingerprint and report
  any surviving identifier; `--config` and `--output` are not required and not used, so
  a delivered tree can be audited without the originals or the original settings. Non-DICOM
  files are scanned as text line by line: a real CloudPACS export ships an XML dump of
  every attribute beside the images, and a DICOM-only scan called such a tree clean. Only
  **text VRs** are searched; binary values are counted and reported as *not searched* (see
  `Coverage`), so the result is a bounded claim rather than a blanket all-clear.
- `--output` is therefore optional at the clap level and validated manually: required in
  the default (anonymise) mode, unused by `--verify`.
- `--require-complete`: with `--verify`, also require `run.json` to describe a complete
  run (`outcome = complete` and `audit_complete`); a clean-but-unfinished tree then fails,
  which is the assertion an acceptance step actually wants. Used without `--verify` it is a
  usage error (exit 2).

## 7. Configuration

```toml
[profile]
base = "ps3.15-basic"                  # ps3.15-basic | research
strategy = "aggressive"                # conservative | aggressive
retain_longitudinal_temporal = false   # Modified Dates option (shift)
retain_longitudinal_full_dates = false # Full Dates option (keep verbatim)
retain_safe_private = false
retain_uids = false
retain_device_identity = false
retain_institution_identity = false
retain_patient_characteristics = false
clean_descriptors = false
clean_structured_content = false
clean_graphics = false

[dates]
strategy = "shift"                     # shift | zero | remove
shift_salt = ""                        # empty => random per run
range_days = 3650

[uids]
strategy = "remap"                     # remap | zero
root = "2.25."
salt = ""                              # empty => random per run

[output]
filename = "sop_instance_uid"          # sop_instance_uid | preserve
layout = "mirror"                     # mirror | series (one dir per remapped series UID)
non_dicom = "skip"                     # skip | error (copying is deliberately not offered)

[limits]
max_input_bytes = 2147483648           # refuse larger inputs (0 = no limit)
max_parallel_bytes = 1073741824        # cap jobs x largest input (0 = no cap)

[dummies]
# optional overrides for the D action; unspecified tags use built-in defaults
# "0008,0070" = "ANON"

[pixels]
mode = "mask"                          # mask | keep
fill = 0                               # 0 | 255 | "max"
on_missing_burnedin = "warn"           # warn | skip | mask_all

[[pixels.regions]]
modality = "US"                        # optional filter
sop_class_uid = "1.2.840..."           # optional filter
shape = "rect"                         # rect | polygon
x = 0
y = 0
width = 200
height = 30
frames = "all"                         # all | [0, 1, 2]

[tags]                                 # user overrides -> action
"0010,0010" = "Z"
```

**Merge precedence:** user `[tags]` > strategy overlay > enabled options > E.1-1 Basic >
built-in overrides.

**Presets:** `ps3.15-basic` (no options) and `research` (basic + `retain_longitudinal_temporal`
+ `retain_safe_private`).

**Strategy:** `aggressive` (default) applies the E.1-1 result as-is, keeping only the
image's clinical/technical parameters. `conservative` overrides demographics
(`0010,0040` sex, `0010,1010` age, `0010,1020` size, `0010,1030` weight) and all
date/time attributes to `K` (retain verbatim), while direct identifiers are still cleared.

**Validation:** action codes and tag syntax; `uids.root` when remapping; region
dimensions; `pixels.fill` named value. Empty `dates.shift_salt` / `uids.salt` mean
"random per run" (documented, not an error).

## 8. De-identification rule engine

Actions:

| Action | Meaning |
|--------|---------|
| `X` | Remove the attribute (for an SQ, remove the whole sequence) |
| `Z` | Set to zero length |
| `D` | Replace with a dummy value, VR-aware (`DA`→`19000101`, `DT`→`19000101000000`, `TM`→`000000`, `UI`→new UID, else configured/`ANONYMOUS`) |
| `U` | Replace with a new UID |
| `C` | Clean: strip this object's own identifiers (patient/physician name components, IDs, institution words) from the text value and keep the rest; non-text values are neutralized, and temporal attributes under the Modified Dates option are shifted instead |
| `K` | Keep |

- **Identifier harvest (drives `C`):** before any attribute is changed, the object's
ow identifying strings are read from the patient/other-patient IDs and accession
number (whole value), the person-name attributes (whole value plus `^`/`=`/`,`/`;`/
whitespace/`()`/`/`-separated components), and the institution name/address/
department (whole value plus its components). Component length floors avoid
over-scrubbing: 3 characters for ASCII name components, 4 for ASCII institution
words, and 2 for non-ASCII text. The lower non-ASCII floor exists because dense
scripts pack a whole name into two characters, while matching is by substring -
harvesting a 2-character ASCII token such as `Li` would tear `Ligament` apart.
- **Scrub:** matching is Unicode-aware (`char::to_lowercase`, not byte folding) and
case-insensitive; the needle keeps its original character count so the removed range
is always the length of the source text it replaced. If nothing matches, the value is
returned **byte for byte** instead of being normalized. After a removal, only
leading/trailing ASCII spaces and `^` are trimmed; nothing else is rewritten, because
collapsing Unicode whitespace corrupts bytes (§18).
- **Character sets:** text is decoded/encoded through the dataset's
`SpecificCharacterSet`, which is itself never modified (it is not in E.1-1), so a
UTF-8 (`ISO_IR 192`) file stays readable and its non-ASCII identifiers are still
scrubbed.
- **Base table:** the full PS3.15 Table E.1-1 (`deid::e1_1::E11`, 653 attributes),
  **generated** from the DICOM standard by `scripts/gen_e1_1.py`. Each row is
  `(tag, basic_action, &[(option_key, action)])`. Ambiguous cells (`X/Z`, `X/Z/D`, …)
  are collapsed to one action, preferring `Z` (keeps the element present, valid for
  Type 1/2 attributes), then `D`, `U`, `X`, `K`, `C`.
- **Option resolution:** for each attribute, the enabled option columns override the
  Basic action in table-column order (later wins).
- **Overrides** applied after the table: `Manufacturer`, `Manufacturer Model Name`,
  `Software Versions` → `X` (not listed in E.1-1); `SOP Class UID`, `Pixel Data`,
  `Burned In Annotation` → `K` (operational).
- **Repeating groups** that cannot be a single tag — Curve Data `(50xx,xxxx)`, Overlay
  Data/Comments `(60xx,3000|4000)` — are matched by group range and removed.
- **Private (odd-group) tags** are removed unless `retain_safe_private` is set, in which
  case they are retained (approximation: no safe-private allowlist).
- **Sequences** with an `X` rule are removed whole; other sequences are recursed into.
  Item-level errors propagate.

## 9. UID and date consistency

**UID remap (stateless, deterministic, thread-safe):**

```
new_uid = <root> . <decimal(HMAC-SHA256(salt, original_uid)[0..16 bytes])>
```

Truncated to ≤ 64 characters, digits and dots only. `SOP Instance UID (0008,0018)` is
remapped and the file meta `Media Storage SOP Instance UID (0002,0003)` is synced to it.
With `[output] filename = "sop_instance_uid"` (default) the output file is named after
the new SOP Instance UID, so the original instance UID cannot leak via the filename.

**Date shifting (deterministic per patient):**

- Offset in days = `HMAC(salt, original_PatientID)` mapped into `[-range_days, +range_days]`.
- Computed **before** PatientID is replaced; all dates for a patient share one offset.
- Applied when `retain_longitudinal_temporal` (Modified Dates) is set, to `DA`/`DT`
  attributes whose resolved action is `C`; time-of-day is preserved. `TM` values are
  not shifted.
- `retain_longitudinal_full_dates` keeps dates verbatim; otherwise each date/time takes
  its E.1-1 action (`Z`/`D`/`X`). `[dates] strategy = "remove"` forces deletion.

**Limit:** with the default random salts, output is not reproducible across runs and
cannot be reversed.

## 10. Pixel pipeline

```
no pixel data? -> done (regions_masked = 0, no transcode)
match regions by modality / SOP Class
BurnedInAnnotation (0028,0301) absent? -> warn | skip | mask_all
transcode to Explicit VR LE when needed (JPEG-LS without `charls` -> Unsupported)
build Geometry from Rows/Columns/Number of Frames/SamplesPerPixel/BitsAllocated
reject PlanarConfiguration = 1 (colour-by-plane)
mask rect/polygon regions per frame (or the whole frame for mask_all)
write PixelData back (OB for ≤8 bits, OW for >8 bits)
```

Returns a `PixelReport { regions_masked, samples_masked }`.

- Configured regions are applied **regardless** of the `BurnedInAnnotation` value.
- `on_missing_burnedin` controls behavior when the tag is **absent**: `warn` (default)
  logs and applies configured regions; `skip` skips pixel processing; `mask_all` masks
  the whole frame(s).
- Output transfer syntax is Explicit VR LE only when pixel data was decoded and rewritten;
  with `mode = "keep"` the original syntax is preserved.
- Multi-frame is handled. `PlanarConfiguration = 1` is rejected as a per-file error
  rather than silently mis-masked.
- JPEG-LS (`1.2.840.10008.1.2.4.80/81`) requires the `charls` feature; otherwise the file
  is reported `unsupported` and the batch continues.
- Default codecs: `native` (RLE, JPEG, Deflate) + `openjp2` (JPEG 2000, pure Rust).

## 11. Discovery and output

- Recursively walk `--input`; a file is DICOM when bytes 128..132 equal `DICM`.
- **Every** file becomes a job; non-DICOM files are logged as `skipped` and not written.
  `[output] non_dicom = "error"` records a per-file `error` instead, so a run that drops
  files cannot report success. Copying is deliberately not offered: putting an
  un-de-identified file into the output tree would contradict what the run certifies.
  Verified non-DICOM files are not silently discarded by `--verify`, which scans them.
- DICOMDIR is detected from the file meta `Media Storage SOP Class UID (0002,0002)`
  (fallback: dataset `(0008,0016)`) and logged as `skipped_dicomdir`.
- The output directory mirrors the input tree. By default each file is renamed to
  `<new SOP Instance UID>.dcm`; `[output] filename = "preserve"` keeps the original
  relative name.
- `[output] layout = "series"` flattens instead: `<out>/<new series UID>/<name>`. The
  directory name is sanitized for path safety (separators dropped; a name that does not
  start alphanumerically is refused), and a file with no usable series UID keeps the
  mirrored path rather than being dropped.
- The whole DICOM file is written via `write_to_file` to a sibling temp path, then
  renamed over the target (atomic; preserves preamble + file meta).

## 12. Audit log (JSON Lines)

One object per walked file:

```json
{
  "input": "rel/path/file.dcm",
  "output": "2.25.1234….dcm",
  "status": "ok",
  "error": null,
  "transfer_syntax_in": "1.2.840.10008.1.2.1",
  "transfer_syntax_out": "1.2.840.10008.1.2.1",
  "tags_changed": 79,
  "regions_masked": 0,
  "samples_masked": 0,
  "output_sha256": "…",
  "duration_ms": 13
}
```

- `status`: `ok`, `skipped`, `skipped_dicomdir`, `unsupported`, `error`.
- `regions_masked` counts mask operations; `samples_masked` counts written samples.
- In `--dry-run`, records go to stdout and `output`/`output_sha256` may be null.

### Completion marker (`run.json`)

`audit.jsonl` is opened before the first file is processed, so a run that is killed
leaves a partial audit and a partial output tree that are indistinguishable from a
finished product. The marker written at the end of a run is what separates them:

```json
{ "tool": "dicom-anonymizer", "version": "0.1.0", "outcome": "complete",
  "total": 70, "ok": 69, "skipped": 1, "unsupported": 0, "error": 0,
  "audit_complete": true }
```

- Absence means the run did not reach the end.
- `outcome` is `complete` only when `error == 0`, otherwise `partial`: the run finished,
  but at least one file was not written.
- `audit_complete` is `false` when a write to the audit sink failed, so records may be
  missing even though the marker exists.
- `--dry-run` writes none, because it produces no tree.
- `--verify` reports the marker when present (`manifest: outcome=Complete …`) and states
  its absence otherwise; a tree produced elsewhere simply has none, which is not an error.

## 13. Error handling and exit codes

- Per-file errors are recorded as `status = error` and do not stop the batch unless
  `--fail-fast`.
- A file needing a transfer syntax it cannot decode is `unsupported`, not `error`.
- Audit-write failures are surfaced and make the run exit `1`.
- A `--check-leaks` hit is reported on stderr and makes the run exit `1`.
- `--verify` exits `0` when nothing matched and `1` when any identifier survived; bad
  arguments or an unreadable/invalid fingerprint file exit `2`. With `--require-complete`,
  a missing or non-complete marker also exits `1` (the tree is not what was asked for).
- Exit codes: `0` all files succeeded/skipped; `1` ≥1 error, audit-write failure, or
  leak/verify hit; `2` fatal (bad args, unreadable config, missing input directory).
- A final summary is printed to stderr.

## 14. Concurrency

`rayon` parallelizes files by default (all logical CPUs); `--jobs 1` or `--fail-fast`
uses the sequential path. Per-file state is local and UID/date mapping are pure
functions, so there is no shared mutable state. Audit lines are collected per file and
emitted in input order.

Progress is counted from whichever thread finishes a file, so the count is truthful
even though completions arrive out of order, and each line is written with a single
`eprintln!` so lines never interleave. Reporting cadence is unit-tested rather than
observed: on the sample dataset the whole batch finishes inside one interval, so only the
final line is ever emitted there.

The job count is reduced when `jobs x largest input` would exceed
`[limits] max_parallel_bytes`, since every file is held whole. The largest input is known
from discovery, so the cap is a static decision taken before any worker starts, rather than
a runtime byte budget that would have to block workers and could deadlock. The run prints
a note when the cap applies.

## 15. Testing

Current suite: **80 unit + 4 integration tests**.

- **Unit:** E.1-1 table size/anchors; option overrides (basic vs with-option); strategy
  override; action semantics (X/Z/D/U/K, VR-aware `D`); sequence `X` removal and
  recursion; repeating groups; private tags; UID determinism/collision-freedom; date
  offset determinism and malformed-date pass-through; mask geometry/bounds; region
  selection; `PixelReport`; JPEG-LS feature gate; discovery (DICOM + non-DICOM);
  config parsing/validation.
- **Integration (`tests/integration.rs`):** built-binary end-to-end over a generated
  fixture tree — mirrored output with `preserve`, default SOP-UID naming and meta sync,
  same source UID maps consistently, non-DICOM logged `skipped`, dry-run writes nothing,
  empty input exits 0.
- **Pixel paths:** JPEG Baseline transcode → mask → uncompressed round-trip (asserts the
  transfer syntax changes back and the fill lands); multi-frame masking of one selected
  frame (other frames untouched).
- **Bounds and sequences:** an out-of-bounds region is a per-file error with no output
  written; an unruled sequence is kept while its nested PHI is removed.
- Fixtures contain no PHI and are generated by test helpers.

**Still not covered:** RLE / JPEG 2000 / JPEG-LS end-to-end (no encoder here, or
feature-gated), and broader interactions between `[dates] strategy` and the option
flags beyond the unit tests.

## 16. Project layout

```
dicom-anonymizer/
  Cargo.toml
  README.md
  dicom-anonymizer.example.toml
  .cargo/config.toml
  .github/workflows/ci.yml  # fmt + clippy(-D warnings) + test + release build
  .github/workflows/release.yml  # tag `v*` -> prebuilt binaries on a GitHub Release
  LICENSE  CHANGELOG.md  # MIT license text; Keep a Changelog history
  scripts/
    gen_e1_1.py            # regenerates src/deid/e1_1.rs from the standard
    setup-toolchain.sh      # recreates the local Windows toolchain shim
  src/
    main.rs cli.rs config.rs discovery.rs writer.rs audit.rs error.rs report.rs verify.rs
    deid/{mod.rs,e1_1.rs,profile.rs,actions.rs,dataset.rs,uids.rs,dates.rs}
    pixel/{mod.rs,geometry.rs,regions.rs,stage.rs}
  tests/{integration.rs,common/mod.rs}
  docs/superpowers/specs/2026-09-21-dicom-anonymizer-design.md
```

## 17. Dependencies

`clap`, `serde`, `toml`, `serde_json`, `walkdir`, `rayon`, `thiserror`, `hmac`, `sha2`,
`chrono`, `dicom-core`, `dicom-object`, `dicom-pixeldata`, `dicom-encoding`,
`dicom-dictionary-std`, `dicom-transfer-syntax-registry` (features `native`, `openjp2`,
`deflate`). Optional feature `charls` enables JPEG-LS.

## 18. Risks and limitations

- **`C`/Clean options are heuristic** — identifiers are harvested from the object
  itself (name components, patient/accession IDs, institution name/address/department
  words) and removed from descriptor text case-insensitively, so useful clinical text
  survives. Unrecognized free-text PHI is not detected, non-text values fall back to
  neutralization, and the Modified Dates case shifts instead.
- **JPEG-LS** needs the C-based CharLS when the `charls` feature is enabled. This host
  has no C toolchain, so that feature is unbuildable here.
- **Lossy JPEG/JPEG 2000 → uncompressed transcode** increases file size; source lossless
  pixel values are preserved.
- **Irreversible** by design; **non-reproducible by default** due to random salts.
- **No OCR**: burned-in text is only masked where configured.
- **Character sets.** Text is decoded/encoded through `dicom-encoding`'s codec table;
  the `encoding` crate is a **non-optional** dependency there, so no Cargo feature is
  needed for non-Latin text. Locked in by test: `ISO_IR 192`, `GB18030`, `GBK`,
  `ISO 2022 IR 58`. Note the trap - DICOM's `ISO 2022 IR 149` is the **Korean** set;
  the Simplified Chinese sets are GB18030 / GBK / IR 58. An unknown or unimplemented
  code (e.g. `ISO 2022 IR 148`/`159`) does not fail: the crate falls back to a default
  codec, the file is still processed (verified: `status = ok`) and still cleaned, and
  accented non-ASCII text under such a fallback is verified to stay readable with the
  identifier still removed.
- **Undeclared character sets are passed through, not repaired.** Text in a file with no
  `SpecificCharacterSet` is decoded single-byte (byte 0x85 becomes U+0085, not a UTF-8
  character) and written back through the same codec, which reproduces the original bytes
  exactly (verified by test on a hand-built fixture). Cleaning runs on that same decoding
  so it still matches, but the output stays exactly as ambiguous as the input.
- **Memory is proportional to the largest files, times the jobs in flight.** Each file
  is read, de-identified, transcoded and serialised whole, so peak usage is several times
  the file size multiplied by the number of parallel jobs. `[limits] max_input_bytes`
  (default 2 GiB, `0` disables) refuses an oversized input *before reading it* as a
  per-file error; `--jobs 1` bounds the multiplier. There is no streaming reader.
- **`PlanarConfiguration = 1`** files are rejected when masking is needed.
- **DICOMDIR** is skipped, so media directory indexes are not updated.
- **Machine-specific build**: this host has no working MinGW `dlltool` and no MSVC
  linker, so the project targets `x86_64-pc-windows-gnullvm` via a `rust-lld` wrapper
  (`.cargo/config.toml`, `scripts/setup-toolchain.sh`). On a normal host, use the
  default toolchain instead.
- Upstream `dicom-rs` API changes may require maintenance.

## 19. Acceptance criteria

1. `cargo build --release` produces one binary.
2. Processing a fixture tree yields a mirrored output tree; all profile/option-covered
   tags are de-identified and configured pixel regions masked.
3. Every walked file yields exactly one audit record; `--dry-run` writes no files.
4. Exit codes match §13.
5. `cargo test` passes; the build is warning-free.
6. CI (`.github/workflows/ci.yml`) passes on Linux/Windows/macOS: `cargo fmt --check`,
   `cargo clippy --all-targets -- -D warnings`, `cargo test --locked`,
   `cargo build --release --locked`. CI drops the host-specific `.cargo/config.toml`.
7. Pushing a `v*` tag builds Linux (gnu), Windows (msvc) and macOS (arm64/x86_64)
   release archives and attaches them to a GitHub Release.
8. `--check-leaks` re-reads the input tree, scans the output tree, reports 0 hits on a
   clean run, and exits `1` naming file/tag/identifier when an identifier survives.
9. A finished run leaves `<out>/run.json` with `outcome = complete`; a run that leaves a
   file un-anonymised records `partial`; `--dry-run` writes no marker.
10. `--verify --require-complete` rejects a tree with no marker and a tree whose marker
    says `partial`, and accepts a finished one.
11. `--verbose` emits a progress line with count, throughput and ETA; without it, and with
    stderr redirected, nothing extra is printed.
12. `--verify` reports coverage: files searched, text values and bytes examined, and how
    many non-text values were deliberately not searched.
13. The job count is reduced so that `jobs x largest input` stays within
    `[limits] max_parallel_bytes`, and the run says so when that happens.

## 20. Implementation deltas from the approved plan

The plan (`docs/superpowers/plans/2026-09-21-dicom-anonymizer.md`) was executed inline
with TDD; the following deviations were ruled and recorded in the execution ledger:

- **Full E.1-1 port replaced the curated table.** The plan started with a ~55-attribute
  curated subset; it was replaced by the complete 653-attribute table, generated from
  the standard (`scripts/gen_e1_1.py`) rather than hand-transcribed.
- **All 11 E.1-1 option columns implemented**, plus the `conservative`/`aggressive`
  strategy selector.
- **SOP Instance UID remap + file-meta sync + output renaming** were added after the
  plan, following a real-data gap analysis.
- **Pixel write-back** uses `write_to_file` (full file incl. preamble/meta) + rename
  instead of dataset-only serialization, so outputs stay reopenable.
- **De-identification markers** `(0012,0062)`/`(0012,0063)` are written.
- **`PixelReport`** separates `regions_masked` from `samples_masked`.
- **`[output] non_dicom = "skip" | "error"`** was added beyond the plan. The sample
  dataset ships a sidecar XML, so the default silently dropped it from the output tree;
  the option lets a run insist on being told. No `copy` mode exists on purpose - a
  de-identification tool must not copy an un-de-identified file into its own output, and
  `--verify` now covers sidecars separately.
- **Non-DICOM files** are audited as `skipped`; unrecoverable transfer syntaxes as
  `unsupported`.
- **Date shift** is gated on `retain_longitudinal_temporal`; `D` is VR-aware; dates are
  detected by VR rather than a hard-coded tag list.
- **Sequence `X`** removes the sequence whole (previously recursion bypassed SQ rules).
- **Real `C`/Clean semantics**: `C` harvests the object's own identifiers (before any
  mutation) and scrubs them from descriptor text instead of neutralizing the value.
  Person names contribute their components; institution-like values also contribute
  length-filtered words, because descriptors often carry a *substring* of the
  institution rather than the whole string.
- **Release packaging**: `Cargo.toml` metadata + `LICENSE` + `CHANGELOG.md`, a
  `--version` flag, and `.github/workflows/release.yml` (tag `v*` → Linux/Windows/macOS
  archives attached to a GitHub Release).
- **`--check-leaks`** was added beyond the plan: the same identifier harvest that
  drives `C` is reused to verify the result, which required grouping harvested
  identifiers by category (`Identifiers { patient, names, orgs }`) so that categories
  the user retained are not reported as leaks.
- **`[output] layout = "series"`** was added beyond the plan, grouping outputs by the
  *remapped* series UID; the series UID is sanitized before being used as a directory
  name so a malformed file cannot steer writes outside the output root.
- **`C` no longer normalizes whitespace when nothing matched.** The clean action now
  returns the value byte-for-byte unless it actually removed an identifier, matching the
  documented "no identifier present → unchanged" behaviour.
- **Non-ASCII identifiers are now harvested and scrubbed.** `C` previously bailed out on
  any value containing a non-ASCII byte and skipped non-ASCII candidates, so a CJK name
  or institution survived untouched. Matching is now Unicode-aware over `char`s with a
  script-aware length floor. Note this was latent for the sample dataset, which declares
  `ISO_IR 100` and has non-ASCII bytes only in binary VRs (US/OB and private tags).
- **The `C` clean action could corrupt text, and whitespace handling was narrowed once a
  real corruption was found.** The fixture for undeclared character sets (hand-built
  bytes, because the writer refuses to produce one) showed byte 0x85 silently becoming a
  space: `split_whitespace` counts NEL (U+0085) and NBSP as whitespace, and those are
  exactly what undeclared single-byte text decodes to. `C` now trims only leading/trailing
  ASCII spaces and `^` and rewrites nothing else.
- **`--verify` scans sidecar files as well as DICOM.** Found against the sample dataset,
  which ships a CloudPACS XML holding all 19 identifiers: verifying a tree containing only
  that file reported **0 hits and exit 0** before this change. The scan now walks non-DICOM
  files line by line and reports a windowed snippet, so one hit inside a 90 KB single-line
  XML is actionable rather than a flood. This also made `--verify` strictly stronger than
  `--check-leaks`, which still inspects only the DICOM files a run wrote.
- **`--verify` reporting its coverage** was added beyond the plan: once the scan was gated
  on the VR, "0 hits" needed its scope stated, so a run reports files searched, text values
  and bytes examined, and how many non-text values were skipped. On the sample tree that is
  `5439 text value(s), 77.8 KB searched; 939 non-text value(s) not searched`.
- **Progress reporting** was added beyond the plan: a batch can run for hours, and the
  only feedback used to be the closing summary. It is gated on `--verbose` or stderr being
  a terminal, so redirected output stays byte-identical to before.
- **`--require-complete`** was added beyond the plan so the completion marker can be
  asserted rather than merely reported: a clean tree and a finished tree are different
  claims, and automated acceptance needs the second one.
- **`--verify` was about 400x slower than it needed to be.** Measured on the sample tree:
  ~6 s per file, ~400 s per batch. `to_str()` stringifies any primitive, so the walk fed
  ~871 KB of rendered pixel data per image into windowed hashing. Scanning is now gated on
  the VR (`is_text_vr`), which took the same 69-file tree to 1 s with identical results.
- **`[limits] max_parallel_bytes`** was added beyond the plan to bound peak memory from
  the other side: `max_input_bytes` refuses one oversized file, but N ordinary files in
  flight multiply by the job count. The cap is computed from the largest input found during
  discovery, so it is static and cannot block or deadlock, and the run reports when it
  applies. Measured on the sample tree: the 1 GiB default never bites (16 jobs, largest file
  902 KiB), while a 2 MiB budget reduces 16 jobs to 2.
- **`[limits] max_input_bytes`** was added beyond the plan after the memory profile was
  written down: nothing stopped a single huge input from exhausting the machine and
  getting the process killed mid-batch. It is a per-file guard checked before the file is
  read, and it fails loudly (a per-file error, which makes `run.json` say `partial`) rather
  than skipping quietly, because a quietly skipped file would be an un-anonymised file.
- **`run.json` completion marker** was added beyond the plan. The audit log is created
  before processing starts, so nothing in a partial tree distinguished a killed run from a
  finished one; the marker supplies exactly that evidence, and `--verify` reports it.
- **`--verify` / `--export-identifiers`** were added beyond the plan, completing
  `--check-leaks` into a workflow that works without the originals. The exported artefact
  is deliberately a salted hash set rather than a plaintext identifier list, so the tool
  does not create a new PHI file to verify its own de-identification. Refactoring the scan
  to share a `for_each_text_value` walk also fixed `--check-leaks`, which had skipped any
  value containing a non-ASCII byte and therefore could not see a CJK leak at all.
- **Character-set support was verified, not assumed.** Tests pin `ISO_IR 192`, `GB18030`,
  `GBK` and `ISO 2022 IR 58` round-tripping through real file IO, plus an unsupported
  code (`ISO 2022 IR 148`) which the crate handles by falling back rather than failing -
  including with accented non-ASCII text. No behaviour change was needed. The finding
  that `ISO 2022 IR 149` is Korean, not Chinese, corrected an assumption made while
  planning this work.
- **Toolchain shim** added for this host (see §18).
- **`chrono`, `dicom-core`, `dicom-encoding`, `dicom-dictionary-std`** were added as
  direct dependencies; `hex` and `dicom-transfer-syntax-registry`'s `deflate` feature
  are used via the registry.
