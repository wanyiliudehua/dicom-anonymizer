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
    /// Output directory (created if needed); inputs are never modified.
    #[arg(short, long)]
    pub output: PathBuf,
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
        assert_eq!(a.output.to_string_lossy(), "out");
        assert!(!a.dry_run);
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
