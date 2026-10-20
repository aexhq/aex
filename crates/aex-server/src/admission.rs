use crate::{
    App,
    brain_contract::CreateSession,
    error::{Error, Result},
    identity::Principal,
};

pub fn request(
    semaphore: std::sync::Arc<tokio::sync::Semaphore>,
    reserve_download: bool,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    let mut permit = semaphore
        .try_acquire_many_owned(if reserve_download { 2 } else { 1 })
        .map_err(|_| Error::capacity())?;
    if reserve_download {
        drop(permit.split(1));
    }
    Ok(permit)
}
pub async fn create(app: &App, principal: &Principal, request: &CreateSession) -> Result<()> {
    if !app.config.harnesses.contains(&request.harness) {
        return Err(Error::invalid("harness is not offered by this deployment"));
    }
    if !app.config.models.contains(&request.config.model) {
        return Err(Error::invalid("model is not offered by this deployment"));
    }
    if request.model_credential.as_ref().is_none_or(|credential| {
        credential.api_key.as_deref().is_none_or(str::is_empty) || credential.reference.is_some()
    }) {
        return Err(Error::invalid(
            "hosted sessions require a private customer model API key",
        ));
    }
    for binding in request.config.bindings.as_deref().unwrap_or_default() {
        match (&binding.executor, &binding.provider) {
            (Some(executor), None) => app.store.own_executor(principal, executor).await?,
            (None, Some(provider)) if app.config.managed_providers.contains_key(provider) => {}
            _ => {
                return Err(Error::invalid(
                    "tool binding is not offered by this deployment",
                ));
            }
        }
    }
    if !request.config.tools.is_empty() {
        return Err(Error::invalid(
            "hosted tools require explicit managed or runtime declarations",
        ));
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
