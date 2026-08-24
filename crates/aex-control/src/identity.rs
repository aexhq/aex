//! Secrets: generation, hashing, custody rules.
//!
//! Two credentials, two jobs: the account token (`aex_at_`) manages money and keys, an API key
//! (`aex_sk_`) runs sessions. Both are 48 characters of uniform base62 after the prefix (~286
//! bits), shown exactly once; the store holds only their SHA-256. The `prefix` column keeps the
//! first characters for recognition in a list — never enough to authenticate.

use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const BASE62: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Clone)]
pub struct TenantToolTokenKey(Arc<[u8]>);

impl TenantToolTokenKey {
    pub fn new(value: impl AsRef<[u8]>) -> Result<Self, &'static str> {
        let value = value.as_ref();
        if !(32..=256).contains(&value.len()) || !value.iter().all(u8::is_ascii_graphic) {
            return Err("must contain 32 through 256 visible ASCII bytes");
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn mint(&self, account_id: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts every key size");
        mac.update(b"aex.tenant-tool.v1\0");
        mac.update(account_id.as_bytes());
        format!(
            "aex_tt_{account_id}.{}",
            hex::encode(mac.finalize().into_bytes())
        )
    }

    pub fn authenticate(&self, token: &str) -> Option<String> {
        let payload = token.strip_prefix("aex_tt_")?;
        let (account_id, signature) = payload.split_once('.')?;
        if account_id.is_empty()
            || account_id.len() > 128
            || !account_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return None;
        }
        let signature = hex::decode(signature).ok()?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).ok()?;
        mac.update(b"aex.tenant-tool.v1\0");
        mac.update(account_id.as_bytes());
        mac.verify_slice(&signature).ok()?;
        Some(account_id.into())
    }
}

impl std::fmt::Debug for TenantToolTokenKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TenantToolTokenKey(<redacted>)")
    }
}

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

    #[test]
    fn tenant_tool_tokens_are_fixed_to_one_account() {
        let key = TenantToolTokenKey::new("k".repeat(32)).unwrap();
        let token = key.mint("acc_one");
        assert_eq!(key.authenticate(&token).as_deref(), Some("acc_one"));
        assert!(
            key.authenticate(&token.replace("acc_one", "acc_two"))
                .is_none()
        );
        assert!(key.authenticate("aex_tt_acc_one.not-hex").is_none());
    }
}
