//! Deterministic, per-patient date shifting.
//!
//! The offset is a pure function of `(salt, original PatientID)`, so all dates
//! for a patient move by the same amount and longitudinal intervals are kept.

use chrono::{Duration, NaiveDate};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct DateShifter {
    salt: Vec<u8>,
    range_days: i64,
}

impl DateShifter {
    pub fn new(salt: &[u8], range_days: i64) -> DateShifter {
        DateShifter {
            salt: salt.to_vec(),
            range_days: range_days.max(1),
        }
    }

    /// Deterministic per-patient offset in `[-range_days, +range_days]`.
    pub fn offset_for(&self, patient_id: &str) -> i64 {
        let mut mac = HmacSha256::new_from_slice(&self.salt).expect("hmac accepts any key");
        mac.update(patient_id.as_bytes());
        let bytes = mac.finalize().into_bytes();
        let n = u64::from_be_bytes(bytes[0..8].try_into().expect("8 bytes"));
        let span = (2 * self.range_days + 1) as u64;
        (n % span) as i64 - self.range_days
    }

    /// `YYYYMMDD` -> shifted `YYYYMMDD`; `None` for anything not a valid date.
    pub fn shift_da(&self, da: &str, offset: i64) -> Option<String> {
        if da.len() != 8 {
            return None;
        }
        let d = NaiveDate::parse_from_str(da, "%Y%m%d").ok()?;
        Some((d + Duration::days(offset)).format("%Y%m%d").to_string())
    }

    /// `YYYYMMDD[HHMMSS[.F...]]` -> date part shifted, time part preserved.
    pub fn shift_dt(&self, dt: &str, offset: i64) -> Option<String> {
        if dt.len() < 8 {
            return None;
        }
        let (d, rest) = dt.split_at(8);
        let shifted = self.shift_da(d, offset)?;
        Some(format!("{shifted}{rest}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_deterministic_and_bounded() {
        let s = DateShifter::new(b"salt", 100);
        let a = s.offset_for("P1");
        assert_eq!(a, s.offset_for("P1"));
        assert!((-100..=100).contains(&a));
    }

    #[test]
    fn shift_da_preserves_interval() {
        let s = DateShifter::new(b"salt", 100);
        let off = s.offset_for("P1");
        let d1 = s.shift_da("20240101", off).unwrap();
        let d2 = s.shift_da("20240111", off).unwrap();
        let a = chrono::NaiveDate::parse_from_str(&d1, "%Y%m%d").unwrap();
        let b = chrono::NaiveDate::parse_from_str(&d2, "%Y%m%d").unwrap();
        assert_eq!((b - a).num_days(), 10);
    }

    #[test]
    fn shift_dt_keeps_time_and_fraction() {
        let s = DateShifter::new(b"salt", 30);
        let out = s.shift_dt("20240101123045.123", 5).unwrap();
        assert!(out.ends_with("123045.123"));
        assert_eq!(&out[0..8], "20240106");
    }

    #[test]
    fn malformed_dates_return_none_not_panic() {
        let s = DateShifter::new(b"salt", 30);
        assert_eq!(s.shift_da("2024", 1), None);
        assert_eq!(s.shift_da("", 1), None);
        assert_eq!(s.shift_dt("2024", 1), None);
        assert_eq!(s.shift_dt("", 1), None);
    }
}
