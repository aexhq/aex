use crate::{
    App,
    error::{Error, Result},
};
use brain_protocol::{Driver, Environment, Tool, ToolPlacement};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Selection {
    Application {
        endpoint: String,
        #[serde(rename = "timeoutMs")]
        timeout_ms: u32,
    },
}

pub fn public_url(config: &super::http::Config, path: &str) -> String {
    let mut url = url::Url::parse(&config.public_url).expect("validated HTTP configuration");
    url.set_path(path);
    url.to_string()
}

pub fn selected(app: &App, env: &Environment) -> bool {
    matches!(&env.driver, Driver::Http { url, .. } if app.config.http_environments.as_ref()
        .is_some_and(|c| *url == public_url(c, "/environments/application")))
}

pub fn selection(env: &Environment) -> Result<super::http::Binding> {
    let Selection::Application {
        endpoint,
        timeout_ms,
    } = serde_json::from_value(env.configuration.clone())?;
    let url =
        url::Url::parse(&endpoint).map_err(|_| Error::invalid("invalid application endpoint"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(1..=300_000).contains(&timeout_ms)
    {
        return Err(Error::invalid(
            "application endpoint requires HTTPS on port 443 and a bounded timeout",
        ));
    }
    if !matches!(&env.driver, Driver::Http { credential: Some(token), .. } if (32..=4096).contains(&token.len()) && token.is_ascii() && !token.bytes().any(|c| c.is_ascii_control()))
    {
        return Err(Error::invalid(
            "application credential must contain 32 to 4096 printable ASCII bytes",
        ));
    }
    Ok(super::http::Binding {
        url: endpoint,
        timeout_ms,
        tools: Vec::new(),
    })
}

pub fn validate_tool(tool: &Tool, placement: &ToolPlacement) -> Result<()> {
    let descriptor = placement
        .implementation
        .as_object()
        .ok_or_else(|| Error::invalid("invalid application Tool"))?;
    if descriptor.get("type") != Some(&json!("application_tool"))
        || descriptor.get("definition") != Some(&super::http::definition(&tool.definition()))
        || !descriptor.contains_key("options")
        || descriptor
            .get("configuration")
            .is_some_and(|v| *v != json!({}))
        || descriptor
            .keys()
            .any(|k| !["type", "definition", "options", "configuration"].contains(&k.as_str()))
    {
        return Err(Error::invalid(
            "application Tool must use its declared contract and options",
        ));
    }
    Ok(())
}

pub async fn callback(
    app: &App,
    headers: &axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<axum::response::Response> {
    use axum::response::IntoResponse;
    let config = app
        .config
        .http_environments
        .as_ref()
        .ok_or_else(Error::missing)?;
    let token = crate::identity::bearer(headers)?;
    let url = format!("{}/v1/callback", config.url.trim_end_matches('/'));
    let response = app
        .application_client
        .post(url)
        .bearer_auth(token)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| Error::ambiguous())?;
    let status = response.status();
    let bytes = app.brain.bytes(response).await?;
    Ok((
        status,
        [
            ("content-type", "application/json"),
            ("cache-control", "no-store"),
        ],
        bytes,
    )
        .into_response())
}
