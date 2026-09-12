pub mod storage;

use crate::{
    App,
    error::{Error, Result},
    identity::{self, Principal},
    store::{Claim, Store, now},
};
use axum::{
    Json,
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use brain_protocol::{FileMediaType, Media};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tokio::sync::OwnedSemaphorePermit;

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub public_origin: String,
    pub bucket: String,
    pub region: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    pub max_bytes: usize,
    pub count_per_account: u32,
    pub bytes_per_account: u64,
    pub ttl_secs: u64,
    pub transfer_timeout_secs: u64,
}

impl Config {
    pub fn validate(&self, request_bytes: usize) -> anyhow::Result<()> {
        for value in std::iter::once(&self.public_origin).chain(self.endpoint.iter()) {
            let url = url::Url::parse(value)?;
            anyhow::ensure!(
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
                    && url.path() == "/",
                "attachment URLs must be HTTPS origins without credentials"
            );
        }
        anyhow::ensure!(
            !self.bucket.is_empty() && !self.region.is_empty(),
            "attachment bucket and region are required"
        );
        anyhow::ensure!(
            self.max_bytes > 0
                && self.max_bytes <= request_bytes
                && self.max_bytes as u64 <= self.bytes_per_account,
            "attachment size exceeds request or account limit"
        );
        anyhow::ensure!(
            self.count_per_account > 0
                && self.bytes_per_account <= i64::MAX as u64
                && self.ttl_secs > 0
                && self.ttl_secs <= (i64::MAX / 2) as u64
                && self.transfer_timeout_secs > 0
                && self.transfer_timeout_secs <= self.ttl_secs,
            "invalid attachment limits"
        );
        Ok(())
    }
}

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub id: String,
    pub media: Media,
    pub expires_at: i64,
}

fn configured(app: &App) -> Result<(&Config, &Arc<dyn storage::Storage>)> {
    match (&app.config.attachments, &app.attachment_storage) {
        (Some(config), Some(storage)) => Ok((config, storage)),
        _ => Err(Error::invalid("attachment storage is not configured")),
    }
}

fn check_type(kind: &str, bytes: &[u8]) -> Result<()> {
    let valid = match kind {
        "application/pdf" => bytes.starts_with(b"%PDF-"),
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(b"\xff\xd8\xff"),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        _ => false,
    };
    if !valid {
        return Err(Error::invalid(
            "unsupported attachment type or mismatched file signature",
        ));
    }
    Ok(())
}

pub async fn upload(
    app: &App,
    principal: &Principal,
    session: &str,
    headers: &HeaderMap,
    bytes: Bytes,
) -> Result<Response> {
    let (config, storage) = configured(app)?;
    let _upload = app.attachment_uploads.read().await;
    if bytes.is_empty() || bytes.len() > config.max_bytes {
        return Err(Error::invalid("attachment exceeds its byte limit"));
    }
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| Error::invalid("attachment Content-Type is required"))?;
    check_type(content_type, &bytes)?;
    let requested_expiry = headers
        .get("x-aex-expires-at")
        .map(|v| {
            v.to_str()
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .ok_or_else(|| Error::invalid("x-aex-expires-at must be Unix seconds"))
        })
        .transpose()?;
    let fingerprint = identity::digest(&serde_json::to_vec(&(
        identity::digest(&bytes),
        content_type,
        requested_expiry,
    ))?);
    let mut tx = app.store.0.begin().await?;
    let claim = Store::claim_in(
        &mut tx,
        principal,
        &format!("attachments:{session}"),
        identity::operation_key(headers)?,
        &fingerprint,
        &app.config.limits,
    )
    .await?;
    sqlx::query("SELECT id FROM sessions WHERE id=$1 AND account=$2 AND state='owned' FOR UPDATE")
        .bind(session)
        .bind(&principal.account)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(Error::missing)?;
    let operation = match claim {
        Claim::Complete(value) => {
            return Ok(Json(serde_json::from_value::<Attachment>(value)?).into_response());
        }
        Claim::New(operation) => operation,
    };
    let expires_at = requested_expiry.unwrap_or(now() + config.ttl_secs as i64);
    if expires_at <= now() || expires_at > now() + config.ttl_secs as i64 {
        return Err(Error::invalid(
            "attachment expiry is outside the configured lifetime",
        ));
    }
    let usage = sqlx::query("SELECT count(*) AS count,coalesce(sum(bytes),0)::bigint AS bytes FROM attachments WHERE account=$1")
        .bind(&principal.account).fetch_one(&mut *tx).await?;
    if usage.get::<i64, _>("count") >= i64::from(config.count_per_account)
        || (usage.get::<i64, _>("bytes") as u64).saturating_add(bytes.len() as u64)
            > config.bytes_per_account
    {
        return Err(Error::capacity());
    }
    let id = identity::random("att");
    let token = identity::random("read");
    let object_key = format!("attachments/{id}");
    sqlx::query("INSERT INTO attachments VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pending',$9)")
        .bind(&id)
        .bind(&principal.account)
        .bind(session)
        .bind(&object_key)
        .bind(content_type)
        .bind(bytes.len() as i64)
        .bind(expires_at)
        .bind(identity::digest(token.as_bytes()))
        .bind(&operation)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    storage.put_new(&object_key, content_type, bytes).await?;
    let url = format!(
        "{}/v1/attachments/{id}/content?token={token}",
        config.public_origin.trim_end_matches('/')
    );
    let attachment = Attachment {
        id: id.clone(),
        expires_at,
        media: if content_type == "application/pdf" {
            Media::File {
                media_type: FileMediaType::Pdf,
                url,
            }
        } else {
            Media::Image { url }
        },
    };
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &principal.account).await?;
    let owned = sqlx::query("SELECT s.id FROM sessions s JOIN accounts a ON a.id=s.account WHERE s.id=$1 AND s.account=$2 AND s.state='owned' AND a.active=1 FOR UPDATE OF s")
        .bind(session).bind(&principal.account).fetch_optional(&mut *tx).await?;
    if owned.is_none() || expires_at <= now() {
        sqlx::query("UPDATE attachments SET state='deleting' WHERE id=$1")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(Error::conflict(
            "attachment publication was revoked or expired",
        ));
    }
    let updated =
        sqlx::query("UPDATE attachments SET state='ready' WHERE id=$1 AND state='pending'")
            .bind(&id)
            .execute(&mut *tx)
            .await?;
    if updated.rows_affected() != 1 {
        return Err(Error::conflict("attachment was revoked"));
    }
    sqlx::query(
        "UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2 AND state='pending'",
    )
    .bind(serde_json::to_string(&attachment)?)
    .bind(&operation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(attachment)).into_response())
}

pub async fn revoke(app: &App, principal: &Principal, session: &str, id: &str) -> Result<Response> {
    configured(app)?;
    app.store.owned(principal, session, true).await?;
    sqlx::query(
        "UPDATE attachments SET state='deleting' WHERE id=$1 AND session=$2 AND account=$3",
    )
    .bind(id)
    .bind(session)
    .bind(&principal.account)
    .execute(&app.store.0)
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn download(
    app: &App,
    id: &str,
    query: Option<&str>,
    head: bool,
    permit: OwnedSemaphorePermit,
) -> Result<Response> {
    let (config, storage) = configured(app)?;
    let token = query
        .and_then(|q| q.strip_prefix("token="))
        .filter(|s| s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        .ok_or_else(Error::missing)?;
    let row = sqlx::query("SELECT f.account,f.object_key,f.content_type,f.bytes FROM attachments f JOIN accounts a ON a.id=f.account JOIN sessions s ON s.id=f.session WHERE f.id=$1 AND f.read_verifier=$2 AND f.state='ready' AND f.expires_at>$3 AND a.active=1 AND s.state='owned'")
        .bind(id).bind(identity::digest(token.as_bytes())).bind(now()).fetch_optional(&app.store.0).await?.ok_or_else(Error::missing)?;
    let account: String = row.get("account");
    let semaphore = app
        .account_requests
        .lock()
        .await
        .entry(account)
        .or_insert_with(|| {
            Arc::new(tokio::sync::Semaphore::new(
                app.config.limits.requests_per_account,
            ))
        })
        .clone();
    let account_permit = semaphore
        .try_acquire_owned()
        .map_err(|_| Error::capacity())?;
    let length: i64 = row.get("bytes");
    let body = if head {
        Body::empty()
    } else {
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(config.transfer_timeout_secs);
        let mut stream = tokio::time::timeout_at(deadline, storage.get(row.get("object_key")))
            .await
            .map_err(|_| Error::internal())??;
        let stream: storage::ObjectStream = Box::pin(async_stream::try_stream! {
            let _permits = (permit, account_permit);
            let mut remaining = length as usize;
            while let Some(chunk) = tokio::time::timeout_at(deadline, stream.next()).await.map_err(|_| Error::internal())? {
                let chunk = chunk?;
                remaining = remaining.checked_sub(chunk.len()).ok_or_else(Error::internal)?;
                yield chunk;
            }
            if remaining != 0 { Err(Error::internal())?; }
        });
        Body::from_stream(stream)
    };
    Response::builder()
        .header("content-type", row.get::<String, _>("content_type"))
        .header("content-length", length)
        .header("cache-control", "private, no-store")
        .header("x-content-type-options", "nosniff")
        .body(body)
        .map_err(|_| Error::internal())
}

#[derive(Default)]
pub struct Cleanup {
    pub deleted: u64,
    pub failed: u64,
}

pub async fn maintain(app: &App) -> Result<Cleanup> {
    if app.config.attachments.is_none() {
        return Ok(Cleanup::default());
    }
    let (_, storage) = configured(app)?;
    let Ok(_uploads) = app.attachment_uploads.try_write() else {
        return Ok(Cleanup::default());
    };
    let drained = !app.accepting.load(Ordering::SeqCst);
    let rows = sqlx::query("SELECT f.id,f.object_key FROM attachments f JOIN sessions s ON s.id=f.session WHERE f.state='deleting' OR f.expires_at<=$1 OR s.state!='owned' OR ($2 AND f.state='pending') ORDER BY f.expires_at,f.id LIMIT 100")
        .bind(now()).bind(drained).fetch_all(&app.store.0).await?;
    let mut result = Cleanup::default();
    // Storage outages must not hold up the local-disk meter until its usage report is stale.
    let deadline = tokio::time::Instant::now()
        + Duration::from_secs((app.config.limits.usage_max_age_secs / 2).max(1));
    for row in rows {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let id: &str = row.get("id");
        sqlx::query("UPDATE attachments SET state='deleting' WHERE id=$1")
            .bind(id)
            .execute(&app.store.0)
            .await?;
        if !matches!(
            tokio::time::timeout_at(deadline, storage.delete(row.get("object_key"))).await,
            Ok(Ok(()))
        ) {
            tracing::warn!(
                attachment = id,
                "attachment cleanup failed; deletion remains pending"
            );
            result.failed += 1;
            break;
        }
        sqlx::query("DELETE FROM attachments WHERE id=$1")
            .bind(id)
            .execute(&app.store.0)
            .await?;
        result.deleted += 1;
    }
    Ok(result)
}
