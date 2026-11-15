use std::fmt;
use std::str::FromStr;

use crate::GfError;

/// A git object id in lowercase hex. Supports both SHA-1 (40 chars) and
/// SHA-256 (64 chars) repositories.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid(String);

impl Oid {
    pub const SHA1_HEX_LEN: usize = 40;
    pub const SHA256_HEX_LEN: usize = 64;

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Abbreviated form for display. Use `git rev-parse --short` when the
    /// abbreviation must be unambiguous within a repository.
    pub fn short(&self) -> &str {
        &self.0[..7]
    }

    /// The all-zero id git uses for "no object" (e.g. a ref being created).
    pub fn is_null(&self) -> bool {
        self.0.bytes().all(|b| b == b'0')
    }
}

impl FromStr for Oid {
    type Err = GfError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid_len = s.len() == Self::SHA1_HEX_LEN || s.len() == Self::SHA256_HEX_LEN;
        if valid_len && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(Self(s.to_ascii_lowercase()))
        } else {
            Err(GfError::InvalidOid(s.to_owned()))
        }
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sha1_and_sha256() {
        let sha1 = "A94A8FE5CCB19BA61C4C0873D391E987982FBBD3";
        let oid: Oid = sha1.parse().expect("valid sha1");
        assert_eq!(oid.as_str(), sha1.to_ascii_lowercase());
        assert_eq!(oid.short(), "a94a8fe");

        assert!("0".repeat(64).parse::<Oid>().is_ok());
    }

    #[test]
    fn rejects_bad_input() {
        assert!("abc".parse::<Oid>().is_err());
        assert!("z".repeat(40).parse::<Oid>().is_err());
        assert!("a".repeat(41).parse::<Oid>().is_err());
    }

    #[test]
    fn detects_null_id() {
        assert!(
            "0".repeat(40)
                .parse::<Oid>()
                .map(|o| o.is_null())
                .unwrap_or(false)
        );
    }
}
