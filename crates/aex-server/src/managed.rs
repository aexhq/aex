pub mod journal;
use crate::{
    App,
    brain_contract::{CreateSession, Tool},
    error::{Error, Result},
    identity::{self, Principal},
    store::Store,
};
use axum::{Json, extract::State, http::HeaderMap};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub accounts: BTreeSet<String>,
    pub operations: BTreeSet<String>,
    pub active_per_account: u32,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub accounts: BTreeSet<String>,
    pub max_lifetime_ms: u32,
    pub specification: Value,
}
impl Policy {
    pub fn validate(&self, name: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            ["application", "browser", "sandbox", "session", "mcp"].contains(&name),
            "unknown managed provider"
        );
        anyhow::ensure!(
            self.active_per_account > 0 && (name == "application" || !self.operations.is_empty()),
            "managed provider requires explicit operations and capacity"
        );
        for profile in self.profiles.values() {
            anyhow::ensure!(
                profile.max_lifetime_ms > 0 && !profile.accounts.is_empty(),
                "sandbox profile requires authority and lifetime"
            );
        }
        Ok(())
    }
    fn allows(&self, principal: &Principal) -> bool {
        self.public || self.accounts.contains(&principal.account)
    }
}
#[derive(Serialize, JsonSchema)]
pub struct Catalog {
    pub providers: BTreeMap<String, ProviderCatalog>,
}
#[derive(Serialize, JsonSchema)]
pub struct ProviderCatalog {
    pub operations: BTreeSet<String>,
    pub profiles: BTreeMap<String, u32>,
}
pub fn catalog(app: &App, principal: &Principal) -> Catalog {
    Catalog {
        providers: app
            .config
            .managed_providers
            .iter()
            .filter(|(_, policy)| policy.allows(principal))
            .map(|(name, policy)| {
                (
                    name.clone(),
                    ProviderCatalog {
                        operations: policy.operations.clone(),
                        profiles: policy
                            .profiles
                            .iter()
                            .filter(|(_, profile)| profile.accounts.contains(&principal.account))
                            .map(|(name, profile)| (name.clone(), profile.max_lifetime_ms))
                            .collect(),
                    },
                )
            })
            .collect(),
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Application {
    pub endpoint: String,
    pub timeout_ms: u32,
    pub tools: Vec<Tool>,
}
fn application(value: Value) -> Result<()> {
    let selected: Application = serde_json::from_value(value)?;
    let url = url::Url::parse(&selected.endpoint)
        .map_err(|_| Error::invalid("invalid application endpoint"))?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(1..=300_000).contains(&selected.timeout_ms)
        || selected.tools.is_empty()
    {
        return Err(Error::invalid(
            "application tools require bounded HTTPS/443 endpoints and explicit descriptors",
        ));
    }
    Ok(())
}
pub async fn reserve_in(
    app: &App,
    tx: &mut Transaction<'_, Postgres>,
    p: &Principal,
    operation: &str,
    request: &mut CreateSession,
    ceiling: Option<i64>,
) -> Result<()> {
    for binding in request.config.bindings.as_mut().into_iter().flatten() {
        let Some(provider) = binding.provider.as_deref() else {
            continue;
        };
        let policy = app
            .config
            .managed_providers
            .get(provider)
            .filter(|policy| policy.allows(p))
            .ok_or_else(Error::denied)?;
        let parameters = binding.parameters.clone().unwrap_or_else(|| json!({}));
        let mut parameters = parameters
            .as_object()
            .cloned()
            .ok_or_else(|| Error::invalid("provider parameters must be an object"))?;
        if parameters.contains_key("authorization") {
            return Err(Error::denied());
        }
        if provider == "sandbox"
            && parameters
                .keys()
                .any(|name| !["profile", "lifetime_ms"].contains(&name.as_str()))
            || ["browser", "session", "mcp"].contains(&provider) && !parameters.is_empty()
        {
            return Err(Error::invalid("unsupported provider parameters"));
        }
        let operations = binding
            .operations
            .as_deref()
            .ok_or_else(|| Error::invalid("explicit operations required"))?;
        if provider == "application" {
            application(Value::Object(parameters.clone()))?;
            let selected: Application = serde_json::from_value(Value::Object(parameters.clone()))?;
            if operations
                .iter()
                .any(|name| !selected.tools.iter().any(|tool| &tool.name == name))
            {
                return Err(Error::invalid("application operation is undeclared"));
            }
        } else if operations
            .iter()
            .any(|name| !policy.operations.contains(name))
        {
            return Err(Error::denied());
        }
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM tool_grants WHERE account=$1 AND provider=$2 AND NOT revoked AND (provider<>'sandbox' OR NOT settled) AND expires_ms>$3").bind(&p.account).bind(provider).bind(crate::model_usage::now_ms()).fetch_one(&mut **tx).await?;
        if count >= i64::from(policy.active_per_account) {
            return Err(Error::capacity());
        }
        let id = identity::random("toolgrant");
        let mut specification = None;
        let lifetime = if provider == "sandbox" {
            let profile = parameters
                .get("profile")
                .and_then(Value::as_str)
                .and_then(|name| policy.profiles.get(name))
                .filter(|profile| profile.accounts.contains(&p.account))
                .ok_or_else(Error::denied)?;
            specification = Some(serde_json::to_string(&profile.specification)?);
            let lifetime = parameters
                .get("lifetime_ms")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0 && *value <= u64::from(profile.max_lifetime_ms))
                .ok_or_else(|| Error::invalid("sandbox lifetime exceeds the selected profile"))?;
            if !crate::billing::reserve_in(
                tx,
                &p.account,
                crate::billing::Reservation {
                    id: &id,
                    resource: &id,
                    meter: crate::billing::Meter::SandboxMs,
                    max_units: lifetime as i64,
                    max_cost: ceiling,
                },
            )
            .await?
            {
                return Err(Error::credits(
                    "managed compute requires prepaid enrollment",
                ));
            }
            lifetime as i64
        } else {
            i64::try_from(app.config.limits.retention_secs)
                .map_err(|_| Error::capacity())?
                .checked_mul(1000)
                .ok_or_else(Error::capacity)?
        };
        parameters.insert("authorization".into(), json!(id));
        binding.parameters = Some(Value::Object(parameters.clone()));
        sqlx::query("INSERT INTO tool_grants(id,account,issuing_key,operation,alias,provider,parameters,profile,expires_ms) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(&id).bind(&p.account).bind(&p.key).bind(operation).bind(&binding.alias).bind(provider).bind(serde_json::to_string(&parameters)?).bind(specification).bind(crate::model_usage::now_ms().checked_add(lifetime).ok_or_else(Error::capacity)?).execute(&mut **tx).await?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    pub tenant: String,
    pub session: String,
    pub provider: String,
    pub alias: String,
    pub parameters: Value,
    #[serde(default)]
    pub action: bool,
}
pub(super) fn service(app: &App, headers: &HeaderMap) -> Result<()> {
    if app.environment_token.as_ref().is_some_and(|token| {
        identity::matches(
            identity::bearer(headers).unwrap_or(""),
            &identity::digest(token.as_bytes()),
        )
    }) {
        Ok(())
    } else {
        Err(Error::denied())
    }
}
pub async fn authorize(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Authorization>,
) -> Result<Json<Value>> {
    service(&app, &headers)?;
    let id = input
        .parameters
        .get("authorization")
        .and_then(Value::as_str)
        .ok_or_else(Error::denied)?;
    let row=sqlx::query("SELECT g.*,c.state AS claim_state FROM tool_grants g JOIN claims c ON c.upstream_key=g.operation WHERE g.id=$1 AND g.account=$2 AND NOT g.revoked AND g.expires_ms>$3")
        .bind(id).bind(&input.tenant).bind(crate::model_usage::now_ms()).fetch_optional(&app.store.0).await?.ok_or_else(Error::denied)?;
    let p = Principal {
        account: row.get("account"),
        key: row.get("issuing_key"),
    };
    app.store.active(&p).await?;
    let parameters: Value = serde_json::from_str(row.get("parameters"))?;
    if row.get::<String, _>("provider") != input.provider
        || row.get::<String, _>("alias") != input.alias
        || (parameters != input.parameters && input.parameters != json!({"authorization":id}))
    {
        return Err(Error::denied());
    }
    match row.get::<Option<String>, _>("session") {
        Some(session) if session == input.session => app.store.owned(&p, &session, false).await?,
        None if !input.action && row.get::<String, _>("claim_state") == "pending" => {}
        _ => return Err(Error::denied()),
    }
    let profile: Option<Value> = row
        .get::<Option<String>, _>("profile")
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    Ok(Json(
        json!({"parameters":parameters,"profile":profile,"expires_ms":row.get::<i64,_>("expires_ms")}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub authorization: String,
    pub tenant: String,
    pub session: Option<String>,
    pub units_ms: i64,
    pub terminal: bool,
    pub receipt: String,
}
pub async fn report(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Usage>,
) -> Result<Json<Value>> {
    service(&app, &headers)?;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &input.tenant).await?;
    let grant=sqlx::query("SELECT id FROM tool_grants WHERE id=$1 AND account=$2 AND session IS NOT DISTINCT FROM $3 AND provider='sandbox'").bind(&input.authorization).bind(&input.tenant).bind(&input.session).fetch_optional(&mut *tx).await?.ok_or_else(Error::denied)?;
    let _: String = grant.get("id");
    crate::billing::meter_in(
        &mut tx,
        &crate::billing::UsageReport {
            id: input.receipt,
            reservation: input.authorization.clone(),
            units: input.units_ms,
            terminal: input.terminal,
        },
    )
    .await?;
    if input.terminal {
        sqlx::query("UPDATE tool_grants SET settled=TRUE WHERE id=$1")
            .bind(input.authorization)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(json!({"recorded":true})))
}
pub async fn revoke(app: &App, session: &str) -> Result<()> {
    sqlx::query("UPDATE tool_grants SET revoked=TRUE WHERE session=$1")
        .bind(session)
        .execute(&app.store.0)
        .await?;
    Ok(())
}
pub async fn settled(app: &App, session: &str) -> Result<bool> {
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tool_grants WHERE session=$1 AND provider IN ('sandbox','browser','application') AND NOT settled)").bind(session).fetch_one(&app.store.0).await?;
    Ok(!pending)
}
pub async fn maintain(app: &App) -> Result<()> {
    sqlx::query("UPDATE tool_grants g SET revoked=TRUE WHERE NOT revoked AND (expires_ms<=$1 OR EXISTS(SELECT 1 FROM accounts a WHERE a.id=g.account AND a.active!=1) OR EXISTS(SELECT 1 FROM api_keys k WHERE k.id=g.issuing_key AND k.active!=1) OR EXISTS(SELECT 1 FROM sessions s WHERE s.id=g.session AND s.state!='owned'))").bind(crate::model_usage::now_ms()).execute(&app.store.0).await?;
    Ok(())
}
pub async fn pending(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    service(&app, &headers)?;
    maintain(&app).await?;
    let rows=sqlx::query("SELECT id,account,session,provider,expires_ms FROM tool_grants WHERE provider IN ('sandbox','browser','application') AND revoked AND NOT settled ORDER BY expires_ms,id").fetch_all(&app.store.0).await?;
    Ok(Json(
        json!({"grants":rows.iter().map(|row|json!({"authorization":row.get::<String,_>("id"),"tenant":row.get::<String,_>("account"),"session":row.get::<Option<String>,_>("session"),"provider":row.get::<String,_>("provider"),"expires_ms":row.get::<i64,_>("expires_ms")})).collect::<Vec<_>>()}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
    pub authorization: String,
    pub tenant: String,
    pub session: Option<String>,
}
pub async fn cleaned(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Cleanup>,
) -> Result<Json<Value>> {
    service(&app, &headers)?;
    let updated=sqlx::query("UPDATE tool_grants SET settled=TRUE,parameters='{}',profile=NULL WHERE id=$1 AND account=$2 AND session IS NOT DISTINCT FROM $3 AND provider IN ('browser','application') AND revoked").bind(input.authorization).bind(input.tenant).bind(input.session).execute(&app.store.0).await?;
    if updated.rows_affected() != 1 {
        return Err(Error::denied());
    }
    Ok(Json(json!({"cleaned":true})))
}
