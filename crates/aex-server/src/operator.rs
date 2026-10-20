use crate::{
    App,
    error::{Error, Result},
    identity,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, Method},
    routing::{get, post},
};
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    CreateAccount,
    IssueKey {
        account: String,
    },
    RevokeKey {
        key: String,
    },
    SuspendAccount {
        account: String,
    },
    ResumeAccount {
        account: String,
    },
    Inspect,
    ResolveCreate {
        account: String,
        client_key: String,
    },
    ForgetExecutor {
        executor: String,
    },
    Drain,
    Resume,
    DeleteOrphan {
        account: String,
        session: String,
    },
    ReconcileTurn {
        session: String,
    },
    ReportUsage {
        observed_at: i64,
        sessions: std::collections::BTreeMap<String, u64>,
    },
    Maintain,
    RecordMeteredUsage {
        report: crate::billing::UsageReport,
    },
    AdjustCredits {
        account: String,
        reference: String,
        delta_micro_usd: i64,
        reason: String,
    },
    Orphans,
}

pub fn router(app: App) -> Router {
    let limit = app.config.limits.response_bytes;
    Router::new()
        .route("/operate", post(operate))
        .route("/tools/authorize", post(crate::managed::authorize))
        .route("/tools/usage", post(crate::managed::report))
        .route("/tools/pending", get(crate::managed::pending))
        .route("/tools/cleaned", post(crate::managed::cleaned))
        .route("/tools/journal", post(crate::managed::journal::handle))
        .layer(axum::extract::DefaultBodyLimit::max(limit))
        .with_state(app)
}

async fn operate(
    State(app): State<App>,
    headers: HeaderMap,
    Json(operation): Json<Operation>,
) -> Result<Json<Value>> {
    if !identity::matches(identity::bearer(&headers)?, &app.operator_verifier) {
        return Err(Error::denied());
    }
    execute(&app, operation).await.map(Json)
}
pub async fn execute(app: &App, operation: Operation) -> Result<Value> {
    let _operator_guard = app.operator_lock.lock().await;
    let result = match operation {
        Operation::RecordMeteredUsage { report } => {
            crate::billing::meter(&app.store, &report).await?;
            if report.terminal {
                sqlx::query("UPDATE tool_grants SET settled=TRUE WHERE id=$1 AND provider='sandbox' AND revoked").bind(&report.reservation).execute(&app.store.0).await?;
            }
            json!({"recorded":true})
        }
        Operation::AdjustCredits {
            account,
            reference,
            delta_micro_usd,
            reason,
        } => {
            crate::billing::adjust(&app.store, &account, &reference, delta_micro_usd, &reason)
                .await?;
            json!({"recorded":true})
        }
        Operation::Orphans => {
            require_drained(app)?;
            let accounts: Vec<String> = sqlx::query_scalar("SELECT id FROM accounts")
                .fetch_all(&app.store.0)
                .await?;
            let mut orphans = Vec::new();
            for account in accounts {
                let mut after = String::new();
                loop {
                    let response = app
                        .brain
                        .request(
                            Method::GET,
                            &format!("/sessions?limit=1000&after={after}"),
                            &HeaderMap::new(),
                            Bytes::new(),
                            &account,
                            None,
                        )
                        .await?;
                    if !response.status().is_success() {
                        return Err(Error::ambiguous());
                    }
                    let inventory: crate::brain_contract::SessionList =
                        serde_json::from_slice(&app.brain.bytes(response).await?)?;
                    let count = inventory.sessions.len();
                    for session in inventory.sessions {
                        after = session.id.clone();
                        if !app.store.contains_session(&session.id).await? {
                            orphans.push(json!({"account":account,"session":session.id}));
                        }
                    }
                    if count < 1000 {
                        break;
                    }
                }
            }
            json!({"sessions":orphans})
        }
        Operation::ReportUsage {
            observed_at,
            sessions,
        } => {
            if observed_at > crate::store::now()
                || crate::store::now().saturating_sub(observed_at)
                    > app.config.limits.usage_max_age_secs as i64
            {
                return Err(Error::invalid("usage report is stale or in the future"));
            }
            app.store.report_usage(observed_at, sessions).await?;
            json!({"recorded":true})
        }
        Operation::Maintain => {
            let turns = crate::turns::maintain(app).await?;
            crate::managed::maintain(app).await?;
            let mut deleted = 0;
            for id in app
                .store
                .expired_sessions(app.config.limits.retention_secs)
                .await?
            {
                let response = match crate::sessions::retire(app, &id).await {
                    Err(error) if error.0 == axum::http::StatusCode::CONFLICT => continue,
                    result => result?,
                };
                if response.status().is_success() {
                    deleted += 1;
                }
            }
            let attachments = crate::attachments::maintain(app).await?;
            json!({"turns_reconciled":turns,"deleted":deleted,"attachments_deleted":attachments.deleted,"attachments_failed":attachments.failed})
        }
        Operation::CreateAccount => {
            let id = app.store.create_account(&app.config.limits).await?;
            json!({"account": id})
        }
        Operation::IssueKey { account } => {
            let (id, token) = app.store.issue_key(&account, &app.config.limits).await?;
            json!({"key": id, "token": token})
        }
        Operation::RevokeKey { key } => {
            app.store.revoke_key(&key).await?;
            json!({"revoked": true})
        }
        Operation::SuspendAccount { account } => {
            app.store.set_account_active(&account, false).await?;
            json!({"active": false})
        }
        Operation::ResumeAccount { account } => {
            app.store.set_account_active(&account, true).await?;
            json!({"active": true})
        }
        Operation::Inspect => {
            let (accounts, claims) = app.store.inspect().await?;
            json!({"accounts":accounts,"pending":claims,"accepting":app.accepting.load(std::sync::atomic::Ordering::SeqCst)})
        }
        Operation::ResolveCreate {
            account,
            client_key,
        } => {
            require_drained(app)?;
            app.store.resolve_create(&account, &client_key).await?;
            json!({"resolved":true})
        }
        Operation::ForgetExecutor { executor } => {
            sqlx::query("DELETE FROM executors WHERE id=$1")
                .bind(executor)
                .execute(&app.store.0)
                .await?;
            json!({"forgotten":true})
        }
        Operation::Drain => {
            app.accepting
                .store(false, std::sync::atomic::Ordering::SeqCst);
            json!({"accepting":false,"requests":app.config.limits.requests - app.requests.available_permits()})
        }
        Operation::Resume => {
            crate::admission::disk(app).await?;
            if !app.brain.ready().await {
                return Err(Error::capacity());
            }
            app.accepting
                .store(true, std::sync::atomic::Ordering::SeqCst);
            json!({"accepting":true})
        }
        Operation::DeleteOrphan { account, session } => {
            require_drained(app)?;
            super::http::route(&Method::DELETE, &format!("/sessions/{session}"))?;
            if app.store.contains_session(&session).await? {
                return Err(Error::conflict("resource has an ownership record"));
            }
            let response = app
                .brain
                .request(
                    Method::DELETE,
                    &format!("/sessions/{session}"),
                    &HeaderMap::new(),
                    Bytes::new(),
                    &account,
                    None,
                )
                .await?;
            if !response.status().is_success() {
                return Err(Error::ambiguous());
            }
            app.brain.bytes(response).await?;
            json!({"deleted":true})
        }
        Operation::ReconcileTurn { session } => {
            require_drained(app)?;
            super::http::route(&Method::GET, &format!("/sessions/{session}"))?;
            let account = app.store.session_account(&session).await?;
            let summary = app.brain.summary(&account, &session).await?;
            if summary.status == "running" {
                return Err(Error::conflict("session is still active"));
            }
            if !crate::turns::reconcile(app, &session).await? {
                return Err(Error::conflict("session is still active"));
            }
            json!({"reconciled":true})
        }
    };
    app.changed.send_modify(|v| *v = v.wrapping_add(1));
    Ok(result)
}
fn require_drained(app: &App) -> Result<()> {
    if app.accepting.load(std::sync::atomic::Ordering::SeqCst)
        || app.requests.available_permits() != app.config.limits.requests
    {
        Err(Error::conflict(
            "drain and wait for in-flight requests first",
        ))
    } else {
        Ok(())
    }
}
