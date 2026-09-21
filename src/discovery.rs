//! Recursive discovery of DICOM files under an input directory.

use crate::error::Result;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub input: PathBuf,
    pub rel: PathBuf,
}

/// A file is DICOM when bytes 128..132 are the `DICM` magic.
pub fn is_dicom_file(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 132];
    match f.read_exact(&mut head) {
        Ok(()) => &head[128..132] == b"DICM",
        Err(_) => false,
    }
}

/// Walk `input`, returning one job per DICOM file with its relative path.
pub fn discover(input: &Path) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    for entry in WalkDir::new(input)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_dicom_file(path) {
            continue;
        }
        let rel = path.strip_prefix(input).unwrap_or(path).to_path_buf();
        jobs.push(Job {
            input: path.to_path_buf(),
            rel,
        });
    }
    jobs.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dicom_like(p: &std::path::Path) {
        let mut v = vec![0u8; 128];
        v.extend_from_slice(b"DICM");
        v.extend_from_slice(&[0u8; 16]);
        std::fs::write(p, v).unwrap();
    }

    #[test]
    fn detects_dicom_by_magic() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.dcm");
        write_dicom_like(&f);
        assert!(is_dicom_file(&f));
    }

    #[test]
    fn rejects_non_dicom() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        std::fs::write(&f, b"not a dicom").unwrap();
        assert!(!is_dicom_file(&f));
    }

    #[test]
    fn discover_walks_and_keeps_relative_paths() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        write_dicom_like(&d.path().join("top.dcm"));
        write_dicom_like(&d.path().join("sub/deep.dcm"));
        std::fs::write(d.path().join("sub/readme.txt"), b"x").unwrap();
        let jobs = discover(d.path()).unwrap();
        let rels: Vec<String> = jobs
            .iter()
            .map(|j| j.rel.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(rels.contains(&"top.dcm".to_string()));
        assert!(rels.contains(&"sub/deep.dcm".to_string()));
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn empty_dir_yields_no_jobs() {
        let d = tempfile::tempdir().unwrap();
        assert!(discover(d.path()).unwrap().is_empty());
    }
}
