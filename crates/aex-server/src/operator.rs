use crate::{
    App,
    error::{Error, Result},
    identity,
};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, Method},
    routing::post,
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
    ForgetHost {
        host: String,
    },
    Drain,
    Resume,
    DeleteOrphan {
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
    Orphans,
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/operate", post(operate))
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
        Operation::Orphans => {
            require_drained(app)?;
            let response = app
                .brain
                .request(
                    Method::GET,
                    "/v1/sessions",
                    &HeaderMap::new(),
                    Bytes::new(),
                    None,
                    None,
                )
                .await?;
            if !response.status().is_success() {
                return Err(Error::ambiguous());
            }
            let inventory: brain_protocol::SessionList =
                serde_json::from_slice(&app.brain.bytes(response).await?)?;
            let mut orphans = Vec::new();
            for session in inventory.sessions {
                if !app
                    .store
                    .contains_session(session.session_id.as_str())
                    .await?
                {
                    orphans.push(session.session_id);
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
            let mut deleted = 0;
            for id in app
                .store
                .expired_sessions(app.config.limits.retention_secs)
                .await?
            {
                let response = crate::sessions::retire(app, &id).await?;
                if response.status().is_success() {
                    deleted += 1;
                }
            }
            let attachments = crate::attachments::maintain(app).await?;
            json!({"deleted":deleted,"attachments_deleted":attachments.deleted,"attachments_failed":attachments.failed})
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
        Operation::ForgetHost { host } => {
            app.store.forget_host(&host).await?;
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
        Operation::DeleteOrphan { session } => {
            require_drained(app)?;
            super::http::route(&Method::DELETE, &format!("/v1/sessions/{session}"))?;
            if app.store.contains_session(&session).await? {
                return Err(Error::conflict("resource has an ownership record"));
            }
            let ended = app
                .brain
                .request(
                    Method::POST,
                    &format!("/v1/sessions/{session}/end"),
                    &HeaderMap::new(),
                    Bytes::new(),
                    Some(&identity::digest(
                        format!("orphan-end:{session}").as_bytes(),
                    )),
                    None,
                )
                .await?;
            if !ended.status().is_success() && ended.status() != axum::http::StatusCode::NOT_FOUND {
                return Err(Error::ambiguous());
            }
            app.brain.bytes(ended).await?;
            let response = app
                .brain
                .request(
                    Method::DELETE,
                    &format!("/v1/sessions/{session}"),
                    &HeaderMap::new(),
                    Bytes::new(),
                    Some(&identity::digest(format!("orphan:{session}").as_bytes())),
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
            super::http::route(&Method::GET, &format!("/v1/sessions/{session}"))?;
            let summary = app.brain.summary(&session).await?;
            if matches!(
                summary.status,
                brain_protocol::SessionStatus::Running
                    | brain_protocol::SessionStatus::Creating
                    | brain_protocol::SessionStatus::Ending
            ) {
                return Err(Error::conflict("session is still active"));
            }
            app.store.finish_turn(&session).await?;
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
