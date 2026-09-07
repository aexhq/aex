use crate::{
    App,
    error::{Error, Result},
    identity::{self, Principal},
};
use axum::{
    body::Bytes,
    http::{HeaderMap, Method},
    response::Response,
};

pub async fn owned(app: &App, p: &Principal, kind: &str, id: &str) -> Result<()> {
    if kind == "agentloops" && app.config.agentloops.contains(id) {
        return Ok(());
    }
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artifacts WHERE account=$1 AND kind=$2 AND id=$3")
            .bind(&p.account)
            .bind(kind)
            .bind(id)
            .fetch_one(&app.store.0)
            .await?;
    if count == 1 {
        Ok(())
    } else {
        Err(Error::invalid("Component must be uploaded by this account"))
    }
}

pub async fn handle(
    app: &App,
    p: &Principal,
    kind: &str,
    id: Option<&str>,
    headers: &HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let (method, path, key) = if let Some(id) = id {
        owned(app, p, kind, id).await?;
        (Method::GET, format!("/v1/{kind}/{id}"), None)
    } else {
        if !body.starts_with(b"\0asm\x0d\0\x01\0") {
            return Err(Error::invalid("expected a WebAssembly Component"));
        }
        let operation_key = identity::operation_key(headers)?;
        crate::admission::disk(app).await?;
        let id = identity::digest(&body);
        let mut tx = app.store.0.begin().await?;
        sqlx::query("SELECT id FROM accounts WHERE id=$1 FOR UPDATE")
            .bind(&p.account)
            .fetch_one(&mut *tx)
            .await?;
        let exists: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM artifacts WHERE account=$1 AND kind=$2 AND id=$3",
        )
        .bind(&p.account)
        .bind(kind)
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
        if exists == 0 {
            use sqlx::Row;
            let row=sqlx::query("SELECT count(*) AS count,coalesce(sum(bytes),0)::bigint AS bytes FROM artifacts WHERE account=$1")
                .bind(&p.account).fetch_one(&mut *tx).await?;
            if row.get::<i64, _>("count") >= i64::from(app.config.limits.artifacts_per_account)
                || (row.get::<i64, _>("bytes") as u64).saturating_add(body.len() as u64)
                    > app.config.limits.artifact_bytes_per_account
            {
                return Err(Error::capacity());
            }
            sqlx::query("INSERT INTO artifacts VALUES($1,$2,$3,$4)")
                .bind(&p.account)
                .bind(kind)
                .bind(id)
                .bind(body.len() as i64)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        (
            Method::POST,
            format!("/v1/{kind}"),
            Some(identity::scoped_key(p, kind, operation_key)),
        )
    };
    // Compilation shares the Brain worker pool; admit one package at a time to bound contention.
    let _guard = app
        .artifact_admission
        .try_lock()
        .map_err(|_| Error::capacity())?;
    let response = app
        .brain
        .request(method, &path, headers, body, key.as_deref(), None)
        .await?;
    crate::http::finite(app, response).await
}
