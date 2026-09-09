use crate::{
    App,
    error::{Error, Result},
    identity::Principal,
};
use brain_protocol::{CreateSessionRequest, Driver};

pub async fn create(app: &App, p: &Principal, request: &CreateSessionRequest) -> Result<()> {
    let selected = format!("{}/{}", request.model.provider, request.model.name);
    if !app.config.models.contains(&selected) {
        return Err(Error::invalid("model is not hosted"));
    }
    if request.model.api_key.is_empty() || request.model.api_key.len() > 16384 {
        return Err(Error::invalid("model key required"));
    }
    let descriptor = request
        .agentloop
        .implementation
        .as_object()
        .ok_or_else(|| Error::invalid("invalid Agentloop implementation"))?;
    if descriptor.len() != 3
        || descriptor.get("type").and_then(|v| v.as_str()) != Some("brain_component")
        || descriptor.get("entrypoint").and_then(|v| v.as_str()) != Some("turn")
        || descriptor.get("id").and_then(|v| v.as_str()).is_none()
    {
        return Err(Error::invalid("Agentloop is not hosted"));
    }
    crate::artifacts::owned(app, p, "agentloops", descriptor["id"].as_str().unwrap()).await?;
    let mut names = std::collections::HashSet::new();
    for environment in &request.environments {
        if !names.insert(environment.name.as_str()) {
            return Err(Error::invalid("duplicate Environment"));
        }
        match &environment.driver {
            Driver::Brain {} => {
                if !environment.configuration.is_null()
                    && environment.configuration != serde_json::json!({})
                {
                    return Err(Error::invalid(
                        "hosted Environment configuration must be empty",
                    ));
                }
            }
            Driver::Host { host_id } => app.store.own_host(p, host_id.as_str()).await?,
            Driver::Http { .. } => {
                return Err(Error::invalid("remote Environments are not hosted"));
            }
        }
    }
    if !request
        .environments
        .iter()
        .any(|e| e.name == request.agentloop.environment && matches!(e.driver, Driver::Brain {}))
    {
        return Err(Error::invalid(
            "Agentloop requires the hosted Brain Environment",
        ));
    }
    for tool in &request.tools {
        for (environment, placement) in &tool.placements {
            let selected = request
                .environments
                .iter()
                .find(|e| e.name == *environment)
                .ok_or_else(|| Error::invalid("unknown Tool Environment"))?;
            if matches!(selected.driver, Driver::Brain {}) {
                let descriptor = placement
                    .implementation
                    .as_object()
                    .ok_or_else(|| Error::invalid("invalid Tool implementation"))?;
                if !descriptor.keys().all(|key| {
                    matches!(key.as_str(), "type" | "entrypoint" | "id" | "configuration")
                }) || descriptor.get("type").and_then(|v| v.as_str()) != Some("brain_component")
                    || descriptor.get("entrypoint").and_then(|v| v.as_str()) != Some("run")
                {
                    return Err(Error::invalid("hosted Tools require a Brain Component"));
                }
                crate::artifacts::owned(
                    app,
                    p,
                    "tools",
                    descriptor
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| Error::invalid("Tool content address required"))?,
                )
                .await?;
            }
        }
    }
    if request.idle_ttl_ms == Some(0) {
        return Err(Error::invalid("idle suspension cannot be disabled"));
    }
    disk(app).await
}

pub async fn disk(app: &App) -> Result<()> {
    let received = app.store.usage_received().await?;
    if !received.is_some_and(|at| {
        crate::store::now().saturating_sub(at) <= app.config.limits.usage_max_age_secs as i64
    }) {
        return Err(Error::capacity());
    }
    let directory = app.config.data_dir.clone();
    let free = tokio::task::spawn_blocking(move || fs2::available_space(directory))
        .await
        .map_err(|_| Error::internal())?
        .map_err(|_| Error::internal())?;
    if free < app.config.limits.minimum_free_disk_bytes {
        Err(Error::capacity())
    } else {
        Ok(())
    }
}
