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
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id=?")
                    .bind(session.session_id.as_str())
                    .fetch_one(&app.store.0)
                    .await?;
                if count == 0 {
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
            let mut tx = app.store.0.begin().await?;
            for (id, bytes) in sessions {
                let bytes =
                    i64::try_from(bytes).map_err(|_| Error::invalid("storage size overflow"))?;
                sqlx::query("UPDATE sessions SET retained_bytes=? WHERE id=? AND state!='deleted' AND active_key IS NULL AND changed_at < ?")
                    .bind(bytes)
                    .bind(id)
                    .bind(observed_at)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("INSERT INTO storage_report VALUES(1,?) ON CONFLICT(singleton) DO UPDATE SET received=excluded.received").bind(observed_at).execute(&mut *tx).await?;
            tx.commit().await?;
            json!({"recorded":true})
        }
        Operation::Maintain => {
            use sqlx::Row;
            let rows = sqlx::query(
                "SELECT id,state,created,active_key FROM sessions WHERE state!='deleted'",
            )
            .fetch_all(&app.store.0)
            .await?;
            let mut deleted = 0;
            for row in rows {
                let id: String = row.get("id");
                if row.get::<String, _>("state") == "deleting"
                    || crate::store::now().saturating_sub(row.get("created"))
                        > app.config.limits.retention_secs as i64
                {
                    let response = crate::sessions::delete(
                        app,
                        &id,
                        &HeaderMap::new(),
                        &identity::digest(format!("retention:{id}").as_bytes()),
                    )
                    .await?;
                    if response.status().is_success() {
                        deleted += 1;
                    }
                }
            }
            json!({"deleted":deleted})
        }
        Operation::CreateAccount => {
            let mut tx = app.store.0.begin().await?;
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
                .fetch_one(&mut *tx)
                .await?;
            if count >= i64::from(app.config.limits.accounts) {
                return Err(Error::capacity());
            }
            let id = identity::random("acct");
            sqlx::query("INSERT INTO accounts(id) VALUES (?)")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            json!({"account": id})
        }
        Operation::IssueKey { account } => {
            let mut tx = app.store.0.begin().await?;
            let exists: i64 =
                sqlx::query_scalar("SELECT count(*) FROM accounts WHERE id=? AND active=1")
                    .bind(&account)
                    .fetch_one(&mut *tx)
                    .await?;
            if exists != 1 {
                return Err(Error::missing());
            }
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM api_keys WHERE account=?")
                .bind(&account)
                .fetch_one(&mut *tx)
                .await?;
            if count >= i64::from(app.config.limits.keys_per_account) {
                return Err(Error::capacity());
            }
            let id = identity::random("key");
            let token = identity::random("aex");
            sqlx::query("INSERT INTO api_keys(id,account,verifier) VALUES (?,?,?)")
                .bind(&id)
                .bind(account)
                .bind(identity::digest(token.as_bytes()))
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            json!({"key": id, "token": token})
        }
        Operation::RevokeKey { key } => {
            sqlx::query("UPDATE api_keys SET active=0 WHERE id=?")
                .bind(key)
                .execute(&app.store.0)
                .await?;
            json!({"revoked": true})
        }
        Operation::SuspendAccount { account } => {
            sqlx::query("UPDATE accounts SET active=0 WHERE id=?")
                .bind(account)
                .execute(&app.store.0)
                .await?;
            json!({"active": false})
        }
        Operation::ResumeAccount { account } => {
            sqlx::query("UPDATE accounts SET active=1 WHERE id=?")
                .bind(account)
                .execute(&app.store.0)
                .await?;
            json!({"active": true})
        }
        Operation::Inspect => {
            use sqlx::Row;
            let accounts: Vec<Value> = sqlx::query("SELECT a.id,a.active,(SELECT count(*) FROM sessions s WHERE s.account=a.id AND s.state!='deleted') AS sessions FROM accounts a").fetch_all(&app.store.0).await?.iter().map(|r| json!({"account": r.get::<String,_>("id"),"active":r.get::<i64,_>("active") == 1,"sessions":r.get::<i64,_>("sessions")})).collect();
            let claims: Vec<Value> = sqlx::query("SELECT account,operation,client_key,created FROM claims WHERE state='pending'").fetch_all(&app.store.0).await?.iter().map(|r| json!({"account":r.get::<String,_>("account"),"operation":r.get::<String,_>("operation"),"client_key":r.get::<String,_>("client_key"),"created":r.get::<i64,_>("created")})).collect();
            json!({"accounts":accounts,"pending":claims,"accepting":app.accepting.load(std::sync::atomic::Ordering::SeqCst)})
        }
        Operation::ResolveCreate {
            account,
            client_key,
        } => {
            require_drained(app)?;
            sqlx::query("UPDATE claims SET state='resolved' WHERE account=? AND client_key=? AND operation='create' AND state='pending'").bind(account).bind(client_key).execute(&app.store.0).await?;
            json!({"resolved":true})
        }
        Operation::ForgetHost { host } => {
            sqlx::query("DELETE FROM hosts WHERE id=?")
                .bind(host)
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
        Operation::DeleteOrphan { session } => {
            require_drained(app)?;
            super::http::route(&Method::DELETE, &format!("/v1/sessions/{session}"))?;
            let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id=?")
                .bind(&session)
                .fetch_one(&app.store.0)
                .await?;
            if owned != 0 {
                return Err(Error::conflict("resource has an ownership record"));
            }
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
