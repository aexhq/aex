use crate::{
    App, billing,
    error::{Error, Result},
    identity::{self, Principal},
    store::Store,
};
use axum::{Json, extract::State, http::HeaderMap};
use brain_protocol::{CreateSessionRequest, Driver, Environment};
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
    pub configuration: Value,
    pub active_per_account: u32,
    pub profiles: BTreeMap<String, PublishedProfile>,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublishedProfile {
    pub accounts: BTreeSet<String>,
    pub specification: Profile,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "EnvironmentProfile")]
pub struct Profile {
    pub max_lifetime_ms: u32,
    #[serde(flatten)]
    pub configuration: BTreeMap<String, Value>,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "EnvironmentSelection")]
pub struct Selection {
    pub profile: String,
    pub lifetime_ms: u32,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorizedSelection {
    pub profile: String,
    pub lifetime_ms: u32,
    pub authorization: String,
}
#[derive(Serialize, JsonSchema)]
#[schemars(rename = "EnvironmentCatalog")]
pub struct Catalog {
    pub driver_url: String,
    pub profiles: BTreeMap<String, Profile>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Authorization {
    pub session_id: String,
    pub environment: String,
    pub configuration: AuthorizedSelection,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Usage {
    pub session_id: String,
    pub environment: String,
    pub authorization: String,
    pub profile: String,
    pub resource_id: Option<String>,
    pub units_ms: i64,
    pub terminal: bool,
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}
impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        for address in [&self.url, &self.public_url] {
            let url = url::Url::parse(address)?;
            anyhow::ensure!(
                url.scheme() == "https"
                    || (url.scheme() == "http"
                        && url
                            .host_str()
                            .and_then(|h| h.parse::<std::net::IpAddr>().ok())
                            .is_some_and(|ip| ip.is_loopback())),
                "managed Environment URL requires HTTPS or literal loopback HTTP"
            );
            anyhow::ensure!(
                url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "invalid managed Environment URL"
            );
        }
        anyhow::ensure!(
            self.active_per_account > 0 && !self.profiles.is_empty(),
            "managed Environment configuration is empty"
        );
        for (id, published) in &self.profiles {
            let p = &published.specification;
            anyhow::ensure!(
                identifier(id) && !published.accounts.is_empty(),
                "profile requires an identifier and explicit account grants"
            );
            anyhow::ensure!(
                (1000..=300_000).contains(&p.max_lifetime_ms),
                "profile lifetime must be 1 to 300 seconds"
            );
        }
        Ok(())
    }
}
pub async fn initialize(store: &Store, config: &Config) -> anyhow::Result<()> {
    for (id, profile) in &config.profiles {
        let document = serde_json::to_string(&profile.specification)?;
        sqlx::query("INSERT INTO environment_profiles VALUES($1,$2) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(&document)
            .execute(&store.0)
            .await?;
        let saved: String =
            sqlx::query_scalar("SELECT document FROM environment_profiles WHERE id=$1")
                .bind(id)
                .fetch_one(&store.0)
                .await?;
        anyhow::ensure!(
            serde_json::from_str::<Value>(&saved)? == serde_json::from_str::<Value>(&document)?,
            "published profile {id} changed; publish a new profile ID"
        );
    }
    Ok(())
}
pub fn selection(app: &App, principal: &Principal, environment: &Environment) -> Result<Selection> {
    let config = app
        .config
        .environments
        .as_ref()
        .ok_or_else(|| Error::invalid("remote Environments are not hosted"))?;
    let Driver::Http { url, credential } = &environment.driver else {
        return Err(Error::invalid(
            "managed Environment requires its catalog driver",
        ));
    };
    if url != &config.public_url || credential.is_some() {
        return Err(Error::invalid(
            "only the managed catalog driver is admitted",
        ));
    }
    let selection: Selection = serde_json::from_value(environment.configuration.clone())?;
    let profile = config
        .profiles
        .get(&selection.profile)
        .filter(|p| p.accounts.contains(&principal.account))
        .ok_or_else(Error::denied)?;
    if selection.lifetime_ms < 1000 || selection.lifetime_ms > profile.specification.max_lifetime_ms
    {
        return Err(Error::invalid(
            "requested Environment lifetime exceeds its profile",
        ));
    }
    Ok(selection)
}
pub fn catalog(app: &App, principal: &Principal) -> Result<Catalog> {
    let config = app
        .config
        .environments
        .as_ref()
        .ok_or_else(|| Error::invalid("managed Environments are not configured"))?;
    Ok(Catalog {
        driver_url: config.public_url.clone(),
        profiles: config
            .profiles
            .iter()
            .filter(|(_, p)| p.accounts.contains(&principal.account))
            .map(|(id, p)| (id.clone(), p.specification.clone()))
            .collect(),
    })
}
pub async fn reserve_in(
    app: &App,
    tx: &mut Transaction<'_, Postgres>,
    principal: &Principal,
    operation: &str,
    request: &mut CreateSessionRequest,
    ceiling: Option<i64>,
) -> Result<()> {
    let selected: Vec<_> = request
        .environments
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.driver, Driver::Http { .. }))
        .map(|(index, e)| Ok((index, selection(app, principal, e)?)))
        .collect::<Result<_>>()?;
    if selected.is_empty() {
        return Ok(());
    }
    let config = app.config.environments.as_ref().unwrap();
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM environment_grants e JOIN credit_reservations r ON r.id=e.id WHERE r.account=$1 AND r.state='open'")
        .bind(&principal.account).fetch_one(&mut **tx).await?;
    if active + selected.len() as i64 > i64::from(config.active_per_account) {
        return Err(Error::capacity());
    }
    let mut reserved = 0_i64;
    for (index, selection) in selected {
        let environment = &mut request.environments[index];
        let id = identity::random("env");
        let paid = billing::reserve_in(
            tx,
            &principal.account,
            billing::Reservation {
                id: &id,
                resource: &id,
                meter: billing::Meter::SandboxMs,
                max_units: i64::from(selection.lifetime_ms),
                max_cost: ceiling,
            },
        )
        .await?;
        if !paid {
            return Err(Error::credits(
                "managed compute requires prepaid enrollment and available credits",
            ));
        }
        reserved +=
            sqlx::query_scalar::<_, i64>("SELECT remaining FROM credit_reservations WHERE id=$1")
                .bind(&id)
                .fetch_one(&mut **tx)
                .await?;
        if ceiling.is_none_or(|maximum| reserved > maximum) {
            return Err(Error::credits(
                "managed Environments exceed the request cost ceiling",
            ));
        }
        sqlx::query("INSERT INTO environment_grants(id,operation,issuing_key,environment,profile,lifetime_ms,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(&id).bind(operation).bind(&principal.key).bind(environment.name.as_str()).bind(&selection.profile).bind(i64::from(selection.lifetime_ms))
            .bind(now_ms()+i64::from(selection.lifetime_ms)).execute(&mut **tx).await?;
        environment.driver = Driver::Http {
            url: config.url.clone(),
            credential: Some(
                app.environment_token
                    .as_ref()
                    .ok_or_else(Error::internal)?
                    .clone(),
            ),
        };
        environment.configuration = serde_json::to_value(AuthorizedSelection {
            profile: selection.profile,
            lifetime_ms: selection.lifetime_ms,
            authorization: id,
        })?;
    }
    Ok(())
}
fn controller(app: &App, headers: &HeaderMap) -> Result<()> {
    if !app.environment_token.as_ref().is_some_and(|token| {
        identity::matches(
            identity::bearer(headers).unwrap_or(""),
            &identity::digest(token.as_bytes()),
        )
    }) {
        return Err(Error::denied());
    }
    Ok(())
}
pub async fn configuration(State(app): State<App>, headers: HeaderMap) -> Result<Json<Value>> {
    controller(&app, &headers)?;
    let config = app
        .config
        .environments
        .as_ref()
        .ok_or_else(Error::missing)?;
    let mut profiles = BTreeMap::new();
    for row in sqlx::query("SELECT id,document FROM environment_profiles")
        .fetch_all(&app.store.0)
        .await?
    {
        profiles.insert(
            row.get::<String, _>("id"),
            serde_json::from_str::<Value>(row.get("document"))?,
        );
    }
    Ok(Json(
        json!({"configuration":config.configuration,"profiles":profiles}),
    ))
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
    let account: String = sqlx::query_scalar("SELECT account FROM credit_reservations WHERE id=$1")
        .bind(&input.configuration.authorization)
        .fetch_optional(&app.store.0)
        .await?
        .ok_or_else(Error::missing)?;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    let row = sqlx::query("SELECT e.*,r.state,w.balance,w.suspended,a.active,k.active AS key_active FROM environment_grants e JOIN credit_reservations r ON r.id=e.id JOIN wallets w ON w.account=r.account JOIN accounts a ON a.id=r.account JOIN api_keys k ON k.id=e.issuing_key WHERE e.id=$1")
        .bind(&input.configuration.authorization).fetch_one(&mut *tx).await?;
    let expires_at: i64 = row.get("expires_at");
    if !identifier(&input.session_id)
        || input.environment != row.get::<String, _>("environment")
        || input.configuration.profile != row.get::<String, _>("profile")
        || i64::from(input.configuration.lifetime_ms) != row.get::<i64, _>("lifetime_ms")
        || row
            .get::<Option<String>, _>("session")
            .is_some_and(|id| id != input.session_id)
        || row.get::<String, _>("state") != "open"
        || expires_at <= now_ms()
        || row.get::<i64, _>("active") != 1
        || row.get::<i64, _>("key_active") != 1
        || row.get::<bool, _>("suspended")
        || row.get::<i64, _>("balance") < 0
    {
        return Err(Error::denied());
    }
    sqlx::query("UPDATE environment_grants SET session=$1 WHERE id=$2")
        .bind(&input.session_id)
        .bind(&input.configuration.authorization)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({"expiresAt":expires_at})))
}
pub async fn report(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<Usage>,
) -> Result<Json<Value>> {
    controller(&app, &headers)?;
    let account: String = sqlx::query_scalar("SELECT account FROM credit_reservations WHERE id=$1")
        .bind(&input.authorization)
        .fetch_optional(&app.store.0)
        .await?
        .ok_or_else(Error::missing)?;
    let mut tx = app.store.0.begin().await?;
    Store::lock_account(&mut tx, &account).await?;
    let row = sqlx::query("SELECT e.*,r.units,r.state FROM environment_grants e JOIN credit_reservations r ON r.id=e.id WHERE e.id=$1").bind(&input.authorization).fetch_one(&mut *tx).await?;
    let sandbox: Option<String> = row.get("sandbox_id");
    if row.get::<Option<String>, _>("session").as_deref() != Some(&input.session_id)
        || row.get::<String, _>("environment") != input.environment
        || row.get::<String, _>("profile") != input.profile
        || input.units_ms < 0
        || input.units_ms > row.get::<i64, _>("lifetime_ms")
        || input
            .resource_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 1024)
        || sandbox
            .as_ref()
            .is_some_and(|id| Some(id) != input.resource_id.as_ref())
        || (input.resource_id.is_none() && input.units_ms != 0)
    {
        return Err(Error::invalid("resource report does not match its grant"));
    }
    let observed: i64 = row.get("units");
    let closed = row.get::<String, _>("state") == "closed";
    if closed && input.terminal && observed != input.units_ms {
        return Err(Error::conflict("terminal resource usage changed"));
    }
    if !closed && (input.units_ms >= observed) {
        billing::meter_in(
            &mut tx,
            &billing::UsageReport {
                id: format!(
                    "{}:{}:{}",
                    input.authorization, input.units_ms, input.terminal
                ),
                reservation: input.authorization.clone(),
                units: input.units_ms,
                terminal: input.terminal,
            },
        )
        .await?;
        sqlx::query("UPDATE environment_grants SET sandbox_id=$1 WHERE id=$2")
            .bind(&input.resource_id)
            .bind(&input.authorization)
            .execute(&mut *tx)
            .await?;
    } else if input.terminal && !closed {
        return Err(Error::conflict("terminal usage precedes observed usage"));
    }
    tx.commit().await?;
    Ok(Json(json!({"recorded":true})))
}
pub async fn maintain(app: &App) -> Result<()> {
    let grants = sqlx::query("SELECT e.id,r.account FROM environment_grants e JOIN credit_reservations r ON r.id=e.id WHERE e.session IS NULL AND e.expires_at<=$1 AND r.state='open'")
        .bind(now_ms()).fetch_all(&app.store.0).await?;
    for grant in grants {
        let mut tx = app.store.0.begin().await?;
        Store::lock_account(&mut tx, grant.get("account")).await?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environment_grants e JOIN credit_reservations r ON r.id=e.id WHERE e.id=$1 AND e.session IS NULL AND r.state='open')")
            .bind(grant.get::<String,_>("id")).fetch_one(&mut *tx).await?;
        if pending {
            let id: String = grant.get("id");
            billing::meter_in(
                &mut tx,
                &billing::UsageReport {
                    id: format!("{id}:unbound_expiry"),
                    reservation: id,
                    units: 0,
                    terminal: true,
                },
            )
            .await?;
        }
        tx.commit().await?;
    }
    Ok(())
}
