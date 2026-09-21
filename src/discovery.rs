//! Recursive discovery of files under an input directory.

use crate::error::Result;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub input: PathBuf,
    pub rel: PathBuf,
    /// Whether the file is DICOM (`DICM` magic at offset 128).
    pub dicom: bool,
    /// Size in bytes, used to bound how many files may be in flight at once.
    pub size: u64,
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

/// Walk `input`, returning one job per file (DICOM and non-DICOM) with its
/// relative path. Non-DICOM files are reported so the audit can log them.
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
        let rel = path.strip_prefix(input).unwrap_or(path).to_path_buf();
        jobs.push(Job {
            input: path.to_path_buf(),
            rel,
            dicom: is_dicom_file(path),
            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
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
    fn discover_reports_dicom_and_non_dicom() {
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
        assert!(rels.contains(&"sub/readme.txt".to_string()));
        assert_eq!(jobs.len(), 3);
        let nd = jobs.iter().find(|j| j.rel.ends_with("readme.txt")).unwrap();
        assert!(!nd.dicom);
        let dd = jobs.iter().find(|j| j.rel.ends_with("top.dcm")).unwrap();
        assert!(dd.dicom);
    }

    #[test]
    fn empty_dir_yields_no_jobs() {
        let d = tempfile::tempdir().unwrap();
        assert!(discover(d.path()).unwrap().is_empty());
    }
}
