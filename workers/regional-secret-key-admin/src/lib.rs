//! Exclusive, idempotent regional branch-key administration kernel.

pub mod config;

pub use config::Config;

use std::collections::BTreeMap;

use aex_wire::ids::{OperationId, PrefixedId as _};
use aex_wire::types::Region;

/// One successful one-shot command outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminOutcome {
    /// First generation created.
    Created,
    /// Exclusive next generation admitted and made current.
    Rotated,
    /// Idempotent no-op; the binary maps this to exit code `3`.
    AlreadyCurrent,
}

/// One admitted generation in the exclusive lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationRecord {
    /// Monotonic generation.
    pub generation: u64,
    /// Previous generation, absent only for generation one.
    pub predecessor: Option<u64>,
    /// Attested durable operation.
    pub attestation: OperationId,
}

/// Pure conditional-write model for one workspace lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyAdmin {
    workspace: String,
    current: Option<u64>,
    generations: BTreeMap<u64, GenerationRecord>,
}

impl KeyAdmin {
    /// Creates an empty workspace lineage.
    ///
    /// # Errors
    ///
    /// Returns [`KeyAdminError::InvalidWorkspace`] for an empty identity.
    pub fn new(workspace: impl Into<String>) -> Result<Self, KeyAdminError> {
        let workspace = workspace.into();
        if workspace.is_empty() {
            return Err(KeyAdminError::InvalidWorkspace);
        }
        Ok(Self {
            workspace,
            current: None,
            generations: BTreeMap::new(),
        })
    }

    /// Conditionally creates generation one.
    ///
    /// # Errors
    ///
    /// Returns [`KeyAdminError`] for a non-one target or conflicting lineage.
    pub fn create(
        &mut self,
        target_generation: u64,
        attestation: OperationId,
    ) -> Result<AdminOutcome, KeyAdminError> {
        if self.current == Some(target_generation) {
            return Ok(AdminOutcome::AlreadyCurrent);
        }
        if self.current.is_some() {
            return Err(KeyAdminError::LineageExists);
        }
        if target_generation != 1 {
            return Err(KeyAdminError::InvalidGeneration);
        }
        self.generations.insert(
            1,
            GenerationRecord {
                generation: 1,
                predecessor: None,
                attestation,
            },
        );
        self.current = Some(1);
        Ok(AdminOutcome::Created)
    }

    /// Conditionally moves exactly one generation forward.
    ///
    /// # Errors
    ///
    /// Returns [`KeyAdminError::ConcurrentRotation`] when the expected pointer
    /// lost, leaving the old current generation unchanged.
    pub fn rotate(
        &mut self,
        expected_current: u64,
        target_generation: u64,
        attestation: OperationId,
    ) -> Result<AdminOutcome, KeyAdminError> {
        if self.current == Some(target_generation) {
            return Ok(AdminOutcome::AlreadyCurrent);
        }
        if self.current != Some(expected_current) {
            return Err(KeyAdminError::ConcurrentRotation);
        }
        if target_generation
            != expected_current
                .checked_add(1)
                .ok_or(KeyAdminError::InvalidGeneration)?
        {
            return Err(KeyAdminError::InvalidGeneration);
        }
        let record = GenerationRecord {
            generation: target_generation,
            predecessor: Some(expected_current),
            attestation,
        };
        self.generations.insert(target_generation, record);
        self.current = Some(target_generation);
        Ok(AdminOutcome::Rotated)
    }

    /// Current generation pointer.
    #[must_use]
    pub const fn current_generation(&self) -> Option<u64> {
        self.current
    }

    /// Workspace whose lineage this model owns.
    #[must_use]
    pub fn workspace(&self) -> &str {
        &self.workspace
    }
}

/// Fully resolved startup binding, before any AWS call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupBinding {
    /// Configured keystore table.
    pub keystore_table: String,
    /// Expected logical keystore name.
    pub expected_logical_name: String,
    /// Secret KMS key ARN.
    pub kms_key_arn: String,
    /// Plane region.
    pub region: Region,
    /// Plane account.
    pub account_id: String,
    /// Attested `op_` identity.
    pub attestation: String,
    /// Any other resource-bearing AEX settings observed in this process.
    pub other_aex_values: BTreeMap<String, String>,
}

/// Validates table, plane, attestation and absence of product bindings.
///
/// # Errors
///
/// Returns [`KeyAdminError`] at the first startup mismatch.
pub fn validate_startup(binding: &StartupBinding) -> Result<OperationId, KeyAdminError> {
    if binding.keystore_table.is_empty() || binding.keystore_table != binding.expected_logical_name
    {
        return Err(KeyAdminError::WrongKeystore);
    }
    if !arn_matches(&binding.kms_key_arn, binding.region, &binding.account_id) {
        return Err(KeyAdminError::OffPlaneKey);
    }
    if binding
        .other_aex_values
        .iter()
        .any(|(key, value)| key.starts_with("AEX_") && !value.is_empty())
    {
        return Err(KeyAdminError::ProductBindingPresent);
    }
    OperationId::parse(&binding.attestation).map_err(|_| KeyAdminError::InvalidAttestation)
}

fn arn_matches(value: &str, region: Region, account: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next() == Some("aws")
        && fields.next() == Some("kms")
        && fields.next() == Some(region.as_str())
        && fields.next() == Some(account)
        && fields
            .next()
            .is_some_and(|resource| resource.starts_with("key/"))
}

/// Typed refusal; no variant carries secret or provider text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyAdminError {
    /// Workspace identity was empty.
    #[error("workspace is invalid")]
    InvalidWorkspace,
    /// Create target was not generation one or rotate skipped a generation.
    #[error("target generation is invalid")]
    InvalidGeneration,
    /// Create raced an existing lineage.
    #[error("a key lineage already exists")]
    LineageExists,
    /// Rotation expected pointer lost the conditional write.
    #[error("concurrent key rotation won")]
    ConcurrentRotation,
    /// Configured table did not equal the declared keystore logical name.
    #[error("keystore table binding is invalid")]
    WrongKeystore,
    /// KMS key was outside the plane region/account.
    #[error("secret KMS key is outside this plane")]
    OffPlaneKey,
    /// Attestation was not a valid operation id.
    #[error("attestation operation is invalid")]
    InvalidAttestation,
    /// A product table variable was present in the admin process.
    #[error("product resource binding is forbidden")]
    ProductBindingPresent,
}
