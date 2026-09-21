//! Command-line interface.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "dicom-anonymizer", about = "Bulk DICOM de-identification")]
pub struct Args {
    /// Input directory tree containing DICOM files.
    #[arg(short, long)]
    pub input: PathBuf,
    /// Output directory (created if needed); inputs are never modified.
    #[arg(short, long)]
    pub output: PathBuf,
    /// TOML config file; defaults to the built-in `ps3.15-basic` preset.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    /// Worker threads (default: logical CPU count).
    #[arg(short, long)]
    pub jobs: Option<usize>,
    /// Run everything except writes; audit records go to stdout.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
    /// Stop at the first per-file error.
    #[arg(long, default_value_t = false)]
    pub fail_fast: bool,
    /// Increase log detail on stderr.
    #[arg(short, long, default_value_t = false)]
    pub verbose: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_required_args() {
        let a = Args::try_parse_from(["dicom-anonymizer", "--input", "in", "--output", "out"])
            .unwrap();
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
