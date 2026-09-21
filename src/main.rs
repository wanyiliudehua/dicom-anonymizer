mod audit;
mod cli;
mod config;
mod deid;
mod discovery;
mod error;
mod pipeline;
mod pixel;
mod report;
mod writer;

use clap::Parser;
use pipeline::Shared;
use std::io::Write;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args = cli::Args::parse();

    let cfg = match &args.config {
        Some(p) => match config::Config::load(p) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        },
        None => config::Config::default(),
    };

    if !args.input.is_dir() {
        eprintln!("error: input `{}` is not a directory", args.input.display());
        return 2;
    }
    if let Err(e) = std::fs::create_dir_all(&args.output) {
        eprintln!("error: cannot create output dir: {e}");
        return 2;
    }

    let uid_salt = if cfg.uids.salt.is_empty() {
        random_salt()
    } else {
        cfg.uids.salt.clone().into_bytes()
    };
    let date_salt = if cfg.dates.shift_salt.is_empty() {
        random_salt()
    } else {
        cfg.dates.shift_salt.clone().into_bytes()
    };

    let shared = Shared {
        uids: deid::uids::UidMapper::new(&cfg.uids.root, &uid_salt),
        dates: deid::dates::DateShifter::new(&date_salt, cfg.dates.range_days),
        cfg,
        out_root: args.output.clone(),
        dry_run: args.dry_run,
    };

    let jobs = match discovery::discover(&args.input) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    // Audit sink: stdout in dry-run, otherwise <output>/audit.jsonl.
    let audit_file = if args.dry_run {
        None
    } else {
        let path = args.output.join("audit.jsonl");
        match std::fs::File::create(&path) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("error: cannot create audit log: {e}");
                return 2;
            }
        }
    };
    let stdout = std::io::stdout();
    let mut log: Box<dyn Write> = match audit_file {
        Some(f) => Box::new(f),
        None => Box::new(stdout),
    };

    let mut summary = report::Summary::default();
    for job in &jobs {
        let mut line = Vec::new();
        match pipeline::process_job(&shared, job, &mut line) {
            Ok(rec) => {
                summary.count(rec.status);
                let _ = log.write_all(&line);
                if args.fail_fast && rec.status == audit::Status::Error {
                    break;
                }
            }
            Err(e) => {
                summary.count(audit::Status::Error);
                eprintln!("error: {}: {e}", job.rel.display());
                if args.fail_fast {
                    break;
                }
            }
        }
    }

    if args.verbose {
        eprintln!("processed {} file(s)", jobs.len());
    }
    eprintln!(
        "ok={} skipped={} unsupported={} error={}",
        summary.ok, summary.skipped, summary.unsupported, summary.error
    );
    summary.exit_code()
}

fn random_salt() -> Vec<u8> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}-{}", std::process::id()).into_bytes()
}
