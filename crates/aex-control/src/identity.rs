//! Secrets: generation, hashing, custody rules.
//!
//! Two credentials, two jobs: the account token (`aex_at_`) manages money and keys, an API key
//! (`aex_sk_`) runs sessions. Both are 48 characters of uniform base62 after the prefix (~286
//! bits), shown exactly once; the store holds only their SHA-256. The `prefix` column keeps the
//! first characters for recognition in a list — never enough to authenticate.

use rand::Rng;
use sha2::{Digest, Sha256};

const BASE62: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn base62(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| BASE62[rng.random_range(0..BASE62.len())] as char)
        .collect()
}

/// A short random id: `acc_`/`key_`/`top_` + 24 base62 chars (fits the contract patterns).
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", base62(24))
}

/// A freshly minted secret and what the store keeps of it.
pub struct Minted {
    pub secret: String,
    pub hash: String,
    pub prefix: String,
}

/// Mint `aex_at_...` / `aex_sk_...`; `kind` is "at" or "sk".
pub fn mint_secret(kind: &str) -> Minted {
    let secret = format!("aex_{kind}_{}", base62(48));
    Minted {
        hash: hash_secret(&secret),
        prefix: secret[..12].to_string(),
        secret,
    }
}

/// SHA-256 of the full secret string, lower-case hex — the only form the store sees.
pub fn hash_secret(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// Compare a presented secret with one stored SHA-256 hex digest without leaking a matching
/// prefix through ordinary string equality. The stored digest is host-owned; malformed values
/// simply fail authentication.
pub fn secret_matches_hash(secret: &str, expected_hex: &str) -> bool {
    let candidate = Sha256::digest(secret.as_bytes());
    let mut expected = [0u8; 32];
    if hex::decode_to_slice(expected_hex, &mut expected).is_err() {
        return false;
    }
    candidate
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (left, right)| {
            difference | (*left ^ right)
        })
        == 0
}

/// The bearer token out of an Authorization header, if any.
pub fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_secrets_match_the_contract_pattern_and_never_repeat() {
        let a = mint_secret("sk");
        let b = mint_secret("sk");
        assert_ne!(a.secret, b.secret);
        assert!(a.secret.starts_with("aex_sk_"));
        assert_eq!(a.secret.len(), "aex_sk_".len() + 48);
        assert!(a.secret[7..].bytes().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(a.prefix.len(), 12);
        assert!(a.secret.starts_with(&a.prefix));
        assert_eq!(a.hash, hash_secret(&a.secret));
        assert_ne!(a.hash, b.hash);
        assert!(secret_matches_hash(&a.secret, &a.hash));
        assert!(!secret_matches_hash(&b.secret, &a.hash));
        assert!(!secret_matches_hash(&a.secret, "not-a-digest"));
    }

    #[test]
    fn ids_fit_the_contract_patterns() {
        let id = new_id("acc");
        assert_eq!(id.len(), 4 + 24);
        assert!(id.starts_with("acc_"));
    }
}
