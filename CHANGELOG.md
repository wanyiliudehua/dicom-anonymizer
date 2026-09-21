# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--export-identifiers FILE` and `--verify FILE`: record what was identifying as a
  **salted fingerprint** (hashes plus lengths, never a name) while anonymising, then later
  verify a tree against it with no need for the original data, config or output tree.
  Hits are reported with the offending substring; `--output` is not required by
  `--verify`.
- `[output] layout = "series"`: flatten the output tree into one directory per
  (remapped) Series Instance UID. Directory names are sanitized for path safety and a
  file with no usable series UID keeps its mirrored path instead of being dropped.
- `--check-leaks`: after the run, re-read the input tree for identifying strings and
  scan every text value in the output tree for them. Any hit is printed with its
  file, tag and identifier, and the run exits `1` — so an attribute the profile was
  told to retain cannot silently carry a name through. UI values are skipped (UIDs are
  remapped, so their digits are expected) and categories the user explicitly retained
  (`retain_institution_identity`) are excluded, keeping the check false-positive-free.

### Tests

- A file that declares **no** `SpecificCharacterSet` while carrying non-ASCII text is
  verified to round-trip **byte-for-byte** and still be cleaned, using a fixture built
  from hand-written bytes because the writer refuses to produce one. An earlier claim
  that such files cannot be written back was wrong and has been corrected here and in
  the README.
- Character-set coverage: `ISO_IR 192`, `GB18030`, `GBK` and `ISO 2022 IR 58` are
  verified to round-trip through real file IO with identifiers still scrubbed, and an
  unsupported code (`ISO 2022 IR 148`) is verified to be processed and cleaned rather
  than failing - including with accented non-ASCII text, where the output stays readable.
  (DICOM's `ISO 2022 IR 149` is the Korean set, not the Chinese one.)

### Fixed

- **The `C` clean action could corrupt text.** After removing an identifier it collapsed
  whitespace with `split_whitespace`, which counts NEL (U+0085) and NBSP as whitespace -
  and those are what undeclared single-byte text decodes to, so byte 0x85 silently became
  a space. `C` now trims only leading/trailing ASCII spaces and `^`, and rewrites nothing
  else.
- `--check-leaks` now sees **non-ASCII** text. Like the scrub before it, the scan skipped
  any value containing a non-ASCII byte, so a CJK identifier left in an output was
  invisible to it.
- The `C` clean action no longer normalizes whitespace in a text value when it removed
  nothing, matching the documented "no identifier present → unchanged" behaviour.
- The `C` clean action now handles **non-ASCII** identifiers. It previously skipped any
  value containing a non-ASCII byte and never harvested non-ASCII candidates, so a CJK
  or Cyrillic patient name or institution name survived cleaning untouched. Matching is
  now Unicode-aware case-insensitive over characters, with a script-aware length floor
  (2 characters for non-ASCII text, 3-4 for ASCII) so short ASCII tokens cannot shred
  clinical words.

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
