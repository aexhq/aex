//! The read-only `regional-authz-projection` reader.
//!
//! Placement, profile and key authorization rows are written by
//! `central-control-worker`; effective-limit rows, including the hot admission
//! subset, are reserved for the regional capacity authority. Every serving
//! regional role holds read actions on the table and nothing else, and this
//! module contains no write operation at all — a source conformance test
//! asserts that, because "read-only by convention" is not a property.
//!
//! [`AuthorizationProjection::read_admission_snapshot`] is the request path.
//! Everything else here is a cold or diagnostic read.
//!
//! It lives behind the `authz-projection` feature so `session-stream-api` and
//! regional readers can read a placement without linking the session row codec
//! (D-21).

use aex_internal_contracts::assertion::{AssertionAudience, AudienceSet};
use aex_wire::ids::{ApiKeyId, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::scopes::{ScopeId, ScopeSet};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;

use crate::attr::{CodecError, Item, Row};
use crate::error::{Idempotence, StoreError, classify};
use crate::paging::{PageBudget, PagePosition};
use crate::plan::key;
use crate::wire_pending::{
    AdmissionSnapshot, EdgeLimits, FeedFrontier, KeyAuthorization, KeyAuthorizationState,
    ProjectedLimitBundle, ProjectedLimitBundleHead, ProjectedWorkspaceLimit, WorkspacePlacement,
    WorkspaceProfile,
};

pub use crate::projection_limit::{
    WORKSPACE_EDGE_LIMITS, WORKSPACE_LIMIT, decode_limit, decode_limit_at, decode_limit_bundle,
    decode_limit_bundle_head, edge_limits_key, limit_bundle_head_key, limit_bundle_key, limit_key,
};

/// The `itemType` of a workspace placement.
pub const WORKSPACE_PLACEMENT: &str = "workspace_placement";
/// The `itemType` of a key authorization row.
pub const KEY_AUTHORIZATION: &str = "key_authorization";
/// The `itemType` of the signed feed frontier.
pub const FEED_FRONTIER: &str = "feed_frontier";
/// The `itemType` of descriptive workspace facts.
pub const WORKSPACE_PROFILE: &str = "workspace_profile";

/// `WS#{workspace_id}` / `PLACEMENT`.
#[must_use]
pub fn placement_key(workspace: WorkspaceId) -> (String, String) {
    let key = crate::keys::authorization_placement(workspace);
    (key.pk, key.sk)
}

/// `KEY#{api_key_id}` / `AUTHZ`.
#[must_use]
pub fn authorization_key(api_key: ApiKeyId) -> (String, String) {
    (format!("KEY#{api_key}"), "AUTHZ".to_owned())
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

    /// Reads one key's authorization row.
    ///
    /// # Errors
    ///
    /// [`StoreError::Misconfigured`] when this region projects no row for the
    /// key — which is what a forged or foreign key id looks like — and any other
    /// [`StoreError`] for a transport or decode failure. The two are different
    /// to a caller: the first is a refusal, the second an outage.
    async fn read_key_authorization(
        &self,
        api_key: ApiKeyId,
    ) -> Result<KeyAuthorization, StoreError>;

    /// Reads the key, placement and edge-limit rows together.
    ///
    /// Three concurrent point reads. They are not one snapshot: a revocation
    /// landing between them can produce a torn set, which `reconcile` refuses
    /// rather than admits, so the race costs one retried request and never an
    /// admission. The rows change only on operator-paced events, and the
    /// admission transaction's own `ConditionCheck` on the placement epochs is
    /// the commit-time fence in any case.
    ///
    /// `workspace` is the identity the presented credential *claims*. It selects
    /// which placement and limit rows are read; whether the claim is true is
    /// decided by comparing it against the key row this returns, which no
    /// caller may skip.
    ///
    /// # Errors
    ///
    /// [`StoreError::Misconfigured`] when a row the snapshot needs is absent, or
    /// when the three rows do not describe one consistent identity — both are
    /// refusals, never optimistic admissions. Any other [`StoreError`] for a
    /// transport or decode failure.
    async fn read_admission_snapshot(
        &self,
        api_key: ApiKeyId,
        workspace: WorkspaceId,
    ) -> Result<AdmissionSnapshot, StoreError>;

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

    /// Strongly reads the complete-set revision fence.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for absence, transport or strict decode failure.
    async fn read_limit_bundle_head(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ProjectedLimitBundleHead, StoreError>;

    /// Strongly reads the complete payload selected by the head.
    ///
    /// # Errors
    ///
    /// [`StoreError`] for absence, transport or strict decode failure.
    async fn read_limit_bundle(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ProjectedLimitBundle, StoreError>;
}

/// Which consistency one projection point read requires.
///
/// The split is deliberate and per call site, never a constructor default: the
/// two fence reads ([`AuthorizationProjection::read_frontier`] and
/// [`WorkspaceProjection::read_limit_bundle_head`], with the bundle payload the
/// head selects) must observe every acknowledged write, while the per-request
/// reads serve rows that change on placement or limit *changes* — rare,
/// operator-paced events — and accept replica lag of at most a second in
/// exchange for half the read cost and none of the strong read's
/// single-partition latency tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Consistency {
    /// A fence: the read must observe every acknowledged write.
    Strong,
    /// A per-request read of a rarely changing row: replica lag is accepted.
    Eventual,
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

    async fn get(
        &self,
        pk: &str,
        sk: &str,
        consistency: Consistency,
    ) -> Result<Option<Item>, StoreError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key(pk, sk)))
            .consistent_read(matches!(consistency, Consistency::Strong))
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        Ok(output.item)
    }

    /// "This region holds no usable record of that."
    ///
    /// One constructor for the whole fail-closed family so an absent row and a
    /// row that contradicts its siblings are indistinguishable to a caller —
    /// which is what stops the difference from being probeable.
    fn absent(&self) -> StoreError {
        StoreError::Misconfigured {
            table: self.table.clone(),
        }
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
            .get(&pk, &sk, Consistency::Eventual)
            .await?
            .ok_or_else(|| StoreError::Misconfigured {
                table: self.table.clone(),
            })?;
        Ok(decode_placement(&item, workspace)?)
    }

    async fn read_key_authorization(
        &self,
        api_key: ApiKeyId,
    ) -> Result<KeyAuthorization, StoreError> {
        let (pk, sk) = authorization_key(api_key);
        let item = self
            .get(&pk, &sk, Consistency::Eventual)
            .await?
            .ok_or_else(|| self.absent())?;
        Ok(decode_key_authorization(&item, api_key)?)
    }

    async fn read_admission_snapshot(
        &self,
        api_key: ApiKeyId,
        workspace: WorkspaceId,
    ) -> Result<AdmissionSnapshot, StoreError> {
        // Three concurrent eventually consistent point reads, not a
        // `TransactGetItems`: this is the hot admission path, and the
        // serializable read cost roughly doubled every request for rows that
        // change only on operator-paced placement, key or limit events. The
        // reads are no longer one snapshot, so a write landing between them can
        // produce a torn set - `reconcile` then refuses it (`absent()` ->
        // `Unauthenticated`) and the caller retries. A torn read is therefore a
        // transient refusal, never an admission the old sequence would have
        // denied; the accepted lag on any single row is bounded by the replica
        // horizon, per the projection-consistency charter.
        let (authorization_partition, authorization_sort) = authorization_key(api_key);
        let (placement_partition, placement_sort) = placement_key(workspace);
        let (limits_partition, limits_sort) = edge_limits_key(workspace);
        let (key_item, placement_item, limits_item) = futures::try_join!(
            self.get(
                &authorization_partition,
                &authorization_sort,
                Consistency::Eventual
            ),
            self.get(&placement_partition, &placement_sort, Consistency::Eventual),
            self.get(&limits_partition, &limits_sort, Consistency::Eventual),
        )?;
        let key_item = key_item.ok_or_else(|| self.absent())?;
        let placement_item = placement_item.ok_or_else(|| self.absent())?;
        let limits_item = limits_item.ok_or_else(|| self.absent())?;

        let key = decode_key_authorization(&key_item, api_key)?;
        let placement = decode_placement(&placement_item, workspace)?;
        let limits = decode_edge_limits(&limits_item, workspace)?;
        reconcile(key, placement, limits, workspace).ok_or_else(|| self.absent())
    }

    async fn read_frontier(&self) -> Result<FeedFrontier, StoreError> {
        let (pk, sk) = frontier_key();
        let item = self
            .get(&pk, &sk, Consistency::Strong)
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
        self.get(&pk, &sk, Consistency::Eventual)
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
        self.get(&pk, &sk, Consistency::Eventual)
            .await?
            .as_ref()
            .map(|item| decode_limit_at(item, workspace, limit).map_err(StoreError::from))
            .transpose()
    }

    async fn page_limits(
        &self,
        workspace: WorkspaceId,
        budget: PageBudget,
        after: Option<&PagePosition>,
    ) -> Result<ProjectionPage<ProjectedWorkspaceLimit>, StoreError> {
        let partition = format!("LIMIT#WS#{workspace}");
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
            // A descriptive listing, not a fence: the complete-set answer is
            // proved by `read_limit_bundle_head`, so this page accepts replica
            // lag like every other per-request projection read.
            .consistent_read(false)
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

    async fn read_limit_bundle_head(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ProjectedLimitBundleHead, StoreError> {
        let (pk, sk) = limit_bundle_head_key(workspace);
        let item = self
            .get(&pk, &sk, Consistency::Strong)
            .await?
            .ok_or_else(|| StoreError::Misconfigured {
                table: self.table.clone(),
            })?;
        Ok(decode_limit_bundle_head(&item, workspace)?)
    }

    async fn read_limit_bundle(
        &self,
        workspace: WorkspaceId,
    ) -> Result<ProjectedLimitBundle, StoreError> {
        let (pk, sk) = limit_bundle_key(workspace);
        // Eventual, alone, and safe to be both.
        //
        // The bundle is one item written in the same transaction as the members
        // and the head, so it can never be internally torn: a reader sees a
        // complete revision or the previous complete revision, never a mixture.
        // Pairing it with the strong head is what an admission *fence* needs —
        // proof of a specific revision — and the strong pair above still exists
        // for exactly that. A customer read of the values in force is
        // descriptive: it carries `revision` and `changedAt` so a caller can
        // always tell which authority revision it is holding, and paying a
        // strong read plus a second round trip to bound a fact that changes on
        // operator-paced events buys nothing anyone can use.
        let item = self
            .get(&pk, &sk, Consistency::Eventual)
            .await?
            .ok_or_else(|| StoreError::Misconfigured {
                table: self.table.clone(),
            })?;
        Ok(decode_limit_bundle(&item, workspace)?)
    }
}

/// Decodes cold descriptive workspace facts.
///
/// `accountRevision` and `accountChangedAt` are **strict**: a row written before
/// they existed fails to decode rather than projecting a partial account state.
/// That is the intent. A state assembled from a row that does not carry one
/// would be a guess, and the route that publishes it answers
/// `account_state_unavailable` for a refusal here rather than inventing `Active`.
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
        account_revision: row.u64("accountRevision")?,
        account_changed_at: row.timestamp("accountChangedAt")?,
        account_pause_reason: row.opt_string("accountPauseReason")?.map(str::to_owned),
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

/// Every key authorization state value, spelled by the type that owns them.
pub const KEY_AUTHORIZATION_STATES: &[&str] = &[
    KeyAuthorizationState::Active.as_str(),
    KeyAuthorizationState::Revoked.as_str(),
];

/// Decodes a key authorization row.
///
/// Every one of the four authentication attributes is required. There is no
/// arm that reads an absent verifier as "not yet replicated" or an absent
/// audience list as "any edge": a row this function cannot fully decode is
/// refused, which costs a `401` on a key nobody can check and never admits one.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute, a
/// verifier that is not exactly 32 bytes, an audience or scope spelling outside
/// its registry, an empty audience set, and [`CodecError::WrongTenant`] when the
/// row names a different key than the one that was read.
pub fn decode_key_authorization(
    item: &Item,
    asserted: ApiKeyId,
) -> Result<KeyAuthorization, CodecError> {
    let row = Row::bind(item, KEY_AUTHORIZATION)?;
    row.owned_by("apiKeyId", &asserted.to_string())?;
    let stored = row.enumerated("state", KEY_AUTHORIZATION_STATES)?;
    let state = KeyAuthorizationState::parse(stored).ok_or_else(|| CodecError::Malformed {
        item_type: KEY_AUTHORIZATION,
        attribute: "state",
        reason: format!("`{stored}` is outside the key authorization vocabulary"),
    })?;
    Ok(KeyAuthorization {
        api_key: asserted,
        workspace: row.id("workspaceId")?,
        organization: row.id("organizationId")?,
        region: row.string("region")?.to_owned(),
        state,
        verifier: row.fixed_bytes::<32>("verifier")?,
        pepper_version: pepper_version(&row)?,
        audiences: audiences(&row)?,
        scopes: scopes(&row)?,
        key_epoch: row.u64("keyEpoch")?,
        projection_sequence: row.u64("projectionSequence")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// The pepper version, narrowed onto the width the credential codec declares.
fn pepper_version(row: &Row<'_>) -> Result<u16, CodecError> {
    let stored = row.u64("pepperVersion")?;
    u16::try_from(stored).map_err(|_| CodecError::Malformed {
        item_type: KEY_AUTHORIZATION,
        attribute: "pepperVersion",
        reason: format!("`{stored}` is not a pepper version"),
    })
}

/// The audiences the key may be presented to.
///
/// A spelling outside the vocabulary is a corrupt row rather than a member to
/// drop, and an empty set is refused: a key row that names no edge would admit
/// nothing anywhere, which is indistinguishable from an unpopulated row.
fn audiences(row: &Row<'_>) -> Result<AudienceSet, CodecError> {
    let mut set = AudienceSet::EMPTY;
    for spelling in row.string_list("audiences")? {
        let audience = AssertionAudience::parse(spelling).ok_or_else(|| CodecError::Malformed {
            item_type: KEY_AUTHORIZATION,
            attribute: "audiences",
            reason: format!("`{spelling}` is outside the audience vocabulary"),
        })?;
        set = set.insert(audience);
    }
    if set.is_empty() {
        return Err(CodecError::Malformed {
            item_type: KEY_AUTHORIZATION,
            attribute: "audiences",
            reason: "a key row that names no audience admits nothing".to_owned(),
        });
    }
    Ok(set)
}

/// The effective scopes the key carries.
///
/// An unknown spelling is refused rather than skipped: silently dropping one
/// turns a key that should have been refused into a key with quietly fewer
/// scopes, and the difference only shows up as a `403` nobody can explain.
fn scopes(row: &Row<'_>) -> Result<ScopeSet, CodecError> {
    let mut scopes = Vec::new();
    for spelling in row.string_list("scopes")? {
        scopes.push(
            ScopeId::parse(spelling).ok_or_else(|| CodecError::Malformed {
                item_type: KEY_AUTHORIZATION,
                attribute: "scopes",
                reason: format!("`{spelling}` is not a scope in the registry"),
            })?,
        );
    }
    Ok(ScopeSet::new(scopes))
}

/// Proves three separately-decoded rows describe one consistent identity.
///
/// `None` is the whole fail-closed family. Every check here is a refusal rather
/// than a store fault: the workspace a credential names is a *claim* that
/// selected which rows were read, so a key row naming a different workspace, or
/// a placement naming a different organization, means the claim was wrong — not
/// that the region is sick. Collapsing them all to one answer is deliberate, so
/// which of them failed is not probeable.
///
/// Split out of the transaction so the safety-critical part is a pure function
/// with no client in the way.
#[must_use]
pub fn reconcile(
    key: KeyAuthorization,
    placement: WorkspacePlacement,
    limits: EdgeLimits,
    workspace: WorkspaceId,
) -> Option<AdmissionSnapshot> {
    let consistent = key.workspace == workspace
        && placement.workspace == workspace
        && limits.workspace == workspace
        && key.organization == placement.organization
        && limits.is_complete();
    consistent.then_some(AdmissionSnapshot {
        key,
        placement,
        limits,
    })
}

/// Decodes the hot admission limit subset.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or foreign-tenant attribute.
pub fn decode_edge_limits(item: &Item, asserted: WorkspaceId) -> Result<EdgeLimits, CodecError> {
    let row = Row::bind(item, WORKSPACE_EDGE_LIMITS)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(EdgeLimits {
        workspace: asserted,
        revision: row.u64("revision")?,
        json_body_bytes: row.u64("jsonBodyBytes")?,
        otlp_body_bytes: row.u64("otlpBodyBytes")?,
        query_page_items: row.u64("queryPageItems")?,
        query_page_bytes: row.u64("queryPageBytes")?,
        changed_at: row.timestamp("changedAt")?,
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
    use aex_wire::models::{LimitMapValue, LimitScalarValue, LimitSource, LimitValue};
    use aex_wire::types::DecimalU128;

    use super::{
        FEED_FRONTIER, KEY_AUTHORIZATION, WORKSPACE_EDGE_LIMITS, WORKSPACE_LIMIT,
        WORKSPACE_PLACEMENT, WORKSPACE_PROFILE, admits_execution, authorization_key,
        decode_edge_limits, decode_frontier, decode_key_authorization, decode_limit,
        decode_limit_at, decode_placement, decode_profile, edge_limits_key, frontier_key, guard,
        limit_key, placement_key, profile_key,
    };
    use crate::attr::{CodecError, ItemBuilder, n, s};
    use crate::wire_pending::KeyAuthorizationState;
    use aex_internal_contracts::assertion::{AssertionAudience, AudienceSet};
    use aex_wire::scopes::{ScopeId, ScopeSet};

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

    /// One well-formed key authorization row, with one attribute replaced.
    ///
    /// The overrides are how each authentication attribute is corrupted in
    /// isolation: the point of every case below is that exactly one thing is
    /// wrong, so an accepted row cannot be accepted for the wrong reason.
    fn key_row(
        state: &str,
        overrides: &[(&str, aws_sdk_dynamodb::types::AttributeValue)],
    ) -> crate::attr::Item {
        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let mut builder = ItemBuilder::new(KEY_AUTHORIZATION)
            .set("apiKeyId", s(api_key.to_string()))
            .set("workspaceId", s(workspace(1).to_string()))
            .set(
                "organizationId",
                s(
                    aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]))
                        .to_string(),
                ),
            )
            .set("region", s("eu-west-1"))
            .set("state", s(state))
            .set("verifier", crate::attr::b(vec![7; 32]))
            .set("pepperVersion", n(3))
            .set(
                "audiences",
                crate::attr::string_list(["regional_session".to_owned()]),
            )
            .set(
                "scopes",
                crate::attr::string_list(["sessions:read".to_owned()]),
            )
            .set("keyEpoch", n(2))
            .set("projectionSequence", n(9))
            .set("updatedAt", s("2026-08-01T00:00:00.000Z"));
        for (attribute, value) in overrides {
            builder = builder.set(attribute, value.clone());
        }
        builder.build()
    }

    #[test]
    fn a_key_authorization_and_a_frontier_decode() {
        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let decoded = decode_key_authorization(&key_row("revoked", &[]), api_key).expect("decodes");
        assert_eq!(decoded.api_key, api_key);
        assert_eq!(decoded.workspace, workspace(1));
        assert_eq!(decoded.state, KeyAuthorizationState::Revoked);
        assert_eq!(decoded.key_epoch, 2);
        assert_eq!(decoded.verifier, [7; 32]);
        assert_eq!(decoded.pepper_version, 3);
        assert_eq!(
            decoded.audiences,
            AudienceSet::EMPTY.insert(AssertionAudience::RegionalSession)
        );
        assert_eq!(decoded.scopes, ScopeSet::new([ScopeId::SessionsRead]));

        assert!(matches!(
            decode_key_authorization(&key_row("suspended", &[]), api_key),
            Err(CodecError::Malformed { .. })
        ));

        let item = ItemBuilder::new(FEED_FRONTIER)
            .set("sequence", n(9))
            .set("signedAt", s("2026-08-01T00:00:00.000Z"))
            .set("signatureKeyId", s("key-1"))
            .set("coveredThrough", s("2026-08-01T00:00:00.000Z"))
            .build();
        assert_eq!(decode_frontier(&item).expect("decodes").sequence, 9);
    }

    #[test]
    fn every_unusable_authentication_attribute_refuses_the_row() {
        // The whole point of replicating the verifier is that a regional edge
        // authenticates from this row. A row it can only partly decode must be
        // refused, because every alternative — an absent verifier read as "check
        // later", an absent audience list read as "any edge", an unknown scope
        // dropped — admits something nobody published.
        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        for (name, attribute, value) in [
            (
                "a verifier of the wrong width",
                "verifier",
                crate::attr::b(vec![7; 31]),
            ),
            (
                "a verifier stored as text",
                "verifier",
                s(aex_wire::types::Region::EuWest1.as_str()),
            ),
            ("a pepper version outside `u16`", "pepperVersion", n(70_000)),
            (
                "an audience outside the vocabulary",
                "audiences",
                crate::attr::string_list(["regional-session".to_owned()]),
            ),
            (
                "no audience at all",
                "audiences",
                crate::attr::string_list([]),
            ),
            (
                "an audience list that is not a list",
                "audiences",
                s("regional_session"),
            ),
            (
                "a scope outside the registry",
                "scopes",
                crate::attr::string_list(["sessions:teleport".to_owned()]),
            ),
        ] {
            assert!(
                decode_key_authorization(&key_row("active", &[(attribute, value)]), api_key)
                    .is_err(),
                "{name} was accepted"
            );
        }

        // A key that legitimately carries no scope still decodes: the scope gate
        // refuses it at the edge, which is a `403` naming the missing scope
        // rather than an undifferentiated `401`.
        let none = decode_key_authorization(
            &key_row("active", &[("scopes", crate::attr::string_list([]))]),
            api_key,
        )
        .expect("an empty scope set is a decision, not a corrupt row");
        assert!(none.scopes.is_empty());
    }

    /// The account fields on the profile row are required, not optional.
    ///
    /// A row written before they existed must refuse rather than decode into a
    /// partial account state, because the route that publishes it would then be
    /// choosing what an absent revision or change instant means — and the only
    /// answers available are a guess and `Active`. Refusing produces
    /// `account_state_unavailable`, which is the true statement.
    #[test]
    fn a_profile_row_without_the_account_fields_refuses_rather_than_half_decoding() {
        for attribute in ["accountRevision", "accountChangedAt"] {
            let mut item = ItemBuilder::new(WORKSPACE_PROFILE)
                .set("workspaceId", s(workspace(1).to_string()))
                .set("name", s("Production"))
                .set("slug", s("production"))
                .set("createdAt", s("2026-08-01T00:00:00.000Z"))
                .set("accountRevision", n(9))
                .set("accountChangedAt", s("2026-08-02T00:00:00.000Z"))
                .build();
            item.remove(attribute);
            assert!(
                matches!(
                    decode_profile(&item, workspace(1)),
                    Err(CodecError::Missing { .. })
                ),
                "an absent `{attribute}` must refuse the row"
            );
        }
    }

    #[test]
    fn every_authentication_attribute_is_required() {
        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        for attribute in ["verifier", "pepperVersion", "audiences", "scopes"] {
            let mut item = key_row("active", &[]);
            item.remove(attribute);
            assert!(
                matches!(
                    decode_key_authorization(&item, api_key),
                    Err(CodecError::Missing { .. })
                ),
                "an absent `{attribute}` must refuse the row"
            );
        }
    }

    #[test]
    fn profile_and_effective_limits_are_separate_typed_rows() {
        let profile = ItemBuilder::new(WORKSPACE_PROFILE)
            .set("workspaceId", s(workspace(1).to_string()))
            .set("name", s("Production"))
            .set("slug", s("production"))
            .set("createdAt", s("2026-08-01T00:00:00.000Z"))
            .set("accountRevision", n(9))
            .set("accountChangedAt", s("2026-08-02T00:00:00.000Z"))
            .build();
        let decoded = decode_profile(&profile, workspace(1)).expect("profile");
        assert_eq!(decoded.name, "Production");
        assert_eq!(decoded.account_revision, 9);
        assert_eq!(
            decoded.account_pause_reason, None,
            "finance publishes a reason exactly when the account is paused, so an              absent one is an active account rather than a missing field"
        );

        let value = LimitValue::Map(LimitMapValue {
            values: std::collections::BTreeMap::from([
                ("items".to_owned(), DecimalU128::new(100)),
                (
                    "serialized_bytes".to_owned(),
                    DecimalU128::new(8 * 1_024 * 1_024),
                ),
            ]),
        });
        let limit = ItemBuilder::new(WORKSPACE_LIMIT)
            .set("pk", s(format!("LIMIT#WS#{}", workspace(1))))
            .set("sk", s("LIMIT#query.page"))
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
        let value = LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(100),
        });
        let limit = ItemBuilder::new(WORKSPACE_LIMIT)
            .set("pk", s(format!("LIMIT#WS#{}", workspace(1))))
            .set("sk", s("LIMIT#query.page"))
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
    fn the_authority_key_shapes_are_disjoint() {
        let (placement_pk, _) = placement_key(workspace(1));
        let profile = profile_key(workspace(1));
        let limit = limit_key(workspace(1), LimitId::QueryPage);
        let edge = edge_limits_key(workspace(1));
        let authorization = authorization_key(ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10])));
        let (frontier_pk, _) = frontier_key();
        assert_eq!(placement_pk, profile.0);
        assert_ne!(placement_pk, limit.0);
        assert!(limit.0.starts_with("LIMIT#"));
        assert_eq!(profile.1, "PROFILE");
        assert_eq!(limit.1, "LIMIT#query.page");
        assert_ne!(placement_pk, authorization.0);
        assert_eq!(authorization.1, "AUTHZ");
        assert_ne!(placement_pk, frontier_pk);
        assert_ne!(authorization.0, frontier_pk);
        // The hot limit subset stays inside the capacity authority's partition
        // fence, which is what keeps it out of the placement writer's reach.
        assert!(edge.0.starts_with("LIMIT#"));
        assert_ne!(edge.0, placement_pk);
        assert_eq!(edge.1, "EDGE_LIMITS");
    }

    #[test]
    fn every_cross_row_identity_mismatch_fails_the_snapshot_closed() {
        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let organization = aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let stamp = aex_wire::types::Timestamp::from_unix_millis(1_000).expect("timestamp");
        let key = |workspace, organization| crate::wire_pending::KeyAuthorization {
            api_key,
            workspace,
            organization,
            region: "eu-west-1".to_owned(),
            state: KeyAuthorizationState::Active,
            verifier: [7; 32],
            pepper_version: 1,
            audiences: AudienceSet::ALL,
            scopes: ScopeSet::new([ScopeId::SessionsRead]),
            key_epoch: 1,
            projection_sequence: 1,
            updated_at: stamp,
        };
        let placement = |workspace, organization| crate::wire_pending::WorkspacePlacement {
            workspace,
            organization,
            plane: "regional".to_owned(),
            region: "eu-west-1".to_owned(),
            status: "active".to_owned(),
            key_epoch: 1,
            account_epoch: 1,
            revocation_epoch: 1,
            feed_sequence: 1,
            updated_at: stamp,
        };
        let limits = |workspace, json| crate::wire_pending::EdgeLimits {
            workspace,
            revision: 1,
            json_body_bytes: json,
            otlp_body_bytes: 1,
            query_page_items: 1,
            query_page_bytes: 1,
            changed_at: stamp,
        };
        let mine = workspace(1);
        let theirs = workspace(9);
        let other_org = aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(1, [5; 10]));

        assert!(
            super::reconcile(
                key(mine, organization),
                placement(mine, organization),
                limits(mine, 1),
                mine
            )
            .is_some(),
            "a consistent set is the only accepted one"
        );

        for (name, key, placement, limits) in [
            (
                "the key row names another workspace",
                key(theirs, organization),
                placement(mine, organization),
                limits(mine, 1),
            ),
            (
                "the placement names another workspace",
                key(mine, organization),
                placement(theirs, organization),
                limits(mine, 1),
            ),
            (
                "the limits name another workspace",
                key(mine, organization),
                placement(mine, organization),
                limits(theirs, 1),
            ),
            (
                "the key row and the placement disagree about the organization",
                key(mine, other_org),
                placement(mine, organization),
                limits(mine, 1),
            ),
            (
                "a ceiling is zero",
                key(mine, organization),
                placement(mine, organization),
                limits(mine, 0),
            ),
        ] {
            assert!(
                super::reconcile(key, placement, limits, mine).is_none(),
                "{name} was admitted"
            );
        }
    }

    #[test]
    fn the_hot_limit_subset_decodes_and_a_zero_ceiling_is_incomplete() {
        let row = |json: u64| {
            ItemBuilder::new(WORKSPACE_EDGE_LIMITS)
                .set("pk", s(format!("LIMIT#WS#{}", workspace(1))))
                .set("sk", s("EDGE_LIMITS"))
                .set("workspaceId", s(workspace(1).to_string()))
                .set("revision", n(4))
                .set("jsonBodyBytes", n(json))
                .set("otlpBodyBytes", n(4 * 1_024 * 1_024))
                .set("queryPageItems", n(1_000))
                .set("queryPageBytes", n(8 * 1_024 * 1_024))
                .set("changedAt", s("2026-08-01T00:00:00.000Z"))
                .build()
        };
        let decoded = decode_edge_limits(&row(65_536), workspace(1)).expect("decodes");
        assert_eq!(decoded.revision, 4);
        assert_eq!(decoded.json_body_bytes, 65_536);
        assert!(decoded.is_complete());

        assert!(
            !decode_edge_limits(&row(0), workspace(1))
                .expect("decodes")
                .is_complete(),
            "a zero ceiling admits nothing and is refused rather than enforced"
        );
        assert!(matches!(
            decode_edge_limits(&row(65_536), workspace(9)),
            Err(CodecError::WrongTenant { .. })
        ));
    }

    #[test]
    fn a_limit_point_read_rejects_identity_that_disagrees_with_the_requested_key() {
        let value = LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(100),
        });
        let corrupt = ItemBuilder::new(WORKSPACE_LIMIT)
            .set("pk", s(format!("LIMIT#WS#{}", workspace(1))))
            .set("sk", s("LIMIT#query.page"))
            .set("workspaceId", s(workspace(1).to_string()))
            .set("limitId", s(LimitId::ApiJsonBody.as_str()))
            .set(
                "effectiveValue",
                s(serde_json::to_string(&value).expect("json")),
            )
            .set("source", s("default"))
            .set("revision", n(1))
            .set("changedAt", s("2026-08-01T00:00:00.000Z"))
            .build();
        assert!(matches!(
            decode_limit_at(&corrupt, workspace(1), LimitId::QueryPage),
            Err(CodecError::Malformed { .. })
        ));
    }
}
