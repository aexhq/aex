//! The read-only authorization surface over the Data `API`.
//!
//! Every method issues **exactly one** statement and opens **no** transaction.
//! That is the pinned per-refresh I/O budget, and `statements.rs` counts it
//! against a stub rather than trusting the code to keep it.

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::Uuid;

use aex_control_app::ports::{
    AccountActorState, AuthorizationReader, CentralActorState, SigningKeyRecord, StoreError,
    WorkspaceKeyState,
};
use aex_control_domain::{Scope, ScopeSet};
use aex_rds_data::{DataApiClient, SqlValue, Statement};

use crate::error::map_store_error;
use crate::rows::{AccountActorRow, CentralActorRow, SigningKeyRow, WorkspaceKeyRow};
use crate::sql;

/// The authorization reader.
#[derive(Debug, Clone)]
pub struct AuroraAuthorizationReader {
    client: DataApiClient,
}

impl AuroraAuthorizationReader {
    /// Builds a reader over a Data `API` client.
    ///
    /// The client must be configured with the `aex_authz` login role, which
    /// holds `SELECT` and nothing else anywhere in the database.
    #[must_use]
    pub const fn new(client: DataApiClient) -> Self {
        Self { client }
    }

    /// The instant the credential-liveness predicates are evaluated against.
    fn now_ms(now: OffsetDateTime) -> i64 {
        i64::try_from(now.unix_timestamp_nanos().div_euclid(1_000_000)).unwrap_or(i64::MAX)
    }

    /// Resolves a workspace-scoped actor through one of the two credential
    /// statements. Both take the same parameters and produce the same row.
    async fn resolve_workspace_actor(
        &self,
        statement: &'static str,
        credential_id: Uuid,
        workspace_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError> {
        let row: Option<AccountActorRow> = self
            .client
            .query_opt(
                Statement::new(statement)
                    .bind("credential_id", SqlValue::Uuid(credential_id))
                    .bind("workspace_id", SqlValue::Uuid(workspace_id))
                    .bind("now_ms", SqlValue::I64(Self::now_ms(now))),
            )
            .await
            .map_err(map_store_error)?;
        Ok(row.map(|row| row.0))
    }

    /// Resolves one central credential and its active memberships in one read.
    async fn resolve_central_actor(
        &self,
        statement: &'static str,
        credential_id: Uuid,
        dashboard_session: bool,
        now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError> {
        let row: Option<CentralActorRow> = self
            .client
            .query_opt(
                Statement::new(statement)
                    .bind("credential_id", SqlValue::Uuid(credential_id))
                    .bind("now_ms", SqlValue::I64(Self::now_ms(now))),
            )
            .await
            .map_err(map_store_error)?;
        Ok(row.map(|row| {
            let mut state = row.0;
            if dashboard_session {
                // The generated contract admits a browser session only on the
                // bootstrap route. Keep that credential at the exact route
                // scope rather than treating it as an unrestricted token.
                state.scopes = ScopeSet::of(&[Scope::AccountRead]);
            }
            state
        }))
    }
}

#[async_trait]
impl AuthorizationReader for AuroraAuthorizationReader {
    async fn resolve_workspace_key(
        &self,
        key_id: Uuid,
    ) -> Result<Option<WorkspaceKeyState>, StoreError> {
        let row: Option<WorkspaceKeyRow> = self
            .client
            .query_opt(
                Statement::new(sql::RESOLVE_WORKSPACE_KEY).bind("key_id", SqlValue::Uuid(key_id)),
            )
            .await
            .map_err(map_store_error)?;
        Ok(row.map(|row| row.0))
    }

    async fn resolve_account_token_for_workspace(
        &self,
        token_id: Uuid,
        workspace_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError> {
        self.resolve_workspace_actor(
            sql::RESOLVE_ACCOUNT_TOKEN_FOR_WORKSPACE,
            token_id,
            workspace_id,
            now,
        )
        .await
    }

    async fn resolve_session_for_workspace(
        &self,
        session_id: Uuid,
        workspace_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<AccountActorState>, StoreError> {
        self.resolve_workspace_actor(
            sql::RESOLVE_SESSION_FOR_WORKSPACE,
            session_id,
            workspace_id,
            now,
        )
        .await
    }

    async fn resolve_account_token_central(
        &self,
        token_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError> {
        self.resolve_central_actor(sql::RESOLVE_ACCOUNT_TOKEN_CENTRAL, token_id, false, now)
            .await
    }

    async fn resolve_dashboard_session_central(
        &self,
        session_id: Uuid,
        now: OffsetDateTime,
    ) -> Result<Option<CentralActorState>, StoreError> {
        self.resolve_central_actor(sql::RESOLVE_SESSION_CENTRAL, session_id, true, now)
            .await
    }

    async fn verification_key_set(&self) -> Result<Vec<SigningKeyRecord>, StoreError> {
        let rows: Vec<SigningKeyRow> = self
            .client
            .query(Statement::new(sql::VERIFICATION_KEY_SET))
            .await
            .map_err(map_store_error)?;
        Ok(rows.into_iter().map(|row| row.0).collect())
    }

    async fn active_signing_key(&self) -> Result<SigningKeyRecord, StoreError> {
        let row: Option<SigningKeyRow> = self
            .client
            .query_opt(Statement::new(sql::ACTIVE_SIGNING_KEY))
            .await
            .map_err(map_store_error)?;
        row.map(|row| row.0).ok_or(StoreError::NotFound)
    }
}
