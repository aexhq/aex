//! Identity-schema row decoders.

use aex_control_domain::Revision;
use aex_identity_domain::{
    DashboardSession, EmailChallenge, ExternalIdentity, NormalizedEmail, PepperVersion, Provider,
    ProviderAccountId, User, UserStatus, Verifier,
};
use aex_rds_data::{DecodeError, Record, Row};
use time::OffsetDateTime;

fn instant(record: &Record<'_>, index: usize) -> Result<OffsetDateTime, DecodeError> {
    record.timestamp_millis(index)
}

fn optional_instant(
    record: &Record<'_>,
    index: usize,
) -> Result<Option<OffsetDateTime>, DecodeError> {
    record.opt(index, Record::timestamp_millis)
}

fn email(record: &Record<'_>, index: usize) -> Result<NormalizedEmail, DecodeError> {
    NormalizedEmail::parse(record.text(index)?).map_err(|_| DecodeError::TypeMismatch {
        index,
        expected: "a normalized email",
    })
}

fn revision(record: &Record<'_>, index: usize) -> Result<Revision, DecodeError> {
    let value = u64::try_from(record.i64(index)?).map_err(|_| DecodeError::Overflow { index })?;
    Revision::new(value).map_err(|_| DecodeError::TypeMismatch {
        index,
        expected: "a positive revision",
    })
}

/// A complete person projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRow(pub User);

impl Row for UserRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(9)?;
        Ok(Self(User {
            id: record.uuid(0)?,
            email: email(record, 1)?,
            email_verified_at: optional_instant(record, 2)?,
            name: record.opt(3, |row, index| Ok(row.text(index)?.to_owned()))?,
            image_url: record.opt(4, |row, index| Ok(row.text(index)?.to_owned()))?,
            status: UserStatus::parse(record.text(5)?).ok_or(DecodeError::TypeMismatch {
                index: 5,
                expected: "a user status",
            })?,
            revision: revision(record, 6)?,
            created_at: instant(record, 7)?,
            updated_at: instant(record, 8)?,
        }))
    }
}

/// A provider-link projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdentityRow(pub ExternalIdentity);

impl Row for ExternalIdentityRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(5)?;
        Ok(Self(ExternalIdentity {
            id: record.uuid(0)?,
            user_id: record.uuid(1)?,
            provider: Provider::parse(record.text(2)?).ok_or(DecodeError::TypeMismatch {
                index: 2,
                expected: "an identity provider",
            })?,
            provider_account_id: ProviderAccountId::parse(record.text(3)?).map_err(|_| {
                DecodeError::TypeMismatch {
                    index: 3,
                    expected: "a provider account id",
                }
            })?,
            linked_at: instant(record, 4)?,
        }))
    }
}

/// An email challenge plus its stored verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailChallengeRow {
    /// The public/domain row.
    pub value: EmailChallenge,
    /// The keyed verifier, never exposed above the adapter.
    pub verifier: Verifier,
}

impl Row for EmailChallengeRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(7)?;
        Ok(Self {
            value: EmailChallenge {
                id: record.uuid(0)?,
                email: email(record, 1)?,
                pepper_version: PepperVersion::new(record.u16(3)?),
                issued_at: instant(record, 4)?,
                expires_at: instant(record, 5)?,
                consumed_at: optional_instant(record, 6)?,
            },
            verifier: Verifier::from_bytes(record.fixed::<32>(2)?),
        })
    }
}

/// A dashboard session, its person and the stored verifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardSessionRow {
    /// The session.
    pub session: DashboardSession,
    /// The person.
    pub user: User,
    /// The keyed verifier.
    pub verifier: Verifier,
}

impl Row for DashboardSessionRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(15)?;
        Ok(Self {
            session: DashboardSession {
                id: record.uuid(0)?,
                user_id: record.uuid(1)?,
                pepper_version: PepperVersion::new(record.u16(3)?),
                issued_at: instant(record, 4)?,
                expires_at: instant(record, 5)?,
                revoked_at: optional_instant(record, 6)?,
            },
            verifier: Verifier::from_bytes(record.fixed::<32>(2)?),
            user: User {
                id: record.uuid(1)?,
                email: email(record, 7)?,
                email_verified_at: optional_instant(record, 8)?,
                name: record.opt(9, |row, index| Ok(row.text(index)?.to_owned()))?,
                image_url: record.opt(10, |row, index| Ok(row.text(index)?.to_owned()))?,
                status: UserStatus::parse(record.text(11)?).ok_or(DecodeError::TypeMismatch {
                    index: 11,
                    expected: "a user status",
                })?,
                revision: revision(record, 12)?,
                created_at: instant(record, 13)?,
                updated_at: instant(record, 14)?,
            },
        })
    }
}

/// One aggregate count.
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
