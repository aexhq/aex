//! Write-only adapters for the regional authorization projection.
//!
//! `central-control-worker` currently enables this module for placement,
//! profile and revocation publication. The effective-limit method is the typed
//! producer seam for the separately owned regional capacity authority; it owns
//! no default or override policy itself. Every write is monotone, so delayed
//! cross-region delivery cannot roll authority state backwards.

use aex_wire::ids::{ApiKeyId, OrganizationId, WorkspaceId};
use aex_wire::limits::LimitId;
use aex_wire::models::{LimitSource, LimitValue};
use aex_wire::types::{Region, Timestamp};
use aws_sdk_dynamodb::Client;

use crate::attr::{Item, ItemBuilder, n, s, stamp};
use crate::projection_limit::WORKSPACE_LIMIT;
use crate::wire_pending::ProjectedWorkspaceLimit;

const WORKSPACE_PLACEMENT: &str = "workspace_placement";
const WORKSPACE_PROFILE: &str = "workspace_profile";
const KEY_REVOCATION: &str = "key_revocation";

const LIMIT_WRITE_CONDITION: &str = "attribute_not_exists(#pk) OR #revision < :revision OR (#revision = :revision AND #value = :value AND #source = :source AND #changed_at = :changed_at)";

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
}

/// One complete effective-limit projection selected by its owning capacity
/// authority.
///
/// This type is deliberately a producer contract, not a source of defaults.
/// Callers must supply a durable effective value and provenance; this adapter
/// never infers either from the generated registry.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitWrite {
    /// Workspace whose admission is governed.
    pub workspace: WorkspaceId,
    /// Registered limit identity.
    pub id: LimitId,
    /// Complete effective value.
    pub effective_value: LimitValue,
    /// Whether the authority selected its default or an approved override.
    pub source: LimitSource,
    /// Monotonic authority revision.
    pub revision: u64,
    /// When the authority changed this effective record.
    pub changed_at: Timestamp,
}

/// A monotone API-key revocation projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationWrite {
    /// Revoked key.
    pub api_key: ApiKeyId,
    /// Revocation instant.
    pub revoked_at: Timestamp,
    /// Monotone key epoch.
    pub epoch: u64,
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

    /// Publishes one effective limit without allowing delayed delivery or an
    /// equal-revision conflict to replace the durable answer.
    ///
    /// A higher stored revision makes this delivery stale and therefore
    /// complete. The same revision is an idempotent replay only when every
    /// projected fact is identical. A transport-ambiguous response is resolved
    /// by a strongly consistent point read; it is never blindly retried.
    ///
    /// # Errors
    ///
    /// Returns a diagnostic for a value whose shape disagrees with the generated
    /// registry, an equal-revision conflict, or a failed write/read resolution.
    pub async fn put_limit(&self, write: &LimitWrite) -> Result<(), String> {
        let item = limit_item(write)?;
        let value = serde_json::to_string(&write.effective_value)
            .map_err(|_| "effective limit could not be encoded".to_owned())?;
        let source = write.source.as_str();
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression(LIMIT_WRITE_CONDITION)
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#revision", "revision")
            .expression_attribute_names("#value", "effectiveValue")
            .expression_attribute_names("#source", "source")
            .expression_attribute_names("#changed_at", "changedAt")
            .expression_attribute_values(":revision", n(write.revision))
            .expression_attribute_values(":value", s(value))
            .expression_attribute_values(":source", s(source))
            .expression_attribute_values(":changed_at", stamp(write.changed_at))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) => match self.resolve_limit_write(write).await {
                Ok(()) => Ok(()),
                Err(resolution) if conditional_put(&error) => Err(resolution),
                Err(_) => Err(error.to_string()),
            },
        }
    }

    async fn resolve_limit_write(&self, write: &LimitWrite) -> Result<(), String> {
        let (pk, sk) = crate::projection_limit::limit_key(write.workspace, write.id);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|_| "effective limit outcome could not be resolved".to_owned())?;
        let current = output
            .item
            .as_ref()
            .map(|item| crate::projection_limit::decode_limit(item, write.workspace))
            .transpose()
            .map_err(|_| "effective limit outcome could not be resolved".to_owned())?;
        classify_limit_replay(current.as_ref(), write)
    }

    /// Publishes a revocation if its epoch does not move the floor backwards.
    ///
    /// # Errors
    ///
    /// Returns a redacted transport diagnostic when the write fails.
    pub async fn put_revocation(&self, write: &RevocationWrite) -> Result<(), String> {
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(revocation_item(write)))
            .condition_expression("attribute_not_exists(#pk) OR #epoch <= :epoch")
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#epoch", "revokedEpoch")
            .expression_attribute_values(":epoch", n(write.epoch))
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
    ItemBuilder::new(WORKSPACE_PROFILE)
        .set("pk", s(format!("WS#{}", write.workspace)))
        .set("sk", s("PROFILE"))
        .set("workspaceId", s(write.workspace.to_string()))
        .set("name", s(write.name.clone()))
        .set("slug", s(write.slug.clone()))
        .set("createdAt", stamp(write.created_at))
        .build()
}

fn limit_item(write: &LimitWrite) -> Result<Item, String> {
    if write.effective_value.shape() != write.id.shape() {
        return Err(format!(
            "effective limit value shape {:?} differs from registered shape {:?}",
            write.effective_value.shape(),
            write.id.shape()
        ));
    }
    let value = serde_json::to_string(&write.effective_value)
        .map_err(|_| "effective limit could not be encoded".to_owned())?;
    Ok(ItemBuilder::new(WORKSPACE_LIMIT)
        .set("pk", s(format!("WS#{}", write.workspace)))
        .set("sk", s(format!("LIMIT#{}", write.id.as_str())))
        .set("workspaceId", s(write.workspace.to_string()))
        .set("limitId", s(write.id.as_str()))
        .set("effectiveValue", s(value))
        .set("source", s(write.source.as_str()))
        .set("revision", n(write.revision))
        .set("changedAt", stamp(write.changed_at))
        .build())
}

fn classify_limit_replay(
    current: Option<&ProjectedWorkspaceLimit>,
    desired: &LimitWrite,
) -> Result<(), String> {
    let Some(current) = current else {
        return Err("effective limit write did not become durable".to_owned());
    };
    if current.workspace != desired.workspace || current.id != desired.id {
        return Err("effective limit point read returned a foreign record".to_owned());
    }
    if current.revision > desired.revision {
        return Ok(());
    }
    if current.effective_value == desired.effective_value
        && current.source == desired.source
        && current.revision == desired.revision
        && current.changed_at == desired.changed_at
    {
        return Ok(());
    }
    Err("effective limit revision conflicts with the durable projection".to_owned())
}

fn revocation_item(write: &RevocationWrite) -> Item {
    ItemBuilder::new(KEY_REVOCATION)
        .set("pk", s(format!("KEY#{}", write.api_key)))
        .set("sk", s("REVOCATION"))
        .set("apiKeyId", s(write.api_key.to_string()))
        .set("revokedAt", stamp(write.revoked_at))
        .set("revokedEpoch", n(write.epoch))
        .build()
}

#[cfg(test)]
mod tests {
    use super::{
        LimitWrite, PlacementWrite, ProfileWrite, RevocationWrite, classify_limit_replay,
        limit_item, placement_item, profile_item, revocation_item,
    };
    use crate::projection::{decode_placement, decode_profile, decode_revocation};
    use crate::projection_limit::decode_limit;
    use crate::wire_pending::ProjectedWorkspaceLimit;
    use aex_wire::ids::{ApiKeyId, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{LimitMapValue, LimitScalarValue, LimitSource, LimitValue};
    use aex_wire::types::DecimalU128;
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
        };
        let decoded = decode_profile(&profile_item(&profile), workspace).expect("profile reader");
        assert_eq!(decoded.name, "Production");

        let api_key = ApiKeyId::from_uuid7(Uuid7::compose(1, [3; 10]));
        let revocation = RevocationWrite {
            api_key,
            revoked_at: now,
            epoch: 7,
        };
        let decoded = decode_revocation(&revocation_item(&revocation)).expect("reader");
        assert_eq!(decoded.api_key, api_key);
        assert_eq!(decoded.revoked_epoch, 7);

        let limit = LimitWrite {
            workspace,
            id: LimitId::QueryPage,
            effective_value: LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(1_000),
            }),
            source: LimitSource::Default,
            revision: 8,
            changed_at: now,
        };
        let decoded = decode_limit(&limit_item(&limit).expect("valid limit"), workspace)
            .expect("limit reader");
        assert_eq!(decoded.id, LimitId::QueryPage);
        assert_eq!(decoded.effective_value, limit.effective_value);
        assert_eq!(decoded.revision, 8);
    }

    #[test]
    fn a_limit_writer_refuses_a_value_with_the_wrong_registered_shape() {
        let write = LimitWrite {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            id: LimitId::QueryPage,
            effective_value: LimitValue::Map(LimitMapValue {
                values: std::collections::BTreeMap::new(),
            }),
            source: LimitSource::WorkspaceOverride,
            revision: 1,
            changed_at: Timestamp::from_unix_millis(1_000).expect("timestamp"),
        };
        let error = limit_item(&write).expect_err("wrong shape must fail closed");
        assert!(error.contains("registered shape"), "{error}");
    }

    #[test]
    fn only_an_exact_equal_revision_is_an_idempotent_limit_replay() {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let now = Timestamp::from_unix_millis(1_000).expect("timestamp");
        let write = LimitWrite {
            workspace,
            id: LimitId::QueryPage,
            effective_value: LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(1_000),
            }),
            source: LimitSource::Default,
            revision: 3,
            changed_at: now,
        };
        let projected = ProjectedWorkspaceLimit {
            workspace,
            id: write.id,
            effective_value: write.effective_value.clone(),
            source: write.source,
            revision: write.revision,
            changed_at: write.changed_at,
        };
        assert!(classify_limit_replay(Some(&projected), &write).is_ok());

        let mut conflicting = projected.clone();
        conflicting.effective_value = LimitValue::Scalar(LimitScalarValue {
            value: DecimalU128::new(999),
        });
        assert!(classify_limit_replay(Some(&conflicting), &write).is_err());

        let mut newer = projected;
        newer.revision += 1;
        assert!(classify_limit_replay(Some(&newer), &write).is_ok());

        let mut foreign = newer;
        foreign.id = LimitId::RequestBodyBytes;
        assert!(classify_limit_replay(Some(&foreign), &write).is_err());
    }
}
