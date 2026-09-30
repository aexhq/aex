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
    if !brain_protocol::ids::is_sha256(id) {
        return Err(Error::invalid("artifact content address required"));
    }
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
        Err(Error::missing())
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
        if kind == "programs" {
            if body.is_empty() || std::str::from_utf8(&body).is_err() {
                return Err(Error::invalid("program must be nonempty UTF-8"));
            }
        } else if !body.starts_with(b"\0asm\x0d\0\x01\0") {
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

pub fn references<'a>(
    implementation: &'a serde_json::Value,
    kind: &'static str,
) -> Result<Vec<(&'static str, &'a str)>> {
    let descriptor = implementation
        .as_object()
        .ok_or_else(|| Error::invalid("invalid hosted implementation"))?;
    let program = match descriptor.get("type").and_then(|v| v.as_str()) {
        Some("brain_component") => false,
        Some("brain_program") => true,
        _ => return Err(Error::invalid("unsupported hosted implementation")),
    };
    if descriptor.get("entrypoint").and_then(|v| v.as_str())
        != Some(if kind == "agentloops" { "turn" } else { "run" })
        || !descriptor.keys().all(|key| {
            matches!(key.as_str(), "type" | "entrypoint" | "id")
                || (program && key == "runtime")
                || (kind == "tools" && key == "configuration")
        })
    {
        return Err(Error::invalid("invalid hosted implementation"));
    }
    let id = descriptor
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| brain_protocol::ids::is_sha256(id))
        .ok_or_else(|| Error::invalid("artifact content address required"))?;
    if program {
        let runtime = descriptor
            .get("runtime")
            .and_then(|v| v.as_str())
            .filter(|id| brain_protocol::ids::is_sha256(id))
            .ok_or_else(|| Error::invalid("runtime content address required"))?;
        Ok(vec![(kind, runtime), ("programs", id)])
    } else {
        Ok(vec![(kind, id)])
    }
}

pub fn session_preparation(
    request: &brain_protocol::CreateSessionRequest,
) -> Result<brain_protocol::BrainPreparation> {
    let mut refs = references(&request.agentloop.implementation, "agentloops")?;
    for tool in &request.tools {
        for (name, placement) in &tool.placements {
            if request.environments.iter().any(|env| {
                env.name == *name && matches!(env.driver, brain_protocol::Driver::Brain {})
            }) {
                refs.extend(references(&placement.implementation, "tools")?);
            }
        }
    }
    let mut preparation = brain_protocol::BrainPreparation::default();
    for (kind, id) in refs {
        match kind {
            "agentloops" => preparation
                .agentloops
                .push(brain_protocol::AgentloopId::new(id)),
            "tools" => preparation.tools.push(brain_protocol::ToolId::new(id)),
            "programs" => preparation
                .programs
                .push(brain_protocol::ProgramId::new(id)),
            _ => unreachable!(),
        }
    }
    Ok(preparation)
}

pub async fn prepare(
    app: &App,
    p: &Principal,
    preparation: &brain_protocol::BrainPreparation,
    access: Option<&crate::clients::Access>,
) -> Result<Response> {
    preparation.validate().map_err(Error::invalid)?;
    for (kind, id) in preparation
        .agentloops
        .iter()
        .map(|id| ("agentloops", id.as_str()))
        .chain(preparation.tools.iter().map(|id| ("tools", id.as_str())))
        .chain(
            preparation
                .programs
                .iter()
                .map(|id| ("programs", id.as_str())),
        )
    {
        if let Some(access) = access {
            access.artifact(kind, id)?;
        }
        owned(app, p, kind, id).await?;
    }
    let _guard = app
        .artifact_admission
        .try_lock()
        .map_err(|_| Error::capacity())?;
    let headers = HeaderMap::from_iter([(
        "content-type".parse().unwrap(),
        "application/json".parse().unwrap(),
    )]);
    let response = app
        .brain
        .request(
            Method::POST,
            "/v1/brain-env/prepare",
            &headers,
            Bytes::from(serde_json::to_vec(preparation)?),
            None,
            None,
        )
        .await?;
    crate::http::finite(app, response).await
}
