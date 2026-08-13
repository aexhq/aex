//! The pepper lifecycle table, read over the Data `API`.
//!
//! One implementation serves both schemas. The two statements are passed in
//! rather than written here, because SQL belongs to the adapter crate that owns
//! the schema and is scanned by that crate's discipline suite:
//! `aex_identity_aurora::sql::{ACTIVE_IDENTITY_PEPPER, IDENTITY_PEPPER_BY_VERSION}`
//! for the identity schema, and
//! `aex_control_aurora::sql::{ACTIVE_CONTROL_PEPPER, CONTROL_PEPPER_BY_VERSION,
//! LIVE_CONTROL_PEPPERS}`
//! for the control one. The composition root names the pair it is entitled to,
//! which is the same place its login role is named.
//!
//! Both statements project `(version, purpose, state, secret_ref)` and no
//! material at all. The role that runs them holds `SELECT` on the table and
//! nothing else; the bytes live in Secrets Manager and are fetched by the
//! keystore, under the version id this row carries.

use aex_identity_app::ports::{PepperPurpose, StoreError};
use aex_identity_domain::PepperVersion;
use aex_rds_data::{DataApiClient, DecodeError, Record, Row, SqlValue, Statement};
use async_trait::async_trait;

use crate::pepper::{PepperDirectory, PepperRecord, PepperState};

/// The two statements one schema's lifecycle table is read by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PepperStatements {
    /// The row whose state is `active`, for one purpose.
    pub active: &'static str,
    /// The row for one `(purpose, version)`.
    pub by_version: &'static str,
    /// The active row plus no more than three retiring rows.
    ///
    /// Four is deliberate: the keystore accepts at most three total rows, so
    /// the fourth is the fail-closed overflow witness rather than a row hidden
    /// by the query.
    pub verification_set: Option<&'static str>,
}

/// One lifecycle row, as the Data `API` projects it.
struct PepperRow(PepperRecord);

impl Row for PepperRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(4)?;
        let version =
            u16::try_from(record.i64(0)?).map_err(|_| DecodeError::Overflow { index: 0 })?;
        let purpose = match record.text(1)? {
            "identity" => PepperPurpose::Identity,
            "api_key" => PepperPurpose::ApiKey,
            "cursor" => PepperPurpose::Cursor,
            _ => {
                return Err(DecodeError::TypeMismatch {
                    index: 1,
                    expected: "a pepper purpose",
                });
            }
        };
        let state = PepperState::parse(record.text(2)?).ok_or(DecodeError::TypeMismatch {
            index: 2,
            expected: "a pepper lifecycle state",
        })?;
        Ok(Self(PepperRecord {
            version: PepperVersion::new(version),
            purpose,
            state,
            secret_ref: record.text(3)?.to_owned(),
        }))
    }
}

/// The pepper lifecycle table over one Data `API` client.
#[derive(Debug, Clone)]
pub struct DataApiPepperDirectory {
    client: DataApiClient,
    statements: PepperStatements,
}

impl DataApiPepperDirectory {
    /// Builds the directory for one schema's statement pair.
    #[must_use]
    pub const fn new(client: DataApiClient, statements: PepperStatements) -> Self {
        Self { client, statements }
    }

    /// Runs one lifecycle read, mapping an absent row to
    /// [`StoreError::NotFound`].
    ///
    /// Absent is deliberately not "fall back to the active row": for
    /// `by_version` that would verify a credential against a pepper its row
    /// does not name, and for `active` it would mint under nothing.
    async fn one(&self, statement: Statement<'_>) -> Result<PepperRecord, StoreError> {
        let row: Option<PepperRow> = self
            .client
            .query_opt(statement)
            .await
            .map_err(aex_identity_aurora::map_store_error)?;
        row.map(|row| row.0).ok_or(StoreError::NotFound)
    }
}

#[async_trait]
impl PepperDirectory for DataApiPepperDirectory {
    async fn active(&self, purpose: PepperPurpose) -> Result<PepperRecord, StoreError> {
        let record = self
            .one(
                Statement::new(self.statements.active)
                    .bind("purpose", SqlValue::Text(purpose.as_str().to_owned())),
            )
            .await?;
        if record.purpose != purpose {
            return Err(StoreError::Decode(
                "the lifecycle table answered a row for another purpose".to_owned(),
            ));
        }
        Ok(record)
    }

    async fn by_version(
        &self,
        purpose: PepperPurpose,
        version: PepperVersion,
    ) -> Result<PepperRecord, StoreError> {
        let record = self
            .one(
                Statement::new(self.statements.by_version)
                    .bind("purpose", SqlValue::Text(purpose.as_str().to_owned()))
                    .bind("version", SqlValue::I64(i64::from(version.get()))),
            )
            .await?;
        if record.purpose != purpose || record.version != version {
            return Err(StoreError::Decode(
                "the lifecycle table answered a row the query did not ask for".to_owned(),
            ));
        }
        Ok(record)
    }

    async fn verification_set(
        &self,
        purpose: PepperPurpose,
    ) -> Result<Vec<PepperRecord>, StoreError> {
        let statement = self.statements.verification_set.ok_or_else(|| {
            StoreError::Fatal("this pepper directory has no verification-set query".to_owned())
        })?;
        let rows: Vec<PepperRow> = self
            .client
            .query(
                Statement::new(statement)
                    .bind("purpose", SqlValue::Text(purpose.as_str().to_owned())),
            )
            .await
            .map_err(aex_identity_aurora::map_store_error)?;
        let records = rows.into_iter().map(|row| row.0).collect::<Vec<_>>();
        if records.iter().any(|record| record.purpose != purpose) {
            return Err(StoreError::Decode(
                "the lifecycle table answered a verification row for another purpose".to_owned(),
            ));
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::PepperStatements;

    /// The pairs the two central deployables are entitled to.
    ///
    /// Named here so a composition that reaches for the other schema's pair is
    /// a visible mistake rather than a silent cross-schema read.
    const IDENTITY: PepperStatements = PepperStatements {
        active: aex_identity_aurora::sql::ACTIVE_IDENTITY_PEPPER,
        by_version: aex_identity_aurora::sql::IDENTITY_PEPPER_BY_VERSION,
        verification_set: None,
    };
    const CONTROL: PepperStatements = PepperStatements {
        active: aex_control_aurora::sql::ACTIVE_CONTROL_PEPPER,
        by_version: aex_control_aurora::sql::CONTROL_PEPPER_BY_VERSION,
        verification_set: Some(aex_control_aurora::sql::LIVE_CONTROL_PEPPERS),
    };

    #[test]
    fn each_pair_reads_its_own_schema_and_no_other() {
        for statement in [IDENTITY.active, IDENTITY.by_version] {
            assert!(
                statement.contains("identity.credential_pepper"),
                "{statement}"
            );
            assert!(!statement.contains("control."), "{statement}");
        }
        for statement in [
            CONTROL.active,
            CONTROL.by_version,
            CONTROL
                .verification_set
                .expect("control verification query"),
        ] {
            assert!(
                statement.contains("control.credential_pepper"),
                "{statement}"
            );
            assert!(!statement.contains("identity."), "{statement}");
        }
    }

    #[test]
    fn every_statement_projects_the_four_lifecycle_columns_and_no_material() {
        for statement in [
            IDENTITY.active,
            IDENTITY.by_version,
            CONTROL.active,
            CONTROL.by_version,
            CONTROL
                .verification_set
                .expect("control verification query"),
        ] {
            assert!(
                statement.contains("version, purpose, state, secret_ref"),
                "the row decoder expects exactly these four: {statement}"
            );
            assert!(
                !statement.to_lowercase().contains("pepper_material"),
                "material is never in the database: {statement}"
            );
        }
    }

    #[test]
    fn the_active_read_relies_on_the_partial_unique_index_rather_than_an_ordering() {
        for statement in [IDENTITY.active, CONTROL.active] {
            assert!(statement.contains("state = 'active'"), "{statement}");
            assert!(
                !statement.contains("ORDER BY"),
                "an ordering would pick a winner where the index guarantees one: {statement}"
            );
        }
    }

    #[test]
    fn a_version_read_is_keyed_by_purpose_and_version_together() {
        for statement in [IDENTITY.by_version, CONTROL.by_version] {
            assert!(
                statement.contains(":purpose") && statement.contains(":version"),
                "{statement}"
            );
        }
    }

    #[test]
    fn a_verification_set_exposes_overflow_instead_of_truncating_silently() {
        assert!(IDENTITY.verification_set.is_none());
        let statement = CONTROL
            .verification_set
            .expect("control verification query");
        assert!(
            statement.contains("state IN ('active', 'retiring')"),
            "{statement}"
        );
        assert!(statement.contains("LIMIT 4"), "{statement}");
    }
}
