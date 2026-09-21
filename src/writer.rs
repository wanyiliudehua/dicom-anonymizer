//! Output path mirroring and atomic file writes.

use crate::error::Result;
use std::path::{Path, PathBuf};

/// Map an input-relative path under the output root.
pub fn output_path(out_root: &Path, rel: &Path) -> PathBuf {
    out_root.join(rel)
}

/// Write `bytes` to `path` atomically: create parents, write a sibling temp
/// file, then rename over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| format!("{e}."))
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mirrors_relative_path_under_output_root() {
        let p = output_path(Path::new("/out"), Path::new("a/b/c.dcm"));
        assert!(p.ends_with("a/b/c.dcm"));
        assert!(p.starts_with("/out"));
    }

    #[test]
    fn write_atomic_creates_dirs_and_no_temp_left() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("x/y.bin");
        write_atomic(&target, b"hello").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
