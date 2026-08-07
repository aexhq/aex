//! Validated start-up configuration for `regional-secret-key-admin`.
//!
//! This binary administers the keystore and nothing else. Startup denial is the
//! whole point of the deployable: it refuses to run when the keystore table is
//! not the configured logical name, when the KMS key resolves to another region
//! or account, or when **any** product-table variable is bound to it.

use aex_regional_http::config::{
    Arn, Lookup, RegionalHttpConfigError, arn_in_region, forbidden, plane_name, region, required,
};
use aex_wire::ids::{OperationId, PrefixedId as _};
use aex_wire::types::Region;

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "regional-secret-key-admin";

/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The keystore table this task administers.
pub const SECRET_KEYSTORE_TABLE: &str = "AEX_SECRET_KEYSTORE_TABLE";
/// The logical keystore name the table must be.
pub const KEYSTORE_LOGICAL_NAME: &str = "AEX_KEYSTORE_LOGICAL_NAME";
/// The secret KMS key.
pub const SECRET_KMS_KEY_ARN: &str = "AEX_SECRET_KMS_KEY_ARN";
/// The attested operation identity this run acts under.
pub const ATTESTATION_OPERATION_ID: &str = "AEX_ATTESTATION_OPERATION_ID";

/// Every variable a healthy `regional-secret-key-admin` requires.
pub const REQUIRED: [&str; 6] = [
    PLANE,
    REGION,
    SECRET_KEYSTORE_TABLE,
    KEYSTORE_LOGICAL_NAME,
    SECRET_KMS_KEY_ARN,
    ATTESTATION_OPERATION_ID,
];

/// Every product-table variable, which this role must never hold.
///
/// Application roles are denied keystore administration by IAM; this role is
/// denied ordinary product access by both IAM and this list, so neither
/// direction depends on a single mechanism.
pub const FORBIDDEN: [(&str, &str); 6] = [
    ("AEX_SESSION_TABLE", "the key admin holds no product table"),
    ("AEX_WORK_TABLE", "the key admin holds no product table"),
    ("AEX_CONTENT_TABLE", "the key admin holds no product table"),
    ("AEX_REGISTRY_TABLE", "the key admin holds no product table"),
    (
        "AEX_SECRET_CUSTODY_TABLE",
        "the key admin administers keys and never reads custody",
    ),
    ("AEX_CONTENT_BUCKET", "the key admin holds no object store"),
];

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane.
    pub plane: aex_identity_domain::assertion::Plane,
    /// Pinned region.
    pub region: Region,
    /// The keystore table.
    pub keystore_table: String,
    /// The logical keystore name the table must carry.
    pub keystore_logical_name: String,
    /// The secret KMS key.
    pub secret_kms_key: Arn,
    /// The attested operation identity.
    pub attestation: OperationId,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns the first [`RegionalHttpConfigError`], naming the offending variable.
    pub fn from_env() -> Result<Self, RegionalHttpConfigError> {
        Self::read(&aex_regional_http::config::Environment)
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn read<L: Lookup + ?Sized>(lookup: &L) -> Result<Self, RegionalHttpConfigError> {
        for (name, reason) in FORBIDDEN {
            forbidden(lookup, name, DEPLOYABLE, reason)?;
        }
        let plane = plane_name(lookup, PLANE)?;
        let region = region(lookup, REGION)?;
        let keystore_table = required(lookup, SECRET_KEYSTORE_TABLE)?;
        let keystore_logical_name = required(lookup, KEYSTORE_LOGICAL_NAME)?;
        // The physical table must carry the configured logical name. Without
        // this the task would happily administer a table that merely looks like
        // a keystore, which is how a lineage ends up split across two tables.
        if !keystore_table.ends_with(&keystore_logical_name) {
            return Err(RegionalHttpConfigError::Invalid {
                name: SECRET_KEYSTORE_TABLE,
                reason: format!(
                    "table `{keystore_table}` is not the `{keystore_logical_name}` keystore"
                ),
            });
        }
        let raw_attestation = required(lookup, ATTESTATION_OPERATION_ID)?;
        let attestation =
            OperationId::parse(&raw_attestation).map_err(|_| RegionalHttpConfigError::Invalid {
                name: ATTESTATION_OPERATION_ID,
                reason: format!("`{raw_attestation}` is not an `op_` identifier"),
            })?;
        Ok(Self {
            plane,
            region,
            keystore_table,
            keystore_logical_name,
            secret_kms_key: arn_in_region(lookup, SECRET_KMS_KEY_ARN, region, "kms")?,
            attestation,
        })
    }
}
