//! Bearer-token authentication with constant-time comparison.
//!
//! We compare SHA-256 digests of fixed length rather than the raw token
//! strings: this makes the comparison constant-time *regardless of input
//! length* (raw `ct_eq` requires equal-length inputs and would otherwise leak
//! length via early rejection). Every configured token is checked on every
//! request so timing does not reveal which token, if any, was close.

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub struct Auth {
    /// SHA-256 of each configured token.
    digests: Vec<[u8; 32]>,
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

impl Auth {
    pub fn new(tokens: &[String]) -> Auth {
        Auth {
            // Trim configured tokens so surrounding whitespace (easy to paste
            // in by accident) matches the trimmed presented token, rather than
            // silently creating a dead, never-matching secret.
            digests: tokens.iter().map(|t| digest(t.trim().as_bytes())).collect(),
        }
    }

    /// Constant-time check of a presented token against the configured set.
    /// Always iterates the full set; never short-circuits.
    pub fn verify(&self, presented: &str) -> bool {
        let got = digest(presented.as_bytes());
        let mut ok = subtle::Choice::from(0u8);
        for d in &self.digests {
            ok |= got.ct_eq(d);
        }
        bool::from(ok)
    }

    /// Extract the bearer token from an `Authorization` header value and verify
    /// it. Returns true only on a well-formed, valid `Bearer <token>`.
    pub fn verify_header(&self, header: Option<&str>) -> bool {
        let Some(value) = header else {
            return false;
        };
        // "<scheme> <token>"; the scheme is case-insensitive per RFC 7235/6750.
        let Some((scheme, token)) = value.split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("bearer") {
            return false;
        }
        let token = token.trim();
        if token.is_empty() {
            return false;
        }
        self.verify(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_rejects_invalid() {
        let a = Auth::new(&["correct-horse-battery-staple-1234".into()]);
        assert!(a.verify("correct-horse-battery-staple-1234"));
        assert!(!a.verify("correct-horse-battery-staple-1235"));
        assert!(!a.verify(""));
        assert!(!a.verify("correct-horse-battery-staple-1234 ")); // exact match only
    }

    #[test]
    fn header_parsing() {
        let a = Auth::new(&["correct-horse-battery-staple-1234".into()]);
        assert!(a.verify_header(Some("Bearer correct-horse-battery-staple-1234")));
        assert!(a.verify_header(Some("bearer correct-horse-battery-staple-1234")));
        assert!(a.verify_header(Some("BEARER correct-horse-battery-staple-1234")));
        assert!(!a.verify_header(Some("Basic correct-horse-battery-staple-1234")));
        assert!(!a.verify_header(Some("correct-horse-battery-staple-1234")));
        assert!(!a.verify_header(None));
        assert!(!a.verify_header(Some("Bearer ")));
    }

    #[test]
    fn configured_token_is_trimmed() {
        let a = Auth::new(&["  padded-token-aaaaaaaaaaaaaaaaaaaa  ".into()]);
        assert!(a.verify_header(Some("Bearer padded-token-aaaaaaaaaaaaaaaaaaaa")));
    }

    #[test]
    fn multiple_tokens() {
        let a = Auth::new(&[
            "first-token-aaaaaaaaaaaaaaaaaaaa".into(),
            "second-token-bbbbbbbbbbbbbbbbbbbb".into(),
        ]);
        assert!(a.verify("first-token-aaaaaaaaaaaaaaaaaaaa"));
        assert!(a.verify("second-token-bbbbbbbbbbbbbbbbbbbb"));
        assert!(!a.verify("third-token-cccccccccccccccccccc"));
    }
}
