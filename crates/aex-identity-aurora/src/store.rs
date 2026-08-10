//! The Aurora implementation of the coarse identity authority.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use aex_control_domain::ScopeSet;
use aex_identity_app::ports::{
    ConsumeDeviceAuthorizationCommand, ConsumeEmailChallengeCommand, CreateDashboardSessionCommand,
    CreateDeviceAuthorizationCommand, DecideDeviceAuthorizationCommand, DeviceConsumeOutcome,
    IdentityStore, IssueEmailChallengeCommand, PepperKeystore, PepperPurpose, ReconcileIdentity,
    ResolveDashboardSessionQuery, ResolveExternalIdentity, ResolvedUser, RevokeAccountTokenCommand,
    RevokeDashboardSessionCommand, SetUserStatusCommand, StoreError, TxOutcome, UnknownCommit,
    UnlinkExternalIdentityCommand,
};
use aex_identity_domain::{
    AccountToken, ChallengeState, DashboardSession, DeviceAuthorization, EmailChallenge,
    ExternalIdentity, PepperVersion, PresentedDigest, TokenOrigin, User, UserStatus, Verifier,
    verify,
};
use aex_rds_data::{DataApiClient, Isolation, SqlValue, Statement, Transaction};

use crate::error::{map_commit_failure, map_store_error};
use crate::rows::{
    AccountTokenRow, CountRow, DashboardSessionRow, DeviceAuthorizationRow, EmailChallengeRow,
    ExternalIdentityRow, UserRow,
};
use crate::sql;

macro_rules! tx_try {
    ($transaction:ident, $future:expr) => {
        match $future.await {
            Ok(value) => value,
            Err(error) => {
                let mapped = map_store_error(error);
                let _ = $transaction.rollback().await;
                return Err(mapped);
            }
        }
    };
}

/// Ends the transaction and reports the domain guard that refused.
///
/// The rollback is the point: a ceremony that returns early without ending its
/// transaction strands it on the service until the idle timeout, and a guard
/// that refuses is the most common early return there is. Spelling it once
/// means no site can forget the half that has no compiler to remind it.
macro_rules! tx_conflict {
    ($transaction:ident, $constraint:expr) => {{
        let _ = $transaction.rollback().await;
        return Err(StoreError::Conflict {
            constraint: $constraint,
        });
    }};
}

/// The identity repository over the one Data API transport.
#[derive(Clone)]
pub struct AuroraIdentityStore {
    client: DataApiClient,
    peppers: Arc<dyn PepperKeystore>,
}

impl fmt::Debug for AuroraIdentityStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuroraIdentityStore")
            .field("client", &self.client)
            .field("peppers", &"<redacted>")
            .finish()
    }
}

impl AuroraIdentityStore {
    /// Builds the identity authority.
    #[must_use]
    pub fn new(client: DataApiClient, peppers: Arc<dyn PepperKeystore>) -> Self {
        Self { client, peppers }
    }

    fn millis(instant: OffsetDateTime) -> i64 {
        i64::try_from(instant.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
    }

    fn optional_text(value: Option<&String>) -> SqlValue {
        value.map_or(SqlValue::Null, |value| SqlValue::Text(value.clone()))
    }

    fn scope_values(scopes: ScopeSet) -> Vec<String> {
        scopes.to_strings()
    }

    async fn commit<T>(
        transaction: Transaction<'_>,
        ceremony: &'static str,
        id: Uuid,
        value: T,
    ) -> Result<TxOutcome<T>, StoreError> {
        match transaction.commit().await {
            Ok(_) => Ok(TxOutcome::Committed(value)),
            Err(failure) => match map_commit_failure(failure) {
                StoreError::Unknown => Ok(TxOutcome::Unknown(UnknownCommit {
                    identity: ReconcileIdentity { ceremony, id },
                })),
                error => Err(error),
            },
        }
    }

    async fn links_in(
        transaction: &mut Transaction<'_>,
        user_id: Uuid,
    ) -> Result<Vec<ExternalIdentity>, aex_rds_data::DataApiError> {
        transaction
            .query::<ExternalIdentityRow>(
                Statement::new(sql::LIST_EXTERNAL_IDENTITIES)
                    .bind("user_id", SqlValue::Uuid(user_id)),
            )
            .await
            .map(|rows| rows.into_iter().map(|row| row.0).collect())
    }

    async fn user_by_email_in(
        transaction: &mut Transaction<'_>,
        email: &str,
    ) -> Result<Option<User>, aex_rds_data::DataApiError> {
        transaction
            .query_opt::<UserRow>(
                Statement::new(sql::FIND_USER_BY_EMAIL)
                    .bind("email", SqlValue::Text(email.to_owned())),
            )
            .await
            .map(|row| row.map(|row| row.0))
    }

    async fn credential_matches(
        &self,
        version: PepperVersion,
        digest: &PresentedDigest,
        stored: &Verifier,
    ) -> Result<bool, StoreError> {
        let pepper = self
            .peppers
            .by_version(PepperPurpose::Identity, version)
            .await?;
        Ok(verify(&pepper, digest, stored))
    }

    async fn resolved_user_in(
        transaction: &mut Transaction<'_>,
        user: User,
        created: bool,
    ) -> Result<ResolvedUser, aex_rds_data::DataApiError> {
        let links = Self::links_in(transaction, user.id).await?;
        Ok(ResolvedUser {
            user,
            created,
            links,
        })
    }

    async fn external_user_in(
        transaction: &mut Transaction<'_>,
        command: &ResolveExternalIdentity,
    ) -> Result<(User, bool), aex_rds_data::DataApiError> {
        if let Some(user) = Self::user_by_email_in(transaction, command.email.as_str()).await? {
            return Ok((user, false));
        }
        transaction
            .execute(
                Statement::new(sql::INSERT_USER)
                    .bind("id", SqlValue::Uuid(command.preassigned_user_id))
                    .bind("email", SqlValue::Text(command.email.as_str().to_owned()))
                    .bind("email_verified", SqlValue::Bool(command.email_verified))
                    .bind("name", Self::optional_text(command.name.as_ref()))
                    .bind("image_url", Self::optional_text(command.image_url.as_ref()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    ),
            )
            .await?;
        Ok((
            User {
                id: command.preassigned_user_id,
                email: command.email.clone(),
                email_verified_at: command.email_verified.then_some(command.now),
                name: command.name.clone(),
                image_url: command.image_url.clone(),
                status: UserStatus::Active,
                revision: aex_control_domain::Revision::INITIAL,
                created_at: command.now,
                updated_at: command.now,
            },
            true,
        ))
    }

    async fn email_challenge_user_in(
        transaction: &mut Transaction<'_>,
        challenge: &EmailChallenge,
        preassigned_user_id: Uuid,
        now: OffsetDateTime,
        replayed: bool,
    ) -> Result<Option<(User, bool)>, aex_rds_data::DataApiError> {
        if let Some(user) = Self::user_by_email_in(transaction, challenge.email.as_str()).await? {
            return Ok(Some((user, false)));
        }
        if replayed {
            return Ok(None);
        }
        transaction
            .execute(
                Statement::new(sql::INSERT_USER)
                    .bind("id", SqlValue::Uuid(preassigned_user_id))
                    .bind("email", SqlValue::Text(challenge.email.as_str().to_owned()))
                    .bind("email_verified", SqlValue::Bool(true))
                    .bind("name", SqlValue::Null)
                    .bind("image_url", SqlValue::Null)
                    .bind("now_ms", SqlValue::TimestampMillis(Self::millis(now))),
            )
            .await?;
        Ok(Some((
            User {
                id: preassigned_user_id,
                email: challenge.email.clone(),
                email_verified_at: Some(now),
                name: None,
                image_url: None,
                status: UserStatus::Active,
                revision: aex_control_domain::Revision::INITIAL,
                created_at: now,
                updated_at: now,
            },
            true,
        )))
    }

    /// Mints the account token a redemption produces.
    ///
    /// Runs **before** `CONSUME_DEVICE_AUTHORIZATION`, never after.
    /// `identity.device_authorization.account_token_id` references
    /// `identity.account_token(id)` and that constraint is not `DEFERRABLE` —
    /// the only deferred constraints in the bundle are the control and finance
    /// constraint triggers — so `PostgreSQL` checks it while the `UPDATE` runs
    /// rather than at `COMMIT`. Consuming first raised 23503 on every single
    /// redemption, which is why no device code had ever been redeemed outside
    /// CI's raw-SQL bypass.
    ///
    /// The order is safe in this direction because both statements share one
    /// serializable transaction: a token whose consumption then loses the race
    /// is rolled back with it, so no orphan token survives, and the conditional
    /// `UPDATE` remains the sole arbiter of who redeems.
    async fn insert_device_account_token(
        transaction: &mut Transaction<'_>,
        command: &ConsumeDeviceAuthorizationCommand,
        user_id: Uuid,
        scopes: ScopeSet,
    ) -> Result<(), aex_rds_data::DataApiError> {
        transaction
            .execute(
                Statement::new(sql::INSERT_ACCOUNT_TOKEN)
                    .bind("id", SqlValue::Uuid(command.preassigned_token_id))
                    .bind("user_id", SqlValue::Uuid(user_id))
                    .bind(
                        "verifier",
                        SqlValue::Bytes(command.token_verifier.as_bytes().to_vec()),
                    )
                    .bind(
                        "pepper_version",
                        SqlValue::I64(i64::from(command.token_pepper_version.get())),
                    )
                    .bind("name", SqlValue::Text(command.token_name.clone()))
                    .bind("scopes", SqlValue::TextArray(Self::scope_values(scopes)))
                    .bind(
                        "issued_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now)),
                    )
                    .bind(
                        "expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.token_expires_at)),
                    ),
            )
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "transport contract tests stay next to the store helpers they exercise"
)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use aex_control_domain::{Scope, ScopeSet};
    use aex_identity_app::ports::{
        ConsumeDeviceAuthorizationCommand, IdentityStore, IssueEmailChallengeCommand,
        PepperKeystore, PepperPurpose, StoreError, TxOutcome,
    };
    use aex_identity_domain::{
        NormalizedEmail, Pepper, PepperVersion, PresentedDigest, Verifier, verifier,
    };
    use aex_rds_data::{
        DataApiClient, DataApiConfig, DatabaseName, ExecuteResponse, ResourceArn, SecretArn,
        TransactionId, Transport, TransportError,
    };
    use async_trait::async_trait;
    use aws_sdk_rdsdata::primitives::Blob;
    use aws_sdk_rdsdata::types::{ArrayValue, Field, SqlParameter};
    use time::{Duration, OffsetDateTime};
    use uuid::Uuid;

    use super::AuroraIdentityStore;
    use crate::sql;

    #[derive(Debug)]
    struct ScriptedTransport {
        statements: Mutex<Vec<String>>,
        commit_is_unknown: bool,
        /// What a read answers, keyed by the statement's own text.
        ///
        /// Empty for the ceremonies that only need the ledger. A ceremony that
        /// resolves before it writes has to be handed the row it resolves, or
        /// it takes the not-found branch and never reaches the write under
        /// test — which is exactly how the redemption order went unexamined.
        records: HashMap<&'static str, Vec<Vec<Field>>>,
    }

    /// The statements this fixture reports one affected row for.
    ///
    /// A conditional `UPDATE` that reports zero is a lost race, and every
    /// ceremony here branches on that, so the fixture has to state which
    /// statements won rather than answer zero for all of them.
    fn affects_one_row(statement: &str) -> bool {
        statement == sql::INSERT_EMAIL_CHALLENGE || statement == sql::CONSUME_DEVICE_AUTHORIZATION
    }

    #[async_trait]
    impl Transport for ScriptedTransport {
        async fn execute(
            &self,
            statement: &str,
            _parameters: Vec<SqlParameter>,
            _transaction: Option<&TransactionId>,
        ) -> Result<ExecuteResponse, TransportError> {
            self.statements
                .lock()
                .expect("statement ledger")
                .push(statement.to_owned());
            Ok(ExecuteResponse {
                records: self.records.get(statement).cloned().unwrap_or_default(),
                rows_affected: u64::from(affects_one_row(statement)),
            })
        }

        async fn begin(&self) -> Result<TransactionId, TransportError> {
            Ok(TransactionId::new("tx-identity"))
        }

        async fn commit(&self, _transaction: &TransactionId) -> Result<String, TransportError> {
            if self.commit_is_unknown {
                Err(TransportError::Indeterminate {
                    message: "response lost".to_owned(),
                })
            } else {
                Ok("Transaction Committed".to_owned())
            }
        }

        async fn rollback(&self, _transaction: &TransactionId) -> Result<(), TransportError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FixedPepper;

    #[async_trait]
    impl PepperKeystore for FixedPepper {
        async fn active(
            &self,
            _purpose: PepperPurpose,
        ) -> Result<(PepperVersion, Pepper), StoreError> {
            Ok((PepperVersion::new(1), Pepper::new([7; 32])))
        }

        async fn by_version(
            &self,
            _purpose: PepperPurpose,
            _version: PepperVersion,
        ) -> Result<Pepper, StoreError> {
            Ok(Pepper::new([7; 32]))
        }
    }

    fn store(commit_is_unknown: bool) -> (AuroraIdentityStore, Arc<ScriptedTransport>) {
        store_answering(HashMap::new(), commit_is_unknown)
    }

    fn store_answering(
        records: HashMap<&'static str, Vec<Vec<Field>>>,
        commit_is_unknown: bool,
    ) -> (AuroraIdentityStore, Arc<ScriptedTransport>) {
        let transport = Arc::new(ScriptedTransport {
            statements: Mutex::new(Vec::new()),
            commit_is_unknown,
            records,
        });
        let config = DataApiConfig::new(
            ResourceArn::parse("arn:aws:rds:eu-west-1:000000000000:cluster:aex").expect("arn"),
            SecretArn::parse("arn:aws:secretsmanager:eu-west-1:000000000000:secret:aex-x")
                .expect("arn"),
            DatabaseName::parse("aex").expect("database"),
        );
        (
            AuroraIdentityStore::new(
                DataApiClient::new(transport.clone(), config),
                Arc::new(FixedPepper),
            ),
            transport,
        )
    }

    fn challenge() -> IssueEmailChallengeCommand {
        IssueEmailChallengeCommand {
            preassigned_id: Uuid::from_u128(7),
            email: NormalizedEmail::parse("alice@example.com").expect("email"),
            verifier: Verifier::from_bytes([9; 32]),
            pepper_version: PepperVersion::new(1),
            issued_at: OffsetDateTime::UNIX_EPOCH,
            expires_at: OffsetDateTime::UNIX_EPOCH + Duration::minutes(15),
        }
    }

    #[tokio::test]
    async fn issuing_a_challenge_is_one_atomic_insert_after_the_replay_lookup() {
        let (store, transport) = store(false);
        let outcome = store
            .issue_email_challenge(&challenge())
            .await
            .expect("issued");
        assert!(matches!(outcome, TxOutcome::Committed(_)));
        assert_eq!(
            transport.statements.lock().expect("ledger").as_slice(),
            [sql::RESOLVE_EMAIL_CHALLENGE, sql::INSERT_EMAIL_CHALLENGE]
        );
    }

    /// The digest the scripted device presents, and the verifier stored for it.
    fn device_digest() -> PresentedDigest {
        PresentedDigest::from_bytes([3; 32])
    }

    /// One `RESOLVE_DEVICE_AUTHORIZATION` record for a live, approved grant.
    ///
    /// The column order is `RESOLVE_DEVICE_AUTHORIZATION`'s own projection; a
    /// record built in any other order fails `expect_arity` or decodes into the
    /// wrong field, so this doubles as a check that the two stay in step.
    fn approved_device_record(user_id: Uuid, now: OffsetDateTime) -> Vec<Field> {
        let stored = verifier(&Pepper::new([7; 32]), &device_digest());
        let scopes = AuroraIdentityStore::scope_values(ScopeSet::of(&[Scope::SessionsRead]));
        vec![
            Field::StringValue(Uuid::from_u128(11).to_string()),
            Field::BlobValue(Blob::new(stored.as_bytes().to_vec())),
            Field::LongValue(1),
            Field::StringValue("approved".to_owned()),
            Field::ArrayValue(ArrayValue::StringValues(
                scopes.into_iter().map(Some).collect(),
            )),
            Field::StringValue(user_id.to_string()),
            Field::LongValue(AuroraIdentityStore::millis(now)),
            Field::IsNull(true),
            Field::IsNull(true),
            Field::LongValue(AuroraIdentityStore::millis(now)),
            Field::LongValue(AuroraIdentityStore::millis(now + Duration::minutes(15))),
            Field::LongValue(5_000),
            Field::IsNull(true),
        ]
    }

    /// Redeeming a device grant must insert the token before it is referenced.
    ///
    /// `identity.device_authorization.account_token_id` references
    /// `identity.account_token(id)` and the constraint is not `DEFERRABLE`, so
    /// the reverse order answers 23503 on every single redemption — which it
    /// did, unnoticed, because the only other way to reach an account token is
    /// CI's raw-SQL bypass. The ledger is the cheapest place to state the
    /// order, and it states it without a container.
    #[tokio::test]
    async fn a_redemption_inserts_the_token_before_the_grant_points_at_it() {
        let now = OffsetDateTime::UNIX_EPOCH + Duration::hours(1);
        let user_id = Uuid::from_u128(23);
        let records = HashMap::from([(
            sql::RESOLVE_DEVICE_AUTHORIZATION,
            vec![approved_device_record(user_id, now)],
        )]);
        let (store, transport) = store_answering(records, false);

        let outcome = store
            .consume_device_authorization(&ConsumeDeviceAuthorizationCommand {
                device_id: Uuid::from_u128(11),
                digest: device_digest(),
                preassigned_token_id: Uuid::from_u128(31),
                token_verifier: Verifier::from_bytes([5; 32]),
                token_pepper_version: PepperVersion::new(1),
                token_name: "scripted device".to_owned(),
                token_expires_at: now + Duration::days(30),
                now,
            })
            .await
            .expect("an approved grant redeems");

        let TxOutcome::Committed(redeemed) = outcome else {
            panic!("a scripted commit is committed");
        };
        assert_eq!(redeemed.token.id, Uuid::from_u128(31));
        assert_eq!(redeemed.grant.account_token_id, Some(Uuid::from_u128(31)));
        assert_eq!(
            transport.statements.lock().expect("ledger").as_slice(),
            [
                // `Isolation::Serializable` is issued as a statement of its
                // own, so the ledger opens with it; redemption arbitrates a
                // race and must not run at a weaker level.
                "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE",
                sql::RESOLVE_DEVICE_AUTHORIZATION,
                sql::INSERT_ACCOUNT_TOKEN,
                sql::CONSUME_DEVICE_AUTHORIZATION,
            ],
            "the token row has to exist before `account_token_id` names it"
        );
    }

    #[tokio::test]
    async fn a_lost_commit_returns_the_preassigned_reconciliation_identity() {
        let (store, _) = store(true);
        let outcome = store
            .issue_email_challenge(&challenge())
            .await
            .expect("unknown is typed, not thrown away");
        let TxOutcome::Unknown(unknown) = outcome else {
            panic!("lost commit must stay unknown");
        };
        assert_eq!(unknown.identity.ceremony, "issue_email_challenge");
        assert_eq!(unknown.identity.id, Uuid::from_u128(7));
    }
}

#[async_trait]
impl IdentityStore for AuroraIdentityStore {
    async fn resolve_or_create_by_external_identity(
        &self,
        command: &ResolveExternalIdentity,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let existing = tx_try!(
            transaction,
            transaction.query_opt::<UserRow>(
                Statement::new(sql::FIND_USER_BY_EXTERNAL_IDENTITY)
                    .bind(
                        "provider",
                        SqlValue::Text(command.provider.as_str().to_owned())
                    )
                    .bind(
                        "provider_account_id",
                        SqlValue::Text(command.provider_account_id.as_str().to_owned()),
                    ),
            )
        );
        if let Some(existing) = existing {
            let resolved = tx_try!(
                transaction,
                Self::resolved_user_in(&mut transaction, existing.0, false)
            );
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(resolved));
        }

        let (user, created) = tx_try!(
            transaction,
            Self::external_user_in(&mut transaction, command)
        );
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_EXTERNAL_IDENTITY)
                    .bind("id", SqlValue::Uuid(command.preassigned_link_id))
                    .bind("user_id", SqlValue::Uuid(user.id))
                    .bind(
                        "provider",
                        SqlValue::Text(command.provider.as_str().to_owned())
                    )
                    .bind(
                        "provider_account_id",
                        SqlValue::Text(command.provider_account_id.as_str().to_owned()),
                    )
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        let resolved = tx_try!(
            transaction,
            Self::resolved_user_in(&mut transaction, user, created)
        );
        Self::commit(
            transaction,
            "resolve_external_identity",
            command.preassigned_user_id,
            resolved,
        )
        .await
    }

    async fn issue_email_challenge(
        &self,
        command: &IssueEmailChallengeCommand,
    ) -> Result<TxOutcome<EmailChallenge>, StoreError> {
        if let Some(existing) = self
            .client
            .query_opt::<EmailChallengeRow>(
                Statement::new(sql::RESOLVE_EMAIL_CHALLENGE)
                    .bind("challenge_id", SqlValue::Uuid(command.preassigned_id)),
            )
            .await
            .map_err(map_store_error)?
        {
            let same = existing.value.email == command.email
                && existing.verifier == command.verifier
                && existing.value.pepper_version == command.pepper_version
                && existing.value.issued_at == command.issued_at
                && existing.value.expires_at == command.expires_at;
            return Ok(if same {
                TxOutcome::Replayed(existing.value)
            } else {
                TxOutcome::IntentConflict
            });
        }
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_EMAIL_CHALLENGE)
                    .bind("id", SqlValue::Uuid(command.preassigned_id))
                    .bind("email", SqlValue::Text(command.email.as_str().to_owned()))
                    .bind(
                        "verifier",
                        SqlValue::Bytes(command.verifier.as_bytes().to_vec()),
                    )
                    .bind(
                        "pepper_version",
                        SqlValue::I64(i64::from(command.pepper_version.get())),
                    )
                    .bind(
                        "issued_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.issued_at)),
                    )
                    .bind(
                        "expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.expires_at)),
                    ),
            )
        );
        let challenge = EmailChallenge {
            id: command.preassigned_id,
            email: command.email.clone(),
            pepper_version: command.pepper_version,
            issued_at: command.issued_at,
            expires_at: command.expires_at,
            consumed_at: None,
        };
        Self::commit(
            transaction,
            "issue_email_challenge",
            command.preassigned_id,
            challenge,
        )
        .await
    }

    async fn consume_email_challenge(
        &self,
        command: &ConsumeEmailChallengeCommand,
    ) -> Result<TxOutcome<ResolvedUser>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let Some(challenge) = tx_try!(
            transaction,
            transaction.query_opt::<EmailChallengeRow>(
                Statement::new(sql::RESOLVE_EMAIL_CHALLENGE)
                    .bind("challenge_id", SqlValue::Uuid(command.challenge_id)),
            )
        ) else {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        };
        let matches = match self
            .credential_matches(
                challenge.value.pepper_version,
                &command.digest,
                &challenge.verifier,
            )
            .await
        {
            Ok(matches) => matches,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };
        if !matches {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        }
        if challenge.value.state_at(command.now) == ChallengeState::Expired {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        }
        let replayed = challenge.value.consumed_at.is_some();
        if !replayed {
            let affected = tx_try!(
                transaction,
                transaction.execute(
                    Statement::new(sql::CONSUME_EMAIL_CHALLENGE)
                        .bind("challenge_id", SqlValue::Uuid(command.challenge_id))
                        .bind(
                            "now_ms",
                            SqlValue::TimestampMillis(Self::millis(command.now))
                        ),
                )
            );
            if affected != 1 {
                tx_conflict!(transaction, "email_challenge_open".to_owned());
            }
        }
        let user = tx_try!(
            transaction,
            Self::email_challenge_user_in(
                &mut transaction,
                &challenge.value,
                command.preassigned_user_id,
                command.now,
                replayed,
            )
        );
        let Some((user, created)) = user else {
            let _ = transaction.rollback().await;
            return Err(StoreError::Fatal(
                "a consumed email challenge has no resolved user".to_owned(),
            ));
        };
        let resolved = tx_try!(
            transaction,
            Self::resolved_user_in(&mut transaction, user, created)
        );
        if replayed {
            let _ = transaction.rollback().await;
            Ok(TxOutcome::Replayed(resolved))
        } else {
            Self::commit(
                transaction,
                "consume_email_challenge",
                command.challenge_id,
                resolved,
            )
            .await
        }
    }

    async fn create_dashboard_session(
        &self,
        command: &CreateDashboardSessionCommand,
    ) -> Result<TxOutcome<DashboardSession>, StoreError> {
        if let Some(existing) = self
            .client
            .query_opt::<DashboardSessionRow>(
                Statement::new(sql::RESOLVE_DASHBOARD_SESSION)
                    .bind("session_id", SqlValue::Uuid(command.preassigned_id)),
            )
            .await
            .map_err(map_store_error)?
        {
            let same = existing.session.user_id == command.user_id
                && existing.verifier == command.verifier
                && existing.session.pepper_version == command.pepper_version
                && existing.session.issued_at == command.issued_at
                && existing.session.expires_at == command.expires_at;
            return Ok(if same {
                TxOutcome::Replayed(existing.session)
            } else {
                TxOutcome::IntentConflict
            });
        }
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        let affected = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_DASHBOARD_SESSION)
                    .bind("id", SqlValue::Uuid(command.preassigned_id))
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind(
                        "verifier",
                        SqlValue::Bytes(command.verifier.as_bytes().to_vec()),
                    )
                    .bind(
                        "pepper_version",
                        SqlValue::I64(i64::from(command.pepper_version.get())),
                    )
                    .bind(
                        "issued_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.issued_at)),
                    )
                    .bind(
                        "expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.expires_at)),
                    ),
            )
        );
        if affected != 1 {
            tx_conflict!(transaction, "dashboard_session_active_user".to_owned());
        }
        let session = DashboardSession {
            id: command.preassigned_id,
            user_id: command.user_id,
            pepper_version: command.pepper_version,
            issued_at: command.issued_at,
            expires_at: command.expires_at,
            revoked_at: None,
        };
        Self::commit(
            transaction,
            "create_dashboard_session",
            command.preassigned_id,
            session,
        )
        .await
    }

    async fn resolve_dashboard_session(
        &self,
        query: &ResolveDashboardSessionQuery,
    ) -> Result<Option<(DashboardSession, User)>, StoreError> {
        let Some(row) = self
            .client
            .query_opt::<DashboardSessionRow>(
                Statement::new(sql::RESOLVE_DASHBOARD_SESSION)
                    .bind("session_id", SqlValue::Uuid(query.session_id)),
            )
            .await
            .map_err(map_store_error)?
        else {
            return Ok(None);
        };
        if !self
            .credential_matches(row.session.pepper_version, &query.digest, &row.verifier)
            .await?
        {
            return Ok(None);
        }
        Ok(Some((row.session, row.user)))
    }

    async fn revoke_dashboard_session(
        &self,
        command: &RevokeDashboardSessionCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        let affected = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::REVOKE_DASHBOARD_SESSION)
                    .bind("session_id", SqlValue::Uuid(command.session_id))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        if affected == 0 {
            let existing = tx_try!(
                transaction,
                transaction.query_opt::<DashboardSessionRow>(
                    Statement::new(sql::RESOLVE_DASHBOARD_SESSION)
                        .bind("session_id", SqlValue::Uuid(command.session_id)),
                )
            );
            let _ = transaction.rollback().await;
            return existing.map_or(Err(StoreError::NotFound), |_| Ok(TxOutcome::Replayed(())));
        }
        Self::commit(
            transaction,
            "revoke_dashboard_session",
            command.session_id,
            (),
        )
        .await
    }

    async fn set_user_status(
        &self,
        command: &SetUserStatusCommand,
    ) -> Result<TxOutcome<User>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let Some(current) = tx_try!(
            transaction,
            transaction.query_opt::<UserRow>(
                Statement::new(sql::FIND_USER_BY_ID)
                    .bind("user_id", SqlValue::Uuid(command.user_id)),
            )
        ) else {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        };
        let desired = if command.enabled {
            UserStatus::Active
        } else {
            UserStatus::Disabled
        };
        if current.0.status == desired {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(current.0));
        }
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::SET_USER_STATUS)
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind("status", SqlValue::Text(desired.as_str().to_owned()))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        tx_try!(
            transaction,
            transaction.query_one::<CountRow>(
                Statement::new(sql::BUMP_USER_EPOCH)
                    .bind("user_id", SqlValue::Uuid(command.user_id)),
            )
        );
        let updated = tx_try!(
            transaction,
            transaction.query_one::<UserRow>(
                Statement::new(sql::FIND_USER_BY_ID)
                    .bind("user_id", SqlValue::Uuid(command.user_id)),
            )
        );
        Self::commit(transaction, "set_user_status", command.user_id, updated.0).await
    }

    async fn unlink_external_identity(
        &self,
        command: &UnlinkExternalIdentityCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let affected = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::UNLINK_EXTERNAL_IDENTITY)
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind(
                        "provider",
                        SqlValue::Text(command.provider.as_str().to_owned())
                    ),
            )
        );
        if affected == 0 {
            tx_conflict!(transaction, "external_identity_last_credential".to_owned());
        }
        Self::commit(transaction, "unlink_external_identity", command.user_id, ()).await
    }

    async fn create_device_authorization(
        &self,
        command: &CreateDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        if let Some(existing) = self
            .client
            .query_opt::<DeviceAuthorizationRow>(
                Statement::new(sql::RESOLVE_DEVICE_AUTHORIZATION)
                    .bind("device_id", SqlValue::Uuid(command.preassigned_id)),
            )
            .await
            .map_err(map_store_error)?
        {
            let same = existing.verifier == command.device_verifier
                && existing.value.pepper_version == command.pepper_version
                && existing.value.requested_scopes == command.requested_scopes
                && existing.value.issued_at == command.issued_at
                && existing.value.expires_at == command.expires_at;
            return Ok(if same {
                TxOutcome::Replayed(existing.value)
            } else {
                TxOutcome::IntentConflict
            });
        }
        let mut transaction = self
            .client
            .begin(Isolation::ReadCommitted)
            .await
            .map_err(map_store_error)?;
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::INSERT_DEVICE_AUTHORIZATION)
                    .bind("id", SqlValue::Uuid(command.preassigned_id))
                    .bind(
                        "device_verifier",
                        SqlValue::Bytes(command.device_verifier.as_bytes().to_vec()),
                    )
                    .bind(
                        "user_code_hash",
                        SqlValue::Bytes(command.user_code_hash.to_vec()),
                    )
                    .bind(
                        "pepper_version",
                        SqlValue::I64(i64::from(command.pepper_version.get())),
                    )
                    .bind(
                        "requested_scopes",
                        SqlValue::TextArray(Self::scope_values(command.requested_scopes)),
                    )
                    .bind(
                        "issued_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.issued_at)),
                    )
                    .bind(
                        "expires_at_ms",
                        SqlValue::TimestampMillis(Self::millis(command.expires_at)),
                    )
                    .bind(
                        "poll_interval_ms",
                        SqlValue::I64(i64::from(command.poll_interval_ms)),
                    ),
            )
        );
        let grant = DeviceAuthorization {
            id: command.preassigned_id,
            state: aex_identity_domain::DeviceState::Pending,
            requested_scopes: command.requested_scopes,
            pepper_version: command.pepper_version,
            approved_by: None,
            approved_at: None,
            consumed_at: None,
            account_token_id: None,
            issued_at: command.issued_at,
            expires_at: command.expires_at,
            poll_interval: Duration::milliseconds(i64::from(command.poll_interval_ms)),
            last_polled_at: None,
        };
        Self::commit(
            transaction,
            "create_device_authorization",
            command.preassigned_id,
            grant,
        )
        .await
    }

    async fn approve_device_authorization(
        &self,
        command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        self.decide_device(command, true).await
    }

    async fn deny_device_authorization(
        &self,
        command: &DecideDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        self.decide_device(command, false).await
    }

    async fn poll_device_authorization(
        &self,
        device_id: Uuid,
        digest: &PresentedDigest,
        now: OffsetDateTime,
    ) -> Result<Option<DeviceAuthorization>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let Some(row) = tx_try!(
            transaction,
            transaction.query_opt::<DeviceAuthorizationRow>(
                Statement::new(sql::RESOLVE_DEVICE_AUTHORIZATION)
                    .bind("device_id", SqlValue::Uuid(device_id)),
            )
        ) else {
            let _ = transaction.rollback().await;
            return Ok(None);
        };
        let matches = match self
            .credential_matches(row.value.pepper_version, digest, &row.verifier)
            .await
        {
            Ok(matches) => matches,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };
        if !matches {
            let _ = transaction.rollback().await;
            return Ok(None);
        }
        let before = row.value;
        let (after, _) = before.poll(now);
        tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::UPDATE_DEVICE_POLL)
                    .bind("device_id", SqlValue::Uuid(device_id))
                    .bind(
                        "poll_interval_ms",
                        SqlValue::I64(
                            i64::try_from(after.poll_interval.whole_milliseconds())
                                .unwrap_or(i64::MAX),
                        ),
                    )
                    .bind("now_ms", SqlValue::TimestampMillis(Self::millis(now))),
            )
        );
        match transaction.commit().await {
            Ok(_) => Ok(Some(before)),
            Err(failure) => Err(map_commit_failure(failure)),
        }
    }

    async fn consume_device_authorization(
        &self,
        command: &ConsumeDeviceAuthorizationCommand,
    ) -> Result<TxOutcome<DeviceConsumeOutcome>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let Some(row) = tx_try!(
            transaction,
            transaction.query_opt::<DeviceAuthorizationRow>(
                Statement::new(sql::RESOLVE_DEVICE_AUTHORIZATION)
                    .bind("device_id", SqlValue::Uuid(command.device_id)),
            )
        ) else {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        };
        let matches = match self
            .credential_matches(row.value.pepper_version, &command.digest, &row.verifier)
            .await
        {
            Ok(matches) => matches,
            Err(error) => {
                let _ = transaction.rollback().await;
                return Err(error);
            }
        };
        if !matches {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        }
        if let Some(token_id) = row.value.account_token_id {
            let token = tx_try!(
                transaction,
                transaction.query_one::<AccountTokenRow>(
                    Statement::new(sql::RESOLVE_ACCOUNT_TOKEN)
                        .bind("token_id", SqlValue::Uuid(token_id)),
                )
            );
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(DeviceConsumeOutcome {
                grant: row.value,
                token: token.0,
            }));
        }
        let Some(user_id) = row.value.approved_by else {
            tx_conflict!(transaction, "device_authorization_approved".to_owned());
        };
        // Mint first, then point the grant at it: the foreign key is not
        // deferrable. See `insert_device_account_token` for why the order is
        // load bearing and why it is safe this way round.
        tx_try!(
            transaction,
            Self::insert_device_account_token(
                &mut transaction,
                command,
                user_id,
                row.value.requested_scopes,
            )
        );
        let affected = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::CONSUME_DEVICE_AUTHORIZATION)
                    .bind("device_id", SqlValue::Uuid(command.device_id))
                    .bind("token_id", SqlValue::Uuid(command.preassigned_token_id))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        if affected != 1 {
            tx_conflict!(transaction, "device_authorization_approved".to_owned());
        }
        // Rolled back explicitly rather than with `?`: an early return that
        // leaves the transaction open strands it on the service until the idle
        // timeout, and every other exit from this ceremony ends it.
        let grant = match row.value.consume(command.preassigned_token_id, command.now) {
            Ok(grant) => grant,
            Err(error) => {
                tx_conflict!(transaction, error.to_string());
            }
        };
        let token = AccountToken {
            id: command.preassigned_token_id,
            user_id,
            name: command.token_name.clone(),
            scopes: grant.requested_scopes,
            origin: TokenOrigin::DeviceFlow,
            pepper_version: command.token_pepper_version,
            issued_at: command.now,
            expires_at: command.token_expires_at,
            revoked_at: None,
        };
        Self::commit(
            transaction,
            "consume_device_authorization",
            command.device_id,
            DeviceConsumeOutcome { grant, token },
        )
        .await
    }

    async fn revoke_account_token(
        &self,
        command: &RevokeAccountTokenCommand,
    ) -> Result<TxOutcome<()>, StoreError> {
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let Some(token) = tx_try!(
            transaction,
            transaction.query_opt::<AccountTokenRow>(
                Statement::new(sql::RESOLVE_ACCOUNT_TOKEN)
                    .bind("token_id", SqlValue::Uuid(command.token_id)),
            )
        ) else {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        };
        if token.0.user_id != command.user_id {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        }
        if token.0.revoked_at.is_some() {
            let _ = transaction.rollback().await;
            return Ok(TxOutcome::Replayed(()));
        }
        let affected = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(sql::REVOKE_ACCOUNT_TOKEN)
                    .bind("token_id", SqlValue::Uuid(command.token_id))
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        if affected != 1 {
            tx_conflict!(transaction, "account_token_live".to_owned());
        }
        let remaining = tx_try!(
            transaction,
            transaction.query_one::<CountRow>(
                Statement::new(sql::COUNT_LIVE_ACCOUNT_TOKENS)
                    .bind("user_id", SqlValue::Uuid(command.user_id))
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        if remaining.0 == 0 {
            tx_try!(
                transaction,
                transaction.query_one::<CountRow>(
                    Statement::new(sql::BUMP_USER_EPOCH)
                        .bind("user_id", SqlValue::Uuid(command.user_id)),
                )
            );
        }
        Self::commit(transaction, "revoke_account_token", command.token_id, ()).await
    }
}

impl AuroraIdentityStore {
    async fn decide_device(
        &self,
        command: &DecideDeviceAuthorizationCommand,
        approve: bool,
    ) -> Result<TxOutcome<DeviceAuthorization>, StoreError> {
        let statement = if approve {
            sql::APPROVE_DEVICE_AUTHORIZATION
        } else {
            sql::DENY_DEVICE_AUTHORIZATION
        };
        let mut transaction = self
            .client
            .begin(Isolation::Serializable)
            .await
            .map_err(map_store_error)?;
        let affected = tx_try!(
            transaction,
            transaction.execute(
                Statement::new(statement)
                    .bind(
                        "user_code_hash",
                        SqlValue::Bytes(command.user_code_hash.to_vec()),
                    )
                    .bind("actor_user_id", SqlValue::Uuid(command.actor_user_id))
                    .bind("actor_session_id", SqlValue::Uuid(command.actor_session_id),)
                    .bind(
                        "now_ms",
                        SqlValue::TimestampMillis(Self::millis(command.now))
                    ),
            )
        );
        if affected != 1 {
            let _ = transaction.rollback().await;
            return Err(StoreError::NotFound);
        }
        let row = tx_try!(
            transaction,
            transaction.query_one::<DeviceAuthorizationRow>(
                Statement::new(sql::RESOLVE_DEVICE_BY_USER_CODE).bind(
                    "user_code_hash",
                    SqlValue::Bytes(command.user_code_hash.to_vec()),
                ),
            )
        );
        Self::commit(
            transaction,
            if approve {
                "approve_device"
            } else {
                "deny_device"
            },
            row.value.id,
            row.value,
        )
        .await
    }
}
