//! Internal wake contracts for durable regional operations.

use aex_wire::ids::WorkspaceId;
use serde::{Deserialize, Serialize};

use crate::SchemaVersion;

/// One best-effort wake for a durable session operation.
///
/// This is only a routing hint. The worker strongly reloads the regional-work
/// row and its operation authority before it claims or changes anything, so a
/// duplicate, delayed or forged payload cannot authorize an effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionOperationWake {
    /// Internal contract version.
    pub schema_version: SchemaVersion,
    /// Tenant used for the authoritative work read.
    pub workspace: WorkspaceId,
    /// Deterministic regional-work identity.
    pub work_id: String,
}

impl SessionOperationWake {
    /// Builds the strict-v1 wake.
    ///
    /// # Errors
    ///
    /// Refuses an identity outside the regional-work prefix or containing the
    /// table key separator.
    pub fn new(
        workspace: WorkspaceId,
        work_id: impl Into<String>,
    ) -> Result<Self, InternalContractsWakeError> {
        let work_id = work_id.into();
        if !work_id.starts_with("wrk_") || work_id.contains('#') || work_id.len() > 128 {
            return Err(InternalContractsWakeError::InvalidWorkId);
        }
        Ok(Self {
            schema_version: SchemaVersion::V1,
            workspace,
            work_id,
        })
    }
}

/// Why an internal operation wake was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InternalContractsWakeError {
    /// The work identity cannot name a regional-work row.
    #[error("invalid regional-work identity")]
    InvalidWorkId,
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};

    use super::SessionOperationWake;

    #[test]
    fn the_wake_is_versioned_tenant_bound_and_strict() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let wake = SessionOperationWake::new(workspace, "wrk_01j0000000000000000000000")
            .expect("valid wake");
        let value = serde_json::to_value(&wake).expect("serializes");
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["workspace"], workspace.to_string());
        assert_eq!(value["workId"], "wrk_01j0000000000000000000000");
        assert!(
            serde_json::from_value::<SessionOperationWake>(serde_json::json!({
                "schemaVersion": 1,
                "workspace": workspace,
                "workId": "wrk_01j0000000000000000000000",
                "extra": true
            }))
            .is_err()
        );
    }
}
