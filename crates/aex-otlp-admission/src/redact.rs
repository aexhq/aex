//! Managed-secret redaction without any decrypt permission.
//!
//! `regional-otlp` must remove AEX-managed secret values the platform itself
//! injected into the guest, but giving the OTLP role decrypt permission would
//! defeat the secret isolation the regional-secret stream builds. The redactor
//! therefore matches a **keyed digest manifest**: `regional-secret-custody`
//! publishes `{len, hmac}` pairs under a regional redaction key, and this module
//! slides a window of each declared length and compares the HMAC.
//!
//! Exceeding [`crate::limits::OtlpLimits::redact_budget_bytes`] fails the batch
//! **closed**. Admitting unredacted bytes is not an acceptable degradation, so
//! there is no "best effort" mode and no partial redaction result.
//!
//! AEX-produced observations are redacted by their producer before
//! `admit_semantic`. The product never claims that arbitrary customer-authored
//! bytes can be recognised as secrets.

use std::collections::BTreeMap;

use aex_observation_domain::canonical::CanonicalValue;
use aex_wire::ids::SessionId;
use hmac::{Hmac, Mac as _};
use sha2::Sha256;

use crate::error::OtlpError;

/// The replacement written over a matched secret.
pub const REDACTED: &str = "[REDACTED]";

/// The shortest window the redactor will consider.
///
/// Below eight bytes a match is far more likely to be a coincidence than a
/// secret, and redacting ordinary text would corrupt customer telemetry.
pub const MIN_SECRET_BYTES: usize = 8;

/// One managed secret, as a length and a keyed digest.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SecretDigest {
    /// The secret's exact byte length.
    pub len: u16,
    /// `HMAC-SHA256(k_region, secret)`.
    pub hmac: [u8; 32],
}

/// Every managed secret injected into one session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretDigestManifest {
    /// The session the manifest belongs to.
    pub session: SessionId,
    /// The custody generation this manifest was written with.
    pub custody_revision: u64,
    /// The digests, in no particular order.
    pub entries: Vec<SecretDigest>,
}

impl SecretDigestManifest {
    /// The distinct window lengths the redactor must slide.
    ///
    /// Bounded by the session's secret count, which is what makes the matching
    /// cost `O(text x distinct_lengths)` rather than `O(text x secrets)`.
    #[must_use]
    pub fn distinct_lengths(&self) -> Vec<usize> {
        let mut lengths: Vec<usize> = self
            .entries
            .iter()
            .map(|entry| usize::from(entry.len))
            .filter(|len| *len >= MIN_SECRET_BYTES)
            .collect();
        lengths.sort_unstable();
        lengths.dedup();
        lengths
    }
}

/// What one redaction pass did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RedactionReport {
    /// How many values were rewritten.
    pub redacted_values: u32,
    /// How many canonical text bytes were scanned.
    pub scanned_bytes: u64,
}

impl RedactionReport {
    /// Folds another report into this one.
    pub const fn merge(&mut self, other: Self) {
        self.redacted_values += other.redacted_values;
        self.scanned_bytes += other.scanned_bytes;
    }
}

/// Removes AEX-managed secret material from a canonical value.
pub trait ManagedSecretRedactor: Send + Sync {
    /// Redacts in place.
    ///
    /// # Errors
    ///
    /// Returns [`OtlpError::RedactionBudgetExhausted`] when the scan budget runs
    /// out. The caller fails the batch; it never admits the partially scanned
    /// value.
    fn redact(&self, value: &mut CanonicalValue) -> Result<RedactionReport, OtlpError>;
}

/// The keyed digest redactor.
pub struct DigestRedactor {
    key: Vec<u8>,
    manifest: SecretDigestManifest,
    budget_bytes: u64,
}

#[allow(
    clippy::missing_fields_in_debug,
    reason = "the redaction key is deliberately never rendered"
)]
impl std::fmt::Debug for DigestRedactor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key never reaches a log line, a span attribute or a panic message.
        formatter
            .debug_struct("DigestRedactor")
            .field("session", &self.manifest.session)
            .field("custody_revision", &self.manifest.custody_revision)
            .field("entries", &self.manifest.entries.len())
            .field("budget_bytes", &self.budget_bytes)
            .finish()
    }
}

impl DigestRedactor {
    /// Builds a redactor over one session's manifest.
    #[must_use]
    pub fn new(key: Vec<u8>, manifest: SecretDigestManifest, budget_bytes: u64) -> Self {
        Self {
            key,
            manifest,
            budget_bytes,
        }
    }

    fn digest(&self, window: &[u8]) -> [u8; 32] {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(&self.key)
            .expect("HMAC accepts a key of any length");
        mac.update(window);
        mac.finalize().into_bytes().into()
    }

    fn index(&self) -> BTreeMap<usize, Vec<[u8; 32]>> {
        let mut index: BTreeMap<usize, Vec<[u8; 32]>> = BTreeMap::new();
        for entry in &self.manifest.entries {
            let len = usize::from(entry.len);
            if len >= MIN_SECRET_BYTES {
                index.entry(len).or_default().push(entry.hmac);
            }
        }
        index
    }

    fn redact_text(
        &self,
        text: &str,
        index: &BTreeMap<usize, Vec<[u8; 32]>>,
        spent: &mut u64,
    ) -> Result<Option<String>, OtlpError> {
        let bytes = text.as_bytes();
        if bytes.len() < MIN_SECRET_BYTES {
            return Ok(None);
        }
        for (len, digests) in index {
            if bytes.len() < *len {
                continue;
            }
            let windows = (bytes.len() - *len + 1) as u64;
            *spent = spent.saturating_add(windows.saturating_mul(*len as u64));
            if *spent > self.budget_bytes {
                return Err(OtlpError::RedactionBudgetExhausted {
                    limit: self.budget_bytes,
                });
            }
            for start in 0..=bytes.len() - *len {
                let window = &bytes[start..start + *len];
                let candidate = self.digest(window);
                if digests.contains(&candidate) {
                    // One hit redacts the whole value: a value that contains a
                    // secret is not safe to publish with the secret cut out,
                    // because the surrounding text can reveal its position and
                    // length.
                    return Ok(Some(REDACTED.to_owned()));
                }
            }
        }
        Ok(None)
    }

    fn walk(
        &self,
        value: &mut CanonicalValue,
        index: &BTreeMap<usize, Vec<[u8; 32]>>,
        spent: &mut u64,
        report: &mut RedactionReport,
    ) -> Result<(), OtlpError> {
        match value {
            CanonicalValue::Str(text) => {
                report.scanned_bytes = report.scanned_bytes.saturating_add(text.len() as u64);
                if let Some(replacement) = self.redact_text(text, index, spent)? {
                    *text = replacement.into_boxed_str();
                    report.redacted_values += 1;
                }
            }
            CanonicalValue::Array(values) => {
                for element in values.iter_mut() {
                    self.walk(element, index, spent, report)?;
                }
            }
            CanonicalValue::Map(entries) => {
                for element in entries.values_mut() {
                    self.walk(element, index, spent, report)?;
                }
            }
            CanonicalValue::Null
            | CanonicalValue::Bool(_)
            | CanonicalValue::Int(_)
            | CanonicalValue::Num(_) => {}
        }
        Ok(())
    }
}

impl ManagedSecretRedactor for DigestRedactor {
    fn redact(&self, value: &mut CanonicalValue) -> Result<RedactionReport, OtlpError> {
        let index = self.index();
        let mut spent = 0u64;
        let mut report = RedactionReport::default();
        if index.is_empty() {
            return Ok(report);
        }
        self.walk(value, &index, &mut spent, &mut report)?;
        Ok(report)
    }
}

/// The redactor a scope with no managed secrets uses.
///
/// A real trait implementation rather than an `Option`, so no call site has to
/// branch and no path can silently skip redaction.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoManagedSecrets;

impl ManagedSecretRedactor for NoManagedSecrets {
    fn redact(&self, _value: &mut CanonicalValue) -> Result<RedactionReport, OtlpError> {
        Ok(RedactionReport::default())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DigestRedactor, ManagedSecretRedactor, NoManagedSecrets, REDACTED, SecretDigest,
        SecretDigestManifest,
    };
    use aex_observation_domain::canonical::CanonicalValue;
    use aex_wire::ids::SessionId;
    use hmac::{Hmac, Mac as _};
    use sha2::Sha256;

    const KEY: &[u8] = b"regional-redaction-key";

    fn session() -> SessionId {
        aex_wire::ids::PrefixedId::parse("ses_0000000003ec1r60r30c1g60r3").expect("fixture parses")
    }

    fn digest(secret: &str) -> SecretDigest {
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(KEY).expect("key");
        mac.update(secret.as_bytes());
        SecretDigest {
            len: u16::try_from(secret.len()).expect("fixture fits"),
            hmac: mac.finalize().into_bytes().into(),
        }
    }

    fn redactor(secrets: &[&str], budget: u64) -> DigestRedactor {
        DigestRedactor::new(
            KEY.to_vec(),
            SecretDigestManifest {
                session: session(),
                custody_revision: 1,
                entries: secrets.iter().copied().map(digest).collect(),
            },
            budget,
        )
    }

    #[test]
    fn a_secret_embedded_in_a_longer_string_is_redacted() {
        let redactor = redactor(&["sk-live-abcdef"], u64::MAX);
        let mut value = CanonicalValue::Str("token is sk-live-abcdef ok".into());
        let report = redactor.redact(&mut value).expect("redacts");
        assert_eq!(report.redacted_values, 1);
        assert_eq!(value.as_str(), Some(REDACTED));
    }

    #[test]
    fn a_secret_shorter_than_the_floor_is_never_matched() {
        // Seven bytes is below the floor, so it is not even indexed; eight is.
        let short = redactor(&["1234567"], u64::MAX);
        let mut value = CanonicalValue::Str("says 1234567 here".into());
        assert_eq!(short.redact(&mut value).expect("scans").redacted_values, 0);

        let exact = redactor(&["12345678"], u64::MAX);
        let mut value = CanonicalValue::Str("says 12345678 here".into());
        assert_eq!(exact.redact(&mut value).expect("scans").redacted_values, 1);
    }

    #[test]
    fn a_secret_split_across_two_attributes_is_not_reassembled() {
        // Neither half is the secret, so neither is redacted. Claiming otherwise
        // would require reassembling arbitrary customer text, which the product
        // does not promise.
        let redactor = redactor(&["abcdefghij"], u64::MAX);
        let mut value = CanonicalValue::Array(vec![
            CanonicalValue::Str("abcde".into()),
            CanonicalValue::Str("fghij".into()),
        ]);
        assert_eq!(
            redactor.redact(&mut value).expect("scans").redacted_values,
            0
        );
    }

    #[test]
    fn an_exhausted_budget_fails_closed_rather_than_admitting() {
        let redactor = redactor(&["abcdefghij"], 4);
        let mut value = CanonicalValue::Str("x".repeat(4096).into());
        let error = redactor.redact(&mut value).expect_err("fails closed");
        assert!(matches!(
            error,
            crate::error::OtlpError::RedactionBudgetExhausted { limit: 4 }
        ));
        assert_ne!(value.as_str(), Some(REDACTED), "nothing was admitted");
    }

    #[test]
    fn nested_maps_and_arrays_are_walked() {
        let redactor = redactor(&["sk-live-abcdef"], u64::MAX);
        let mut inner = std::collections::BTreeMap::new();
        inner.insert(
            "deep".to_owned(),
            CanonicalValue::Array(vec![CanonicalValue::Str("sk-live-abcdef".into())]),
        );
        let mut value = CanonicalValue::Map(inner);
        assert_eq!(
            redactor
                .redact(&mut value)
                .expect("redacts")
                .redacted_values,
            1
        );
    }

    #[test]
    fn a_scope_with_no_managed_secrets_uses_a_real_implementation() {
        let mut value = CanonicalValue::Str("anything".into());
        let report = NoManagedSecrets.redact(&mut value).expect("no-op");
        assert_eq!(report.redacted_values, 0);
        assert_eq!(value.as_str(), Some("anything"));
    }

    #[test]
    fn a_multi_byte_boundary_never_produces_invalid_utf8() {
        let redactor = redactor(&["\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}"], u64::MAX);
        let mut value =
            CanonicalValue::Str("prefix \u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}".into());
        let report = redactor.redact(&mut value).expect("redacts");
        assert_eq!(report.redacted_values, 1);
        assert_eq!(value.as_str(), Some(REDACTED));
    }

    #[test]
    fn the_debug_rendering_never_carries_the_key() {
        let redactor = redactor(&["sk-live-abcdef"], 64);
        let rendered = format!("{redactor:?}");
        assert!(!rendered.contains("regional-redaction-key"));
        assert!(rendered.contains("custody_revision"));
    }
}
