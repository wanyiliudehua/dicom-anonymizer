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

## Limits

Irreversible by design. JPEG-LS requires `--features charls`. DICOMDIR files
are skipped. Burned-in masking uses configured regions only; there is no OCR.
With the default random salts, results are not reproducible across runs; set
`dates.shift_salt` and `uids.salt` for reproducibility.
