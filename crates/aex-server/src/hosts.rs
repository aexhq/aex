use crate::{
    App,
    brain_contract::{ExecutorCredential, ExecutorRegistration},
    error::{Error, Result},
    identity::Principal,
};
use axum::{
    Json,
    body::Bytes,
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use sqlx::Row;

pub async fn register(
    app: &App,
    principal: &Principal,
    headers: &HeaderMap,
    body: Bytes,
    browser: Option<&crate::clients::Access>,
) -> Result<Response> {
    let registration: ExecutorRegistration = serde_json::from_slice(&body)?;
    if let Some(session) = &registration.session {
        app.store.owned(principal, session, false).await?;
    }
    if let Some(access) = browser {
        access.executor(&registration)?;
    }
    let _guard = app
        .host_registration
        .try_lock()
        .map_err(|_| Error::capacity())?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM executors WHERE account=$1")
        .bind(&principal.account)
        .fetch_one(&app.store.0)
        .await?;
    let replacing = if let Some(session) = &registration.session {
        let saved = app.brain.summary(&principal.account, session).await?;
        let saved_id = saved
            .config
            .bindings
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|binding| binding.alias == registration.binding)
            .and_then(|binding| binding.executor.as_deref());
        if let Some(id) = saved_id {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM executors WHERE id=$1 AND account=$2)",
            )
            .bind(id)
            .bind(&principal.account)
            .fetch_one(&app.store.0)
            .await?
        } else {
            false
        }
    } else {
        false
    };
    if count >= i64::from(app.config.limits.executors_per_account) && !replacing {
        return Err(Error::capacity());
    }
    let response = app
        .brain
        .request(
            Method::POST,
            "/executors",
            headers,
            body,
            &principal.account,
            None,
        )
        .await?;
    if !response.status().is_success() {
        return crate::http::finite(app, response).await;
    }
    let credential: ExecutorCredential = serde_json::from_slice(&app.brain.bytes(response).await?)
        .map_err(|_| Error::ambiguous())?;
    sqlx::query("INSERT INTO executors(id,account,issuing_key,verifier,browser_grant,created,session) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(id) DO UPDATE SET issuing_key=EXCLUDED.issuing_key,verifier=EXCLUDED.verifier,browser_grant=EXCLUDED.browser_grant,session=EXCLUDED.session WHERE executors.account=EXCLUDED.account")
        .bind(&credential.id).bind(&principal.account).bind(&principal.key).bind(crate::identity::digest(credential.token.as_bytes()))
        .bind(browser.map(|access|access.id.as_str())).bind(crate::store::now()).bind(&registration.session).execute(&app.store.0).await?;
    Ok((axum::http::StatusCode::CREATED, Json(credential)).into_response())
}
impl crate::store::Store {
    pub async fn own_executor(&self, principal: &Principal, id: &str) -> Result<()> {
        let row = sqlx::query(
            "SELECT issuing_key,browser_grant,session FROM executors WHERE id=$1 AND account=$2",
        )
        .bind(id)
        .bind(&principal.account)
        .fetch_optional(&self.0)
        .await?
        .ok_or_else(Error::missing)?;
        self.active(&Principal {
            account: principal.account.clone(),
            key: row.get("issuing_key"),
        })
        .await?;
        if let Some(session) = row.get::<Option<String>, _>("session") {
            self.owned(principal, &session, false).await?;
        }
        if let Some(grant) = row.get::<Option<String>, _>("browser_grant") {
            let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM browser_grants WHERE id=$1 AND active AND expires_at>$2)").bind(grant).bind(crate::store::now()).fetch_one(&self.0).await?;
            if !active {
                return Err(Error::denied());
            }
        }
        Ok(())
    }
    pub async fn executor(&self, id: &str, token: &str) -> Result<Principal> {
        let row = sqlx::query("SELECT account,issuing_key,verifier FROM executors WHERE id=$1")
            .bind(id)
            .fetch_optional(&self.0)
            .await?
            .ok_or_else(Error::denied)?;
        if !crate::identity::matches(token, row.get("verifier")) {
            return Err(Error::denied());
        }
        let principal = Principal {
            account: row.get("account"),
            key: row.get("issuing_key"),
        };
        self.own_executor(&principal, id).await?;
        Ok(principal)
    }
    pub async fn session_account(&self, id: &str) -> Result<String> {
        sqlx::query_scalar("SELECT account FROM sessions WHERE id=$1 AND runtime='brain'")
            .bind(id)
            .fetch_optional(&self.0)
            .await?
            .ok_or_else(Error::missing)
    }
}
