//! Output path mirroring.

use std::path::{Path, PathBuf};

/// Map an input-relative path under the output root.
pub fn output_path(out_root: &Path, rel: &Path) -> PathBuf {
    out_root.join(rel)
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
}
