//! Command-line interface.

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "dicom-anonymizer",
    version,
    about = "Bulk DICOM de-identification"
)]
pub struct Args {
    /// Input directory tree containing DICOM files.
    #[arg(short, long)]
    pub input: PathBuf,
    /// Output directory (created if needed); inputs are never modified. Required
    /// unless `--verify` is used.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// TOML config file; defaults to the built-in `ps3.15-basic` preset.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
    /// De-identification strategy: `conservative` or `aggressive`.
    #[arg(long)]
    pub strategy: Option<String>,
    /// Worker threads (default: all logical CPUs; `1` runs sequentially).
    #[arg(short, long)]
    pub jobs: Option<usize>,
    /// Run everything except writes; audit records go to stdout.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,
    /// After the run, verify that no identifier harvested from the input tree
    /// survives in the output tree (exits 1 on any hit).
    #[arg(long, default_value_t = false)]
    pub check_leaks: bool,
    /// Write a salted fingerprint of the identifiers found in the input tree to
    /// FILE, so the run can be re-verified later without the original data.
    #[arg(long, value_name = "FILE")]
    pub export_identifiers: Option<PathBuf>,
    /// Verify `--input` against a fingerprint written by `--export-identifiers`
    /// instead of anonymizing; exits 1 if anything survived.
    #[arg(long, value_name = "FILE")]
    pub verify: Option<PathBuf>,
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
        let a =
            Args::try_parse_from(["dicom-anonymizer", "--input", "in", "--output", "out"]).unwrap();
        assert_eq!(a.input.to_string_lossy(), "in");
        assert_eq!(a.output.as_deref().unwrap().to_string_lossy(), "out");
        assert!(!a.dry_run);
    }

    #[test]
    fn output_is_optional_so_verification_can_stand_alone() {
        let a = Args::try_parse_from(["x", "-i", "in", "--verify", "fp.json"]).unwrap();
        assert!(a.output.is_none());
        assert_eq!(a.verify.as_deref().unwrap().to_string_lossy(), "fp.json");
    }

    #[test]
    fn export_identifiers_parses() {
        let a = Args::try_parse_from([
            "x",
            "-i",
            "in",
            "-o",
            "out",
            "--export-identifiers",
            "fp.json",
        ])
        .unwrap();
        assert!(a.export_identifiers.is_some());
        assert!(a.verify.is_none());
    }

    #[test]
    fn jobs_defaults_to_none() {
        let a = Args::try_parse_from(["x", "-i", "in", "-o", "out"]).unwrap();
        assert_eq!(a.jobs, None);
    }

    #[test]
    fn parses_strategy_flag() {
        let a = Args::try_parse_from(["x", "-i", "in", "-o", "out", "--strategy", "conservative"])
            .unwrap();
        assert_eq!(a.strategy.as_deref(), Some("conservative"));
    }

    #[test]
    fn exposes_a_version() {
        use clap::CommandFactory;
        assert!(Args::command().get_version().is_some());
    }

    #[test]
    fn check_leaks_is_opt_in() {
        let a = Args::try_parse_from(["x", "-i", "in", "-o", "out"]).unwrap();
        assert!(!a.check_leaks);
        let b = Args::try_parse_from(["x", "-i", "in", "-o", "out", "--check-leaks"]).unwrap();
        assert!(b.check_leaks);
    }
}
