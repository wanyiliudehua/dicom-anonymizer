# Design: `dicom-anonymizer` — Rust DICOM De-identification Tool

- **Status:** Approved for planning
- **Date:** 2026-09-21
- **Path:** `C:\Users\YJSYY\projects\dicom-anonymizer`
- **Process:** architectural (brainstorming → spec → writing-plans)

## 1. Purpose

A standalone, single-binary Rust command-line tool that de-identifies DICOM medical
images in bulk. It walks an input directory tree, removes or replaces patient-identifying
metadata per DICOM PS3.15, masks burned-in pixel annotations according to configuration,
writes results to a mirrored output directory, and records a JSON audit trail.

Intended use: preparing image datasets for research sharing and downstream analysis
(e.g. AI training data) without leaking patient identity.

### Success criteria

1. A single self-contained executable (no runtime interpreter) processes a directory
   tree of DICOM files.
2. All tags covered by the selected PS3.15 profile are de-identified; no covered
   identifier survives in the output.
3. Burned-in pixel regions listed in configuration are filled for matching images.
4. Output is written to a separate directory, preserving the input tree structure;
   the input is never modified.
5. Every processed file produces one JSON Lines audit record.
6. A failure on one file does not abort the batch (unless `--fail-fast`).

## 2. Decisions (locked)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Scope | Full data pipeline (batch, audit, options) |
| 2 | Burned-in pixel handling | Tag-driven (`BurnedInAnnotation`) + configured regions; **no OCR** |
| 3 | De-id policy | Irreversible; PS3.15 Basic profile + optional Options, config-driven |
| 4 | Transfer syntaxes | Full set incl. JPEG-LS / JPEG 2000; masked pixels **transcoded to uncompressed** |
| 5 | Deliverable form | Pure CLI binary (single package) |
| 5 | Config format | TOML with built-in presets + user override |
| 5 | Output strategy | Separate output directory only (no in-place) |
| 5 | Audit log | JSON Lines |
| 6 | Implementation base | `dicom-rs` v0.10 ecosystem (`native` + `openjp2`; `charls` optional) |

## 3. Non-goals (YAGNI)

- No OCR / text recognition in pixels.
- No network, PACS, DICOMweb, or DIMSE services.
- No reversible pseudonymization, persistent mapping table, or key management.
- No GUI.
- No in-place overwrite of source files.
- No DICOMDIR directory-record rewriting (DICOMDIR files are skipped; see §11).

## 4. Architecture

Single package producing one binary. Internally layered; the pixel codec is behind a
trait to allow a future pure-Rust (no C dependency) implementation.

```
CLI (clap)
  -> config (TOML: presets + overrides)
  -> discovery (walk tree, DICM magic detection, build jobs)
  -> deid engine (PS3.15 actions on dataset)
  -> pixel stage (decode, mask regions, transcode to Explicit VR LE)
  -> writer (mirror path, atomic temp+rename)
  -> audit (JSON Lines)
```

Data flow per file (parallelizable):

```
parse dataset
  -> determine original PatientID (for date offset)
  -> apply tag rules (X/Z/D/U/C/K), recursing into sequences
  -> if pixels.mode == mask: decode -> mask configured regions -> transcode
  -> serialize Explicit VR LE
  -> atomic write to mirrored output path
  -> emit one audit record
```

## 5. Components

| Module | Responsibility | Key deps |
|--------|----------------|----------|
| `main` | Orchestration, rayon pool, exit code | `rayon` |
| `cli` | Argument parsing & validation | `clap` |
| `config` | TOML load, preset selection, merge, validation | `serde`, `toml` |
| `discovery` | Recursive walk, DICM detection, job list, relative paths | `walkdir` |
| `deid::profile` | PS3.15 rule tables encoded as data | `dicom-object` |
| `deid::actions` | Apply X/Z/D/U/C/K, recurse sequences | `dicom-object` |
| `deid::options` | PS3.15 option switches | — |
| `deid::uids` | Deterministic UID remapping | `hmac`, `sha2` |
| `deid::dates` | Deterministic per-patient date shifting | `hmac`, `sha2` |
| `pixel::regions` | Match config regions by modality/SOP Class | — |
| `pixel::mask` | Apply rect/polygon fills per frame | `dicom-pixeldata` |
| `writer` | Mirrored path, temp+rename atomic write | std |
| `audit` | JSON Lines record emission | `serde_json` |
| `error` | Error types, per-file isolation | `thiserror` |
| `report` | Run summary and exit code selection | — |

## 6. CLI

```
dicom-anonymizer \
  --input <DIR> \
  --output <DIR> \
  [--config <FILE>] \
  [--jobs N] \
  [--dry-run] \
  [--fail-fast] \
  [--verbose]
```

- `--input` / `--output`: required; must be distinct existing dirs (output may be created).
- `--config`: optional; when omitted, the built-in `ps3.15-basic` preset is used.
- `--jobs`: worker count; default = logical CPU count.
- `--dry-run`: runs the full pipeline up to (not including) writes; emits audit records
  to stdout only and produces no files.
- `--fail-fast`: abort on first per-file error.
- `--verbose`: increase log detail on stderr.

## 7. Configuration

```toml
[profile]
base = "ps3.15-basic"                  # ps3.15-basic | research
retain_longitudinal_temporal = true    # date shifting, preserves intervals
retain_safe_private = false
clean_descriptors = true
clean_structured_content = false

[dates]
strategy = "shift"                     # shift | zero | remove
shift_salt = ""                        # empty => random per run

[uids]
strategy = "remap"                     # remap | zero
root = "2.25."                         # valid UID root for generated UIDs

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

[tags]                                 # user overrides: "GGGG,EEEE" or "GGgg" -> action
"0010,0010" = "Z"
"0008,0080" = "X"
```

**Merge precedence:** user config > selected preset > built-in defaults.

**Presets:**
- `ps3.15-basic`: PS3.15 Basic Application Level Confidentiality Profile, no options.
- `research`: `ps3.15-basic` + `retain_longitudinal_temporal` + `retain_safe_private`.

**Validation:** reject unknown action codes; reject `root` that is not a valid UID root;
reject region with non-positive dimensions; reject `shift` with both empty salt and
a requirement for reproducibility (documented, not an error).

## 8. De-identification rule engine

Actions applied to tags:

| Action | Meaning |
|--------|---------|
| `X` | Remove the attribute |
| `Z` | Set to zero length |
| `D` | Replace with a dummy value (per-tag dummy table) |
| `U` | Replace with a new UID |
| `C` | Clean recursively, preserving structure |
| `K` | Keep |

- The base profile is PS3.15 Table E.1-1, encoded as a static data table in
  `deid::profile` covering patient, study, series, equipment, institution, physician,
  and identification-related tags.
- Sequence (SQ) attributes are traversed recursively.
- Private tags: removed unless `retain_safe_private` is set and the tag is on the
  safe-private allowlist.
- `clean_descriptors` removes identifying text in descriptor fields; `clean_structured_content`
  applies to SR content.

## 9. UID and date consistency

**UID remap (stateless, deterministic, thread-safe):**

```
new_uid = <root> . <decimal(HMAC-SHA256(salt, original_uid)[0..16 bytes])>
```

Truncated so the total length is <= 64 characters and contains only digits and dots.
Because the mapping is a pure function, repeated references within a batch map to the
same new UID without shared mutable state, and results are reproducible across runs
when `salt` is fixed.

**Date shifting (deterministic per patient):**

- Offset in days = `HMAC(salt, original_PatientID)` mapped into a configured range.
- Computed **before** PatientID is replaced.
- Applied to `DA` and `DT` values; time-of-day is preserved. All dates for a patient
  share one offset, preserving longitudinal intervals.

**Limitation (irreversible):** with the default random `shift_salt`, output is not
reproducible across runs and cannot be reversed. Set `shift_salt` for reproducibility.

## 10. Pixel pipeline

```
read BurnedInAnnotation (0028,0301)
  -> match regions by modality / SOP Class UID
  -> decode pixel data via dicom-pixeldata (per transfer syntax)
  -> for each frame, apply rect/polygon fills (value = fill)
  -> write back as Explicit VR LE (uncompressed)
  -> adjust dependent attributes for the transcode
```

- `on_missing_burnedin` controls behavior when `(0028,0301)` is absent:
  `warn` (default), `skip`, or `mask_all`.
- Multi-frame (`NumberOfFrames`) and `PlanarConfiguration` are handled.
- JPEG-LS requires the optional `charls` feature. If it is not compiled in and a JPEG-LS
  file is encountered, the file is recorded as `unsupported` and the batch continues.
- Default enabled codecs: `native` (RLE, JPEG, Deflate) + `openjp2` (JPEG 2000, pure Rust).

## 11. Discovery and output

- Recursively walk `--input`; a file is DICOM if bytes 128..132 equal `DICM`.
- Non-DICOM files are skipped and logged as `skipped`.
- The output path mirrors the input's relative path under `--output`.
- Files whose SOP Class UID is Media Storage Directory Storage (`1.2.840.10008.1.3.10`)
  are skipped (logged as `skipped_dicomdir`) to avoid stale directory records.
- Writes use a temporary file in the destination directory followed by an atomic rename.

## 12. Audit log (JSON Lines)

One object per processed file. Required fields:

```json
{
  "input": "rel/path/file.dcm",
  "output": "rel/path/file.dcm",
  "status": "ok",
  "error": null,
  "transfer_syntax_in": "1.2.840.10008.1.2.4.90",
  "transfer_syntax_out": "1.2.840.10008.1.2.1",
  "tags_changed": 42,
  "regions_masked": 2,
  "output_sha256": "…",
  "duration_ms": 37
}
```

`status` is one of `ok`, `skipped`, `skipped_dicomdir`, `unsupported`, `error`.
In `--dry-run`, records are written to stdout and `output`/`output_sha256` may be null.

## 13. Error handling and exit codes

- Per-file errors are caught, recorded as `status=error`, and do not stop the batch
  unless `--fail-fast` is set.
- Exit codes: `0` all files succeeded or were skipped; `1` at least one file errored;
  `2` fatal error (invalid arguments, unreadable config, missing input directory).
- A final summary is printed to stderr: counts by status.

## 14. Concurrency

`rayon` parallelizes files across `--jobs` workers. All per-file state is local; the
UID and date mappings are pure deterministic functions, so no shared mutable state or
locks are required. The audit writer and summary aggregator are the only shared sinks
and use a `Mutex`/channel.

## 15. Testing strategy (TDD)

- **Unit:** rule table actions; preset merge precedence; config validation; UID
  determinism and no-collision; date-offset determinism; mask geometry bounds.
- **Integration / golden files:** synthetic DICOM fixtures generated programmatically
  (uncompressed, RLE, JPEG, JPEG 2000, sequences, private tags). Run the binary and
  assert: covered tags are de-identified, configured pixel regions are filled, audit
  JSON is correct.
- **Property tests (proptest):** UID remap is a function, deterministic and collision-free
  over a sample; mask rectangles never exceed frame bounds.
- **CLI e2e:** exit codes for all-ok / partial-failure / fatal; `--dry-run` writes no files.
- Fixtures contain no PHI and are generated by test helpers.

## 16. Project layout

```
dicom-anonymizer/
  Cargo.toml
  README.md
  dicom-anonymizer.example.toml
  src/
    main.rs
    cli.rs
    config.rs
    discovery.rs
    deid/{mod.rs,profile.rs,actions.rs,options.rs,uids.rs,dates.rs}
    pixel/{mod.rs,regions.rs,mask.rs}
    writer.rs
    audit.rs
    error.rs
    report.rs
  tests/
    integration.rs
    fixtures/            # generated, no PHI
  docs/superpowers/specs/2026-09-21-dicom-anonymizer-design.md
```

## 17. Dependencies (initial)

`clap`, `serde`, `toml`, `serde_json`, `walkdir`, `rayon`, `thiserror`, `hmac`, `sha2`,
`hex`, `dicom-object`, `dicom-pixeldata`, `dicom-transfer-syntax-registry`.
Optional feature `charls` enables JPEG-LS.

## 18. Risks and limitations

- **JPEG-LS** needs the C-based CharLS when the `charls` feature is enabled; without it,
  such files are reported `unsupported` rather than processed.
- **Lossy JPEG/JPEG 2000 → uncompressed transcode** increases file size; source lossless
  pixel values are preserved.
- **Irreversible** by design: no path back to original identifiers.
- **Non-reproducible by default** due to random salt.
- **DICOMDIR** is skipped, so media directory indexes are not updated.
- Upstream `dicom-rs` API changes may require maintenance.

## 19. Acceptance criteria

1. `cargo build --release` produces one binary that runs on the target OS without Rust
   installed.
2. Processing a fixture tree yields a mirrored output tree with all profile-covered tags
   de-identified and configured pixel regions masked.
3. Every file yields exactly one audit record; `--dry-run` writes no files.
4. Exit codes match §13 for all-ok, partial-failure, and fatal cases.
5. `cargo test` passes including the integration and property tests.
