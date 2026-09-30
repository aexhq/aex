use crate::{
    App,
    error::{Error, Result},
    identity::Principal,
};
use brain_protocol::{CreateSessionRequest, Driver};

pub fn request(
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    reserve_download: bool,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    // A model turn can wait on a provider fetching its attachment through this same service.
    let mut permit = semaphore
        .try_acquire_many_owned(if reserve_download { 2 } else { 1 })
        .map_err(|_| Error::capacity())?;
    if reserve_download {
        drop(permit.split(1));
    }
    Ok(permit)
}

pub async fn create(app: &App, p: &Principal, request: &CreateSessionRequest) -> Result<()> {
    for (kind, id) in crate::artifacts::references(&request.agentloop.implementation, "agentloops")?
    {
        crate::artifacts::owned(app, p, kind, id).await?;
    }
    let mut names = std::collections::HashSet::new();
    for environment in &request.environments {
        if environment.lifecycle.is_none() {
            return Err(Error::invalid(
                "each Environment requires an explicit lifecycle",
            ));
        }
        if !matches!(environment.driver, Driver::Host { .. })
            && (environment.template.is_some()
                || environment.methods.values().any(|method| {
                    method.effect == brain_protocol::EnvironmentMethodEffect::Replace
                }))
        {
            return Err(Error::invalid(
                "hosted Environment grants cover one declared resource; templates and replacement methods require caller-operated Environments",
            ));
        }
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
                if crate::environments::application::selected(app, environment) {
                    crate::environments::application::selection(environment)?;
                } else if crate::environments::http::selected(app, environment) {
                    crate::environments::http::selection(app, p, environment)?;
                } else {
                    crate::environments::selection(app, p, environment)?;
                }
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
                for (kind, id) in crate::artifacts::references(&placement.implementation, "tools")?
                {
                    crate::artifacts::owned(app, p, kind, id).await?;
                }
            } else if matches!(selected.driver, Driver::Http { .. }) {
                if crate::environments::application::selected(app, selected) {
                    crate::environments::application::validate_tool(tool, placement)?;
                    if !tool.environments.is_empty() {
                        return Err(Error::invalid(
                            "application Tools currently support completion services only",
                        ));
                    }
                } else if crate::environments::http::selected(app, selected) {
                    let (_, binding) = crate::environments::http::selection(app, p, selected)?;
                    crate::environments::http::validate_tool(binding, tool, placement)?;
                } else {
                    crate::environments::selection(app, p, selected)?;
                }
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
