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
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;

use crate::attr::{CodecError, Item, Row};
use crate::error::{Idempotence, StoreError, classify};
use crate::plan::key;
use crate::wire_pending::{FeedFrontier, KeyRevocation, WorkspacePlacement};

/// The `itemType` of a workspace placement.
pub const WORKSPACE_PLACEMENT: &str = "workspace_placement";
/// The `itemType` of a key revocation.
pub const KEY_REVOCATION: &str = "key_revocation";
/// The `itemType` of the signed feed frontier.
pub const FEED_FRONTIER: &str = "feed_frontier";

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

/// Every placement status value.
pub const PLACEMENT_STATUSES: &[&str] = &["active", "paused", "deleting"];

// TODO(cross-stream): replaced by aex_workspace_domain::AuthorizationProjection
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

    use super::{
        FEED_FRONTIER, KEY_REVOCATION, WORKSPACE_PLACEMENT, admits_execution, decode_frontier,
        decode_placement, decode_revocation, frontier_key, guard, placement_key, revocation_key,
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
    fn the_three_key_shapes_are_disjoint() {
        let (placement_pk, _) = placement_key(workspace(1));
        let (revocation_pk, _) = revocation_key(ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10])));
        let (frontier_pk, _) = frontier_key();
        assert_ne!(placement_pk, revocation_pk);
        assert_ne!(placement_pk, frontier_pk);
        assert_ne!(revocation_pk, frontier_pk);
    }
}
