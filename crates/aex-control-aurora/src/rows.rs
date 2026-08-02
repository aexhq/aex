//! Row to domain conversion.
//!
//! Every timestamp arrives as a `bigint` of epoch milliseconds, never as a Data
//! `API` timestamp string, so a decode has no dependence on the session time
//! zone or `DateStyle`.

use aex_control_app::ports::{
    AccountActorState, CentralActorState, SigningKeyRecord, WorkspaceKeyState,
};
use aex_control_domain::{
    AccountState, ApiKey, Epoch, Fence, IntentHash, Invitation, InvitationStatus, Lease,
    LeaseOwner, Membership, MembershipStatus, Operation, OperationKind, OperationStatus,
    OperationVisibility, OrgMembership, OrgRole, Organization, OrganizationStatus, OutboxMessage,
    Revision, ScopeSet, Slug, Topic, Workspace, WorkspaceStatus,
};
use aex_rds_data::{DecodeError, Record, Row};
use time::OffsetDateTime;
use uuid::Uuid;

/// Decodes a `text[]` of scope spellings.
fn scopes(record: &Record<'_>, index: usize) -> Result<ScopeSet, DecodeError> {
    let spellings = record.text_array(index)?;
    ScopeSet::from_strings(&spellings).map_err(|_| DecodeError::TypeMismatch {
        index,
        expected: "a registry scope list",
    })
}

/// Decodes a region name.
fn region(record: &Record<'_>, index: usize) -> Result<aex_wire::types::Region, DecodeError> {
    aex_wire::types::Region::from_name(record.text(index)?).ok_or(DecodeError::TypeMismatch {
        index,
        expected: "a launch region",
    })
}

fn instant(record: &Record<'_>, index: usize) -> Result<OffsetDateTime, DecodeError> {
    record.timestamp_millis(index)
}

fn optional_instant(
    record: &Record<'_>,
    index: usize,
) -> Result<Option<OffsetDateTime>, DecodeError> {
    record.opt(index, Record::timestamp_millis)
}

fn revision(record: &Record<'_>, index: usize) -> Result<Revision, DecodeError> {
    let value = u64::try_from(record.i64(index)?).map_err(|_| DecodeError::Overflow { index })?;
    Revision::new(value).map_err(|_| DecodeError::TypeMismatch {
        index,
        expected: "a positive revision",
    })
}

/// A control organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationRow(pub Organization);

impl Row for OrganizationRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(8)?;
        Ok(Self(Organization {
            id: record.uuid(0)?,
            name: record.text(1)?.to_owned(),
            slug: Slug::parse(record.text(2)?).map_err(|_| DecodeError::TypeMismatch {
                index: 2,
                expected: "an organization slug",
            })?,
            status: OrganizationStatus::parse(record.text(3)?).ok_or(
                DecodeError::TypeMismatch {
                    index: 3,
                    expected: "an organization status",
                },
            )?,
            revision: revision(record, 4)?,
            created_at: instant(record, 5)?,
            updated_at: instant(record, 6)?,
            created_by_user_id: record.uuid(7)?,
        }))
    }
}

/// A control membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipRow(pub Membership);

impl Row for MembershipRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(8)?;
        Ok(Self(Membership {
            id: record.uuid(0)?,
            organization_id: record.uuid(1)?,
            user_id: record.uuid(2)?,
            role: OrgRole::parse(record.text(3)?).ok_or(DecodeError::TypeMismatch {
                index: 3,
                expected: "an organization role",
            })?,
            status: MembershipStatus::parse(record.text(4)?).ok_or(DecodeError::TypeMismatch {
                index: 4,
                expected: "a membership status",
            })?,
            revision: revision(record, 5)?,
            created_at: instant(record, 6)?,
            updated_at: instant(record, 7)?,
        }))
    }
}

/// A control invitation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationRow(pub Invitation);

impl Row for InvitationRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(10)?;
        Ok(Self(Invitation {
            id: record.uuid(0)?,
            organization_id: record.uuid(1)?,
            email: record.text(2)?.to_owned(),
            role: OrgRole::parse(record.text(3)?).ok_or(DecodeError::TypeMismatch {
                index: 3,
                expected: "an organization role",
            })?,
            status: InvitationStatus::parse(record.text(4)?).ok_or(DecodeError::TypeMismatch {
                index: 4,
                expected: "an invitation status",
            })?,
            invited_by_user_id: record.uuid(5)?,
            accepted_user_id: record.opt(6, Record::uuid)?,
            created_at: instant(record, 7)?,
            expires_at: instant(record, 8)?,
            resolved_at: optional_instant(record, 9)?,
        }))
    }
}

/// A control workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow(pub Workspace);

impl Row for WorkspaceRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(16)?;
        Ok(Self(Workspace {
            id: record.uuid(0)?,
            organization_id: record.uuid(1)?,
            name: record.text(2)?.to_owned(),
            slug: Slug::parse(record.text(3)?).map_err(|_| DecodeError::TypeMismatch {
                index: 3,
                expected: "a workspace slug",
            })?,
            region: region(record, 4)?,
            status: WorkspaceStatus::parse(record.text(5)?).ok_or(DecodeError::TypeMismatch {
                index: 5,
                expected: "a workspace status",
            })?,
            provision_operation_id: record.uuid(6)?,
            provision_fence: Fence::new(
                u64::try_from(record.i64(7)?).map_err(|_| DecodeError::Overflow { index: 7 })?,
            ),
            deletion_operation_id: record.opt(8, Record::uuid)?,
            deletion_fence: record
                .opt(9, Record::i64)?
                .map(|value| {
                    u64::try_from(value)
                        .map(Fence::new)
                        .map_err(|_| DecodeError::Overflow { index: 9 })
                })
                .transpose()?,
            revision: revision(record, 10)?,
            created_at: instant(record, 11)?,
            updated_at: instant(record, 12)?,
            activated_at: optional_instant(record, 13)?,
            deleted_at: optional_instant(record, 14)?,
            created_by_user_id: record.uuid(15)?,
        }))
    }
}

/// A workspace API key without its verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyRow(pub ApiKey);

impl Row for ApiKeyRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(11)?;
        Ok(Self(ApiKey {
            id: record.uuid(0)?,
            workspace_id: record.uuid(1)?,
            organization_id: record.uuid(2)?,
            name: record.text(3)?.to_owned(),
            scopes: scopes(record, 4)?,
            region: region(record, 5)?,
            pepper_version: record.u16(6)?,
            created_at: instant(record, 7)?,
            revoked_at: optional_instant(record, 8)?,
            revision: revision(record, 9)?,
            created_by_user_id: record.uuid(10)?,
        }))
    }
}

/// A durable operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRow(pub Operation);

impl Row for OperationRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(18)?;
        let kind = OperationKind::parse(record.text(1)?).ok_or(DecodeError::TypeMismatch {
            index: 1,
            expected: "a central operation kind",
        })?;
        let visibility = match record.text(2)? {
            "public" => OperationVisibility::Public,
            "internal" => OperationVisibility::Internal,
            _ => {
                return Err(DecodeError::TypeMismatch {
                    index: 2,
                    expected: "an operation visibility",
                });
            }
        };
        Ok(Self(Operation {
            id: record.uuid(0)?,
            kind,
            visibility,
            organization_id: record.uuid(3)?,
            workspace_id: record.opt(4, Record::uuid)?,
            principal_id: record.uuid(5)?,
            scopes: scopes(record, 6)?,
            status: OperationStatus::parse(record.text(7)?).ok_or(DecodeError::TypeMismatch {
                index: 7,
                expected: "an operation status",
            })?,
            intent_hash: IntentHash::from_bytes(record.fixed::<32>(8)?),
            fence: Fence::new(
                u64::try_from(record.i64(9)?).map_err(|_| DecodeError::Overflow { index: 9 })?,
            ),
            attempt: u32::try_from(record.i64(10)?)
                .map_err(|_| DecodeError::Overflow { index: 10 })?,
            lease: match (
                record.opt(11, |row, index| Ok(row.text(index)?.to_owned()))?,
                optional_instant(record, 12)?,
            ) {
                (Some(owner), Some(expires_at)) => Some(Lease {
                    owner: LeaseOwner::new(owner),
                    expires_at,
                }),
                (None, None) => None,
                _ => {
                    return Err(DecodeError::TypeMismatch {
                        index: 11,
                        expected: "a complete operation lease",
                    });
                }
            },
            created_at: instant(record, 13)?,
            started_at: optional_instant(record, 14)?,
            updated_at: instant(record, 15)?,
            terminal_at: optional_instant(record, 16)?,
            due_at: optional_instant(record, 17)?,
        }))
    }
}

/// A transactional-outbox row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow(pub OutboxMessage);

impl Row for OutboxRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(12)?;
        Ok(Self(OutboxMessage {
            id: record.uuid(0)?,
            topic: Topic::parse(record.text(1)?).ok_or(DecodeError::TypeMismatch {
                index: 1,
                expected: "an outbox topic",
            })?,
            dedupe_key: record.text(2)?.to_owned(),
            group_key: record.text(3)?.to_owned(),
            payload: record.json(4)?,
            attempts: u32::try_from(record.i64(5)?)
                .map_err(|_| DecodeError::Overflow { index: 5 })?,
            available_at: instant(record, 6)?,
            claimed_by: record.opt(7, |row, index| Ok(row.text(index)?.to_owned()))?,
            claimed_until: optional_instant(record, 8)?,
            dispatched_at: optional_instant(record, 9)?,
            last_error: record.opt(10, |row, index| Ok(row.text(index)?.to_owned()))?,
            created_at: instant(record, 11)?,
        }))
    }
}

/// The projection of [`crate::sql::GET_ACCOUNT_STATE`].
///
/// One column, and it is the only column the edge is entitled to. A row type
/// rather than a bare `String` so the spelling is parsed once, here, and an
/// unrecognised one is a decode failure instead of a silent `Active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountStateRow(pub AccountState);

impl Row for AccountStateRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(AccountState::parse(record.text(0)?).ok_or(
            DecodeError::TypeMismatch {
                index: 0,
                expected: "an account state",
            },
        )?))
    }
}

/// A single aggregate count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountRow(pub u64);

impl Row for CountRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(
            u64::try_from(record.i64(0)?).map_err(|_| DecodeError::Overflow { index: 0 })?,
        ))
    }
}

/// One UUID projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UuidRow(pub uuid::Uuid);

impl Row for UuidRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        Ok(Self(record.uuid(0)?))
    }
}

/// The replay facts needed before a mutation runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyRow {
    /// The canonical intent originally admitted.
    pub intent_hash: IntentHash,
    /// `in_flight` or `completed`.
    pub state: String,
    /// The adapter-owned replay body.
    pub response_body: Option<serde_json::Value>,
    /// The operation created by operation-idempotent commands.
    pub operation_id: Option<uuid::Uuid>,
}

impl Row for IdempotencyRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(4)?;
        Ok(Self {
            intent_hash: IntentHash::from_bytes(record.fixed::<32>(0)?),
            state: record.text(1)?.to_owned(),
            response_body: record.opt(2, Record::json::<serde_json::Value>)?,
            operation_id: record.opt(3, Record::uuid)?,
        })
    }
}

/// The projection of [`crate::sql::RESOLVE_WORKSPACE_KEY`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceKeyRow(pub WorkspaceKeyState);

impl Row for WorkspaceKeyRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(14)?;
        Ok(Self(WorkspaceKeyState {
            key_id: record.uuid(0)?,
            workspace_id: record.uuid(1)?,
            organization_id: record.uuid(2)?,
            scopes: scopes(record, 3)?,
            verifier: record.fixed::<32>(4)?,
            pepper_version: record.u16(5)?,
            key_revoked: record.bool(6)?,
            region: region(record, 7)?,
            workspace_status: WorkspaceStatus::parse(record.text(8)?).ok_or(
                DecodeError::TypeMismatch {
                    index: 8,
                    expected: "a workspace status",
                },
            )?,
            organization_status: OrganizationStatus::parse(record.text(9)?).ok_or(
                DecodeError::TypeMismatch {
                    index: 9,
                    expected: "an organization status",
                },
            )?,
            account_state: AccountState::parse(record.text(10)?).ok_or(
                DecodeError::TypeMismatch {
                    index: 10,
                    expected: "an account state",
                },
            )?,
            epoch_key: epoch(record, 11)?,
            epoch_workspace: epoch(record, 12)?,
            epoch_account: epoch(record, 13)?,
        }))
    }
}

/// Decodes a `bigint` epoch.
fn epoch(record: &Record<'_>, index: usize) -> Result<Epoch, DecodeError> {
    let value = record.i64(index)?;
    u64::try_from(value)
        .map(Epoch::new)
        .map_err(|_| DecodeError::Overflow { index })
}

/// The projection of the two workspace-scoped actor statements.
///
/// One row type for both, because a browser session and an account token are
/// the same principal reaching the same surface. Two row types would be two
/// places for the same shape to drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountActorRow(pub AccountActorState);

impl Row for AccountActorRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(18)?;
        Ok(Self(AccountActorState {
            credential_id: record.uuid(0)?,
            user_id: record.uuid(1)?,
            membership_id: record.uuid(2)?,
            role: OrgRole::parse(record.text(3)?).ok_or(DecodeError::TypeMismatch {
                index: 3,
                expected: "an organization role",
            })?,
            scopes: scopes(record, 4)?,
            verifier: record.fixed::<32>(5)?,
            pepper_version: record.u16(6)?,
            credential_revoked: record.bool(7)?,
            credential_expired: record.bool(8)?,
            user_active: record.bool(9)?,
            workspace_id: record.uuid(10)?,
            organization_id: record.uuid(11)?,
            region: region(record, 12)?,
            account_state: AccountState::parse(record.text(13)?).ok_or(
                DecodeError::TypeMismatch {
                    index: 13,
                    expected: "an account state",
                },
            )?,
            epoch_user: epoch(record, 14)?,
            epoch_membership: epoch(record, 15)?,
            epoch_workspace: epoch(record, 16)?,
            epoch_account: epoch(record, 17)?,
        }))
    }
}

/// A central actor and its complete active-membership snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CentralActorRow(pub CentralActorState);

impl Row for CentralActorRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(9)?;
        let membership_document = record.json::<serde_json::Value>(8)?;
        let membership_values = membership_document
            .as_array()
            .ok_or(DecodeError::BadJson { index: 8 })?;
        if membership_values.len() > aex_control_app::ports::MAX_PAGE_LIMIT as usize {
            return Err(DecodeError::Overflow { index: 8 });
        }
        let memberships = membership_values
            .iter()
            .map(|value| membership(value, 8))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(CentralActorState {
            credential_id: record.uuid(0)?,
            user_id: record.uuid(1)?,
            scopes: scopes(record, 2)?,
            verifier: record.fixed::<32>(3)?,
            pepper_version: record.u16(4)?,
            credential_revoked: record.bool(5)?,
            credential_expired: record.bool(6)?,
            user_active: record.bool(7)?,
            memberships,
        }))
    }
}

fn membership(value: &serde_json::Value, index: usize) -> Result<OrgMembership, DecodeError> {
    let fields = value
        .as_array()
        .filter(|fields| fields.len() == 3)
        .ok_or(DecodeError::BadJson { index })?;
    let organization_id = fields[0]
        .as_str()
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or(DecodeError::BadJson { index })?;
    let membership_id = fields[1]
        .as_str()
        .and_then(|text| Uuid::parse_str(text).ok())
        .ok_or(DecodeError::BadJson { index })?;
    let role = fields[2]
        .as_str()
        .and_then(OrgRole::parse)
        .ok_or(DecodeError::BadJson { index })?;
    Ok(OrgMembership {
        organization_id,
        membership_id,
        role,
    })
}

/// The projection of the signing-key statements.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningKeyRow(pub SigningKeyRecord);

impl Row for SigningKeyRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(5)?;
        let retires_at_ms = record.i64(4)?;
        Ok(Self(SigningKeyRecord {
            kid: record.uuid(0)?,
            public_key: record.fixed::<32>(1)?,
            secret_ref: record.text(2)?.to_owned(),
            state: record.text(3)?.to_owned(),
            retires_at: OffsetDateTime::from_unix_timestamp_nanos(
                i128::from(retires_at_ms) * 1_000_000,
            )
            .map_err(|_| DecodeError::BadTimestamp { index: 4 })?,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountStateRow, CentralActorRow, SigningKeyRow, WorkspaceKeyRow};
    use aex_rds_data::{DecodeError, Record, Row};
    use aws_sdk_rdsdata::types::{ArrayValue, Field};
    use aws_smithy_types::Blob;
    use uuid::Uuid;

    fn key_fields(account_status: &str) -> Vec<Field> {
        vec![
            Field::StringValue(Uuid::from_u128(1).to_string()),
            Field::StringValue(Uuid::from_u128(2).to_string()),
            Field::StringValue(Uuid::from_u128(3).to_string()),
            Field::ArrayValue(ArrayValue::StringValues(vec![Some(
                "sessions:read".to_owned(),
            )])),
            Field::BlobValue(Blob::new(vec![7_u8; 32])),
            Field::LongValue(1),
            Field::BooleanValue(false),
            Field::StringValue("eu-west-1".to_owned()),
            Field::StringValue("active".to_owned()),
            Field::StringValue("active".to_owned()),
            Field::StringValue(account_status.to_owned()),
            Field::LongValue(3),
            Field::LongValue(1),
            Field::LongValue(0),
        ]
    }

    #[test]
    fn a_central_actor_decodes_the_complete_membership_snapshot() {
        let organization_id = Uuid::from_u128(21);
        let membership_id = Uuid::from_u128(22);
        let fields = vec![
            Field::StringValue(Uuid::from_u128(1).to_string()),
            Field::StringValue(Uuid::from_u128(2).to_string()),
            Field::ArrayValue(ArrayValue::StringValues(vec![Some(
                "account:read".to_owned(),
            )])),
            Field::BlobValue(Blob::new(vec![7_u8; 32])),
            Field::LongValue(1),
            Field::BooleanValue(false),
            Field::BooleanValue(false),
            Field::BooleanValue(true),
            Field::StringValue(
                serde_json::json!([[organization_id, membership_id, "admin"]]).to_string(),
            ),
        ];
        let row = CentralActorRow::from_record(&Record::new(&fields)).expect("decodes");
        assert_eq!(row.0.memberships.len(), 1);
        assert_eq!(row.0.memberships[0].organization_id, organization_id);
        assert_eq!(row.0.memberships[0].membership_id, membership_id);
        assert_eq!(
            row.0.memberships[0].role,
            aex_control_domain::OrgRole::Admin
        );
    }

    #[test]
    fn the_authorization_row_decodes_every_column() {
        let fields = key_fields("active");
        let row = WorkspaceKeyRow::from_record(&Record::new(&fields)).expect("decodes");
        assert_eq!(row.0.key_id, Uuid::from_u128(1));
        assert_eq!(row.0.region, aex_wire::types::Region::EuWest1);
        assert_eq!(row.0.epoch_key.get(), 3);
        assert_eq!(row.0.epoch_account.get(), 0);
        assert!(!row.0.key_revoked);
        assert_eq!(row.0.epoch_subjects().len(), 3);
    }

    #[test]
    fn an_absent_finance_row_decodes_as_unavailable_and_never_as_active() {
        let fields = key_fields("unavailable");
        let row = WorkspaceKeyRow::from_record(&Record::new(&fields)).expect("decodes");
        assert_eq!(
            row.0.account_state,
            aex_control_domain::AccountState::Unavailable
        );
    }

    #[test]
    fn an_unknown_scope_spelling_is_a_decode_failure_rather_than_a_dropped_scope() {
        let mut fields = key_fields("active");
        fields[3] = Field::ArrayValue(ArrayValue::StringValues(vec![Some(
            "sessions:teleport".to_owned(),
        )]));
        assert_eq!(
            WorkspaceKeyRow::from_record(&Record::new(&fields)),
            Err(DecodeError::TypeMismatch {
                index: 3,
                expected: "a registry scope list"
            })
        );
    }

    #[test]
    fn an_unknown_region_or_status_is_a_decode_failure() {
        let mut fields = key_fields("active");
        fields[7] = Field::StringValue("mars-central-1".to_owned());
        assert!(WorkspaceKeyRow::from_record(&Record::new(&fields)).is_err());

        let mut fields = key_fields("active");
        fields[8] = Field::StringValue("archived".to_owned());
        assert!(WorkspaceKeyRow::from_record(&Record::new(&fields)).is_err());
    }

    #[test]
    fn a_wrong_arity_is_refused_before_any_column_is_read() {
        let fields = key_fields("active");
        assert_eq!(
            WorkspaceKeyRow::from_record(&Record::new(&fields[..13])),
            Err(DecodeError::ArityMismatch {
                expected: 14,
                actual: 13
            })
        );
    }

    #[test]
    fn the_coarse_account_state_decodes_every_spelling_and_refuses_an_unknown_one() {
        for (spelling, expected) in [
            ("active", aex_control_domain::AccountState::Active),
            (
                "paused_top_up_required",
                aex_control_domain::AccountState::PausedTopUpRequired,
            ),
            ("unavailable", aex_control_domain::AccountState::Unavailable),
        ] {
            let fields = vec![Field::StringValue(spelling.to_owned())];
            let row = AccountStateRow::from_record(&Record::new(&fields))
                .unwrap_or_else(|error| panic!("`{spelling}` decodes: {error}"));
            assert_eq!(row.0, expected, "{spelling}");
        }

        // A spelling this process does not know is a decode failure. Mapping it
        // onto `Active` would admit an account nobody established the state of.
        let fields = vec![Field::StringValue("suspended".to_owned())];
        assert_eq!(
            AccountStateRow::from_record(&Record::new(&fields)),
            Err(DecodeError::TypeMismatch {
                index: 0,
                expected: "an account state"
            })
        );
    }

    #[test]
    fn a_signing_key_decodes_its_retirement_from_epoch_millis() {
        let fields = vec![
            Field::StringValue(Uuid::from_u128(9).to_string()),
            Field::BlobValue(Blob::new(vec![1_u8; 32])),
            Field::StringValue("aex/prd/authz-signing/9".to_owned()),
            Field::StringValue("active".to_owned()),
            Field::LongValue(1_767_225_600_000),
        ];
        let row = SigningKeyRow::from_record(&Record::new(&fields)).expect("decodes");
        assert_eq!(row.0.kid, Uuid::from_u128(9));
        assert_eq!(row.0.state, "active");
        assert_eq!(
            i64::try_from(row.0.retires_at.unix_timestamp_nanos() / 1_000_000).expect("in range"),
            1_767_225_600_000
        );
    }
}
