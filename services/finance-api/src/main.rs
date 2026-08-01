//! `finance-api` Lambda composition root.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

/// Exact validated finance API configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Config {
    aurora_cluster_arn: String,
    aurora_secret_arn: String,
    database_name: String,
    role: String,
    stripe_command_edge_arn: String,
    default_pricing_version: String,
    plane: String,
    region: String,
    transaction_deadline_ms: u64,
    page_limit: u32,
}

impl Config {
    fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let config = Self {
            aurora_cluster_arn: required(&lookup, "AEX_AURORA_CLUSTER_ARN")?,
            aurora_secret_arn: required(&lookup, "AEX_AURORA_SECRET_ARN")?,
            database_name: required(&lookup, "AEX_DATABASE_NAME")?,
            role: required(&lookup, "AEX_DATABASE_ROLE")?,
            stripe_command_edge_arn: required(&lookup, "AEX_STRIPE_COMMAND_EDGE_ARN")?,
            default_pricing_version: required(&lookup, "AEX_DEFAULT_PRICING_VERSION")?,
            plane: required(&lookup, "AEX_PLANE")?,
            region: required(&lookup, "AEX_REGION")?,
            transaction_deadline_ms: positive_u64(&lookup, "AEX_TX_DEADLINE_MS")?,
            page_limit: positive_u32(&lookup, "AEX_PAGE_LIMIT")?,
        };
        if config.role != "aex_finance_api" {
            return Err(ConfigError::WrongRole(config.role));
        }
        if !matches!(config.plane.as_str(), "dev" | "prd") {
            return Err(ConfigError::InvalidPlane(config.plane));
        }
        Ok(config)
    }
}

fn required(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<String, ConfigError> {
    lookup(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn positive_u64(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<u64, ConfigError> {
    let raw = required(lookup, name)?;
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(ConfigError::InvalidPositiveInteger(name))
}

fn positive_u32(
    lookup: &impl Fn(&str) -> Option<String>,
    name: &'static str,
) -> Result<u32, ConfigError> {
    let raw = required(lookup, name)?;
    raw.parse::<u32>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(ConfigError::InvalidPositiveInteger(name))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum ConfigError {
    #[error("required variable `{0}` is missing")]
    Missing(&'static str),
    #[error("variable `{0}` must be a positive integer")]
    InvalidPositiveInteger(&'static str),
    #[error("finance API must use role `aex_finance_api`, got `{0}`")]
    WrongRole(String),
    #[error("plane must be `dev` or `prd`, got `{0}`")]
    InvalidPlane(String),
}

#[derive(Debug)]
struct AppState {
    ready: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    status: &'static str,
}

fn app(ready: bool) -> Router {
    Router::new()
        .route("/internal/healthz", get(healthz))
        .route("/internal/readyz", get(readyz))
        .with_state(Arc::new(AppState { ready }))
}

async fn healthz() -> Json<Health> {
    Json(Health { status: "ok" })
}

async fn readyz(State(state): State<Arc<AppState>>) -> (StatusCode, Json<Health>) {
    if state.ready {
        (StatusCode::OK, Json(Health { status: "ready" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Health {
                status: "not_ready",
            }),
        )
    }
}

#[tokio::main]
async fn main() -> Result<(), lambda_http::Error> {
    let config = Config::from_env().map_err(|error| lambda_http::Error::from(error.to_string()))?;
    let db = aex_finance_aurora::store::FinanceDbConfig {
        cluster_arn: config.aurora_cluster_arn,
        secret_arn: config.aurora_secret_arn,
        database: config.database_name,
        transaction_deadline_ms: config.transaction_deadline_ms,
    };
    db.validate()
        .map_err(|error| lambda_http::Error::from(error.to_string()))?;
    // Readiness becomes true only after the deployable-specific role probe is
    // composed. The uncredentialed rewrite run cannot perform that AWS probe.
    lambda_http::run(app(false)).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    use super::{Config, ConfigError, app};

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("AEX_AURORA_CLUSTER_ARN", "arn:cluster:fixture".into()),
            ("AEX_AURORA_SECRET_ARN", "arn:secret:fixture".into()),
            ("AEX_DATABASE_NAME", "aex".into()),
            ("AEX_DATABASE_ROLE", "aex_finance_api".into()),
            ("AEX_STRIPE_COMMAND_EDGE_ARN", "arn:lambda:stripe".into()),
            ("AEX_DEFAULT_PRICING_VERSION", "synthetic-zero-v1".into()),
            ("AEX_PLANE", "dev".into()),
            ("AEX_REGION", "eu-west-1".into()),
            ("AEX_TX_DEADLINE_MS", "5000".into()),
            ("AEX_PAGE_LIMIT", "100".into()),
        ])
    }

    #[test]
    fn configuration_is_total_and_role_specific() {
        let vars = complete();
        assert!(Config::from_lookup(|name| vars.get(name).cloned()).is_ok());
        for name in vars.keys() {
            let mut missing = vars.clone();
            missing.remove(name);
            assert!(
                matches!(
                    Config::from_lookup(|key| missing.get(key).cloned()),
                    Err(ConfigError::Missing(_))
                ),
                "missing {name}"
            );
        }
        let mut wrong = vars;
        wrong.insert("AEX_DATABASE_ROLE", "aex_finance_ingest".into());
        assert!(matches!(
            Config::from_lookup(|name| wrong.get(name).cloned()),
            Err(ConfigError::WrongRole(_))
        ));
    }

    #[tokio::test]
    async fn health_and_readiness_are_distinct() {
        let live = app(false);
        let health = live
            .clone()
            .oneshot(
                Request::get("/internal/healthz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(health.status(), StatusCode::OK);
        let ready = live
            .oneshot(
                Request::get("/internal/readyz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        let ready = app(true)
            .oneshot(
                Request::get("/internal/readyz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(ready.status(), StatusCode::OK);
    }
}
