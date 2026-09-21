//! Deterministic, stateless UID remapping.
//!
//! The mapping is a pure function of `(salt, original)`, so it needs no shared
//! state and is safe to call from parallel workers; repeated references to the
//! same original UID always map to the same new UID.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub struct UidMapper {
    prefix: String,
    salt: Vec<u8>,
}

impl UidMapper {
    pub fn new(root: &str, salt: &[u8]) -> UidMapper {
        let mut prefix = root.trim().to_string();
        if !prefix.ends_with('.') {
            prefix.push('.');
        }
        UidMapper {
            prefix,
            salt: salt.to_vec(),
        }
    }

    pub fn map(&self, original: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.salt).expect("hmac accepts any key");
        mac.update(original.as_bytes());
        let bytes = mac.finalize().into_bytes();
        let n: u128 = u128::from_be_bytes(bytes[0..16].try_into().expect("16 bytes"));
        let mut s = format!("{}{}", self.prefix, n);
        s.truncate(64);
        while s.ends_with('.') {
            s.pop();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_input() {
        let m = UidMapper::new("2.25.", b"salt");
        assert_eq!(m.map("1.2.3"), m.map("1.2.3"));
    }

    #[test]
    fn different_inputs_differ() {
        let m = UidMapper::new("2.25.", b"salt");
        assert_ne!(m.map("1.2.3"), m.map("1.2.4"));
    }

    #[test]
    fn has_root_prefix_and_valid_chars() {
        let m = UidMapper::new("2.25.", b"salt");
        let u = m.map("1.2.840.10008.5.1.4.1.1.2");
        assert!(u.starts_with("2.25."));
        assert!(u.chars().all(|c| c.is_ascii_digit() || c == '.'));
        assert!(u.len() <= 64 && u.len() >= 3);
    }

    #[test]
    fn no_collisions_over_sample() {
        let m = UidMapper::new("2.25.", b"salt");
        let mut seen = std::collections::HashSet::new();
        for i in 0..1000u32 {
            assert!(
                seen.insert(m.map(&format!("1.2.3.{i}"))),
                "collision at {i}"
            );
        }
    }
}
