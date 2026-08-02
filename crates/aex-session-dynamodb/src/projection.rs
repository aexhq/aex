//! The read-only `regional-authz-projection` reader.
//!
//! This table is written **only** by `central-control-worker`. Every regional
//! role holds `dynamodb:GetItem` and `dynamodb:Query` on it and nothing else,
//! and this module contains no write operation at all — a source conformance
//! test asserts that, because "read-only by convention" is not a property.
//!
//! It lives behind the `authz-projection` feature so `regional-secret-api` and
//! `regional-otlp` can read a placement without linking the session row codec
//! (D-21).

use aex_wire::ids::{ApiKeyId, WorkspaceId};
use aex_wire::limits::{LimitId, LimitShape};
use aex_wire::models::{LimitSource, LimitValue};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;

use crate::attr::{CodecError, Item, Row};
use crate::error::{Idempotence, StoreError, classify};
use crate::paging::{PageBudget, PagePosition};
use crate::plan::key;
use crate::wire_pending::{
    FeedFrontier, KeyRevocation, ProjectedWorkspaceLimit, WorkspacePlacement, WorkspaceProfile,
};

/// The `itemType` of a workspace placement.
pub const WORKSPACE_PLACEMENT: &str = "workspace_placement";
/// The `itemType` of a key revocation.
pub const KEY_REVOCATION: &str = "key_revocation";
/// The `itemType` of the signed feed frontier.
pub const FEED_FRONTIER: &str = "feed_frontier";
/// The `itemType` of descriptive workspace facts.
pub const WORKSPACE_PROFILE: &str = "workspace_profile";
/// The `itemType` of a durable effective workspace limit.
pub const WORKSPACE_LIMIT: &str = "workspace_limit";

/// `WS#{workspace_id}` / `PLACEMENT`.
#[must_use]
pub fn placement_key(workspace: WorkspaceId) -> (String, String) {
    (format!("WS#{workspace}"), "PLACEMENT".to_owned())
}

/// `KEY#{api_key_id}` / `REVOCATION`.
#[must_use]
pub fn revocation_key(api_key: ApiKeyId) -> (String, String) {
    (format!("KEY#{api_key}"), "REVOCATION".to_owned())
}

/// `FEED` / `FRONTIER`.
#[must_use]
pub fn frontier_key() -> (String, String) {
    ("FEED".to_owned(), "FRONTIER".to_owned())
}

/// `WS#{workspace_id}` / `PROFILE`.
#[must_use]
pub fn profile_key(workspace: WorkspaceId) -> (String, String) {
    (format!("WS#{workspace}"), "PROFILE".to_owned())
}

/// `WS#{workspace_id}` / `LIMIT#{limit_id}`.
#[must_use]
pub fn limit_key(workspace: WorkspaceId, limit: LimitId) -> (String, String) {
    (
        format!("WS#{workspace}"),
        format!("LIMIT#{}", limit.as_str()),
    )
}

/// One bounded page from the descriptive workspace projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionPage<T> {
    /// Decoded rows.
    pub items: Vec<T>,
    /// The authority position to bind into an edge-owned continuation.
    pub next: Option<PagePosition>,
}

/// Every placement status value.
pub const PLACEMENT_STATUSES: &[&str] = &["active", "paused", "deleting"];

// TODO(cross-stream): `aex-workspace-domain` publishes no authorization projection. Its
// modules are `grant`, `persist`, `registry` and `upload`; the nearest thing to an
// authorization decision there is `aex_workspace_domain::grant::DownloadGrant`, which is a
// per-object grant rather than a workspace projection.
/// The read-only authorization projection.
#[async_trait]
pub trait AuthorizationProjection: Send + Sync + 'static {
    /// Reads one workspace's placement.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for any transport or decode failure. An absent placement
    /// is [`StoreError::Misconfigured`], not `Ok(None)`: a regional plane that
    /// cannot resolve a workspace it was asked about must fail closed.
    async fn read_placement(
        &self,
        workspace: WorkspaceId,
    ) -> Result<WorkspacePlacement, StoreError>;

    /// Reads one key's revocation, when it has one.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for any transport or decode failure.
    async fn read_key_revocation(
        &self,
        api_key: ApiKeyId,
    ) -> Result<Option<KeyRevocation>, StoreError>;

    /// Reads how far the projection is proved current.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for any transport or decode failure.
    async fn read_frontier(&self) -> Result<FeedFrontier, StoreError>;
}

/// The cold workspace-description surface.
///
/// This is deliberately separate from [`AuthorizationProjection`]. A display
/// name or effective-limit read must not widen the placement capability every
/// request uses to authorize work.
#[async_trait]
pub trait WorkspaceProjection: Send + Sync + 'static {
    /// Reads descriptive profile facts.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for transport or strict decode failure.
    async fn read_profile(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Option<WorkspaceProfile>, StoreError>;

    /// Reads one durable effective limit.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for transport or strict decode failure.
    async fn read_limit(
        &self,
        workspace: WorkspaceId,
        limit: LimitId,
    ) -> Result<Option<ProjectedWorkspaceLimit>, StoreError>;

    /// Lists durable effective limits in registry-key order.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for transport, cursor-position or strict decode failure.
    async fn page_limits(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<ProjectionPage<ProjectedWorkspaceLimit>, StoreError>;
}

/// The reader.
#[derive(Debug, Clone)]
pub struct ProjectionReader {
    client: Client,
    table: String,
}

impl ProjectionReader {
    /// Binds a reader to a physical table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    async fn get(&self, pk: &str, sk: &str) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            // Every authority point read is strongly consistent. An eventually
            // consistent authorization read would admit a request against a
            // placement that was revoked seconds ago.
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }
}

#[async_trait]
impl AuthorizationProjection for ProjectionReader {
    async fn read_placement(
        &self,
        workspace: WorkspaceId,
    ) -> Result<WorkspacePlacement, StoreError> {
        let (pk, sk) = placement_key(workspace);
        let item = self
            .get(&pk, &sk)
            .await?
            .ok_or_else(|| StoreError::Misconfigured {
                table: self.table.clone(),
            })?;
        Ok(decode_placement(&item, workspace)?)
    }

    async fn read_key_revocation(
        &self,
        api_key: ApiKeyId,
    ) -> Result<Option<KeyRevocation>, StoreError> {
        let (pk, sk) = revocation_key(api_key);
        match self.get(&pk, &sk).await? {
            None => Ok(None),
            Some(item) => Ok(Some(decode_revocation(&item)?)),
        }
    }

    async fn read_frontier(&self) -> Result<FeedFrontier, StoreError> {
        let (pk, sk) = frontier_key();
        let item = self
            .get(&pk, &sk)
            .await?
            .ok_or_else(|| StoreError::Misconfigured {
                table: self.table.clone(),
            })?;
        Ok(decode_frontier(&item)?)
    }
}

#[async_trait]
impl WorkspaceProjection for ProjectionReader {
    async fn read_profile(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Option<WorkspaceProfile>, StoreError> {
        let (pk, sk) = profile_key(workspace);
        self.get(&pk, &sk)
            .await?
            .as_ref()
            .map(|item| decode_profile(item, workspace).map_err(StoreError::from))
            .transpose()
    }

    async fn read_limit(
        &self,
        workspace: WorkspaceId,
        limit: LimitId,
    ) -> Result<Option<ProjectedWorkspaceLimit>, StoreError> {
        let (pk, sk) = limit_key(workspace, limit);
        self.get(&pk, &sk)
            .await?
            .as_ref()
            .map(|item| decode_limit(item, workspace).map_err(StoreError::from))
            .transpose()
    }

    async fn page_limits(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<ProjectionPage<ProjectedWorkspaceLimit>, StoreError> {
        let partition = format!("WS#{workspace}");
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("#pk = :pk AND begins_with(#sk, :prefix)")
            .expression_attribute_names("#pk", crate::attr::PK)
            .expression_attribute_names("#sk", crate::attr::SK)
            .expression_attribute_values(":pk", crate::attr::s(partition))
            .expression_attribute_values(":prefix", crate::attr::s("LIMIT#"))
            .limit(budget.limit())
            .consistent_read(true)
            .set_exclusive_start_key(after.map(|position| position.to_exclusive_start(None, None)))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let items = output
            .items
            .unwrap_or_default()
            .iter()
            .map(|item| decode_limit(item, workspace).map_err(StoreError::from))
            .collect::<Result<Vec<_>, _>>()?;
        let next = output
            .last_evaluated_key
            .as_ref()
            .map(|last| PagePosition::from_last_evaluated(last, None, None))
            .transpose()
            .map_err(|error| StoreError::Invalid {
                detail: error.to_string(),
            })?;
        Ok(ProjectionPage { items, next })
    }
}

/// Decodes cold descriptive workspace facts.
///
/// # Errors
///
/// [`CodecError`] for a missing, mistyped or foreign-tenant field.
pub fn decode_profile(item: &Item, asserted: WorkspaceId) -> Result<WorkspaceProfile, CodecError> {
    let row = Row::bind(item, WORKSPACE_PROFILE)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(WorkspaceProfile {
        workspace: asserted,
        name: row.string("name")?.to_owned(),
        slug: row.string("slug")?.to_owned(),
        created_at: row.timestamp("createdAt")?,
    })
}

/// Decodes one durable effective limit and checks its registered shape.
///
/// # Errors
///
/// [`CodecError`] for a missing, mistyped, foreign-tenant, unknown-limit or
/// wrong-shape field.
pub fn decode_limit(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<ProjectedWorkspaceLimit, CodecError> {
    let row = Row::bind(item, WORKSPACE_LIMIT)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let id = LimitId::parse(row.string("limitId")?).ok_or_else(|| CodecError::Malformed {
        item_type: WORKSPACE_LIMIT,
        attribute: "limitId",
        reason: "outside the generated limit registry".to_owned(),
    })?;
    let effective_value = serde_json::from_str::<LimitValue>(row.string("effectiveValue")?)
        .map_err(|error| CodecError::Malformed {
            item_type: WORKSPACE_LIMIT,
            attribute: "effectiveValue",
            reason: error.to_string(),
        })?;
    let actual_shape = match &effective_value {
        LimitValue::Scalar(_) => LimitShape::Scalar,
        LimitValue::Map(_) => LimitShape::Map,
    };
    if actual_shape != id.shape() {
        return Err(CodecError::Malformed {
            item_type: WORKSPACE_LIMIT,
            attribute: "effectiveValue",
            reason: format!(
                "shape {actual_shape:?} does not match registered {:?}",
                id.shape()
            ),
        });
    }
    let source = match row.string("source")? {
        "default" => LimitSource::Default,
        "workspace_override" => LimitSource::WorkspaceOverride,
        _ => {
            return Err(CodecError::Malformed {
                item_type: WORKSPACE_LIMIT,
                attribute: "source",
                reason: "expected `default` or `workspace_override`".to_owned(),
            });
        }
    };
    Ok(ProjectedWorkspaceLimit {
        workspace: asserted,
        id,
        effective_value,
        source,
        revision: row.u64("revision")?,
        changed_at: row.timestamp("changedAt")?,
    })
}

/// Decodes a placement.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute.
pub fn decode_placement(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<WorkspacePlacement, CodecError> {
    let row = Row::bind(item, WORKSPACE_PLACEMENT)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(WorkspacePlacement {
        workspace: asserted,
        organization: row.id("organizationId")?,
        plane: row.string("plane")?.to_owned(),
        region: row.string("region")?.to_owned(),
        status: row.enumerated("status", PLACEMENT_STATUSES)?.to_owned(),
        key_epoch: row.u64("keyEpoch")?,
        account_epoch: row.u64("accountEpoch")?,
        revocation_epoch: row.u64("revocationEpoch")?,
        feed_sequence: row.u64("feedSequence")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// Decodes a key revocation.
///
/// # Errors
///
/// [`CodecError`] as above.
pub fn decode_revocation(item: &Item) -> Result<KeyRevocation, CodecError> {
    let row = Row::bind(item, KEY_REVOCATION)?;
    Ok(KeyRevocation {
        api_key: row.id::<ApiKeyId>("apiKeyId")?,
        revoked_at: row.timestamp("revokedAt")?,
        revoked_epoch: row.u64("revokedEpoch")?,
    })
}

/// Decodes the signed feed frontier.
///
/// # Errors
///
/// [`CodecError`] as above.
pub fn decode_frontier(item: &Item) -> Result<FeedFrontier, CodecError> {
    let row = Row::bind(item, FEED_FRONTIER)?;
    Ok(FeedFrontier {
        sequence: row.u64("sequence")?,
        signed_at: row.timestamp("signedAt")?,
        signature_key_id: row.string("signatureKeyId")?.to_owned(),
        covered_through: row.timestamp("coveredThrough")?,
    })
}

/// Whether a placement permits execution right now.
///
/// The epochs are compared by the admission transaction's `ConditionCheck`, not
/// here; this is the startup-and-readiness answer, not the commit fence.
#[must_use]
pub fn admits_execution(placement: &WorkspacePlacement) -> bool {
    placement.status == "active"
}

/// The exact placement guard values an admission fences on.
#[must_use]
pub fn guard(placement: &WorkspacePlacement) -> crate::wire_pending::PlacementGuard {
    crate::wire_pending::PlacementGuard {
        workspace: placement.workspace,
        key_epoch: placement.key_epoch,
        account_epoch: placement.account_epoch,
        revocation_epoch: placement.revocation_epoch,
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{ApiKeyId, PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{LimitScalarValue, LimitSource, LimitValue};
    use aex_wire::types::DecimalU128;

    use super::{
        FEED_FRONTIER, KEY_REVOCATION, WORKSPACE_LIMIT, WORKSPACE_PLACEMENT, WORKSPACE_PROFILE,
        admits_execution, decode_frontier, decode_limit, decode_placement, decode_profile,
        decode_revocation, frontier_key, guard, limit_key, placement_key, profile_key,
        revocation_key,
    };
    use crate::attr::{CodecError, ItemBuilder, n, s};

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn placement_item(status: &str) -> crate::attr::Item {
        ItemBuilder::new(WORKSPACE_PLACEMENT)
            .set("workspaceId", s(workspace(1).to_string()))
            .set(
                "organizationId",
                s(
                    aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]))
                        .to_string(),
                ),
            )
            .set("plane", s("dev"))
            .set("region", s("eu-west-1"))
            .set("status", s(status))
            .set("keyEpoch", n(4))
            .set("accountEpoch", n(5))
            .set("revocationEpoch", n(6))
            .set("feedSequence", n(77))
            .set("updatedAt", s("2026-08-01T00:00:00.000Z"))
            .build()
    }

    #[test]
    fn a_placement_decodes_and_yields_the_exact_guard_values() {
        let placement = decode_placement(&placement_item("active"), workspace(1)).expect("decodes");
        assert!(admits_execution(&placement));
        let guard = guard(&placement);
        assert_eq!(guard.key_epoch, 4);
        assert_eq!(guard.account_epoch, 5);
        assert_eq!(guard.revocation_epoch, 6);
    }

    #[test]
    fn a_paused_workspace_does_not_admit_execution() {
        let placement = decode_placement(&placement_item("paused"), workspace(1)).expect("decodes");
        assert!(!admits_execution(&placement));
    }

    #[test]
    fn a_placement_status_outside_the_vocabulary_is_corrupt() {
        let error = decode_placement(&placement_item("teleported"), workspace(1))
            .expect_err("outside the vocabulary");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }

    #[test]
    fn a_placement_for_another_workspace_is_rejected_after_read() {
        let error =
            decode_placement(&placement_item("active"), workspace(9)).expect_err("wrong tenant");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }

    #[test]
    fn a_revocation_and_a_frontier_decode() {
        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let item = ItemBuilder::new(KEY_REVOCATION)
            .set("apiKeyId", s(api_key.to_string()))
            .set("revokedAt", s("2026-08-01T00:00:00.000Z"))
            .set("revokedEpoch", n(2))
            .build();
        assert_eq!(decode_revocation(&item).expect("decodes").api_key, api_key);

        let item = ItemBuilder::new(FEED_FRONTIER)
            .set("sequence", n(9))
            .set("signedAt", s("2026-08-01T00:00:00.000Z"))
            .set("signatureKeyId", s("key-1"))
            .set("coveredThrough", s("2026-08-01T00:00:00.000Z"))
            .build();
        assert_eq!(decode_frontier(&item).expect("decodes").sequence, 9);
    }

    #[test]
    fn profile_and_effective_limits_are_separate_typed_rows() {
        let profile = ItemBuilder::new(WORKSPACE_PROFILE)
            .set("workspaceId", s(workspace(1).to_string()))
            .set("name", s("Production"))
            .set("slug", s("production"))
            .set("createdAt", s("2026-08-01T00:00:00.000Z"))
            .build();
        let decoded = decode_profile(&profile, workspace(1)).expect("profile");
        assert_eq!(decoded.name, "Production");

        let value = LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(100),
        });
        let limit = ItemBuilder::new(WORKSPACE_LIMIT)
            .set("workspaceId", s(workspace(1).to_string()))
            .set("limitId", s(LimitId::QueryPage.as_str()))
            .set(
                "effectiveValue",
                s(serde_json::to_string(&value).expect("json")),
            )
            .set("source", s("workspace_override"))
            .set("revision", n(3))
            .set("changedAt", s("2026-08-01T00:00:00.000Z"))
            .build();
        let decoded = decode_limit(&limit, workspace(1)).expect("limit");
        assert_eq!(decoded.id, LimitId::QueryPage);
        assert_eq!(decoded.effective_value, value);
        assert_eq!(decoded.source, LimitSource::WorkspaceOverride);

        let placement = placement_item("active");
        assert!(!placement.contains_key("name"));
        assert!(!placement.contains_key("slug"));
        assert!(!placement.contains_key("effectiveValue"));
    }

    #[test]
    fn a_limit_value_with_the_wrong_registered_shape_is_corrupt() {
        let value = aex_wire::models::LimitValue::Map(aex_wire::models::LimitMapValue {
            values: std::collections::BTreeMap::new(),
        });
        let limit = ItemBuilder::new(WORKSPACE_LIMIT)
            .set("workspaceId", s(workspace(1).to_string()))
            .set("limitId", s(LimitId::QueryPage.as_str()))
            .set(
                "effectiveValue",
                s(serde_json::to_string(&value).expect("json")),
            )
            .set("source", s("default"))
            .set("revision", n(1))
            .set("changedAt", s("2026-08-01T00:00:00.000Z"))
            .build();
        assert!(matches!(
            decode_limit(&limit, workspace(1)),
            Err(CodecError::Malformed { .. })
        ));
    }

    #[test]
    fn the_three_key_shapes_are_disjoint() {
        let (placement_pk, _) = placement_key(workspace(1));
        let profile = profile_key(workspace(1));
        let limit = limit_key(workspace(1), LimitId::QueryPage);
        let (revocation_pk, _) = revocation_key(ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10])));
        let (frontier_pk, _) = frontier_key();
        assert_eq!(placement_pk, profile.0);
        assert_eq!(placement_pk, limit.0);
        assert_eq!(profile.1, "PROFILE");
        assert_eq!(limit.1, "LIMIT#query.page");
        assert_ne!(placement_pk, revocation_pk);
        assert_ne!(placement_pk, frontier_pk);
        assert_ne!(revocation_pk, frontier_pk);
    }
}
