use crate::error::{Error, Result};
use axum::http::HeaderMap;
use rand::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug)]
pub struct Principal {
    pub account: String,
    pub key: String,
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn random(prefix: &str) -> String {
    let mut bytes = [0; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}
pub fn bearer(headers: &HeaderMap) -> Result<&str> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty() && v.len() <= 1024)
        .ok_or_else(Error::denied)
}
pub fn matches(token: &str, verifier: &str) -> bool {
    bool::from(
        digest(token.as_bytes())
            .as_bytes()
            .ct_eq(verifier.as_bytes()),
    )
}
pub fn operation_key(headers: &HeaderMap) -> Result<&str> {
    headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 256)
        .ok_or_else(|| Error::invalid("idempotency-key must be 1 to 256 bytes"))
}
pub fn scoped_key(principal: &Principal, path: &str, key: &str) -> String {
    digest(
        serde_json::to_vec(&(&principal.account, path, key))
            .unwrap()
            .as_slice(),
    )
}
