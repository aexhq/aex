//! Row to domain conversion.
//!
//! Every timestamp arrives as a `bigint` of epoch milliseconds, never as a Data
//! `API` timestamp string, so a decode has no dependence on the session time
//! zone or `DateStyle`.

use aex_control_app::ports::{AccountActorState, SigningKeyRecord, WorkspaceKeyState};
use aex_control_domain::{
    AccountState, Epoch, OrgRole, OrganizationStatus, ScopeSet, WorkspaceStatus,
};
use aex_rds_data::{DecodeError, Record, Row};
use time::OffsetDateTime;

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
    use super::{SigningKeyRow, WorkspaceKeyRow};
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
