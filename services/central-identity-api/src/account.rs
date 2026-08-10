//! The one central account-state read.
//!
//! `account_get` answers one question — may this account consume paid capacity,
//! and if not, what restores it — for a dashboard or a CLI. It is deliberately
//! not the dashboard shell: a client polling for a pause should not pay for
//! every organization, every workspace and the caller's email address.
//!
//! # Who may ask
//!
//! User tokens only. A workspace API key asking whether its own account is
//! paused must ask its own region, where `workspace_current_get` answers from
//! the projection the edge already read, through the same mapping. Admitting the
//! key here would put the SDK's most frequent check on a cross-plane round trip,
//! which is exactly the hop that verifying keys in the region removed.
//!
//! # Why two statements rather than one join
//!
//! "You are not a member of that organization" and "that organization's account
//! state cannot be established" are different answers — `403` and a retryable
//! `503` — and a caller acts on them differently. One joined query returning no
//! row could only say "nothing", so the membership check runs first and on its
//! own.

use std::sync::Arc;

use aex_control_domain::{
    AccountProfile, AccountProjectionError, AccountState, account_operational_state,
};
use aex_identity_app::ports::RequestContext as IdentityContext;
use aex_rds_data::{DataApiClient, DecodeError, Record, Row, SqlValue, Statement};
use aex_wire::error::{ErrorCode, WireError, WireResult};
use aex_wire::idempotency::PrincipalScope;
use aex_wire::ids::PrefixedId as _;
use aex_wire::models::{AccountGetQuery, AccountOperationalState};
use aex_wire::server::{IdentityApi, RequestContext};

/// Reads the account state one organization publishes.
///
/// A narrow port rather than the whole control store: this deployable's login
/// role holds `SELECT` on exactly two tables outside its own schema, and a store
/// that could reach further would be a privilege this role does not have,
/// discovered at runtime instead of at review.
#[async_trait::async_trait]
pub trait AccountReader: Send + Sync {
    /// Whether `user` is an active member of `organization`.
    ///
    /// # Errors
    ///
    /// Returns a redacted diagnostic when the authority cannot answer. An
    /// unreachable authority is never read as "not a member".
    async fn is_member(&self, organization: uuid::Uuid, user: uuid::Uuid) -> Result<bool, String>;

    /// One organization's published account profile.
    ///
    /// # Errors
    ///
    /// Identical to [`AccountReader::is_member`].
    async fn account_profile(
        &self,
        organization: uuid::Uuid,
    ) -> Result<Option<AccountProfile>, String>;
}

/// The Aurora implementation, over the two granted statements.
#[derive(Debug, Clone)]
pub struct AuroraAccountReader {
    client: DataApiClient,
}

impl AuroraAccountReader {
    /// Binds the reader to the cluster this deployable already opened.
    #[must_use]
    pub const fn new(client: DataApiClient) -> Self {
        Self { client }
    }
}

/// The `SELECT 1` membership probe.
struct MemberRow;

impl Row for MemberRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(1)?;
        record.i64(0)?;
        Ok(Self)
    }
}

/// The published account state, decoded strictly.
///
/// `Unavailable` is refused rather than decoded: it is not a state an account
/// is in, it is the absence of one, and a row carrying it is corrupt.
struct AccountProfileRow(AccountProfile);

impl Row for AccountProfileRow {
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError> {
        record.expect_arity(4)?;
        let state = AccountState::parse(record.text(0)?).ok_or(DecodeError::TypeMismatch {
            index: 0,
            expected: "an account state",
        })?;
        if state == AccountState::Unavailable {
            return Err(DecodeError::TypeMismatch {
                index: 0,
                expected: "a durable active or paused account profile",
            });
        }
        Ok(Self(AccountProfile {
            state,
            reason: record.opt(1, |row, index| Ok(row.text(index)?.to_owned()))?,
            revision: u64::try_from(record.i64(2)?)
                .map_err(|_| DecodeError::Overflow { index: 2 })?,
            changed_at: time::OffsetDateTime::from_unix_timestamp_nanos(
                i128::from(record.i64(3)?) * 1_000_000,
            )
            .map_err(|_| DecodeError::Overflow { index: 3 })?,
        }))
    }
}

#[async_trait::async_trait]
impl AccountReader for AuroraAccountReader {
    async fn is_member(&self, organization: uuid::Uuid, user: uuid::Uuid) -> Result<bool, String> {
        self.client
            .query_opt::<MemberRow>(
                Statement::new(aex_identity_aurora::sql::CALLER_ORGANIZATION_MEMBERSHIP)
                    .bind("organization_id", SqlValue::Uuid(organization))
                    .bind("user_id", SqlValue::Uuid(user)),
            )
            .await
            .map(|row| row.is_some())
            .map_err(|error| error.to_string())
    }

    async fn account_profile(
        &self,
        organization: uuid::Uuid,
    ) -> Result<Option<AccountProfile>, String> {
        self.client
            .query_opt::<AccountProfileRow>(
                Statement::new(aex_identity_aurora::sql::ACCOUNT_OPERATIONAL_STATE)
                    .bind("organization_id", SqlValue::Uuid(organization)),
            )
            .await
            .map(|row| row.map(|row| row.0))
            .map_err(|error| error.to_string())
    }
}

/// The mounted `central:identity` handler.
pub struct AccountService {
    reader: Arc<dyn AccountReader>,
}

impl std::fmt::Debug for AccountService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("AccountService").finish()
    }
}

impl AccountService {
    /// Builds the service over its one authority.
    #[must_use]
    pub fn new(reader: Arc<dyn AccountReader>) -> Self {
        Self { reader }
    }
}

impl IdentityApi for AccountService {
    async fn account_get(
        &self,
        cx: &RequestContext,
        query: AccountGetQuery,
    ) -> WireResult<AccountOperationalState> {
        // The only admitted principal. The route table no longer offers a
        // workspace key an alternative here, so this arm is the edge's decision
        // restated rather than a second policy.
        let PrincipalScope::Account { user, .. } = cx.principal else {
            return Err(WireError::new(ErrorCode::Unauthenticated));
        };
        let organization = uuid::Uuid::from_bytes(*query.organization_id.uuid7().as_bytes());
        if !self
            .reader
            .is_member(
                organization,
                uuid::Uuid::from_bytes(*user.uuid7().as_bytes()),
            )
            .await
            .map_err(|_| WireError::new(ErrorCode::AccountStateUnavailable))?
        {
            // Deliberately `forbidden` for both "not a member" and "no such
            // organization": the two are distinguishable only to somebody who is
            // already a member, and answering `404` for one of them turns this
            // route into an organization-existence oracle.
            return Err(WireError::new(ErrorCode::Forbidden));
        }
        let profile = self
            .reader
            .account_profile(organization)
            .await
            .map_err(|_| WireError::new(ErrorCode::AccountStateUnavailable))?
            .ok_or_else(|| WireError::new(ErrorCode::AccountStateUnavailable))?;
        account_operational_state(&profile).map_err(|error| match error {
            AccountProjectionError::Unavailable => {
                WireError::new(ErrorCode::AccountStateUnavailable)
            }
            // A corrupt projected row is still not an answer, so it is still not
            // `Active`. The route declares no other code that could carry it.
            AccountProjectionError::MissingPauseCause
            | AccountProjectionError::UnknownPauseCause(_)
            | AccountProjectionError::UnrepresentableInstant => {
                WireError::new(ErrorCode::AccountStateUnavailable)
            }
        })
    }
}

/// The identity request context this deployable already builds.
///
/// Re-exported so the composition root binds one clock to both services rather
/// than two that could disagree about "now".
pub type Context = IdentityContext;

#[cfg(test)]
mod tests {
    use super::{AccountReader, AccountService};
    use aex_control_domain::{AccountProfile, AccountState};
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::PrincipalScope;
    use aex_wire::ids::{OrganizationId, PrefixedId as _, UserId, Uuid7};
    use aex_wire::models::AccountGetQuery;
    use aex_wire::routes::RouteId;
    use aex_wire::server::{AcceptKind, IdentityApi as _, RequestContext};
    use aex_wire::scopes::ScopeSet;
    use aex_wire::types::RequestId;
    use std::sync::Arc;

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime")
            .block_on(future)
    }

    fn organization() -> OrganizationId {
        OrganizationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
    }

    fn user() -> UserId {
        UserId::from_uuid7(Uuid7::compose(1_754_051_696_789, [5; 10]))
    }

    struct Fake {
        member: bool,
        profile: Option<AccountProfile>,
        reachable: bool,
    }

    #[async_trait::async_trait]
    impl AccountReader for Fake {
        async fn is_member(
            &self,
            _organization: uuid::Uuid,
            _user: uuid::Uuid,
        ) -> Result<bool, String> {
            if self.reachable {
                Ok(self.member)
            } else {
                Err("aurora_unavailable".to_owned())
            }
        }

        async fn account_profile(
            &self,
            _organization: uuid::Uuid,
        ) -> Result<Option<AccountProfile>, String> {
            if self.reachable {
                Ok(self.profile.clone())
            } else {
                Err("aurora_unavailable".to_owned())
            }
        }
    }

    fn context(principal: PrincipalScope) -> RequestContext {
        RequestContext {
            request_id: RequestId::parse("req-account-1").expect("a request id"),
            route: RouteId::AccountGet,
            principal,
            granted_scopes: ScopeSet::default(),
            idempotency_key: None,
            operation_id: None,
            if_match: None,
            accept: AcceptKind::Json,
        }
    }

    fn service(fake: Fake) -> AccountService {
        AccountService::new(Arc::new(fake))
    }

    fn query() -> AccountGetQuery {
        AccountGetQuery {
            organization_id: organization(),
        }
    }

    #[test]
    fn a_member_reads_the_state_the_one_mapping_produces() {
        let service = service(Fake {
            member: true,
            profile: Some(AccountProfile {
                state: AccountState::PausedTopUpRequired,
                reason: Some("top_up_required".to_owned()),
                revision: 11,
                changed_at: time::OffsetDateTime::from_unix_timestamp(1_800_000_000)
                    .expect("an instant"),
            }),
            reachable: true,
        });
        let state = run(service.account_get(
            &context(PrincipalScope::Account {
                user: user(),
                organization: Some(organization()),
            }),
            query(),
        ))
        .expect("a member reads its account");
        let encoded = serde_json::to_value(state).expect("the state encodes");
        assert_eq!(encoded["status"], "paused");
        assert_eq!(encoded["reason"], "top_up_required");
        assert_eq!(encoded["revision"], 11);
        assert_eq!(
            encoded["minimumRestoreCents"], "2000",
            "the one hold a payment clears must name the amount that clears it"
        );
    }

    /// A workspace key is refused here and told nothing about the account.
    ///
    /// The route table stopped offering it this answer; this is the handler
    /// agreeing rather than trusting that it never arrives.
    #[test]
    fn a_workspace_key_is_refused_and_must_ask_its_own_region() {
        let service = service(Fake {
            member: true,
            profile: None,
            reachable: true,
        });
        let error = run(service.account_get(
            &context(PrincipalScope::WorkspaceKey {
                key: aex_wire::ids::ApiKeyId::from_uuid7(Uuid7::compose(1, [1; 10])),
                workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
                organization: organization(),
            }),
            query(),
        ))
        .expect_err("a key has no answer here");
        assert_eq!(error.code, ErrorCode::Unauthenticated);
    }

    /// Membership is checked before the account is read, and an unreachable
    /// authority is never read as "not a member" — that would turn an outage
    /// into a permanent-looking `403`.
    #[test]
    fn a_non_member_is_forbidden_and_an_unreachable_authority_is_not() {
        let forbidden = run(service(Fake {
            member: false,
            profile: None,
            reachable: true,
        })
        .account_get(
            &context(PrincipalScope::Account {
                user: user(),
                organization: Some(organization()),
            }),
            query(),
        ))
        .expect_err("a stranger reads nothing");
        assert_eq!(forbidden.code, ErrorCode::Forbidden);

        let unavailable = run(service(Fake {
            member: true,
            profile: None,
            reachable: false,
        })
        .account_get(
            &context(PrincipalScope::Account {
                user: user(),
                organization: Some(organization()),
            }),
            query(),
        ))
        .expect_err("an unreachable authority answers nothing");
        assert_eq!(unavailable.code, ErrorCode::AccountStateUnavailable);
    }

    /// An organization with no account row is not an active account.
    #[test]
    fn an_absent_account_row_is_never_reported_as_active() {
        let error = run(service(Fake {
            member: true,
            profile: None,
            reachable: true,
        })
        .account_get(
            &context(PrincipalScope::Account {
                user: user(),
                organization: Some(organization()),
            }),
            query(),
        ))
        .expect_err("an absent row is not a state");
        assert_eq!(error.code, ErrorCode::AccountStateUnavailable);
    }
}
