//! Central control's write-only regional authorization projection adapter.
//!
//! `central-control-worker` enables this module for placement, profile and
//! revocation publication. Effective-limit construction is deliberately absent:
//! it belongs to the separately featured regional capacity producer. Every
//! write here is monotone, so delayed cross-region delivery cannot roll central
//! control state backwards.

use aex_internal_contracts::assertion::AudienceSet;
use aex_wire::ids::{ApiKeyId, OrganizationId, WorkspaceId};
use aex_wire::scopes::ScopeSet;
use aex_wire::types::{Region, Timestamp};
use aws_sdk_dynamodb::Client;

use crate::attr::{Item, ItemBuilder, b, n, s, stamp, string_list};
use crate::wire_pending::KeyAuthorizationState;

const WORKSPACE_PLACEMENT: &str = "workspace_placement";
const WORKSPACE_PROFILE: &str = "workspace_profile";
const KEY_AUTHORIZATION: &str = "key_authorization";

/// The monotone fence a key authorization publication must satisfy.
///
/// `(keyEpoch, projectionSequence)`, in that order, rather than either alone:
/// creation and revocation arrive as separate outbox messages, so a delayed
/// creation must lose to a revocation that already raised the epoch, and two
/// publications at the same epoch must still order by position.
const KEY_AUTHORIZATION_CONDITION: &str = "attribute_not_exists(#pk) OR #epoch < :epoch \
     OR (#epoch = :epoch AND #sequence <= :sequence)";

/// A complete workspace placement projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementWrite {
    /// Workspace being projected.
    pub workspace: WorkspaceId,
    /// Owning organization.
    pub organization: OrganizationId,
    /// Immutable placement region.
    pub region: Region,
    /// Active, paused, or deleting.
    pub status: String,
    /// Current key epoch floor.
    pub key_epoch: u64,
    /// Current account epoch floor.
    pub account_epoch: u64,
    /// Current workspace revocation floor.
    pub revocation_epoch: u64,
    /// Monotone feed position.
    pub feed_sequence: u64,
    /// Projection timestamp.
    pub updated_at: Timestamp,
}

/// Descriptive workspace facts kept off the hot placement row.
///
/// The three account fields ride here rather than on the placement row for one
/// reason: placement is read on **every** request and never cached, while this
/// row is read only by the cold routes that publish the detail. Central control
/// already holds the account projection in the same `WorkspaceView` it uses to
/// write both rows, so carrying them costs no additional read.
///
/// The placement row keeps its three-value `status` and its `accountEpoch`,
/// which is all the hot admission path gates on. What is here is detail *about*
/// that gate, never a second copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileWrite {
    /// Workspace being described.
    pub workspace: WorkspaceId,
    /// Display name.
    pub name: String,
    /// Stable URL-safe slug.
    pub slug: String,
    /// Creation instant.
    pub created_at: Timestamp,
    /// Finance's monotone account revision, carried by copy so a stale reader
    /// can always say which revision it is holding.
    pub account_revision: u64,
    /// When finance last changed the account state. Not the projection write
    /// instant, which is a different fact and lives on the placement row.
    pub account_changed_at: Timestamp,
    /// The durable pause reason, present exactly when the account is paused.
    pub account_pause_reason: Option<String>,
}

/// A monotone API-key authorization projection.
///
/// The same row is published when the key is created and again when it is
/// revoked. A revocation therefore raises an existing row rather than creating
/// the only row a region ever had for that key, which is what lets an absent row
/// mean "no such key here" instead of "not revoked yet".
///
/// The four authentication fields — verifier, pepper version, audiences and
/// scopes — are republished on **every** publication rather than only on
/// creation, because a revocation publication overwrites the whole row and a
/// partial write would leave a live key without the material to check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyAuthorizationWrite {
    /// The key.
    pub api_key: ApiKeyId,
    /// The workspace it authorizes.
    pub workspace: WorkspaceId,
    /// The organization that owns that workspace.
    pub organization: OrganizationId,
    /// The region it is pinned to.
    pub region: Region,
    /// Whether it may still authorize.
    pub state: KeyAuthorizationState,
    /// The stored keyed verifier, replicated verbatim from the control plane.
    pub verifier: [u8; 32],
    /// Which pepper version the verifier was computed under.
    pub pepper_version: u16,
    /// The regional edges the key may be presented to.
    pub audiences: AudienceSet,
    /// The effective scopes, computed centrally.
    pub scopes: ScopeSet,
    /// Monotone key epoch.
    pub key_epoch: u64,
    /// Monotone projection position.
    pub projection_sequence: u64,
    /// Projection timestamp.
    pub updated_at: Timestamp,
}

/// One region's projection writer.
#[derive(Debug, Clone)]
pub struct ProjectionWriter {
    client: Client,
    table: String,
}

impl ProjectionWriter {
    /// Binds one regional projection table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// Performs a strongly consistent point read for startup readiness.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport diagnostic when the table cannot be read.
    pub async fn probe(&self) -> Result<(), String> {
        self.client
            .get_item()
            .table_name(&self.table)
            .key("pk", s("FEED"))
            .key("sk", s("FRONTIER"))
            .consistent_read(true)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Publishes a placement if it is at least as new as the stored feed row.
    ///
    /// # Errors
    ///
    /// Returns a diagnostic for an invalid status or failed write.
    pub async fn put_placement(&self, write: &PlacementWrite) -> Result<(), String> {
        if !matches!(write.status.as_str(), "active" | "paused" | "deleting") {
            return Err("placement status is outside the closed vocabulary".to_owned());
        }
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(placement_item(write)))
            .condition_expression("attribute_not_exists(#pk) OR #sequence <= :sequence")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#sequence", "feedSequence")
            .expression_attribute_values(":sequence", n(write.feed_sequence))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            // A newer projection already won; this stale delivery is fully
            // handled and must not poison the queue batch.
            Err(error) if conditional_put(&error) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Publishes the current descriptive profile.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport diagnostic when the write fails.
    pub async fn put_profile(&self, write: &ProfileWrite) -> Result<(), String> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(profile_item(write)))
            .send()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Publishes a key authorization row if it does not move the row backwards.
    ///
    /// The fence is `(keyEpoch, projectionSequence)` rather than either alone:
    /// creation and revocation are published from different outbox messages, and
    /// a delayed creation must never overwrite a revocation that already raised
    /// the epoch.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport diagnostic when the write fails.
    pub async fn put_key_authorization(&self, write: &KeyAuthorizationWrite) -> Result<(), String> {
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(key_authorization_item(write)))
            .condition_expression(KEY_AUTHORIZATION_CONDITION)
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#epoch", "keyEpoch")
            .expression_attribute_names("#sequence", "projectionSequence")
            .expression_attribute_values(":epoch", n(write.key_epoch))
            .expression_attribute_values(":sequence", n(write.projection_sequence))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) if conditional_put(&error) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

fn conditional_put<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::put_item::PutItemError,
        R,
    >,
) -> bool {
    error.as_service_error().is_some_and(|service| {
        matches!(
            service,
            aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
        )
    })
}

fn placement_item(write: &PlacementWrite) -> Item {
    ItemBuilder::new(WORKSPACE_PLACEMENT)
        .set("pk", s(format!("WS#{}", write.workspace)))
        .set("sk", s("PLACEMENT"))
        .set("workspaceId", s(write.workspace.to_string()))
        .set("organizationId", s(write.organization.to_string()))
        .set("plane", s("regional"))
        .set("region", s(write.region.as_str()))
        .set("status", s(write.status.clone()))
        .set("keyEpoch", n(write.key_epoch))
        .set("accountEpoch", n(write.account_epoch))
        .set("revocationEpoch", n(write.revocation_epoch))
        .set("feedSequence", n(write.feed_sequence))
        .set("updatedAt", stamp(write.updated_at))
        .build()
}

fn profile_item(write: &ProfileWrite) -> Item {
    let mut item = ItemBuilder::new(WORKSPACE_PROFILE)
        .set("pk", s(format!("WS#{}", write.workspace)))
        .set("sk", s("PROFILE"))
        .set("workspaceId", s(write.workspace.to_string()))
        .set("name", s(write.name.clone()))
        .set("slug", s(write.slug.clone()))
        .set("createdAt", stamp(write.created_at))
        .set("accountRevision", n(write.account_revision))
        .set("accountChangedAt", stamp(write.account_changed_at));
    // Absent rather than empty when the account is active: an absent reason and
    // a reason spelled `""` would decode the same way, and one of them is a
    // paused account whose remedy was lost.
    if let Some(reason) = &write.account_pause_reason {
        item = item.set("accountPauseReason", s(reason.clone()));
    }
    item.build()
}

fn key_authorization_item(write: &KeyAuthorizationWrite) -> Item {
    ItemBuilder::new(KEY_AUTHORIZATION)
        .set("pk", s(format!("KEY#{}", write.api_key)))
        .set("sk", s("AUTHZ"))
        .set("apiKeyId", s(write.api_key.to_string()))
        .set("workspaceId", s(write.workspace.to_string()))
        .set("organizationId", s(write.organization.to_string()))
        .set("region", s(write.region.as_str()))
        .set("state", s(write.state.as_str()))
        .set("verifier", b(write.verifier.to_vec()))
        .set("pepperVersion", n(u64::from(write.pepper_version)))
        .set("audiences", string_list(write.audiences.to_strings()))
        .set(
            "scopes",
            string_list(
                write
                    .scopes
                    .as_slice()
                    .iter()
                    .map(|scope| scope.as_str().to_owned()),
            ),
        )
        .set("keyEpoch", n(write.key_epoch))
        .set("projectionSequence", n(write.projection_sequence))
        .set("updatedAt", stamp(write.updated_at))
        .build()
}

#[cfg(test)]
mod tests {
    use super::{
        KEY_AUTHORIZATION_CONDITION, KeyAuthorizationWrite, PlacementWrite, ProfileWrite,
        key_authorization_item, placement_item, profile_item,
    };
    use crate::projection::{decode_key_authorization, decode_placement, decode_profile};
    use crate::wire_pending::KeyAuthorizationState;
    use aex_internal_contracts::assertion::{AssertionAudience, AudienceSet};
    use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::scopes::{ScopeId, ScopeSet};
    use aex_wire::types::{Region, Timestamp};

    #[test]
    fn writer_rows_are_exactly_the_rows_the_regional_reader_accepts() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let now = Timestamp::from_unix_millis(1_000).expect("timestamp");
        let placement = PlacementWrite {
            workspace,
            organization,
            region: Region::EuWest1,
            status: "active".to_owned(),
            key_epoch: 3,
            account_epoch: 4,
            revocation_epoch: 5,
            feed_sequence: 6,
            updated_at: now,
        };
        let decoded = decode_placement(&placement_item(&placement), workspace).expect("reader");
        assert_eq!(decoded.organization, organization);
        assert_eq!(decoded.feed_sequence, 6);

        let profile = ProfileWrite {
            workspace,
            name: "Production".to_owned(),
            slug: "production".to_owned(),
            created_at: now,
            account_revision: 1,
            account_changed_at: now,
            account_pause_reason: None,
        };
        let decoded = decode_profile(&profile_item(&profile), workspace).expect("profile reader");
        assert_eq!(decoded.name, "Production");

        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let audiences = AudienceSet::EMPTY
            .insert(AssertionAudience::RegionalSession)
            .insert(AssertionAudience::RegionalStream);
        let scopes = ScopeSet::new([ScopeId::SessionsRead, ScopeId::TelemetryWrite]);
        for state in KeyAuthorizationState::ALL {
            let authorization = KeyAuthorizationWrite {
                api_key,
                workspace,
                organization,
                region: Region::EuWest1,
                state,
                verifier: [23; 32],
                pepper_version: 4,
                audiences,
                scopes: scopes.clone(),
                key_epoch: 7,
                projection_sequence: 8,
                updated_at: now,
            };
            let decoded =
                decode_key_authorization(&key_authorization_item(&authorization), api_key)
                    .expect("reader");
            assert_eq!(decoded.workspace, workspace);
            assert_eq!(decoded.organization, organization);
            assert_eq!(decoded.state, state);
            assert_eq!(decoded.key_epoch, 7);
            assert_eq!(decoded.projection_sequence, 8);
            // The four authentication fields survive a revocation publication,
            // which overwrites the whole row rather than amending it.
            assert_eq!(decoded.verifier, [23; 32]);
            assert_eq!(decoded.pepper_version, 4);
            assert_eq!(decoded.audiences, audiences);
            assert_eq!(decoded.scopes, scopes);
        }

        // A row published for one key never decodes as another's.
        let other = ApiKeyId::from_uuid7(Uuid7::compose(1, [4; 10]));
        let authorization = KeyAuthorizationWrite {
            api_key,
            workspace,
            organization,
            region: Region::EuWest1,
            state: KeyAuthorizationState::Active,
            verifier: [23; 32],
            pepper_version: 1,
            audiences,
            scopes,
            key_epoch: 1,
            projection_sequence: 1,
            updated_at: now,
        };
        assert!(
            decode_key_authorization(&key_authorization_item(&authorization), other).is_err(),
            "a key authorization row is bound to the key it names"
        );
    }

    #[test]
    fn a_published_row_never_renders_the_verifier_as_text() {
        // The verifier is stored as binary rather than as a base64 string so it
        // cannot be copied out of a console listing or a log line by eye, and so
        // a codec cannot accept a second spelling of the same 32 bytes.
        let item = key_authorization_item(&KeyAuthorizationWrite {
            api_key: ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            region: Region::EuWest1,
            state: KeyAuthorizationState::Active,
            verifier: [23; 32],
            pepper_version: 1,
            audiences: AudienceSet::ALL,
            scopes: ScopeSet::new([ScopeId::SessionsRead]),
            key_epoch: 1,
            projection_sequence: 1,
            updated_at: Timestamp::from_unix_millis(1_000).expect("timestamp"),
        });
        assert!(
            item.get("verifier")
                .expect("the row carries a verifier")
                .as_b()
                .is_ok(),
            "the verifier must be a binary attribute"
        );
    }

    #[test]
    fn a_key_authorization_publication_can_only_move_the_row_forward() {
        // A delayed creation must never overwrite a revocation that already
        // raised the epoch, so the fence names both ordering terms.
        for required in [
            "attribute_not_exists(#pk)",
            "#epoch < :epoch",
            "#epoch = :epoch AND #sequence <= :sequence",
        ] {
            assert!(
                KEY_AUTHORIZATION_CONDITION.contains(required),
                "the fence omitted `{required}`: {KEY_AUTHORIZATION_CONDITION}"
            );
        }
    }
}
