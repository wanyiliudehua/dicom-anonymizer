//! Run summary and exit-code selection.

use crate::audit::Status;

#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub ok: usize,
    pub skipped: usize,
    pub unsupported: usize,
    pub error: usize,
}

impl Summary {
    pub fn count(&mut self, status: Status) {
        match status {
            Status::Ok => self.ok += 1,
            Status::Skipped | Status::SkippedDicomdir => self.skipped += 1,
            Status::Unsupported => self.unsupported += 1,
            Status::Error => self.error += 1,
        }
    }

    /// 0 when every file succeeded or was skipped, 1 on any error.
    pub fn exit_code(&self) -> i32 {
        if self.error > 0 {
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::Status;

    #[test]
    fn all_ok_is_exit_zero() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        assert_eq!(s.exit_code(), 0);
    }

    #[test]
    fn any_error_is_exit_one() {
        let mut s = Summary::default();
        s.count(Status::Ok);
        s.count(Status::Error);
        assert_eq!(s.exit_code(), 1);
    }

    #[test]
    fn skipped_only_is_exit_zero() {
        let mut s = Summary::default();
        s.count(Status::Skipped);
        s.count(Status::SkippedDicomdir);
        assert_eq!(s.exit_code(), 0);
    }
}
