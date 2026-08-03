//! Shared fixtures for the `aex-secret-aws` test targets.

#![allow(dead_code, reason = "each test target uses a different subset")]
#![allow(missing_docs, reason = "the module doc states what these fixtures are")]

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use aex_secret_aws::envelope::{BranchKeyMaterial, Entropy, KEY_VERSION_BYTES};
use aex_secret_aws::keystore::{BranchKeyProvider, KeyMaterialError, RewrappedBranchKey};
use aex_secret_domain::context::{EncryptionContext, Plane};
use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::secret::{SecretName, SourceGeneration};
use aex_wire::ids::{OrganizationId, PrefixedId, Uuid7, WorkspaceId};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use zeroize::Zeroizing;

type ContextPairs = BTreeMap<String, String>;
type RewrapCall = (ContextPairs, ContextPairs);

#[must_use]
pub fn now() -> Timestamp {
    Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
}

#[must_use]
pub fn workspace(byte: u8) -> WorkspaceId {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
}

#[must_use]
pub fn context(name: &str, workspace_byte: u8) -> EncryptionContext {
    EncryptionContext {
        plane: Plane::Prd,
        region: Region::ALL[0],
        organization: OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10])),
        workspace: workspace(workspace_byte),
        name: SecretName::parse(name).expect("an ASCII name"),
        generation: SourceGeneration(3),
        custody_revision: Some(CustodyRevision(2)),
    }
}

#[must_use]
pub fn session_context(name: &str, revision: u64) -> EncryptionContext {
    EncryptionContext {
        custody_revision: Some(CustodyRevision(revision)),
        ..context(name, 1)
    }
}

/// Entropy a test can pin, so a frame is byte-stable.
pub struct Pinned(pub u8);

impl Entropy for Pinned {
    fn fill(&self, bytes: &mut [u8]) {
        bytes.fill(self.0);
    }
}

struct FakeInner {
    material: Vec<u8>,
    calls: AtomicUsize,
    rewrap_calls: AtomicUsize,
    seen: Mutex<Vec<ContextPairs>>,
    rewrap_seen: Mutex<Vec<RewrapCall>>,
    bindings: Mutex<HashMap<Vec<u8>, ContextPairs>>,
    denied: bool,
}

/// A provider that hands out fixed material and records what it was asked.
///
/// Cloneable, so a test can hold one handle while the adapter holds another —
/// which is how the recorded calls are read back without any unsafe pointer
/// games, and `unsafe_code` is forbidden here anyway.
#[derive(Clone)]
pub struct FakeKeys {
    inner: std::sync::Arc<FakeInner>,
}

impl FakeKeys {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(FakeInner {
                material: vec![0x2a; 32],
                calls: AtomicUsize::new(0),
                rewrap_calls: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
                rewrap_seen: Mutex::new(Vec::new()),
                bindings: Mutex::new(HashMap::new()),
                denied: false,
            }),
        }
    }

    #[must_use]
    pub fn denied() -> Self {
        Self {
            inner: std::sync::Arc::new(FakeInner {
                material: vec![0x2a; 32],
                calls: AtomicUsize::new(0),
                rewrap_calls: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
                rewrap_seen: Mutex::new(Vec::new()),
                bindings: Mutex::new(HashMap::new()),
                denied: true,
            }),
        }
    }

    #[must_use]
    pub fn calls(&self) -> usize {
        self.inner.calls.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn rewrap_calls(&self) -> usize {
        self.inner.rewrap_calls.load(Ordering::Relaxed)
    }

    /// Every encryption context the provider was asked with.
    ///
    /// # Panics
    ///
    /// If the lock was poisoned.
    #[must_use]
    pub fn contexts(&self) -> Vec<ContextPairs> {
        self.inner.seen.lock().expect("the lock").clone()
    }

    #[must_use]
    pub fn rewrap_contexts(&self) -> Vec<RewrapCall> {
        self.inner.rewrap_seen.lock().expect("the lock").clone()
    }
}

impl Default for FakeKeys {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl BranchKeyProvider for FakeKeys {
    async fn material(
        &self,
        branch_key_id: &str,
        version: [u8; KEY_VERSION_BYTES],
        wrapped: &[u8],
        context: &BTreeMap<String, String>,
    ) -> Result<BranchKeyMaterial, KeyMaterialError> {
        self.inner.calls.fetch_add(1, Ordering::Relaxed);
        self.inner
            .seen
            .lock()
            .expect("the lock")
            .push(context.clone());
        if self.inner.denied {
            return Err(KeyMaterialError::Denied);
        }
        let mut bindings = self.inner.bindings.lock().expect("the lock");
        match bindings.get(wrapped) {
            Some(bound) if bound != context => return Err(KeyMaterialError::ContextMismatch),
            Some(_) => {}
            None => {
                bindings.insert(wrapped.to_vec(), context.clone());
            }
        }
        Ok(BranchKeyMaterial {
            branch_key_id: branch_key_id.to_owned(),
            version,
            material: Zeroizing::new(self.inner.material.clone()),
        })
    }

    async fn rewrap(
        &self,
        wrapped: &[u8],
        source_context: &BTreeMap<String, String>,
        destination_context: &BTreeMap<String, String>,
    ) -> Result<RewrappedBranchKey, KeyMaterialError> {
        let call = self.inner.rewrap_calls.fetch_add(1, Ordering::Relaxed);
        self.inner
            .rewrap_seen
            .lock()
            .expect("the lock")
            .push((source_context.clone(), destination_context.clone()));
        if self.inner.denied {
            return Err(KeyMaterialError::Denied);
        }
        let mut bindings = self.inner.bindings.lock().expect("the lock");
        if bindings.get(wrapped) != Some(source_context) {
            return Err(KeyMaterialError::ContextMismatch);
        }
        let mut destination = b"aex-test-rewrapped-v1".to_vec();
        destination.extend_from_slice(&call.to_le_bytes());
        bindings.insert(destination.clone(), destination_context.clone());
        RewrappedBranchKey::from_provider(destination)
    }
}

/// The wrapped branch key a test presents. Its bytes decide the key version.
pub const WRAPPED: &[u8] = b"a wrapped branch key";

#[must_use]
pub fn plaintext(value: &str) -> aex_secret_domain::plaintext::SecretPlaintext {
    aex_secret_domain::plaintext::SecretPlaintext::new(value.as_bytes().to_vec())
        .expect("a non-empty value")
}
