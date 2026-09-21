# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-09-21

Initial release.

### Added

- **Full DICOM PS3.15 Table E.1-1** coverage: all 653 listed attributes with the
  Table E.1-1 action codes (`D`, `Z`, `X`, `U`, `K`, `C`) and all 11 option
  columns exposed as `[profile]` flags.
- **Actions**: `X` (remove, including whole sequences), `Z` (zero-length),
  `U` (UID remap), `D` (VR-aware dummy replacement), `K` (keep), and `C`
  (clean); repeating-group attributes are handled by group.
- **UID remapping**: deterministic, HMAC-based `SOPInstanceUID` remap with
  file-meta synchronisation and a configurable output filename
  (`[output] filename = "sop_instance_uid" | "preserve"`).
- **Date shifting**: stable per-patient offsets via HMAC with configurable
  `[dates] strategy = "conservative" | "aggressive"`, applied across DA/TM/DT.
- **De-identification markers**: `(0012,0062) = "YES"` and `(0012,0063)`.
- **Burned-in pixel masking** for rectangles and whole-frame regions, with a
  per-file `PixelReport { regions_masked, samples_masked }`.
- **Real "Clean" semantics**: the object's own identifiers (patient name
  components, patient/accession IDs, institution name/address/department,
  physician and operator names) are harvested *before* anonymisation and
  scrubbed out of descriptor text, so useful clinical text survives.
- **Audit trail**: JSON Lines per-file audit plus a run summary; every file is
  classified `ok`, `skipped` (not DICOM), `unsupported`, or `error` without
  aborting the run.
- **Parallel by default** across all CPUs (`--jobs N` to limit,
  `--fail-fast` / `--jobs 1` for sequential, stop-on-first-error behaviour).
- **Safety**: writes only to a separate output tree; refuses to write into the
  input tree and never modifies inputs in place.
- `--version` / `--help` standard CLI output.
- CI workflow (fmt, clippy `-D warnings`, tests, release build on Linux/Windows/
  macOS) and a tag-driven release workflow producing prebuilt archives for
  `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`,
  `aarch64-apple-darwin` and `x86_64-apple-darwin`.

### Known limitations

- No OCR: burned-in text is masked only where regions are configured.
- De-identification is irreversible; outputs are never intended to be re-linked.
- `C` (clean) is heuristic, not NLP-based: it removes known identifiers rather
  than recognising arbitrary free-text PHI.
- JPEG-LS requires building with the optional `charls` feature (needs a C
  toolchain); such files are reported as `unsupported` otherwise.
- `PlanarConfiguration = 1` colour images are rejected when pixel masking is
  required.

[Unreleased]: https://github.com/OWNER/REPO/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/OWNER/REPO/releases/tag/v0.1.0
