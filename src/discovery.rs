//! Recursive discovery of files under an input directory.

use crate::error::Result;
use std::io::Read;
use std::path::{Path, PathBuf};

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
///
/// ponytail: unreadable directories are skipped, matching the walker this
/// replaced; make them loud if a skipped subtree ever needs to be auditable.
pub fn discover(input: &Path) -> Result<Vec<Job>> {
    let mut jobs = Vec::new();
    let mut dirs = vec![input.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // `file_type` does not follow symlinks, so a linked directory is not
            // descended into and a linked file is not reported - the same rule the
            // previous walker applied with follow_links(false).
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                dirs.push(path);
            } else if ft.is_file() {
                let rel = path.strip_prefix(input).unwrap_or(&path).to_path_buf();
                let dicom = is_dicom_file(&path);
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                jobs.push(Job {
                    input: path,
                    rel,
                    dicom,
                    size,
                });
            }
        }
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
