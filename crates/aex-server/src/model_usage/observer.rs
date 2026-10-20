use super::*;
use std::{collections::HashMap, time::Duration};
use tokio_util::sync::CancellationToken;

pub async fn run(app: App, stop: CancellationToken) {
    let mut tasks = HashMap::<String, tokio::task::JoinHandle<()>>::new();
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_ =stop.cancelled()=>break,_ =interval.tick()=>{}}
        tasks.retain(|_, task| !task.is_finished());
        let ids=match sqlx::query_scalar::<_,String>("SELECT id FROM sessions WHERE runtime='brain' AND state!='deleted' AND NOT model_complete").fetch_all(&app.store.0).await {
            Ok(ids)=>ids,Err(error)=>{tracing::error!(%error,"usage discovery failed");continue;}
        };
        for id in ids {
            if let std::collections::hash_map::Entry::Vacant(entry) = tasks.entry(id.clone()) {
                let app = app.clone();
                entry.insert(tokio::spawn(async move {
                    if let Err(error)=reconcile(&app,&id).await {tracing::warn!(session=%id,code=%error.1.code,"model accounting awaits reconciliation");}
                }));
            }
        }
        if let Err(error) = enforce(&app).await {
            tracing::warn!(code=%error.1.code,"spending-limit enforcement failed");
        }
    }
    for (_, task) in tasks {
        task.abort();
    }
}
async fn enforce(app: &App) -> Result<()> {
    let accounts:Vec<String>=sqlx::query_scalar("SELECT w.account FROM wallets w JOIN pricebooks p ON p.id=w.pricebook WHERE p.document::jsonb->'rates' ? 'model_tokens'").fetch_all(&app.store.0).await?;
    for account in accounts {
        let mut tx = app.store.0.begin().await?;
        Store::lock_account(&mut tx, &account).await?;
        let denied = match admit_in(&mut tx, &account).await {
            Ok(_) => false,
            Err(error) if error.0 == axum::http::StatusCode::PAYMENT_REQUIRED => true,
            Err(error) => return Err(error),
        };
        tx.commit().await?;
        if denied {
            for session in app.store.session_ids(&account).await? {
                let current = app.brain.summary(&account, &session).await?;
                if current.status != "running" {
                    continue;
                }
                let response = app
                    .brain
                    .request(
                        Method::POST,
                        &format!("/sessions/{session}/events"),
                        &HeaderMap::new(),
                        Bytes::from_static(br#"{"type":"user.interrupt"}"#),
                        &account,
                        None,
                    )
                    .await?;
                if !response.status().is_success() {
                    return Err(Error::ambiguous());
                }
                app.brain.bytes(response).await?;
            }
        }
    }
    Ok(())
}
