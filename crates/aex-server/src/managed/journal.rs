use crate::{App, error::Result};
use axum::{Json, extract::State, http::HeaderMap};
use serde::Deserialize;
use serde_json::Value;

// The tool controller owns these opaque records. This private store lets its
// stateless deployment acknowledge a finish only after PostgreSQL commits it.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Get { id: String },
    Set { id: String, value: Value },
    Completion { verifier: String },
    Delete { id: String },
    DeleteGrant { grant: String },
}
pub async fn handle(
    State(app): State<App>,
    headers: HeaderMap,
    Json(action): Json<Action>,
) -> Result<Json<Value>> {
    super::service(&app, &headers)?;
    let value = match action {
        Action::Get { id } => {
            sqlx::query_scalar::<_, String>("SELECT value FROM tool_service_jobs WHERE id=$1")
                .bind(id)
                .fetch_optional(&app.store.0)
                .await?
        }
        Action::Completion { verifier } => {
            sqlx::query_scalar::<_, String>(
                "SELECT value FROM tool_service_jobs WHERE value::jsonb->>'verifier'=$1",
            )
            .bind(verifier)
            .fetch_optional(&app.store.0)
            .await?
        }
        Action::Set { id, value } => {
            let mut tx = app.store.0.begin().await?;
            if let Some(grant) = value.get("grant").and_then(Value::as_str) {
                let revoked = sqlx::query_scalar::<_, bool>(
                    "SELECT revoked FROM tool_grants WHERE id=$1 FOR UPDATE",
                )
                .bind(grant)
                .fetch_optional(&mut *tx)
                .await?;
                if revoked != Some(false) {
                    return Err(crate::error::Error::conflict("tool binding was revoked"));
                }
            }
            sqlx::query("INSERT INTO tool_service_jobs(id,value) VALUES($1,$2) ON CONFLICT(id) DO UPDATE SET value=EXCLUDED.value").bind(id).bind(value.to_string()).execute(&mut *tx).await?;
            tx.commit().await?;
            None
        }
        Action::Delete { id } => {
            sqlx::query("DELETE FROM tool_service_jobs WHERE id=$1")
                .bind(id)
                .execute(&app.store.0)
                .await?;
            None
        }
        Action::DeleteGrant { grant } => {
            sqlx::query("DELETE FROM tool_service_jobs WHERE value::jsonb->>'grant'=$1")
                .bind(grant)
                .execute(&app.store.0)
                .await?;
            None
        }
    };
    Ok(Json(
        value
            .map(|value| serde_json::from_str(&value))
            .transpose()?
            .unwrap_or(Value::Null),
    ))
}
