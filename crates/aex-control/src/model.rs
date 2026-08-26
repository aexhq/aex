//! Hosted model resolution.
//!
//! Brain speaks two shapes and knows nothing about providers. Which provider names Aex serves on a
//! customer's behalf is Aex's policy, so this is where a name becomes the dialect Brain must
//! speak, the endpoint that speaks it, and the context window the session seals.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{Error, Result};

#[derive(Deserialize)]
struct Catalog {
    source: String,
    generated: String,
    providers: HashMap<String, Provider>,
}

#[derive(Deserialize)]
pub struct Provider {
    /// Which request and response shape Brain must speak to this provider.
    pub dialect: String,
    /// The provider's full API root. Absent providers require an explicit `model.base_url`.
    pub base_url: Option<String>,
    /// Sealed context window per model id.
    pub models: HashMap<String, u64>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../model-catalog.json"))
        .expect("the generated model catalog parses")
});

pub fn provider(id: &str) -> Option<&'static Provider> {
    CATALOG.providers.get(id)
}

/// Which models.dev projection this process carries: an operator reading a session refusal needs
/// to know how old the provider table it was refused by is.
pub fn catalog_provenance() -> (&'static str, &'static str, usize) {
    (&CATALOG.source, &CATALOG.generated, CATALOG.providers.len())
}

/// Resolve the provider a hosted create named into the model configuration Brain takes, and
/// return that provider so the session's read view can report the name the customer chose.
pub fn seal_model(root: &mut Map<String, Value>) -> Result<String> {
    let model = root
        .get_mut("model")
        .ok_or_else(|| Error::Invalid("model is required".into()))?
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("model must be an object".into()))?;
    if model.contains_key("dialect") {
        return Err(Error::Invalid(
            "model.dialect is resolved by Aex; name a provider instead".into(),
        ));
    }
    let provider_id = match model.remove("provider") {
        Some(Value::String(value)) => value,
        Some(_) => return Err(Error::Invalid("model.provider must be a string".into())),
        None => {
            return Err(Error::Invalid(
                "model.provider must name an LLM API provider".into(),
            ));
        }
    };
    let catalogued = provider(&provider_id).ok_or_else(|| {
        Error::Unprocessable(format!("Aex does not serve provider {provider_id}"))
    })?;
    let name = model
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid("model.name must name a model".into()))?
        .to_owned();
    let base_url = match model.get("base_url") {
        None => catalogued.base_url.clone().ok_or_else(|| {
            Error::Unprocessable(format!(
                "provider {provider_id} publishes no endpoint; send model.base_url"
            ))
        })?,
        Some(Value::String(url)) => url.clone(),
        Some(_) => return Err(Error::Invalid("model.base_url must be a string".into())),
    };
    // Plain HTTP is a local development affordance in the neutral engine, never a hosted one.
    if !base_url.starts_with("https://") {
        return Err(Error::Unprocessable(
            "model.base_url must be an https endpoint".into(),
        ));
    }
    if !model.contains_key("context_window_tokens") {
        let window = catalogued.models.get(&name).ok_or_else(|| {
            Error::Unprocessable(format!(
                "{provider_id}/{name} is absent from the model catalog; send context_window_tokens"
            ))
        })?;
        model.insert("context_window_tokens".into(), json!(window));
    }
    model.insert("dialect".into(), json!(catalogued.dialect));
    model.insert("base_url".into(), json!(base_url));
    Ok(provider_id)
}

/// Put the provider the customer named back onto a session Brain returned. Brain reports the
/// dialect and endpoint it was given; the name that chose them is Aex's to remember.
pub fn restore_provider(session: &mut Value, provider_id: &str) {
    if let Some(model) = session.get_mut("model").and_then(Value::as_object_mut) {
        model.insert("provider".into(), json!(provider_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scheduled refresh regenerates the catalog weekly. This bound turns a refresh that
    /// stopped running into a red build instead of a quietly ageing provider table.
    const MAX_CATALOG_AGE_DAYS: i64 = 45;

    #[test]
    fn the_catalog_is_the_generated_models_dev_projection() {
        assert_eq!(CATALOG.source, "https://models.dev/api.json");
        let generated = chrono::NaiveDate::parse_from_str(&CATALOG.generated, "%Y-%m-%d")
            .expect("the catalog records the date it was generated");
        let age = (chrono::Utc::now().date_naive() - generated).num_days();
        assert!(
            (0..=MAX_CATALOG_AGE_DAYS).contains(&age),
            "catalog generated {} ({age} days ago); run tools/generate-model-catalog.py",
            CATALOG.generated
        );
        for (provider, dialect) in [("openai", "openai"), ("anthropic", "anthropic")] {
            assert_eq!(super::provider(provider).expect(provider).dialect, dialect);
        }
    }

    fn seal(model: Value) -> Result<Value> {
        let mut root = json!({ "model": model }).as_object().unwrap().clone();
        let provider = seal_model(&mut root)?;
        let mut sealed = root.remove("model").unwrap();
        sealed
            .as_object_mut()
            .unwrap()
            .insert("resolved_provider".into(), json!(provider));
        Ok(sealed)
    }

    #[test]
    fn a_named_provider_resolves_to_a_dialect_endpoint_and_window() {
        let sealed = seal(json!({"provider":"openai","name":"gpt-4.1-nano","api_key":"sk-test"}))
            .expect("openai is served");
        assert_eq!(sealed["dialect"], "openai");
        assert_eq!(sealed["base_url"], "https://api.openai.com/v1");
        assert_eq!(sealed["api_key"], "sk-test");
        assert_eq!(sealed["resolved_provider"], "openai");
        assert!(sealed["context_window_tokens"].as_u64().unwrap() > 8192);
        assert!(sealed.get("provider").is_none(), "Brain takes no provider");

        let anthropic =
            seal(json!({"provider":"anthropic","name":"claude-sonnet-5","api_key":"sk-test"}))
                .expect("anthropic is served");
        assert_eq!(anthropic["dialect"], "anthropic");
        assert_eq!(anthropic["base_url"], "https://api.anthropic.com/v1");
    }

    #[test]
    fn a_gateway_takes_the_caller_endpoint_and_keeps_the_catalogued_dialect() {
        let sealed = seal(json!({
            "provider": "vercel",
            "name": "openai/gpt-4.1-nano",
            "api_key": "sk-test",
            "base_url": "https://ai-gateway.vercel.sh/v1"
        }))
        .expect("a gateway with an explicit endpoint is served");
        assert_eq!(sealed["dialect"], "openai");
        assert_eq!(sealed["base_url"], "https://ai-gateway.vercel.sh/v1");

        let endpointless = seal(json!({
            "provider": "vercel",
            "name": "openai/gpt-4.1-nano",
            "api_key": "sk-test"
        }))
        .unwrap_err();
        assert_eq!(endpointless.status(), 422);
    }

    #[test]
    fn a_provider_or_model_aex_does_not_serve_is_refused_before_brain() {
        let unserved = seal(json!({
            "provider": "google",
            "name": "gemini-2.5-flash",
            "api_key": "sk-test"
        }))
        .unwrap_err();
        assert_eq!(unserved.status(), 422);

        let unknown_model = seal(json!({
            "provider": "openai",
            "name": "private-finetune",
            "api_key": "sk-test"
        }))
        .unwrap_err();
        assert_eq!(unknown_model.status(), 422);

        let sealed = seal(json!({
            "provider": "openai",
            "name": "private-finetune",
            "api_key": "sk-test",
            "context_window_tokens": 32768
        }))
        .expect("an explicit window seals a model the catalog does not carry");
        assert_eq!(sealed["context_window_tokens"], 32768);
    }

    #[test]
    fn the_hosted_plane_refuses_plaintext_and_a_caller_chosen_dialect() {
        let insecure = seal(json!({
            "provider": "openai",
            "name": "gpt-4.1-nano",
            "api_key": "sk-test",
            "base_url": "http://127.0.0.1:11434/v1"
        }))
        .unwrap_err();
        assert_eq!(insecure.status(), 422);

        let dialect = seal(json!({
            "dialect": "openai",
            "base_url": "https://api.openai.com/v1",
            "name": "gpt-4.1-nano",
            "api_key": "sk-test"
        }))
        .unwrap_err();
        assert_eq!(dialect.status(), 400);
    }

    #[test]
    fn a_returned_session_reports_the_provider_the_customer_named() {
        let mut session = json!({"id":"ses_1","model":{"dialect":"openai","name":"gpt-4.1-nano"}});
        restore_provider(&mut session, "openrouter");
        assert_eq!(session["model"]["provider"], "openrouter");
    }
}
