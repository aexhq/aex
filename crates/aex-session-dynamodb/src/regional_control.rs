//! Fenced regional workspace authority for central direct invokes.
//!
//! A workspace identifier is preassigned centrally, but its empty regional
//! half is durable here. The row is a tombstone after deletion: identifiers are
//! never resurrected, and an absent delete writes a tombstone so a delayed
//! provision cannot arrive afterwards and create the workspace.

use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::control::{
    RegionalControlEnvelope, RegionalControlOutcome, RegionalControlRequest, RegionalRefusal,
};
use aex_wire::ids::{OrganizationId, WorkspaceId};
use aex_wire::types::Region;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure;

use crate::attr::{Item, ItemBuilder, Row, n, s};

/// The stored discriminator for one regional workspace authority row.
pub const REGIONAL_WORKSPACE_CONTROL: &str = "regional_workspace_control";
const STATES: &[&str] = &["active", "deleted"];
const MAX_CAS_ATTEMPTS: u32 = 3;

/// `CONTROL#<workspace>` / `WORKSPACE`.
#[must_use]
pub fn workspace_key(workspace: WorkspaceId) -> (String, String) {
    (format!("CONTROL#{workspace}"), "WORKSPACE".to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Deleted,
}

impl State {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceRow {
    workspace: WorkspaceId,
    organization: Option<OrganizationId>,
    region: Region,
    state: State,
    fence: u64,
    intent_hash: Option<String>,
    revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Answer(RegionalControlOutcome),
    Write {
        row: WorkspaceRow,
        outcome: RegionalControlOutcome,
    },
}

/// DynamoDB-backed regional workspace control authority.
#[derive(Debug, Clone)]
pub struct RegionalWorkspaceStore {
    client: Client,
    table: String,
    region: Region,
}

impl RegionalWorkspaceStore {
    /// Binds the authority to exactly one regional table and region.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>, region: Region) -> Self {
        Self {
            client,
            table: table.into(),
            region,
        }
    }

    /// Performs a strongly consistent read used by startup readiness.
    ///
    /// # Errors
    ///
    /// Returns the AWS diagnostic when the table cannot be read.
    pub async fn probe(&self) -> Result<(), String> {
        self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", s("CONTROL#PROBE"))
            .key("sk", s("WORKSPACE"))
            .consistent_read(true)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Applies one versioned request and echoes its request identity.
    pub async fn handle(
        &self,
        request: RegionalControlEnvelope<RegionalControlRequest>,
    ) -> RegionalControlEnvelope<RegionalControlOutcome> {
        let outcome = if request.schema_version == SchemaVersion::V1 {
            self.apply(&request.payload).await
        } else {
            refused(RegionalRefusal::Internal)
        };
        RegionalControlEnvelope {
            schema_version: SchemaVersion::V1,
            request_id: request.request_id,
            payload: outcome,
        }
    }

    async fn apply(&self, request: &RegionalControlRequest) -> RegionalControlOutcome {
        if request_region(request) != self.region {
            return refused(RegionalRefusal::RegionUnavailable);
        }
        let workspace = request_workspace(request);
        for _ in 0..MAX_CAS_ATTEMPTS {
            let Ok(current) = self.read(workspace).await else {
                return refused(RegionalRefusal::Internal);
            };
            match decide(current.as_ref(), request) {
                Decision::Answer(outcome) => return outcome,
                Decision::Write { row, outcome } => {
                    match self.compare_and_set(current.as_ref(), &row).await {
                        Ok(true) => return outcome,
                        Ok(false) => {}
                        Err(()) => {
                            // Resolve an ambiguous write by reading the target. An
                            // identical durable row is the answer, never a reason
                            // to issue a second write under a fresh identity.
                            if self.read(workspace).await.ok().flatten().as_ref() == Some(&row) {
                                return outcome;
                            }
                            return refused(RegionalRefusal::Internal);
                        }
                    }
                }
            }
        }
        refused(RegionalRefusal::RegionUnavailable)
    }

    async fn read(&self, workspace: WorkspaceId) -> Result<Option<WorkspaceRow>, ()> {
        let (pk, sk) = workspace_key(workspace);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| ())?;
        output
            .item
            .as_ref()
            .map(|item| decode(item, workspace).map_err(|_| ()))
            .transpose()
    }

    async fn compare_and_set(
        &self,
        current: Option<&WorkspaceRow>,
        desired: &WorkspaceRow,
    ) -> Result<bool, ()> {
        let mut put = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(encode(desired)))
            .return_values_on_condition_check_failure(ReturnValuesOnConditionCheckFailure::AllOld);
        put = match current {
            None => put
                .condition_expression("attribute_not_exists(#pk)")
                .expression_attribute_names("#pk", "pk"),
            Some(current) => put
                .condition_expression("#revision = :revision")
                .expression_attribute_names("#revision", "revision")
                .expression_attribute_values(":revision", n(current.revision)),
        };
        match put.send().await {
            Ok(_) => Ok(true),
            Err(error)
                if error.as_service_error().is_some_and(|service| {
                    matches!(
                        service,
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
                    )
                }) => Ok(false),
            Err(_) => Err(()),
        }
    }
}

fn request_workspace(request: &RegionalControlRequest) -> WorkspaceId {
    match request {
        RegionalControlRequest::ProvisionWorkspace { workspace, .. }
        | RegionalControlRequest::DeleteWorkspace { workspace, .. } => *workspace,
    }
}

fn request_region(request: &RegionalControlRequest) -> Region {
    match request {
        RegionalControlRequest::ProvisionWorkspace { region, .. }
        | RegionalControlRequest::DeleteWorkspace { region, .. } => *region,
    }
}

fn decide(current: Option<&WorkspaceRow>, request: &RegionalControlRequest) -> Decision {
    match request {
        RegionalControlRequest::ProvisionWorkspace {
            workspace,
            organization,
            region,
            fence,
            intent_hash,
        } => decide_provision(
            current,
            *workspace,
            *organization,
            *region,
            *fence,
            intent_hash,
        ),
        RegionalControlRequest::DeleteWorkspace {
            workspace,
            region,
            fence,
        } => decide_delete(current, *workspace, *region, *fence),
    }
}

fn decide_provision(
    current: Option<&WorkspaceRow>,
    workspace: WorkspaceId,
    organization: OrganizationId,
    region: Region,
    fence: u64,
    intent_hash: &str,
) -> Decision {
    let success = |created| RegionalControlOutcome::WorkspaceProvisioned { workspace, created };
    let Some(current) = current else {
        return Decision::Write {
            row: WorkspaceRow {
                workspace,
                organization: Some(organization),
                region,
                state: State::Active,
                fence,
                intent_hash: Some(intent_hash.to_owned()),
                revision: 1,
            },
            outcome: success(true),
        };
    };
    if current
        .organization
        .is_some_and(|owner| owner != organization)
    {
        return Decision::Answer(refused(RegionalRefusal::OrganizationMismatch));
    }
    if current.fence > fence {
        return Decision::Answer(refused(RegionalRefusal::FenceSuperseded));
    }
    if current.state == State::Deleted || current.intent_hash.as_deref() != Some(intent_hash) {
        return Decision::Answer(refused(RegionalRefusal::IntentConflict));
    }
    if current.fence == fence {
        return Decision::Answer(success(false));
    }
    let mut advanced = current.clone();
    advanced.fence = fence;
    advanced.revision = advanced.revision.saturating_add(1);
    Decision::Write {
        row: advanced,
        outcome: success(false),
    }
}

fn decide_delete(
    current: Option<&WorkspaceRow>,
    workspace: WorkspaceId,
    region: Region,
    fence: u64,
) -> Decision {
    let success = |removed| RegionalControlOutcome::WorkspaceDeleted { workspace, removed };
    let Some(current) = current else {
        return Decision::Write {
            row: WorkspaceRow {
                workspace,
                organization: None,
                region,
                state: State::Deleted,
                fence,
                intent_hash: None,
                revision: 1,
            },
            outcome: success(false),
        };
    };
    if current.fence > fence {
        return Decision::Answer(refused(RegionalRefusal::FenceSuperseded));
    }
    if current.state == State::Deleted && current.fence == fence {
        return Decision::Answer(success(false));
    }
    let removed = current.state == State::Active;
    let mut tombstone = current.clone();
    tombstone.state = State::Deleted;
    tombstone.fence = fence;
    tombstone.revision = tombstone.revision.saturating_add(1);
    Decision::Write {
        row: tombstone,
        outcome: success(removed),
    }
}

fn refused(reason: RegionalRefusal) -> RegionalControlOutcome {
    RegionalControlOutcome::Refused { reason }
}

fn encode(row: &WorkspaceRow) -> Item {
    let (pk, sk) = workspace_key(row.workspace);
    ItemBuilder::new(REGIONAL_WORKSPACE_CONTROL)
        .set("pk", s(pk))
        .set("sk", s(sk))
        .set("workspaceId", s(row.workspace.to_string()))
        .set_opt(
            "organizationId",
            row.organization.map(|id| s(id.to_string())),
        )
        .set("region", s(row.region.as_str()))
        .set("state", s(row.state.as_str()))
        .set("fence", n(row.fence))
        .set_opt(
            "intentHash",
            row.intent_hash.as_ref().map(|hash| s(hash.clone())),
        )
        .set("revision", n(row.revision))
        .build()
}

fn decode(item: &Item, asserted: WorkspaceId) -> Result<WorkspaceRow, crate::attr::CodecError> {
    let row = Row::bind(item, REGIONAL_WORKSPACE_CONTROL)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let state = match row.enumerated("state", STATES)? {
        "active" => State::Active,
        "deleted" => State::Deleted,
        _ => unreachable!("enumerated returned outside its vocabulary"),
    };
    let region_text = row.string("region")?;
    let region =
        Region::from_name(region_text).ok_or_else(|| crate::attr::CodecError::Malformed {
            item_type: REGIONAL_WORKSPACE_CONTROL,
            attribute: "region",
            reason: format!("`{region_text}` is not a launch region"),
        })?;
    Ok(WorkspaceRow {
        workspace: asserted,
        organization: row.opt_id("organizationId")?,
        region,
        state,
        fence: row.u64("fence")?,
        intent_hash: row.opt_string("intentHash")?.map(str::to_owned),
        revision: row.u64("revision")?,
    })
}

#[cfg(test)]
mod tests {
    use super::{Decision, State, WorkspaceRow, decide, decode, encode};
    use aex_internal_contracts::control::{
        RegionalControlOutcome, RegionalControlRequest, RegionalRefusal,
    };
    use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Region;

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn organization(byte: u8) -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn provision(fence: u64, intent: &str) -> RegionalControlRequest {
        RegionalControlRequest::ProvisionWorkspace {
            workspace: workspace(),
            organization: organization(2),
            region: Region::EuWest1,
            fence,
            intent_hash: intent.to_owned(),
        }
    }

    fn written(decision: Decision) -> WorkspaceRow {
        match decision {
            Decision::Write { row, .. } => row,
            Decision::Answer(answer) => panic!("expected a write, got {answer:?}"),
        }
    }

    #[test]
    fn provision_replay_is_one_workspace_and_a_newer_same_intent_advances_the_fence() {
        let first = written(decide(None, &provision(1, "aa")));
        assert_eq!(first.state, State::Active);
        assert!(matches!(
            decide(Some(&first), &provision(1, "aa")),
            Decision::Answer(RegionalControlOutcome::WorkspaceProvisioned { created: false, .. })
        ));
        let advanced = written(decide(Some(&first), &provision(2, "aa")));
        assert_eq!(advanced.fence, 2);
        assert_eq!(advanced.revision, 2);
    }

    #[test]
    fn intent_tenant_and_stale_fence_conflicts_are_typed() {
        let current = written(decide(None, &provision(3, "aa")));
        for (request, expected) in [
            (provision(2, "aa"), RegionalRefusal::FenceSuperseded),
            (provision(3, "bb"), RegionalRefusal::IntentConflict),
            (
                RegionalControlRequest::ProvisionWorkspace {
                    workspace: workspace(),
                    organization: organization(9),
                    region: Region::EuWest1,
                    fence: 3,
                    intent_hash: "aa".to_owned(),
                },
                RegionalRefusal::OrganizationMismatch,
            ),
        ] {
            assert!(matches!(
                decide(Some(&current), &request),
                Decision::Answer(RegionalControlOutcome::Refused { reason }) if reason == expected
            ));
        }
    }

    #[test]
    fn deletion_is_idempotent_and_tombstones_an_absent_workspace() {
        let delete = RegionalControlRequest::DeleteWorkspace {
            workspace: workspace(),
            region: Region::EuWest1,
            fence: 4,
        };
        let absent = written(decide(None, &delete));
        assert_eq!(absent.state, State::Deleted);
        assert!(matches!(
            decide(Some(&absent), &delete),
            Decision::Answer(RegionalControlOutcome::WorkspaceDeleted { removed: false, .. })
        ));
        assert!(matches!(
            decide(Some(&absent), &provision(5, "aa")),
            Decision::Answer(RegionalControlOutcome::Refused {
                reason: RegionalRefusal::IntentConflict
            })
        ));
    }

    #[test]
    fn the_authority_row_codec_is_strict_and_lossless() {
        let row = written(decide(None, &provision(7, "deadbeef")));
        assert_eq!(decode(&encode(&row), workspace()).expect("decodes"), row);
        let wrong = WorkspaceId::from_uuid7(Uuid7::compose(1, [9; 10]));
        assert!(decode(&encode(&row), wrong).is_err());
    }
}
