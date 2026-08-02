//! Provider-independent administration orchestration.
//!
//! The operation identity is also the generation identity. A retry therefore
//! targets the same version row, while a distinct operation can never be
//! mistaken for the retry of an earlier rotation.

use aex_secret_keystore_dynamodb::branch_key::{BranchKeyId, VERSION_PREFIX};
use aex_wire::ids::{OperationId, WorkspaceId};
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};

use crate::AdminOutcome;

/// One active generation read from the authoritative key store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveGeneration {
    /// Version-row sort key the active record names.
    pub version: String,
    /// Monotonic hierarchy generation.
    pub hierarchy_version: u64,
    /// Root KMS key that wraps this generation.
    pub kms_arn: String,
}

/// The immutable input to one generation write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationSpec {
    /// Workspace-derived branch-key identity.
    pub branch_key_id: BranchKeyId,
    /// Stable version-row sort key derived from the attested operation.
    pub version: String,
    /// Monotonic hierarchy generation.
    pub hierarchy_version: u64,
    /// Attested durable operation.
    pub attestation: OperationId,
    /// SHA-256 of the operator-supplied rotation reason; absent for creation.
    ///
    /// The raw reason is intentionally not sent to KMS because encryption
    /// context is visible in `CloudTrail`.
    pub reason_digest: Option<String>,
}

/// Conditional provider-write outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// The exact generation was committed.
    Applied,
    /// An authoritative conditional fence moved first.
    Conflict,
}

/// The narrow provider boundary the one-shot composition supplies.
#[async_trait]
pub trait KeyAdminPort: Send + Sync {
    /// Strongly reads the active generation.
    async fn active(
        &self,
        branch_key_id: &BranchKeyId,
    ) -> Result<Option<ActiveGeneration>, AdminRunError>;

    /// Creates generation one and its active pointer atomically.
    async fn create(&self, generation: &GenerationSpec) -> Result<ApplyOutcome, AdminRunError>;

    /// Writes the next immutable generation and moves the active pointer under
    /// the exact previously observed version/generation fence.
    async fn rotate(
        &self,
        expected: &ActiveGeneration,
        generation: &GenerationSpec,
    ) -> Result<ApplyOutcome, AdminRunError>;

    /// Proves the active wrapped material can still be opened by its declared
    /// root key and exact stored encryption context.
    async fn verify(
        &self,
        branch_key_id: &BranchKeyId,
        expected: &ActiveGeneration,
    ) -> Result<(), AdminRunError>;
}

/// One requested one-shot action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCommand<'a> {
    /// Create generation one.
    Create,
    /// Rotate exactly one generation.
    Rotate {
        /// Operator-supplied reason, persisted only as a digest.
        reason: &'a str,
    },
    /// Verify without mutating the lineage.
    Verify,
}

/// Executes one idempotent, fenced key administration command.
///
/// # Errors
///
/// Returns a typed lineage, provider or verification refusal. A conditional
/// conflict is resolved with a new strong read before it is classified, so an
/// acknowledged write whose response was lost becomes `AlreadyCurrent`.
pub async fn execute<P: KeyAdminPort + ?Sized>(
    port: &P,
    command: AdminCommand<'_>,
    workspace: WorkspaceId,
    attestation: OperationId,
) -> Result<AdminOutcome, AdminRunError> {
    let branch_key_id = BranchKeyId::of(workspace);
    let target_version = version_for(attestation);
    let observed = port.active(&branch_key_id).await?;

    match command {
        AdminCommand::Create => {
            if let Some(active) = observed {
                return if active.version == target_version {
                    Ok(AdminOutcome::AlreadyCurrent)
                } else {
                    Err(AdminRunError::LineageExists)
                };
            }
            let generation = GenerationSpec {
                branch_key_id: branch_key_id.clone(),
                version: target_version.clone(),
                hierarchy_version: 1,
                attestation,
                reason_digest: None,
            };
            resolve_apply(
                port,
                &branch_key_id,
                &target_version,
                port.create(&generation).await?,
                AdminOutcome::Created,
            )
            .await
        }
        AdminCommand::Rotate { reason } => {
            let active = observed.ok_or(AdminRunError::LineageMissing)?;
            if active.version == target_version {
                return Ok(AdminOutcome::AlreadyCurrent);
            }
            let hierarchy_version = active
                .hierarchy_version
                .checked_add(1)
                .ok_or(AdminRunError::GenerationOverflow)?;
            let generation = GenerationSpec {
                branch_key_id: branch_key_id.clone(),
                version: target_version.clone(),
                hierarchy_version,
                attestation,
                reason_digest: Some(reason_digest(reason)),
            };
            resolve_apply(
                port,
                &branch_key_id,
                &target_version,
                port.rotate(&active, &generation).await?,
                AdminOutcome::Rotated,
            )
            .await
        }
        AdminCommand::Verify => {
            let active = observed.ok_or(AdminRunError::LineageMissing)?;
            port.verify(&branch_key_id, &active).await?;
            Ok(AdminOutcome::AlreadyCurrent)
        }
    }
}

async fn resolve_apply<P: KeyAdminPort + ?Sized>(
    port: &P,
    branch_key_id: &BranchKeyId,
    target_version: &str,
    applied: ApplyOutcome,
    success: AdminOutcome,
) -> Result<AdminOutcome, AdminRunError> {
    if applied == ApplyOutcome::Applied {
        return Ok(success);
    }
    match port.active(branch_key_id).await? {
        Some(active) if active.version == target_version => Ok(AdminOutcome::AlreadyCurrent),
        _ => Err(AdminRunError::ConcurrentMutation),
    }
}

fn version_for(attestation: OperationId) -> String {
    format!("{VERSION_PREFIX}{attestation}")
}

fn reason_digest(reason: &str) -> String {
    let digest = Sha256::digest(reason.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

/// Typed administration refusal. No variant carries key material, ciphertext
/// or a raw rotation reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdminRunError {
    /// A different generation already owns the lineage.
    #[error("a key lineage already exists under another operation")]
    LineageExists,
    /// Rotation or verification was requested before creation.
    #[error("the workspace has no active key lineage")]
    LineageMissing,
    /// The hierarchy counter cannot advance.
    #[error("the key hierarchy generation is exhausted")]
    GenerationOverflow,
    /// A different conditional mutation won.
    #[error("a concurrent key administration mutation won")]
    ConcurrentMutation,
    /// The provider rejected or could not complete an operation.
    #[error("key administration provider `{operation}` failed with `{code}`")]
    Provider {
        /// Provider operation name, never a user value.
        operation: &'static str,
        /// Stable provider error code, never provider detail text.
        code: String,
    },
    /// The stored generation is structurally invalid.
    #[error("the active key generation is invalid: {reason}")]
    InvalidStored {
        /// Non-secret structural reason.
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_wire::ids::{PrefixedId as _, Uuid7};

    use super::*;

    #[derive(Debug, Default)]
    struct MemoryPort {
        active: Mutex<Option<ActiveGeneration>>,
        conflict_once: Mutex<bool>,
        verified: Mutex<u64>,
    }

    #[async_trait]
    impl KeyAdminPort for MemoryPort {
        async fn active(
            &self,
            _branch_key_id: &BranchKeyId,
        ) -> Result<Option<ActiveGeneration>, AdminRunError> {
            Ok(self.active.lock().expect("active lock").clone())
        }

        async fn create(&self, generation: &GenerationSpec) -> Result<ApplyOutcome, AdminRunError> {
            Ok(self.apply(generation, None))
        }

        async fn rotate(
            &self,
            expected: &ActiveGeneration,
            generation: &GenerationSpec,
        ) -> Result<ApplyOutcome, AdminRunError> {
            Ok(self.apply(generation, Some(expected)))
        }

        async fn verify(
            &self,
            _branch_key_id: &BranchKeyId,
            _expected: &ActiveGeneration,
        ) -> Result<(), AdminRunError> {
            *self.verified.lock().expect("verified lock") += 1;
            Ok(())
        }
    }

    impl MemoryPort {
        fn apply(
            &self,
            generation: &GenerationSpec,
            expected: Option<&ActiveGeneration>,
        ) -> ApplyOutcome {
            let mut active = self.active.lock().expect("active lock");
            if *self.conflict_once.lock().expect("conflict lock") {
                *active = Some(ActiveGeneration {
                    version: generation.version.clone(),
                    hierarchy_version: generation.hierarchy_version,
                    kms_arn: "arn:root".to_owned(),
                });
                *self.conflict_once.lock().expect("conflict lock") = false;
                return ApplyOutcome::Conflict;
            }
            if expected.is_some_and(|value| active.as_ref() != Some(value))
                || (expected.is_none() && active.is_some())
            {
                return ApplyOutcome::Conflict;
            }
            *active = Some(ActiveGeneration {
                version: generation.version.clone(),
                hierarchy_version: generation.hierarchy_version,
                kms_arn: "arn:root".to_owned(),
            });
            ApplyOutcome::Applied
        }
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn operation(byte: u8) -> OperationId {
        OperationId::from_uuid7(Uuid7::compose(u64::from(byte), [byte; 10]))
    }

    #[tokio::test]
    async fn create_rotate_and_verify_use_one_monotonic_lineage() {
        let port = MemoryPort::default();
        assert_eq!(
            execute(&port, AdminCommand::Create, workspace(), operation(1))
                .await
                .expect("create"),
            AdminOutcome::Created
        );
        assert_eq!(
            execute(
                &port,
                AdminCommand::Rotate {
                    reason: "scheduled"
                },
                workspace(),
                operation(2),
            )
            .await
            .expect("rotate"),
            AdminOutcome::Rotated
        );
        assert_eq!(
            execute(&port, AdminCommand::Verify, workspace(), operation(3))
                .await
                .expect("verify"),
            AdminOutcome::AlreadyCurrent
        );
        assert_eq!(*port.verified.lock().expect("verified lock"), 1);
        assert_eq!(
            port.active
                .lock()
                .expect("active lock")
                .as_ref()
                .expect("active")
                .hierarchy_version,
            2
        );
    }

    #[tokio::test]
    async fn an_acknowledgement_lost_after_commit_resolves_as_already_current() {
        let port = MemoryPort::default();
        *port.conflict_once.lock().expect("conflict lock") = true;
        assert_eq!(
            execute(&port, AdminCommand::Create, workspace(), operation(1))
                .await
                .expect("recovered"),
            AdminOutcome::AlreadyCurrent
        );
    }

    #[tokio::test]
    async fn a_distinct_create_never_adopts_an_existing_lineage() {
        let port = MemoryPort::default();
        execute(&port, AdminCommand::Create, workspace(), operation(1))
            .await
            .expect("first create");
        assert_eq!(
            execute(&port, AdminCommand::Create, workspace(), operation(2)).await,
            Err(AdminRunError::LineageExists)
        );
    }

    #[test]
    fn a_rotation_reason_is_not_retained_in_plaintext() {
        let raw = "customer supplied incident detail";
        let digest = reason_digest(raw);
        assert_eq!(digest.len(), 64);
        assert!(!digest.contains(raw));
    }
}
