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
use aws_sdk_kms::primitives::Blob;
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
    /// The caller is denied. Expected for an ordinary regional edge role.
    #[error("the caller is denied `kms:Decrypt`")]
    Denied,
    /// The ciphertext or its context did not match the key.
    #[error("the wrapped key did not decrypt under this context")]
    ContextMismatch,
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
}

/// The KMS-backed provider.
#[derive(Debug, Clone)]
pub struct KmsBranchKeys {
    client: KmsClient,
    key_arn: String,
}

impl KmsBranchKeys {
    /// Binds a provider to a client and the root key branch keys are wrapped
    /// under.
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
        let response = request.send().await.map_err(|error| {
            use aws_smithy_types::error::metadata::ProvideErrorMetadata as _;
            match error.code().unwrap_or("Unknown") {
                "KMSInvalidStateException"
                | "DisabledException"
                | "KeyUnavailableException"
                | "NotFoundException" => KeyMaterialError::KeyUnavailable,
                "AccessDeniedException" => KeyMaterialError::Denied,
                "IncorrectKeyException" | "InvalidCiphertextException" => {
                    KeyMaterialError::ContextMismatch
                }
                other => KeyMaterialError::Unavailable {
                    detail: other.to_owned(),
                },
            }
        })?;
        let plaintext = response.plaintext.ok_or(KeyMaterialError::Empty)?;
        Ok(BranchKeyMaterial {
            branch_key_id: branch_key_id.to_owned(),
            version,
            material: Zeroizing::new(plaintext.into_inner()),
        })
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

    fn slot(&self, branch_key_id: &str, version: [u8; KEY_VERSION_BYTES]) -> String {
        format!(
            "{}|{branch_key_id}|{}",
            self.partition,
            hex::encode(version)
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
        now: Timestamp,
    ) -> Option<BranchKeyMaterial> {
        let slot = self.slot(branch_key_id, version);
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
    pub fn put(&self, material: BranchKeyMaterial, now: Timestamp) {
        let slot = self.slot(&material.branch_key_id, material.version);
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

    use super::{BranchKeyCache, CACHE_CAPACITY, CACHE_TTL};
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
        cache.put(material("wsp_1"), at(0));
        assert!(
            cache
                .get("wsp_1", [1; KEY_VERSION_BYTES], at(1_000))
                .is_some()
        );
        let ttl = i64::try_from(CACHE_TTL.as_millis()).expect("600 seconds");
        assert!(
            cache
                .get("wsp_1", [1; KEY_VERSION_BYTES], at(ttl + 1))
                .is_none(),
            "expired material must not be served"
        );
        assert!(cache.is_empty(), "an expired entry is dropped, not kept");
    }

    #[test]
    fn two_roles_in_one_process_never_share_an_entry() {
        let first = BranchKeyCache::new("role-a");
        let second = BranchKeyCache::new("role-b");
        first.put(material("wsp_1"), at(0));
        assert!(
            second.get("wsp_1", [1; KEY_VERSION_BYTES], at(0)).is_none(),
            "a role that may decrypt must not warm a cache another role reads"
        );
    }

    #[test]
    fn a_new_key_version_is_a_different_entry() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), at(0));
        assert!(
            cache.get("wsp_1", [2; KEY_VERSION_BYTES], at(0)).is_none(),
            "rotation must not be served stale material"
        );
    }

    #[test]
    fn the_cache_is_bounded_however_many_workspaces_pass_through_it() {
        let cache = BranchKeyCache::with_capacity("role-a", 4, Duration::from_mins(10));
        for index in 0..64 {
            cache.put(material(&format!("wsp_{index}")), at(0));
        }
        assert!(cache.len() <= 4, "{} entries", cache.len());
        assert_eq!(CACHE_CAPACITY, 256);
    }

    #[test]
    fn clearing_drops_every_entry() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), at(0));
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn the_cache_never_prints_the_material_it_holds() {
        let cache = BranchKeyCache::new("role-a");
        cache.put(material("wsp_1"), at(0));
        let printed = format!("{cache:?}");
        assert!(printed.contains("<redacted>"), "{printed}");
    }
}
