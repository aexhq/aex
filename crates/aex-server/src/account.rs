use crate::{
    App,
    error::{Error, Result},
    identity::{self, digest, random},
    store::now,
};
use axum::{
    Json,
    body::Bytes,
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SignIn {
    pub subject: String,
    pub email: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeyInput {
    pub name: String,
}

#[derive(Serialize, JsonSchema)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub created: i64,
    pub active: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct IssuedKey {
    pub key: ApiKey,
    pub token: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Usage {
    pub sessions: i64,
    pub active_turns: i64,
    pub retained_bytes: i64,
    pub measured_at: Option<i64>,
}

#[derive(Serialize, JsonSchema)]
pub struct Account {
    pub id: String,
    pub email: Option<String>,
    pub created: i64,
    pub billing: Billing,
    pub usage: Usage,
    pub limits: crate::config::Limits,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    PreviewCustomerModelKeys,
}

fn key(row: &sqlx::postgres::PgRow) -> ApiKey {
    ApiKey {
        id: row.get("id"),
        name: row.get("name"),
        prefix: row.get("prefix"),
        created: row.get("created"),
        active: row.get::<i64, _>("active") == 1,
    }
}

async fn dashboard_account(app: &App, token: &str) -> Result<String> {
    sqlx::query_scalar("SELECT a.id FROM dashboard_sessions d JOIN accounts a ON a.id=d.account WHERE d.verifier=$1 AND d.expires>$2 AND a.active=1")
        .bind(digest(token.as_bytes())).bind(now()).fetch_optional(&app.store.0).await?
        .ok_or_else(Error::denied)
}

pub fn route(method: &Method, path: &str) -> bool {
    matches!(
        (method.as_str(), path),
        ("POST", "/v1/accounts")
            | ("GET", "/v1/account" | "/v1/usage" | "/v1/keys")
            | ("POST", "/v1/keys")
            | ("DELETE", "/v1/account/session")
    ) || (matches!(method.as_str(), "PATCH" | "DELETE")
        && path
            .strip_prefix("/v1/keys/")
            .is_some_and(|id| !id.is_empty() && !id.contains('/')))
}

pub async fn handle(
    app: &App,
    method: &Method,
    path: &str,
    token: &str,
    body: Bytes,
) -> Result<Response> {
    if path == "/v1/accounts" {
        if !identity::matches(token, &app.site_verifier) {
            return Err(Error::denied());
        }
        let input: SignIn = serde_json::from_slice(&body)?;
        if input.subject.is_empty()
            || input.subject.len() > 256
            || input.email.len() > 320
            || !input.email.contains('@')
        {
            return Err(Error::invalid("invalid verified identity"));
        }
        let mut tx = app.store.0.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(8172301)")
            .execute(&mut *tx)
            .await?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM accounts WHERE identity_subject=$1")
                .bind(&input.subject)
                .fetch_optional(&mut *tx)
                .await?;
        let account = if let Some(account) = existing {
            account
        } else {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
                .fetch_one(&mut *tx)
                .await?;
            if count >= i64::from(app.config.limits.accounts) {
                return Err(Error::capacity());
            }
            let id = random("acct");
            sqlx::query("INSERT INTO accounts(id,identity_subject,email) VALUES ($1,$2,$3)")
                .bind(&id)
                .bind(input.subject)
                .bind(&input.email)
                .execute(&mut *tx)
                .await?;
            id
        };
        let active: i64 = sqlx::query_scalar("SELECT active FROM accounts WHERE id=$1 FOR UPDATE")
            .bind(&account)
            .fetch_one(&mut *tx)
            .await?;
        if active != 1 {
            return Err(Error::denied());
        }
        sqlx::query("UPDATE accounts SET email=$1 WHERE id=$2")
            .bind(input.email)
            .bind(&account)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM dashboard_sessions WHERE expires<=$1 OR verifier IN (SELECT verifier FROM dashboard_sessions WHERE account=$2 ORDER BY expires DESC OFFSET 7)")
            .bind(now()).bind(&account).execute(&mut *tx).await?;
        let token = random("aex_account");
        let expires = now() + 7 * 24 * 3600;
        sqlx::query("INSERT INTO dashboard_sessions VALUES ($1,$2,$3)")
            .bind(digest(token.as_bytes()))
            .bind(&account)
            .bind(expires)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(Json(serde_json::json!({"token":token,"expires":expires})).into_response());
    }
    let account =
        if matches!(path, "/v1/account" | "/v1/usage") && !token.starts_with("aex_account_") {
            app.store.principal(token).await?.account
        } else {
            dashboard_account(app, token).await?
        };
    tracing::Span::current().record("account", &account);
    if matches!(path, "/v1/account" | "/v1/usage") {
        let row=sqlx::query("SELECT count(*) AS sessions,count(active_key) AS active_turns,coalesce(sum(retained_bytes),0)::bigint AS retained_bytes FROM sessions WHERE account=$1 AND state!='deleted'")
            .bind(&account).fetch_one(&app.store.0).await?;
        let usage = Usage {
            sessions: row.get("sessions"),
            active_turns: row.get("active_turns"),
            retained_bytes: row.get("retained_bytes"),
            measured_at: app.store.usage_received().await?,
        };
        if path == "/v1/usage" {
            return Ok(Json(usage).into_response());
        }
        let row = sqlx::query("SELECT email,created FROM accounts WHERE id=$1")
            .bind(&account)
            .fetch_one(&app.store.0)
            .await?;
        return Ok(Json(Account {
            id: account,
            email: row.get("email"),
            created: row.get("created"),
            billing: Billing::PreviewCustomerModelKeys,
            usage,
            limits: app.config.limits.clone(),
        })
        .into_response());
    }
    if path == "/v1/account/session" {
        sqlx::query("DELETE FROM dashboard_sessions WHERE verifier=$1")
            .bind(digest(token.as_bytes()))
            .execute(&app.store.0)
            .await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if path == "/v1/keys" && method == Method::GET {
        let rows=sqlx::query("SELECT id,name,prefix,created,active FROM api_keys WHERE account=$1 ORDER BY created DESC,id")
            .bind(&account).fetch_all(&app.store.0).await?;
        return Ok(Json(rows.iter().map(key).collect::<Vec<_>>()).into_response());
    }
    if method == Method::DELETE {
        let changed = sqlx::query("UPDATE api_keys SET active=0 WHERE id=$1 AND account=$2")
            .bind(path.strip_prefix("/v1/keys/").ok_or_else(Error::missing)?)
            .bind(&account)
            .execute(&app.store.0)
            .await?;
        if changed.rows_affected() != 1 {
            return Err(Error::missing());
        }
        app.changed.send_modify(|v| *v = v.wrapping_add(1));
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let input: KeyInput = serde_json::from_slice(&body)?;
    let name = input.name.trim();
    if name.is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        return Err(Error::invalid(
            "key name must be 1 to 80 bytes without control characters",
        ));
    }
    if method == Method::POST {
        let (id, token) = app
            .store
            .issue_named_key(&account, name, &app.config.limits)
            .await?;
        let row = sqlx::query("SELECT id,name,prefix,created,active FROM api_keys WHERE id=$1")
            .bind(id)
            .fetch_one(&app.store.0)
            .await?;
        return Ok((
            StatusCode::CREATED,
            Json(IssuedKey {
                key: key(&row),
                token,
            }),
        )
            .into_response());
    }
    let row=sqlx::query("UPDATE api_keys SET name=$1 WHERE id=$2 AND account=$3 RETURNING id,name,prefix,created,active")
        .bind(name).bind(path.strip_prefix("/v1/keys/").ok_or_else(Error::missing)?).bind(account)
        .fetch_optional(&app.store.0).await?.ok_or_else(Error::missing)?;
    Ok(Json(key(&row)).into_response())
}
