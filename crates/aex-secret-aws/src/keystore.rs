//! Branch-key material: where it comes from, and the bounded zeroizing cache in
//! front of it.
//!
//! The cache is why a KMS blip does not immediately become a secret outage, and
//! why a workspace's key-store row is read a handful of times an hour rather
//! than once per operation. It is bounded, it expires, and it zeroizes on
//! eviction and on drop — an unbounded cache of decrypted key material is a
//! memory-resident copy of every secret the process has ever touched.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use aex_wire::types::Timestamp;
use async_trait::async_trait;
use aws_sdk_kms::Client as KmsClient;
use aws_sdk_kms::error::SdkError;
use aws_sdk_kms::primitives::Blob;
use aws_smithy_types::error::metadata::ProvideErrorMetadata;
use zeroize::Zeroizing;

use crate::envelope::{BranchKeyMaterial, KEY_VERSION_BYTES};

/// How long decrypted branch material may be held.
#[allow(
    clippy::duration_suboptimal_units,
    reason = "`Duration::from_mins` is not const-stable on the pinned toolchain"
)]
pub const CACHE_TTL: Duration = Duration::from_secs(600);

/// How many workspaces' material one process may hold at once.
pub const CACHE_CAPACITY: usize = 256;

/// Why branch material could not be obtained.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyMaterialError {
    /// The key is disabled, pending deletion or absent.
    ///
    /// Every secret operation fails **closed** on this; unrelated reads stay
    /// available.
    #[error("the key is unavailable")]
    KeyUnavailable,
    /// The caller is denied the required KMS operation. Expected for an
    /// ordinary regional edge role that has neither decrypt nor rewrap custody.
    #[error("the caller is denied the required KMS branch-key operation")]
    Denied,
    /// The ciphertext or its context did not match the key.
    #[error("the wrapped key did not decrypt under this context")]
    ContextMismatch,
    /// The configured root key is not the key that wrapped this branch key.
    #[error("the wrapped branch key does not belong to the configured KMS root key")]
    RootKeyMismatch,
    /// The request was invalid, which is a composition or adapter defect.
    #[error("the KMS branch-key request is invalid")]
    InvalidRequest,
    /// KMS asked the caller to reduce its request rate.
    #[error("KMS throttled the branch-key request")]
    Throttled,
    /// KMS was unavailable.
    #[error("KMS is unavailable: {detail}")]
    Unavailable {
        /// What the service reported.
        detail: String,
    },
    /// The service returned no plaintext at all.
    #[error("KMS returned no key material")]
    Empty,
}

impl KeyMaterialError {
    /// Whether the operation can be retried without resolving an external
    /// commit first.
    ///
    /// `Decrypt` is a read and `ReEncrypt` creates no durable provider-side
    /// object: if no ciphertext reached the caller, retrying cannot duplicate
    /// state. The caller still owns the bounded retry policy and deadline.
    #[must_use]
    pub const fn retryable(&self) -> bool {
        matches!(self, Self::Throttled | Self::Unavailable { .. })
    }
}

/// Ciphertext for the same branch key, newly authenticated under a destination
/// encryption context.
///
/// The bytes are not plaintext key material, but they are still custody
/// material: `Debug` never renders them, and the type prevents a caller from
/// confusing a successful `ReEncrypt` response with the source ciphertext.
#[derive(Clone, PartialEq, Eq)]
pub struct RewrappedBranchKey(Vec<u8>);

impl RewrappedBranchKey {
    /// Wraps bytes emitted by a provider implementation.
    ///
    /// This constructor exists for faithful non-AWS implementations in tests;
    /// production obtains the type only from KMS `ReEncrypt`.
    ///
    /// # Errors
    ///
    /// [`KeyMaterialError::Empty`] when the provider emitted no ciphertext.
    pub fn from_provider(bytes: Vec<u8>) -> Result<Self, KeyMaterialError> {
        if bytes.is_empty() {
            return Err(KeyMaterialError::Empty);
        }
        Ok(Self(bytes))
    }

    /// Borrows the destination-bound ciphertext.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the ciphertext for durable custody storage.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl std::fmt::Debug for RewrappedBranchKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RewrappedBranchKey(<redacted>)")
    }
}

/// Where branch-key material comes from.
///
/// A port rather than a concrete client, so the seal and open paths can be
/// asserted without a network and the KMS request shape can be asserted without
/// a key.
#[async_trait]
pub trait BranchKeyProvider: Send + Sync {
    /// Unwraps the material for one branch key version.
    ///
    /// # Errors
    ///
    /// [`KeyMaterialError`] for every KMS condition, each mapped to a typed
    /// outcome rather than to a string.
    async fn material(
        &self,
        branch_key_id: &str,
        version: [u8; KEY_VERSION_BYTES],
        wrapped: &[u8],
        context: &BTreeMap<String, String>,
    ) -> Result<BranchKeyMaterial, KeyMaterialError>;

    /// Rewraps one KMS ciphertext under an exact destination context without
    /// returning plaintext branch-key material.
    ///
    /// Implementations must authenticate the complete, case-sensitive source
    /// context and bind the complete destination context. A subset match is a
    /// contract violation. Dropping this future cancels its in-flight work;
    /// implementations must not detach it. The caller-provided provider client
    /// owns attempt/operation deadlines and bounded retries.
    ///
    /// # Errors
    ///
    /// [`KeyMaterialError`] for every provider condition. An unavailable or
    /// throttled result is safe to retry because no durable provider-side
    /// mutation needs resolution.
    async fn rewrap(
        &self,
        wrapped: &[u8],
        source_context: &BTreeMap<String, String>,
        destination_context: &BTreeMap<String, String>,
    ) -> Result<RewrappedBranchKey, KeyMaterialError>;
}

/// The KMS-backed provider.
#[derive(Debug, Clone)]
pub struct KmsBranchKeys {
    client: KmsClient,
    key_arn: String,
}

impl KmsBranchKeys {
    /// Binds a provider to a client and the exact root-key ARN branch keys are
    /// wrapped under. A key id or alias is not accepted semantically: KMS
    /// responses name canonical ARNs, and every response is compared byte for
    /// byte with this value.
    #[must_use]
    pub fn new(client: KmsClient, key_arn: impl Into<String>) -> Self {
        Self {
            client,
            key_arn: key_arn.into(),
        }
    }

    /// The root key ARN.
    #[must_use]
    pub fn key_arn(&self) -> &str {
        &self.key_arn
    }
}

#[async_trait]
impl BranchKeyProvider for KmsBranchKeys {
    async fn material(
        &self,
        branch_key_id: &str,
        version: [u8; KEY_VERSION_BYTES],
        wrapped: &[u8],
        context: &BTreeMap<String, String>,
    ) -> Result<BranchKeyMaterial, KeyMaterialError> {
        let mut request = self
            .client
            .decrypt()
            .key_id(&self.key_arn)
            .ciphertext_blob(Blob::new(wrapped.to_vec()));
        for (key, value) in context {
            request = request.encryption_context(key, value);
        }
        let response = request.send().await.map_err(|error| classify(&error))?;
        let plaintext = response.plaintext.ok_or(KeyMaterialError::Empty)?;
        Ok(BranchKeyMaterial {
            branch_key_id: branch_key_id.to_owned(),
            version,
            material: Zeroizing::new(plaintext.into_inner()),
        })
    }

    async fn rewrap(
        &self,
        wrapped: &[u8],
        source_context: &BTreeMap<String, String>,
        destination_context: &BTreeMap<String, String>,
    ) -> Result<RewrappedBranchKey, KeyMaterialError> {
        let mut request = self
            .client
            .re_encrypt()
            .ciphertext_blob(Blob::new(wrapped.to_vec()))
            .source_key_id(&self.key_arn)
            .destination_key_id(&self.key_arn);
        for (key, value) in source_context {
            request = request.source_encryption_context(key, value);
        }
        for (key, value) in destination_context {
            request = request.destination_encryption_context(key, value);
        }
        let response = request.send().await.map_err(|error| classify(&error))?;
        validate_root_key_response(&self.key_arn, response.source_key_id(), response.key_id())?;
        let ciphertext = response
            .ciphertext_blob
            .ok_or(KeyMaterialError::Empty)?
            .into_inner();
        RewrappedBranchKey::from_provider(ciphertext)
    }
}

fn validate_root_key_response(
    configured_arn: &str,
    source_key_id: Option<&str>,
    destination_key_id: Option<&str>,
) -> Result<(), KeyMaterialError> {
    if source_key_id == Some(configured_arn) && destination_key_id == Some(configured_arn) {
        Ok(())
    } else {
        Err(KeyMaterialError::RootKeyMismatch)
    }
}

fn classify<E, R>(error: &SdkError<E, R>) -> KeyMaterialError
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ConstructionFailure(_) => KeyMaterialError::InvalidRequest,
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            KeyMaterialError::Unavailable {
                detail: "the request did not complete".to_owned(),
            }
        }
        SdkError::ServiceError(service) => classify_code(service.err().code().unwrap_or("Unknown")),
        _ => KeyMaterialError::Unavailable {
            detail: "an unrecognised SDK failure".to_owned(),
        },
    }
}

fn classify_code(code: &str) -> KeyMaterialError {
    match code {
        "KMSInvalidStateException" | "DisabledException" | "NotFoundException" => {
            KeyMaterialError::KeyUnavailable
        }
        "AccessDeniedException" => KeyMaterialError::Denied,
        "InvalidCiphertextException" => KeyMaterialError::ContextMismatch,
        "IncorrectKeyException" => KeyMaterialError::RootKeyMismatch,
        "InvalidGrantTokenException" | "InvalidKeyUsageException" | "DryRunOperationException" => {
            KeyMaterialError::InvalidRequest
        }
        "ThrottlingException" => KeyMaterialError::Throttled,
        "DependencyTimeoutException" | "KeyUnavailableException" | "KMSInternalException" => {
            KeyMaterialError::Unavailable {
                detail: format!("the service reported `{code}`"),
            }
        }
        other => KeyMaterialError::Unavailable {
            detail: format!("the service reported `{other}`"),
        },
    }
}

#[derive(Clone)]
struct Entry {
    material: BranchKeyMaterial,
    expires_at_millis: i64,
}

/// A bounded, expiring, zeroizing cache in front of a [`BranchKeyProvider`].
///
/// The partition keeps two roles sharing one process image from sharing an
/// entry, so a role that may decrypt cannot warm a cache another role then
/// reads.
pub struct BranchKeyCache {
    partition: String,
    entries: Mutex<HashMap<String, Entry>>,
    capacity: usize,
    ttl: Duration,
}

impl std::fmt::Debug for BranchKeyCache {
    /// Prints the bounds, never an entry: the entries are decrypted key
    /// material, and a `Debug` that included them would put every secret the
    /// process has touched into whatever printed it.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BranchKeyCache")
            .field("partition", &self.partition)
            .field("capacity", &self.capacity)
            .field("ttl", &self.ttl)
            .field("entries", &"<redacted>")
            .finish()
    }
}

impl BranchKeyCache {
    /// A cache bound to one process partition.
    ///
    /// The composition derives the partition from
    /// `sha256(plane ‖ region ‖ role ‖ process id)`.
    #[must_use]
    pub fn new(partition: impl Into<String>) -> Self {
        Self {
            partition: partition.into(),
            entries: Mutex::new(HashMap::new()),
            capacity: CACHE_CAPACITY,
            ttl: CACHE_TTL,
        }
    }

    /// A cache with a smaller bound, for a test that asserts eviction.
    #[must_use]
    pub fn with_capacity(partition: impl Into<String>, capacity: usize, ttl: Duration) -> Self {
        Self {
            partition: partition.into(),
            entries: Mutex::new(HashMap::new()),
            capacity,
            ttl,
        }
    }

    /// How many entries are held.
    ///
    /// # Panics
    ///
    /// If the lock was poisoned by a panic inside a previous call.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().expect("the cache lock").len()
    }

    /// Whether the cache holds nothing.
    ///
    /// # Panics
    ///
    /// As [`BranchKeyCache::len`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops every entry, zeroizing as it goes.
    ///
    /// # Panics
    ///
    /// As [`BranchKeyCache::len`].
    pub fn clear(&self) {
        self.entries.lock().expect("the cache lock").clear();
    }

    fn slot(
        &self,
        branch_key_id: &str,
        version: [u8; KEY_VERSION_BYTES],
        context_digest: [u8; 32],
    ) -> String {
        format!(
            "{}|{branch_key_id}|{}|{}",
            self.partition,
            hex::encode(version),
            hex::encode(context_digest)
        )
    }

    /// Reads a live entry.
    ///
    /// # Panics
    ///
    /// As [`BranchKeyCache::len`].
    #[must_use]
    pub fn get(
        &self,
        branch_key_id: &str,
        version: [u8; KEY_VERSION_BYTES],
        context_digest: [u8; 32],
        now: Timestamp,
    ) -> Option<BranchKeyMaterial> {
        let slot = self.slot(branch_key_id, version, context_digest);
        let mut entries = self.entries.lock().expect("the cache lock");
        match entries.get(&slot) {
            Some(entry) if entry.expires_at_millis > now.unix_millis() => {
                Some(entry.material.clone())
            }
            Some(_) => {
                // Expiry drops the entry, which zeroizes the material rather
                // than leaving it readable until the slot is reused.
                entries.remove(&slot);
                None
            }
            None => None,
        }
    }

    /// Stores an entry, evicting when the bound is reached.
    ///
    /// # Panics
    ///
    /// As [`BranchKeyCache::len`].
    pub fn put(&self, material: BranchKeyMaterial, context_digest: [u8; 32], now: Timestamp) {
        let slot = self.slot(&material.branch_key_id, material.version, context_digest);
        let mut entries = self.entries.lock().expect("the cache lock");
        if entries.len() >= self.capacity && !entries.contains_key(&slot) {
            // The bound is what matters, not which entry goes: the thing being
            // prevented is an unbounded cache of decrypted key material.
            if let Some(victim) = entries.keys().next().cloned() {
                entries.remove(&victim);
            }
        }
        let ttl_millis = i64::try_from(self.ttl.as_millis()).unwrap_or(i64::MAX);
        entries.insert(
            slot,
            Entry {
                material,
                expires_at_millis: now.unix_millis().saturating_add(ttl_millis),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aex_wire::types::Timestamp;
    use zeroize::Zeroizing;

    use super::{
        BranchKeyCache, CACHE_CAPACITY, CACHE_TTL, KeyMaterialError, RewrappedBranchKey,
        classify_code, validate_root_key_response,
    };
    use crate::envelope::{BranchKeyMaterial, KEY_VERSION_BYTES};

    fn material(id: &str) -> BranchKeyMaterial {
        BranchKeyMaterial {
            branch_key_id: id.to_owned(),
            version: [1; KEY_VERSION_BYTES],
            material: Zeroizing::new(vec![0x2a; 32]),
        }
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    #[test]
    fn material_is_returned_until_it_expires_and_never_after() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), [1; 32], at(0));
        assert!(
            cache
                .get("wsp_1", [1; KEY_VERSION_BYTES], [1; 32], at(1_000))
                .is_some()
        );
        let ttl = i64::try_from(CACHE_TTL.as_millis()).expect("600 seconds");
        assert!(
            cache
                .get("wsp_1", [1; KEY_VERSION_BYTES], [1; 32], at(ttl + 1),)
                .is_none(),
            "expired material must not be served"
        );
        assert!(cache.is_empty(), "an expired entry is dropped, not kept");
    }

    #[test]
    fn two_roles_in_one_process_never_share_an_entry() {
        let first = BranchKeyCache::new("role-a");
        let second = BranchKeyCache::new("role-b");
        first.put(material("wsp_1"), [1; 32], at(0));
        assert!(
            second
                .get("wsp_1", [1; KEY_VERSION_BYTES], [1; 32], at(0))
                .is_none(),
            "a role that may decrypt must not warm a cache another role reads"
        );
    }

    #[test]
    fn a_new_key_version_is_a_different_entry() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), [1; 32], at(0));
        assert!(
            cache
                .get("wsp_1", [2; KEY_VERSION_BYTES], [1; 32], at(0))
                .is_none(),
            "rotation must not be served stale material"
        );
    }

    #[test]
    fn the_same_wrapped_key_under_another_context_is_a_cache_miss() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), [1; 32], at(0));
        assert!(
            cache
                .get("wsp_1", [1; KEY_VERSION_BYTES], [2; 32], at(0))
                .is_none(),
            "a cached KMS decrypt must not bypass exact encryption-context equality"
        );
    }

    #[test]
    fn the_cache_is_bounded_however_many_workspaces_pass_through_it() {
        let cache = BranchKeyCache::with_capacity("role-a", 4, Duration::from_mins(10));
        for index in 0..64 {
            cache.put(material(&format!("wsp_{index}")), [1; 32], at(0));
        }
        assert!(cache.len() <= 4, "{} entries", cache.len());
        assert_eq!(CACHE_CAPACITY, 256);
    }

    #[test]
    fn clearing_drops_every_entry() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), [1; 32], at(0));
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn the_cache_never_prints_the_material_it_holds() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), [1; 32], at(0));
        let printed = format!("{cache:?}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }

    #[test]
    fn rewrapped_ciphertext_is_redacted_even_though_it_is_not_plaintext() {
        let wrapped = RewrappedBranchKey::from_provider(b"provider-ciphertext".to_vec())
            .expect("non-empty provider ciphertext");
        assert_eq!(format!("{wrapped:?}"), "RewrappedBranchKey(<redacted>)");
        assert_eq!(wrapped.as_bytes(), b"provider-ciphertext");
    }

    #[test]
    fn empty_rewrapped_ciphertext_is_not_a_valid_provider_result() {
        assert_eq!(
            RewrappedBranchKey::from_provider(Vec::new()).unwrap_err(),
            KeyMaterialError::Empty
        );
    }

    #[test]
    fn reencrypt_must_attest_the_configured_arn_on_both_sides() {
        let root = "arn:aws:kms:eu-west-1:000000000000:key/secret";
        assert_eq!(
            validate_root_key_response(root, Some(root), Some(root)),
            Ok(())
        );
        for (source, destination) in [
            (None, Some(root)),
            (Some(root), None),
            (
                Some("arn:aws:kms:eu-west-1:000000000000:key/other"),
                Some(root),
            ),
            (
                Some(root),
                Some("arn:aws:kms:eu-west-1:000000000000:key/other"),
            ),
        ] {
            assert_eq!(
                validate_root_key_response(root, source, destination),
                Err(KeyMaterialError::RootKeyMismatch)
            );
        }
    }

    #[test]
    fn provider_errors_preserve_retry_and_integrity_semantics() {
        assert_eq!(
            classify_code("InvalidCiphertextException"),
            KeyMaterialError::ContextMismatch
        );
        assert_eq!(
            classify_code("IncorrectKeyException"),
            KeyMaterialError::RootKeyMismatch
        );
        assert_eq!(
            classify_code("AccessDeniedException"),
            KeyMaterialError::Denied
        );
        assert!(classify_code("ThrottlingException").retryable());
        assert!(classify_code("DependencyTimeoutException").retryable());
        assert!(!classify_code("DisabledException").retryable());
    }
}
