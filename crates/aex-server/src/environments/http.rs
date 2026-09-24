use super::{controller, identifier};
use crate::{
    App,
    error::{Error, Result},
    identity::{self, Principal},
    store::Store,
};
use axum::{Json, extract::State, http::HeaderMap};
use brain_protocol::{CreateSessionRequest, Driver, Environment, ToolDefinition};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub url: String,
    pub public_url: String,
    pub bindings: BTreeMap<String, PublishedBinding>,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishedBinding {
    pub accounts: BTreeSet<String>,
    pub credential_env: String,
    pub specification: Binding,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub url: String,
    pub timeout_ms: u32,
    pub tools: Vec<ToolDefinition>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpSelection {
    pub binding: String,
}
#[derive(Serialize, JsonSchema)]
pub struct HttpCatalog {
    pub driver_url: String,
    pub bindings: BTreeMap<String, u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Authorization {
    pub session_id: String,
    pub environment: String,
    pub configuration: Option<AuthorizedSelection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizedSelection {
    pub binding: String,
    pub authorization: String,
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        let driver = url::Url::parse(&self.url)?;
        anyhow::ensure!(
            driver.scheme() == "http"
                && driver
                    .host_str()
                    .and_then(|h| h.parse::<std::net::IpAddr>().ok())
                    .is_some_and(|ip| ip.is_loopback()),
            "HTTP bridge must use literal loopback"
        );
        for address in [&self.url, &self.public_url] {
            let url = url::Url::parse(address)?;
            anyhow::ensure!(
                url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "invalid HTTP driver URL"
            );
        }
        anyhow::ensure!(
            url::Url::parse(&self.public_url)?.scheme() == "https",
            "public HTTP driver must use HTTPS"
        );
        for (id, published) in &self.bindings {
            let binding = &published.specification;
            let url = url::Url::parse(&binding.url)?;
            anyhow::ensure!(
                identifier(id)
                    && !published.accounts.is_empty()
                    && !published.credential_env.is_empty(),
                "HTTP binding requires explicit account and credential grants"
            );
            anyhow::ensure!(
                url.scheme() == "https"
                    && url.host_str().is_some()
                    && url.port_or_known_default() == Some(443)
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "HTTP endpoint must use HTTPS on port 443 without URL credentials"
            );
            anyhow::ensure!(
                (1..=300_000).contains(&binding.timeout_ms) && !binding.tools.is_empty(),
                "HTTP binding needs tools and a bounded call timeout"
            );
            let mut names = BTreeSet::new();
            for tool in &binding.tools {
                anyhow::ensure!(
                    identifier(&tool.name)
                        && names.insert(&tool.name)
                        && tool.input_schema.is_object()
                        && tool.output_schema.as_ref().is_none_or(Value::is_object),
                    "invalid HTTP Tool catalogue"
                );
            }
        }
        Ok(())
    }
}
pub async fn initialize(store: &Store, config: &Config) -> anyhow::Result<()> {
    for (id, published) in &config.bindings {
        let document = serde_json::to_string(&published.specification)?;
        sqlx::query("INSERT INTO http_bindings VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(&document)
            .execute(&store.0)
            .await?;
        let saved: String = sqlx::query_scalar("SELECT document FROM http_bindings WHERE id=$1")
            .bind(id)
            .fetch_one(&store.0)
            .await?;
        anyhow::ensure!(
            serde_json::from_str::<Value>(&saved)? == serde_json::from_str::<Value>(&document)?,
            "HTTP binding {id} changed; publish a new binding ID"
        );
    }
    Ok(())
}
pub fn selected(app: &App, environment: &Environment) -> bool {
    matches!(&environment.driver, Driver::Http { url, .. } if app.config.http_environments.as_ref().is_some_and(|c| &c.public_url == url))
}
pub fn selection<'a>(
    app: &'a App,
    principal: &Principal,
    environment: &Environment,
) -> Result<(&'a str, &'a Binding)> {
    let config = app
        .config
        .http_environments
        .as_ref()
        .ok_or_else(Error::denied)?;
    if !selected(app, environment)
        || !matches!(
            &environment.driver,
            Driver::Http {
                credential: None,
                ..
            }
        )
    {
        return Err(Error::denied());
    }
    let input: HttpSelection = serde_json::from_value(environment.configuration.clone())?;
    let (id, published) = config
        .bindings
        .get_key_value(&input.binding)
        .filter(|(_, p)| p.accounts.contains(&principal.account))
        .ok_or_else(Error::denied)?;
    Ok((id, &published.specification))
}
pub fn definition(tool: &ToolDefinition) -> Value {
    let mut value =
        json!({"name":tool.name,"description":tool.description,"inputSchema":tool.input_schema});
    if let Some(output) = &tool.output_schema {
        value["outputSchema"] = output.clone();
    }
    value
}
pub fn validate_tool(
    binding: &Binding,
    tool: &brain_protocol::Tool,
    placement: &brain_protocol::ToolPlacement,
) -> Result<()> {
    let expected = definition(&tool.definition());
    if !binding.tools.iter().any(|t| definition(t) == expected)
        || (placement.implementation != json!({"type":"http_tool","definition":expected})
            && placement.implementation
                != json!({"type":"http_tool","definition":expected,"configuration":{}}))
    {
        return Err(Error::invalid(
            "HTTP Tool differs from the approved contract",
        ));
    }
    Ok(())
}
pub fn catalog(app: &App, principal: &Principal) -> Result<HttpCatalog> {
    let config = app
        .config
        .http_environments
        .as_ref()
        .ok_or_else(|| Error::invalid("HTTP tools are not configured"))?;
    Ok(HttpCatalog {
        driver_url: config.public_url.clone(),
        bindings: config
            .bindings
            .iter()
            .filter(|(_, p)| p.accounts.contains(&principal.account))
            .map(|(id, p)| (id.clone(), p.specification.timeout_ms))
            .collect(),
    })
}
pub async fn reserve_in(
    app: &App,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    operation: &str,
    request: &mut CreateSessionRequest,
) -> Result<()> {
    for environment in &mut request.environments {
        if !selected(app, environment) {
            continue;
        }
        let (binding, _) = selection(app, principal, environment)?;
        let id = identity::random("http");
        sqlx::query("INSERT INTO http_grants(id,account,operation,issuing_key,environment,binding) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(&id).bind(&principal.account).bind(operation).bind(&principal.key).bind(environment.name.as_str()).bind(binding).execute(&mut **tx).await?;
        environment.driver = Driver::Http {
            url: app.config.http_environments.as_ref().unwrap().url.clone(),
            credential: Some(
                app.environment_token
                    .as_ref()
                    .ok_or_else(Error::internal)?
                    .clone(),
            ),
        };
        environment.configuration = json!({"binding":binding,"authorization":id});
    }
    Ok(())
}
pub async fn authorize(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Authorization>,
) -> Result<Json<Value>> {
    controller(&app, &headers)?;
    if !app.accepting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(Error::capacity());
    }
    let mut tx = app.store.0.begin().await?;
    let row = if let Some(selection) = &input.configuration {
        sqlx::query("SELECT * FROM http_grants WHERE id=$1 AND binding=$2 FOR UPDATE")
            .bind(&selection.authorization)
            .bind(&selection.binding)
            .fetch_optional(&mut *tx)
            .await?
    } else {
        sqlx::query("SELECT * FROM http_grants WHERE session=$1 AND environment=$2 FOR UPDATE")
            .bind(&input.session_id)
            .bind(&input.environment)
            .fetch_optional(&mut *tx)
            .await?
    }
    .ok_or_else(Error::denied)?;
    let account: String = row.get("account");
    let principal = Principal {
        account: account.clone(),
        key: row.get("issuing_key"),
    };
    app.store.active(&principal).await?;
    if !identifier(&input.session_id)
        || row.get::<String, _>("environment") != input.environment
        || row
            .get::<Option<String>, _>("session")
            .is_some_and(|s| s != input.session_id)
    {
        return Err(Error::denied());
    }
    if input.configuration.is_none() {
        app.store
            .owned(&principal, &input.session_id, false)
            .await?;
    }
    let binding: String = row.get("binding");
    let published = app
        .config
        .http_environments
        .as_ref()
        .and_then(|c| c.bindings.get(&binding))
        .filter(|p| p.accounts.contains(&account))
        .ok_or_else(Error::denied)?;
    let token = std::env::var(&published.credential_env).map_err(|_| Error::internal())?;
    if token.len() < 32 {
        return Err(Error::internal());
    }
    sqlx::query("UPDATE http_grants SET session=$1 WHERE id=$2")
        .bind(&input.session_id)
        .bind(row.get::<String, _>("id"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let spec = &published.specification;
    Ok(Json(
        json!({"url":spec.url,"token":token,"timeoutMs":spec.timeout_ms,"tools":spec.tools.iter().map(definition).collect::<Vec<_>>()}),
    ))
}
