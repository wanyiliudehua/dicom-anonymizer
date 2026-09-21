mod audit;
mod cli;
mod config;
mod deid;
mod discovery;
mod error;
mod pipeline;
mod pixel;
mod report;
mod verify;
mod writer;

use clap::Parser;
use pipeline::Shared;
use std::io::Write;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args = cli::Args::parse();

    let mut cfg = match &args.config {
        Some(p) => match config::Config::load(p) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        },
        None => config::Config::default(),
    };

    if let Some(s) = &args.strategy {
        cfg.profile.strategy = match s.to_ascii_lowercase().as_str() {
            "conservative" => config::Strategy::Conservative,
            "aggressive" => config::Strategy::Aggressive,
            other => {
                eprintln!("error: unknown --strategy `{other}` (use conservative|aggressive)");
                return 2;
            }
        };
    }

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

    let mut audit_failed = false;
    let mut summary = report::Summary::default();
    if args.fail_fast || args.jobs == Some(1) {
        for job in &jobs {
            let mut line = Vec::new();
            match pipeline::process_job(&shared, job, &mut line) {
                Ok(rec) => {
                    summary.count(rec.status);
                    if audit::write_raw(&mut log, &line).is_err() {
                        audit_failed = true;
                        eprintln!("error: failed to write audit record");
                    }
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
    } else {
        use rayon::prelude::*;
        if let Some(n) = args.jobs {
            let _ = rayon::ThreadPoolBuilder::new()
                .num_threads(n)
                .build_global();
        }
        // `Shared` is immutable, and UID/date mapping are pure functions, so
        // file processing parallelizes without any shared mutable state.
        let results: Vec<(report::Summary, Vec<u8>)> = jobs
            .par_iter()
            .map(|job| {
                let mut line = Vec::new();
                let mut s = report::Summary::default();
                match pipeline::process_job(&shared, job, &mut line) {
                    Ok(rec) => s.count(rec.status),
                    Err(_) => s.count(audit::Status::Error),
                }
                (s, line)
            })
            .collect();
        for (s, line) in &results {
            summary.ok += s.ok;
            summary.skipped += s.skipped;
            summary.unsupported += s.unsupported;
            summary.error += s.error;
            if audit::write_raw(&mut log, line).is_err() {
                audit_failed = true;
                eprintln!("error: failed to write audit record");
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
    if audit_failed {
        return 1;
    }
    if args.check_leaks {
        if args.dry_run {
            eprintln!(
                "note: --check-leaks needs an output tree, and --dry-run writes none; skipping"
            );
        } else if !check_leaks(&args.input, &args.output, &shared.cfg) {
            return 1;
        }
    }
    summary.exit_code()
}

/// Re-read the input tree to learn what was identifying, then re-read the
/// output tree and report anything that survived. Returns true when clean.
fn check_leaks(input: &std::path::Path, output: &std::path::Path, cfg: &config::Config) -> bool {
    let harvested = match verify::harvest(input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: leak check could not read the input tree: {e}");
            return false;
        }
    };
    let ids = verify::check_set(&harvested, cfg);
    let leaks = match verify::scan(output, &ids) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: leak check could not read the output tree: {e}");
            return false;
        }
    };
    for l in &leaks {
        eprintln!(
            "leak: {} ({:04X},{:04X}) still contains `{}`",
            l.file, l.tag.0, l.tag.1, l.identifier
        );
    }
    eprintln!(
        "leak check: {} hit(s) from {} identifier(s)",
        leaks.len(),
        ids.len()
    );
    leaks.is_empty()
}

fn random_salt() -> Vec<u8> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos}-{}", std::process::id()).into_bytes()
}
