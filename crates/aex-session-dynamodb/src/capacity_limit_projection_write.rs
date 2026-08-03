//! Regional capacity's write-only effective-limit projection adapter.
//!
//! This module is exposed only by `capacity-limit-projection-write`. Central
//! control's producer feature cannot construct [`LimitWrite`] or
//! [`CapacityLimitProjectionWriter`]. The adapter transports an authority
//! decision and never supplies a default, admits an override or creates a
//! production call site.

use aex_wire::ids::WorkspaceId;
use aex_wire::limits::LimitId;
use aex_wire::models::{LimitSource, LimitValue};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::Client;

use crate::attr::{Item, ItemBuilder, n, s, stamp};
use crate::error::{Idempotence, Resolution, StoreError, classify};
use crate::plan::Participant;
use crate::projection_limit::{WORKSPACE_LIMIT, decode_limit_at, limit_key};
use crate::wire_pending::ProjectedWorkspaceLimit;

const LIMIT_PARTICIPANT: Participant = Participant::new("authz.limit");

const LIMIT_WRITE_CONDITION: &str = "attribute_not_exists(#pk) OR (#item_type = :item_type AND #workspace_id = :workspace_id AND #limit_id = :limit_id AND (#revision < :revision OR (#revision = :revision AND #value = :value AND #source = :source AND #changed_at = :changed_at)))";

/// One complete effective-limit projection selected by regional capacity.
///
/// This is a producer contract, not a source of defaults. Callers must supply a
/// durable effective value and provenance; the adapter never infers either
/// from generated registry metadata.
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

/// The regional capacity authority's narrow projection writer.
#[derive(Debug, Clone)]
pub struct CapacityLimitProjectionWriter {
    client: Client,
    table: String,
}

impl CapacityLimitProjectionWriter {
    /// Binds one regional projection table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// Publishes one effective limit without allowing delayed delivery or an
    /// equal-revision conflict to replace the durable answer.
    ///
    /// A higher stored revision makes this delivery stale and complete. The
    /// same revision is replay success only when immutable identity, value,
    /// provenance and change instant all agree. Only a failed condition or a
    /// true [`StoreError::CommitAmbiguous`] performs a strong resolution read.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Invalid`] for a value whose shape disagrees with
    /// the generated registry, [`StoreError::PreconditionFailed`] for an
    /// equal-revision conflict, [`StoreError::Corrupt`] for stored identity
    /// drift, or the definitive transport error without a resolution read.
    pub async fn put_limit(&self, write: &LimitWrite) -> Result<(), StoreError> {
        let item = limit_item(write)?;
        let value = encoded_value(write)?;
        let result = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .condition_expression(LIMIT_WRITE_CONDITION)
            .expression_attribute_names("#pk", "pk")
            .expression_attribute_names("#item_type", "itemType")
            .expression_attribute_names("#workspace_id", "workspaceId")
            .expression_attribute_names("#limit_id", "limitId")
            .expression_attribute_names("#revision", "revision")
            .expression_attribute_names("#value", "effectiveValue")
            .expression_attribute_names("#source", "source")
            .expression_attribute_names("#changed_at", "changedAt")
            .expression_attribute_values(":item_type", s(WORKSPACE_LIMIT))
            .expression_attribute_values(":workspace_id", s(write.workspace.to_string()))
            .expression_attribute_values(":limit_id", s(write.id.as_str()))
            .expression_attribute_values(":revision", n(write.revision))
            .expression_attribute_values(":value", s(value))
            .expression_attribute_values(":source", s(write.source.as_str()))
            .expression_attribute_values(":changed_at", stamp(write.changed_at))
            .send()
            .await;
        let Err(error) = result else {
            return Ok(());
        };

        if conditional_put(&error) {
            return self.resolve_limit_write(write).await;
        }
        let classified = classify(&error, Idempotence::Write(Resolution::TargetItem));
        if limit_write_needs_resolution(&classified) {
            self.resolve_limit_write(write).await
        } else {
            Err(classified)
        }
    }

    async fn resolve_limit_write(&self, write: &LimitWrite) -> Result<(), StoreError> {
        let (pk, sk) = limit_key(write.workspace, write.id);
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", s(pk))
            .key("sk", s(sk))
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(&error, Idempotence::Read))?;
        let current = output
            .item
            .as_ref()
            .map(|item| decode_limit_at(item, write.workspace, write.id))
            .transpose()?;
        classify_limit_replay(current.as_ref(), write)
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

const fn limit_write_needs_resolution(error: &StoreError) -> bool {
    matches!(error, StoreError::CommitAmbiguous { .. })
}

fn encoded_value(write: &LimitWrite) -> Result<String, StoreError> {
    serde_json::to_string(&write.effective_value).map_err(|_| StoreError::Invalid {
        detail: "effective limit could not be encoded".to_owned(),
    })
}

fn limit_item(write: &LimitWrite) -> Result<Item, StoreError> {
    if write.effective_value.shape() != write.id.shape() {
        return Err(StoreError::Invalid {
            detail: format!(
                "effective limit value shape {:?} differs from registered shape {:?}",
                write.effective_value.shape(),
                write.id.shape()
            ),
        });
    }
    let value = encoded_value(write)?;
    let (pk, sk) = limit_key(write.workspace, write.id);
    Ok(ItemBuilder::new(WORKSPACE_LIMIT)
        .set("pk", s(pk))
        .set("sk", s(sk))
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
) -> Result<(), StoreError> {
    let Some(current) = current else {
        return Err(StoreError::CommitAmbiguous {
            resolve_by: Resolution::TargetItem,
        });
    };
    if current.workspace != desired.workspace || current.id != desired.id {
        return Err(StoreError::Invalid {
            detail: "effective limit point read returned a foreign identity".to_owned(),
        });
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
    Err(StoreError::PreconditionFailed {
        participant: LIMIT_PARTICIPANT,
        observed: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        LIMIT_WRITE_CONDITION, LimitWrite, classify_limit_replay, limit_item,
        limit_write_needs_resolution,
    };
    use crate::error::{Resolution, StoreError};
    use crate::projection_limit::decode_limit_at;
    use crate::wire_pending::ProjectedWorkspaceLimit;
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{LimitMapValue, LimitScalarValue, LimitSource, LimitValue};
    use aex_wire::types::{DecimalU128, Timestamp};

    fn write() -> LimitWrite {
        LimitWrite {
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            id: LimitId::QueryPage,
            effective_value: LimitValue::Map(LimitMapValue {
                values: std::collections::BTreeMap::from([
                    ("items".to_owned(), DecimalU128::new(1_000)),
                    (
                        "serialized_bytes".to_owned(),
                        DecimalU128::new(8 * 1_024 * 1_024),
                    ),
                ]),
            }),
            source: LimitSource::Default,
            revision: 3,
            changed_at: Timestamp::from_unix_millis(1_000).expect("timestamp"),
        }
    }

    #[test]
    fn the_equal_revision_condition_binds_every_immutable_identity_field() {
        for required in [
            "#item_type = :item_type",
            "#workspace_id = :workspace_id",
            "#limit_id = :limit_id",
            "#revision = :revision",
            "#value = :value",
            "#source = :source",
            "#changed_at = :changed_at",
        ] {
            assert!(
                LIMIT_WRITE_CONDITION.contains(required),
                "condition omitted `{required}`: {LIMIT_WRITE_CONDITION}"
            );
        }
    }

    #[test]
    fn the_capacity_writer_row_round_trips_through_the_shared_codec() {
        let write = write();
        let decoded = decode_limit_at(
            &limit_item(&write).expect("valid limit"),
            write.workspace,
            write.id,
        )
        .expect("limit reader");
        assert_eq!(decoded.id, write.id);
        assert_eq!(decoded.effective_value, write.effective_value);
        assert_eq!(decoded.revision, write.revision);
    }

    #[test]
    fn a_limit_writer_refuses_a_value_with_the_wrong_registered_shape() {
        let mut write = write();
        write.effective_value = LimitValue::Map(LimitMapValue {
            values: std::collections::BTreeMap::new(),
        });
        let error = limit_item(&write).expect_err("wrong shape must fail closed");
        assert!(matches!(error, StoreError::Invalid { .. }), "{error}");
    }

    #[test]
    fn only_an_exact_equal_revision_is_an_idempotent_limit_replay() {
        let write = write();
        let projected = ProjectedWorkspaceLimit {
            workspace: write.workspace,
            id: write.id,
            effective_value: write.effective_value.clone(),
            source: write.source,
            revision: write.revision,
            changed_at: write.changed_at,
        };
        assert!(classify_limit_replay(Some(&projected), &write).is_ok());

        let mut conflicting = projected.clone();
        conflicting.effective_value = LimitValue::Map(LimitMapValue {
            values: std::collections::BTreeMap::from([
                ("items".to_owned(), DecimalU128::new(999)),
                (
                    "serialized_bytes".to_owned(),
                    DecimalU128::new(8 * 1_024 * 1_024),
                ),
            ]),
        });
        assert!(matches!(
            classify_limit_replay(Some(&conflicting), &write),
            Err(StoreError::PreconditionFailed { .. })
        ));

        let mut newer = projected;
        newer.revision += 1;
        assert!(classify_limit_replay(Some(&newer), &write).is_ok());
    }

    #[test]
    fn lower_equal_and_newer_durable_revisions_have_distinct_results() {
        let write = write();
        let exact = ProjectedWorkspaceLimit {
            workspace: write.workspace,
            id: write.id,
            effective_value: write.effective_value.clone(),
            source: write.source,
            revision: write.revision,
            changed_at: write.changed_at,
        };

        let mut lower = exact.clone();
        lower.revision -= 1;
        assert!(matches!(
            classify_limit_replay(Some(&lower), &write),
            Err(StoreError::PreconditionFailed { .. })
        ));

        assert!(classify_limit_replay(Some(&exact), &write).is_ok());

        let mut newer = exact;
        newer.revision += 1;
        assert!(classify_limit_replay(Some(&newer), &write).is_ok());
    }

    #[test]
    fn only_commit_ambiguous_requires_a_resolution_read() {
        assert!(limit_write_needs_resolution(&StoreError::CommitAmbiguous {
            resolve_by: Resolution::TargetItem,
        }));
        for definitive in [
            StoreError::Denied,
            StoreError::Throttled {
                retry_after: std::time::Duration::from_millis(50),
            },
            StoreError::Misconfigured {
                table: "regional-authz-projection".to_owned(),
            },
            StoreError::Invalid {
                detail: "validation".to_owned(),
            },
        ] {
            assert!(
                !limit_write_needs_resolution(&definitive),
                "definitive error requested a read: {definitive}"
            );
        }
    }
}
