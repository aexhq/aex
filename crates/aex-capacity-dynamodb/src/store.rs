//! Atomic persistence for workspace capacity authority and serving projections.

use aex_session_dynamodb::attr::{CodecError, Item, ItemBuilder, Row, n, s, stamp};
use aex_session_dynamodb::capacity_limit_projection_write::{
    LimitBundleWrite, LimitWrite, edge_limits_put, limit_bundle_head_put, limit_bundle_put,
    limit_put,
};
use aex_session_dynamodb::error::{
    Idempotence, Resolution, StoreError, classify, decode_cancellation_with_resolution,
};
use aex_session_dynamodb::plan::{Participant, TransactionPlan};
use aex_wire::ids::{PrefixedId as _, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;
use sha2::{Digest as _, Sha256};

use crate::defaults::CapacityDefaults;
use crate::model::{
    CapacityCommand, CapacityError, CapacityState, PlannedCapacity, plan_capacity_change,
};

const WORKSPACE_CAPACITY: &str = "workspace_capacity";
const CAPACITY_AUDIT: &str = "capacity_audit";
const STATE_SK: &str = "STATE";
const AUTHORITY_PARTICIPANT: Participant = Participant::new("capacity.authority");
const AUDIT_PARTICIPANT: Participant = Participant::new("capacity.audit");
const PROJECTION_MEMBER_PARTICIPANT: Participant = Participant::new("capacity.projection_member");
const PROJECTION_BUNDLE_PARTICIPANT: Participant = Participant::new("capacity.projection_bundle");
const PROJECTION_HEAD_PARTICIPANT: Participant = Participant::new("capacity.projection_head");
const PROJECTION_EDGE_PARTICIPANT: Participant = Participant::new("capacity.projection_edge");

/// The index a defaults-revision sweep enumerates over.
pub const ALL_WORKSPACES_INDEX: &str = "gsi_all_workspaces";
/// The one partition every authority row is written into for that enumeration.
pub const ALL_WORKSPACES_PARTITION: &str = "WORKSPACES";

const CREATE_CONDITION: &str = "attribute_not_exists(#pk)";
const REPLACE_CONDITION: &str =
    "#item_type = :item_type AND #workspace_id = :workspace_id AND #revision = :expected_revision";

/// One durable capacity transition failure.
#[derive(Debug, thiserror::Error)]
pub enum CapacityStoreError {
    /// Pure authority policy rejected the command.
    #[error(transparent)]
    Capacity(#[from] CapacityError),
    /// Persistence or projection publication failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Result of one authority command.
#[derive(Clone, Debug, PartialEq)]
pub struct AppliedCapacity {
    /// Complete durable answer.
    pub state: CapacityState,
    /// Whether this command published a new authority revision.
    pub changed: bool,
}

/// The one regional capacity authority adapter.
#[derive(Clone, Debug)]
pub struct CapacityStore {
    client: Client,
    authority_table: String,
    projection_table: String,
}

impl CapacityStore {
    /// Binds the authority and public projection tables named by composition.
    #[must_use]
    pub fn new(
        client: Client,
        authority_table: impl Into<String>,
        projection_table: impl Into<String>,
    ) -> Self {
        Self {
            client,
            authority_table: authority_table.into(),
            projection_table: projection_table.into(),
        }
    }

    /// Strongly reads one workspace's capacity authority.
    ///
    /// # Errors
    ///
    /// Returns a typed store failure; corrupt rows never become defaults.
    pub async fn load(&self, workspace: WorkspaceId) -> Result<Option<CapacityState>, StoreError> {
        let (pk, sk) = state_key(workspace);
        let output = self
            .client
            .get_item()
            .table_name(&self.authority_table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        output
            .item()
            .map(|item| decode_state(item, workspace).map_err(StoreError::from))
            .transpose()
    }

    /// Lists the workspaces this region holds a capacity authority for.
    ///
    /// The enumeration a defaults-revision sweep walks, over `gsi_all_workspaces`.
    /// It is a **query on one partition**, not a scan: every authority row
    /// carries the same `gsiAllPk`, sorted by workspace id, so a sweep resumes
    /// exactly where it stopped and cannot revisit or skip a workspace when it
    /// is restarted.
    ///
    /// The index is sparse and only the authority row carries the key, so the
    /// immutable `capacity_audit` siblings — one per revision, unboundedly many —
    /// never appear here.
    ///
    /// # Errors
    ///
    /// Returns a typed store failure. A page that cannot be decoded is an error
    /// rather than a short page: a sweep that silently skipped a workspace would
    /// leave exactly the incomplete effective set it exists to repair.
    pub async fn page_workspaces(
        &self,
        budget: u32,
        after: Option<WorkspaceId>,
    ) -> Result<Vec<WorkspaceId>, StoreError> {
        let mut query = self
            .client
            .query()
            .table_name(&self.authority_table)
            .index_name(ALL_WORKSPACES_INDEX)
            .key_condition_expression("#pk = :pk")
            .expression_attribute_names("#pk", "gsiAllPk")
            .expression_attribute_values(":pk", s(ALL_WORKSPACES_PARTITION))
            .limit(i32::try_from(budget.max(1)).unwrap_or(i32::MAX));
        if let Some(after) = after {
            query = query.set_exclusive_start_key(Some(std::collections::HashMap::from([
                ("pk".to_owned(), s(workspace_pk(after))),
                ("sk".to_owned(), s(STATE_SK.to_owned())),
                ("gsiAllPk".to_owned(), s(ALL_WORKSPACES_PARTITION)),
                ("gsiAllSk".to_owned(), s(after.to_string())),
            ])));
        }
        let output = query
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        output
            .items
            .unwrap_or_default()
            .iter()
            .map(|item| {
                // A `KEYS_ONLY` projection carries no `itemType` and no body,
                // so the identity is the index sort key itself — which is the
                // whole reason the sweep sorts by workspace id rather than by
                // anything it would have to project a body to read.
                let raw = item
                    .get("gsiAllSk")
                    .and_then(|value| value.as_s().ok())
                    .ok_or_else(|| StoreError::Invalid {
                        detail: "a capacity enumeration row carries no workspace id".to_owned(),
                    })?;
                WorkspaceId::parse(raw).map_err(|error| StoreError::Invalid {
                    detail: format!("`{raw}` is not a workspace id: {error}"),
                })
            })
            .collect()
    }

    /// Plans and atomically commits authority, audit, all member rows and the
    /// completeness-fenced bundle. An ambiguous commit is resolved by one
    /// strong authority read and is never blindly retried.
    ///
    /// # Errors
    ///
    /// Returns policy, contention, precondition, corruption or transport
    /// failures. A lost revision race is reported fail closed.
    pub async fn apply(
        &self,
        defaults: &CapacityDefaults,
        command: &CapacityCommand,
        now: Timestamp,
    ) -> Result<AppliedCapacity, CapacityStoreError> {
        let current = self.load(command.workspace()).await?;
        let planned = plan_capacity_change(current.as_ref(), defaults, command, now)?;
        if !planned.changed {
            return Ok(AppliedCapacity {
                state: planned.state,
                changed: false,
            });
        }
        let plan = build_plan(
            &self.authority_table,
            &self.projection_table,
            current.as_ref(),
            &planned,
        )?;
        match plan.compile(&self.client)?.send().await {
            Ok(_) => Ok(AppliedCapacity {
                state: planned.state,
                changed: true,
            }),
            Err(error) => {
                let classified = if let Some(service) = error.as_service_error() {
                    decode_cancellation_with_resolution(
                        service,
                        plan.participants(),
                        Resolution::TargetItem,
                    )
                } else {
                    classify(&error, Idempotence::Write(Resolution::TargetItem))
                };
                if matches!(classified, StoreError::CommitAmbiguous { .. }) {
                    let resolved = self.load(command.workspace()).await?;
                    if resolved.as_ref() == Some(&planned.state) {
                        return Ok(AppliedCapacity {
                            state: planned.state,
                            changed: true,
                        });
                    }
                }
                Err(CapacityStoreError::Store(classified))
            }
        }
    }
}

fn build_plan(
    authority_table: &str,
    projection_table: &str,
    current: Option<&CapacityState>,
    planned: &PlannedCapacity,
) -> Result<TransactionPlan, StoreError> {
    let state = &planned.state;
    let mut plan = TransactionPlan::new(format!(
        "capacity:{}:{}",
        state.workspace_id, state.revision
    ));
    plan.put(
        AUTHORITY_PARTICIPANT,
        authority_put(authority_table, current, state)?,
    )?;
    plan.put(AUDIT_PARTICIPANT, audit_put(authority_table, state)?)?;

    let projected = state.projected_limits();
    for limit in &projected {
        plan.put(
            PROJECTION_MEMBER_PARTICIPANT,
            limit_put(
                projection_table,
                &LimitWrite {
                    workspace: state.workspace_id,
                    id: limit.id,
                    effective_value: limit.effective_value.clone(),
                    source: limit.source,
                    revision: state.revision,
                    changed_at: state.changed_at,
                },
            )?,
        )?;
    }
    let bundle = LimitBundleWrite {
        workspace: state.workspace_id,
        revision: state.revision,
        defaults_revision: state.defaults_revision,
        changed_at: state.changed_at,
        limits: projected,
    };
    // Payload is written before the head inside the transaction for readable
    // diagnostics. Atomic visibility, not action order, is the correctness fence.
    plan.put(
        PROJECTION_BUNDLE_PARTICIPANT,
        limit_bundle_put(projection_table, &bundle)?,
    )?;
    plan.put(
        PROJECTION_HEAD_PARTICIPANT,
        limit_bundle_head_put(projection_table, &bundle)?,
    )?;
    // The hot admission subset is cut from the same bundle in the same
    // transaction, so a request edge reading it alone can never see a revision
    // the authority has not committed.
    plan.put(
        PROJECTION_EDGE_PARTICIPANT,
        edge_limits_put(projection_table, &bundle)?,
    )?;
    Ok(plan)
}

fn authority_put(
    table: &str,
    current: Option<&CapacityState>,
    state: &CapacityState,
) -> Result<aws_sdk_dynamodb::types::builders::PutBuilder, StoreError> {
    let encoded = serde_json::to_string(state).map_err(|error| StoreError::Invalid {
        detail: format!("capacity state could not be encoded: {error}"),
    })?;
    let (pk, sk) = state_key(state.workspace_id);
    let item = ItemBuilder::new(WORKSPACE_CAPACITY)
        .set("pk", s(pk))
        .set("sk", s(sk))
        .set("workspaceId", s(state.workspace_id.to_string()))
        .set("revision", n(state.revision))
        .set("defaultsRevision", n(state.defaults_revision))
        .set("defaultsDigest", s(state.defaults_digest.clone()))
        .set("capacityFence", n(state.capacity_fence))
        .set("state", s(encoded))
        .set("changedAt", stamp(state.changed_at))
        .set("gsiAllPk", s(ALL_WORKSPACES_PARTITION))
        .set("gsiAllSk", s(state.workspace_id.to_string()))
        .build();
    let mut put = aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .expression_attribute_names("#pk", "pk");
    if let Some(current) = current {
        put = put
            .condition_expression(REPLACE_CONDITION)
            .expression_attribute_names("#item_type", "itemType")
            .expression_attribute_names("#workspace_id", "workspaceId")
            .expression_attribute_names("#revision", "revision")
            .expression_attribute_values(":item_type", s(WORKSPACE_CAPACITY))
            .expression_attribute_values(":workspace_id", s(state.workspace_id.to_string()))
            .expression_attribute_values(":expected_revision", n(current.revision));
    } else {
        put = put.condition_expression(CREATE_CONDITION);
    }
    Ok(put)
}

fn audit_put(
    table: &str,
    state: &CapacityState,
) -> Result<aws_sdk_dynamodb::types::builders::PutBuilder, StoreError> {
    let encoded = serde_json::to_vec(state).map_err(|error| StoreError::Invalid {
        detail: format!("capacity audit state could not be encoded: {error}"),
    })?;
    let digest = hex::encode(Sha256::digest(encoded));
    let pk = workspace_pk(state.workspace_id);
    let item = ItemBuilder::new(CAPACITY_AUDIT)
        .set("pk", s(pk))
        .set("sk", s(format!("AUDIT#{:020}", state.revision)))
        .set("workspaceId", s(state.workspace_id.to_string()))
        .set("revision", n(state.revision))
        .set("stateDigest", s(digest))
        .set(
            "approvalId",
            s(state
                .last_approval_id
                .as_deref()
                .unwrap_or("system.reconcile")),
        )
        .set("changedAt", stamp(state.changed_at))
        .build();
    Ok(aws_sdk_dynamodb::types::Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(CREATE_CONDITION)
        .expression_attribute_names("#pk", "pk"))
}

fn decode_state(item: &Item, asserted: WorkspaceId) -> Result<CapacityState, CodecError> {
    let row = Row::bind(item, WORKSPACE_CAPACITY)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let expected = state_key(asserted);
    if row.string("pk")? != expected.0 || row.string("sk")? != expected.1 {
        return Err(malformed(
            "pk",
            "authority row key disagrees with workspace identity",
        ));
    }
    let state = serde_json::from_str::<CapacityState>(row.string("state")?)
        .map_err(|error| malformed("state", &format!("capacity state JSON is invalid: {error}")))?;
    if state.workspace_id != asserted
        || state.revision != row.u64("revision")?
        || state.defaults_revision != row.u64("defaultsRevision")?
        || state.defaults_digest != row.string("defaultsDigest")?
        || state.capacity_fence != row.u64("capacityFence")?
        || state.changed_at != row.timestamp("changedAt")?
    {
        return Err(malformed(
            "state",
            "encoded capacity state disagrees with its indexed authority fields",
        ));
    }
    Ok(state)
}

fn malformed(attribute: &'static str, reason: &str) -> CodecError {
    CodecError::Malformed {
        item_type: WORKSPACE_CAPACITY,
        attribute,
        reason: reason.to_owned(),
    }
}

fn state_key(workspace: WorkspaceId) -> (String, String) {
    (workspace_pk(workspace), STATE_SK.to_owned())
}

fn workspace_pk(workspace: WorkspaceId) -> String {
    format!("WS#{workspace}")
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{AUTHORITY_PARTICIPANT, PROJECTION_EDGE_PARTICIPANT, build_plan, decode_state};
    use crate::defaults::canonical_defaults;
    use crate::model::{CapacityCommand, plan_capacity_change};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [3; 10]))
    }

    #[test]
    fn one_commit_contains_authority_audit_complete_members_bundle_head_and_edge_limits() {
        let state = plan_capacity_change(
            None,
            &canonical_defaults().expect("defaults"),
            &CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            },
            Timestamp::from_unix_millis(1).expect("timestamp"),
        )
        .expect("plan")
        .state;
        let plan = build_plan(
            "authority-table",
            "projection-table",
            None,
            &crate::model::PlannedCapacity {
                state,
                changed: true,
            },
        )
        .expect("transaction");
        // Authority, audit, every member row, the bundle payload, its head, and
        // the hot admission subset a request edge reads on its own.
        assert_eq!(plan.len(), 2 + aex_wire::limits::LimitId::ALL.len() + 3);
        assert_eq!(plan.participants()[0], AUTHORITY_PARTICIPANT);
        assert!(
            plan.participants().contains(&PROJECTION_EDGE_PARTICIPANT),
            "the edge-limit row must be published by the same transaction"
        );
    }

    #[test]
    fn authority_codec_rejects_identity_and_duplicate_field_drift() {
        let state = plan_capacity_change(
            None,
            &canonical_defaults().expect("defaults"),
            &CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            },
            Timestamp::from_unix_millis(1).expect("timestamp"),
        )
        .expect("plan")
        .state;
        let action = super::authority_put("authority-table", None, &state)
            .expect("put")
            .build()
            .expect("complete");
        assert_eq!(
            decode_state(action.item(), workspace()).expect("decode"),
            state
        );

        let mut corrupt = action.item().clone();
        corrupt.insert("revision".to_owned(), aex_session_dynamodb::attr::n(99));
        assert!(decode_state(&corrupt, workspace()).is_err());
    }
}
