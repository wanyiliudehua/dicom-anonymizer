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
use std::io::IsTerminal;
use std::io::Write;

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args = cli::Args::parse();

    // Verification is a separate, config-free mode: a fingerprint already records
    // what was identifying when it was exported.
    if let Some(path) = args.verify.clone() {
        return run_verify(&args.input, &path, args.require_complete);
    }
    if args.require_complete {
        eprintln!("error: --require-complete only applies to --verify");
        return 2;
    }

    let Some(output) = args.output.clone() else {
        eprintln!("error: --output is required (unless --verify is used)");
        return 2;
    };

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
    if let Err(e) = std::fs::create_dir_all(&output) {
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
        out_root: output.clone(),
        dry_run: args.dry_run,
    };

    let jobs = match discovery::discover(&args.input) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };

    // Optional: record what was identifying, as salted hashes, so this run can be
    // re-verified later without the originals and without storing PHI again.
    if let Some(path) = &args.export_identifiers {
        if let Err(e) = export_fingerprint(&args.input, &shared.cfg, path) {
            eprintln!("error: cannot export identifiers: {e}");
            return 2;
        }
    }

    // Audit sink: stdout in dry-run, otherwise <output>/audit.jsonl.
    let audit_file = if args.dry_run {
        None
    } else {
        let path = output.join("audit.jsonl");
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
    // Progress is for a human watching a long batch: asked for with --verbose, or
    // offered when stderr is a terminal. Redirected output stays quiet.
    let progress =
        report::Progress::new(jobs.len(), args.verbose || std::io::stderr().is_terminal());
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
            if let Some(line) = progress.tick() {
                eprintln!("{line}");
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
                // Reported from whichever thread finishes, so the counter stays honest
                // even though completions arrive out of order.
                if let Some(line) = progress.tick() {
                    eprintln!("{line}");
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

    let mut leak_failed = false;
    if args.check_leaks {
        if args.dry_run {
            eprintln!(
                "note: --check-leaks needs an output tree, and --dry-run writes none; skipping"
            );
        } else if !check_leaks(&args.input, &output, &shared.cfg) {
            leak_failed = true;
        }
    }

    // Mark the tree as finished. Written last, and never in dry-run (which produces
    // no tree), so its presence means the run reached the end.
    if !args.dry_run {
        let manifest = report::Manifest::new(&summary, jobs.len(), !audit_failed);
        if let Err(e) = manifest.write(&output) {
            eprintln!("error: cannot write {}: {e}", report::MANIFEST_FILE);
            return 1;
        }
    }

    if audit_failed || leak_failed {
        return 1;
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

/// Salt for a fingerprint: random per export, so one artefact cannot be compared
/// against another run's.
fn random_salt_str() -> String {
    String::from_utf8_lossy(&random_salt()).into_owned()
}

/// Write the identifiers of `input` as salted hashes.
fn export_fingerprint(
    input: &std::path::Path,
    cfg: &config::Config,
    path: &std::path::Path,
) -> Result<(), String> {
    let harvested = verify::harvest(input).map_err(|e| e.to_string())?;
    let ids = verify::check_set(&harvested, cfg);
    let fp = verify::fingerprint(&ids, &random_salt_str());
    let json = serde_json::to_string_pretty(&fp).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())?;
    eprintln!(
        "exported {} identifier fingerprint(s) to {}",
        fp.hashes.len(),
        path.display()
    );
    Ok(())
}

/// Scan `input` against a fingerprint and report anything that survived.
fn run_verify(input: &std::path::Path, path: &std::path::Path, require_complete: bool) -> i32 {
    if !input.is_dir() {
        eprintln!("error: input `{}` is not a directory", input.display());
        return 2;
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read fingerprint {}: {e}", path.display());
            return 2;
        }
    };
    let fp: verify::Fingerprint = match serde_json::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: {} is not a fingerprint: {e}", path.display());
            return 2;
        }
    };
    // A tree nobody here produced may have no marker. Without --require-complete that is
    // worth saying but not worth failing; with it, an unfinished tree is exactly what the
    // caller asked to be told about, and being clean is not the same as being complete.
    let mut tree_ok = true;
    match report::Manifest::load(input) {
        Ok(Some(m)) => {
            eprintln!(
                "manifest: outcome={:?} total={} ok={} skipped={} unsupported={} error={} audit_complete={}",
                m.outcome, m.total, m.ok, m.skipped, m.unsupported, m.error, m.audit_complete
            );
            if require_complete && (m.outcome != report::Outcome::Complete || !m.audit_complete) {
                tree_ok = false;
                eprintln!(
                    "error: --require-complete, but this is not a complete run \
                     (outcome={:?}, audit_complete={})",
                    m.outcome, m.audit_complete
                );
            }
        }
        Ok(None) => {
            eprintln!(
                "manifest: none (no {}) - the tree may be incomplete or produced elsewhere",
                report::MANIFEST_FILE
            );
            if require_complete {
                tree_ok = false;
                eprintln!(
                    "error: --require-complete, but the tree carries no {}",
                    report::MANIFEST_FILE
                );
            }
        }
        Err(e) => {
            eprintln!("manifest: unreadable: {e}");
            if require_complete {
                tree_ok = false;
            }
        }
    }

    let hits = match verify::scan_fingerprint(input, &fp) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: cannot scan {}: {e}", input.display());
            return 2;
        }
    };
    for h in &hits {
        match h.location {
            verify::Location::Tag(t) => eprintln!(
                "leak: {} ({:04X},{:04X}) contains `{}`",
                h.file, t.0, t.1, h.matched
            ),
            verify::Location::Line(n) => {
                eprintln!("leak: {} line {n} contains `{}`", h.file, h.matched)
            }
        }
    }
    // One hit per line says where to look; the surrounding text is in `value`.
    let files_hit = {
        let mut f: Vec<&str> = hits.iter().map(|h| h.file.as_str()).collect();
        f.sort_unstable();
        f.dedup();
        f.len()
    };
    eprintln!(
        "verify: {} hit(s) in {files_hit} file(s), using {} fingerprint(s) over {}",
        hits.len(),
        fp.hashes.len(),
        input.display()
    );
    if tree_ok && hits.is_empty() {
        0
    } else {
        1
    }
}
