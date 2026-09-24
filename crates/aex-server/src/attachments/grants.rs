use super::*;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UploadGrantInput {
    pub content_type: String,
    pub bytes: usize,
    pub expires_at: Option<i64>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
pub struct UploadGrant {
    pub id: String,
    pub upload_url: String,
    pub token: String,
    pub expires_at: i64,
    pub attachment_expires_at: i64,
    pub bytes: usize,
    pub content_type: String,
}
#[derive(Serialize, JsonSchema)]
pub struct AttachmentLimits {
    pub max_bytes: usize,
    pub count_per_account: u32,
    pub bytes_per_account: u64,
    pub ttl_secs: u64,
    pub session_retention_secs: u64,
    pub region: String,
    pub upload_grant_secs: u64,
}
pub fn limits(app: &App) -> Result<AttachmentLimits> {
    let (config, _) = configured(app)?;
    Ok(AttachmentLimits {
        max_bytes: config.max_bytes,
        count_per_account: config.count_per_account,
        bytes_per_account: config.bytes_per_account,
        ttl_secs: config.ttl_secs,
        session_retention_secs: app.config.limits.retention_secs,
        region: config.region.clone(),
        upload_grant_secs: 600,
    })
}
pub async fn create(
    app: &App,
    principal: &Principal,
    session: &str,
    headers: &HeaderMap,
    input: UploadGrantInput,
) -> Result<Response> {
    let (config, _) = configured(app)?;
    if input.bytes == 0
        || input.bytes > config.max_bytes
        || !matches!(
            input.content_type.as_str(),
            "application/pdf" | "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        )
    {
        return Err(Error::invalid("unsupported attachment type or byte limit"));
    }
    let fingerprint = identity::digest(&serde_json::to_vec(&(
        &input,
        crate::billing::maximum(headers)?,
        headers
            .get("x-aex-download-budget-bytes")
            .and_then(|h| h.to_str().ok()),
    ))?);
    let mut tx = app.store.0.begin().await?;
    let claim = Store::claim_in(
        &mut tx,
        principal,
        &format!("upload-grant:{session}"),
        identity::operation_key(headers)?,
        &fingerprint,
        &app.config.limits,
    )
    .await?;
    let created: i64 = sqlx::query_scalar(
        "SELECT created FROM sessions WHERE id=$1 AND account=$2 AND state='owned' FOR UPDATE",
    )
    .bind(session)
    .bind(&principal.account)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(Error::missing)?;
    let operation = match claim {
        Claim::Complete(value) => {
            return Ok(Json(serde_json::from_value::<UploadGrant>(value)?).into_response());
        }
        Claim::New(operation) => operation,
    };
    let latest =
        (now() + config.ttl_secs as i64).min(created + app.config.limits.retention_secs as i64);
    let expires_at = input.expires_at.unwrap_or(latest);
    if expires_at <= now() || expires_at > latest {
        return Err(Error::invalid(
            "attachment expiry exceeds session or file retention",
        ));
    }
    capacity(&mut tx, principal, config, input.bytes).await?;
    let id = identity::random("att");
    let read_token = identity::random("read");
    let upload_token = identity::random("upload");
    let attachment = media(config, &id, &read_token, &input.content_type, expires_at);
    reserve_storage(
        &mut tx,
        principal,
        &id,
        input.bytes as i64,
        expires_at,
        headers,
    )
    .await?;
    sqlx::query("INSERT INTO attachments(id,account,session,object_key,content_type,bytes,expires_at,read_verifier,state,operation_key,metadata) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'pending',$9,$10)")
        .bind(&id).bind(&principal.account).bind(session).bind(format!("attachments/{id}")).bind(&input.content_type).bind(input.bytes as i64).bind(expires_at).bind(identity::digest(read_token.as_bytes())).bind(&operation).bind(serde_json::to_string(&attachment)?).execute(&mut *tx).await?;
    let grant = UploadGrant {
        id: id.clone(),
        upload_url: format!(
            "{}/v1/attachments/{id}/upload",
            config.public_origin.trim_end_matches('/')
        ),
        token: upload_token.clone(),
        expires_at: (now() + 600).min(expires_at),
        attachment_expires_at: expires_at,
        bytes: input.bytes,
        content_type: input.content_type,
    };
    sqlx::query("INSERT INTO attachment_upload_grants(attachment,issuing_key,verifier,expires_at) VALUES($1,$2,$3,$4)")
        .bind(&id).bind(&principal.key).bind(identity::digest(upload_token.as_bytes())).bind(grant.expires_at).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE claims SET state='complete',result=$1 WHERE upstream_key=$2 AND state='pending'",
    )
    .bind(serde_json::to_string(&grant)?)
    .bind(operation)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(grant)).into_response())
}
pub async fn principal(app: &App, id: &str, token: &str) -> Result<Principal> {
    let row = sqlx::query("SELECT f.account,g.issuing_key FROM attachment_upload_grants g JOIN attachments f ON f.id=g.attachment JOIN sessions s ON s.id=f.session WHERE g.attachment=$1 AND g.verifier=$2 AND g.expires_at>$3 AND f.expires_at>$3 AND f.state IN ('pending','ready') AND s.state='owned'")
        .bind(id).bind(identity::digest(token.as_bytes())).bind(now()).fetch_optional(&app.store.0).await?.ok_or_else(Error::denied)?;
    let principal = Principal {
        account: row.get("account"),
        key: row.get("issuing_key"),
    };
    app.store.active(&principal).await?;
    Ok(principal)
}
pub async fn receive(
    app: &App,
    principal: &Principal,
    id: &str,
    headers: &HeaderMap,
    bytes: Bytes,
) -> Result<Response> {
    let (config, storage) = configured(app)?;
    let _upload = app.attachment_uploads.read().await;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &principal.account).await?;
    let row = sqlx::query("SELECT f.*,g.digest,g.expires_at AS grant_expiry FROM attachments f JOIN attachment_upload_grants g ON g.attachment=f.id WHERE f.id=$1 AND f.account=$2 FOR UPDATE OF f,g")
        .bind(id).bind(&principal.account).fetch_optional(&mut *tx).await?.ok_or_else(Error::missing)?;
    let content_type: String = row.get("content_type");
    if bytes.len() as i64 != row.get::<i64, _>("bytes")
        || bytes.len() > config.max_bytes
        || headers.get("content-type").and_then(|h| h.to_str().ok()) != Some(&content_type)
    {
        return Err(Error::invalid(
            "upload does not match its granted type and size",
        ));
    }
    check_type(&content_type, &bytes)?;
    let digest = identity::digest(&bytes);
    let state: String = row.get("state");
    let attachment: Attachment = serde_json::from_str(row.get("metadata"))?;
    if row.get::<i64, _>("grant_expiry") <= now()
        || attachment.expires_at <= now()
        || state == "deleting"
    {
        return Err(Error::denied());
    }
    if let Some(saved) = row.get::<Option<String>, _>("digest") {
        if saved != digest {
            return Err(Error::conflict("upload grant was used for different bytes"));
        }
        if state == "ready" {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        return Err(Error::ambiguous());
    }
    let object_key: String = row.get("object_key");
    let session: String = row.get("session");
    sqlx::query("UPDATE attachment_upload_grants SET digest=$1 WHERE attachment=$2")
        .bind(digest)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tokio::time::timeout(
        Duration::from_secs(config.transfer_timeout_secs),
        storage.put_new(&object_key, &content_type, bytes),
    )
    .await
    .map_err(|_| Error::ambiguous())??;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &principal.account).await?;
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions s JOIN accounts a ON a.id=s.account JOIN api_keys k ON k.account=a.id WHERE s.id=$1 AND s.account=$2 AND s.state='owned' AND a.active=1 AND k.id=$3 AND k.active=1)")
        .bind(&session).bind(&principal.account).bind(&principal.key).fetch_one(&mut *tx).await?;
    if !owned || attachment.expires_at <= now() || row.get::<i64, _>("grant_expiry") <= now() {
        sqlx::query("UPDATE attachments SET state='deleting' WHERE id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Err(Error::denied());
    }
    let updated = sqlx::query(
        "UPDATE attachments SET state='ready',published_at=$2 WHERE id=$1 AND state='pending'",
    )
    .bind(id)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(Error::conflict("attachment was revoked"));
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
pub async fn metadata(
    app: &App,
    principal: &Principal,
    session: &str,
    id: &str,
) -> Result<Response> {
    configured(app)?;
    app.store.owned(principal, session, false).await?;
    let row = sqlx::query("SELECT state,metadata FROM attachments WHERE id=$1 AND session=$2 AND account=$3 AND expires_at>$4")
        .bind(id).bind(session).bind(&principal.account).bind(now()).fetch_optional(&app.store.0).await?.ok_or_else(Error::missing)?;
    if row.get::<String, _>("state") != "ready" {
        return Err(Error::conflict("attachment is not ready"));
    }
    Ok(Json(serde_json::from_str::<Attachment>(row.get("metadata"))?).into_response())
}
