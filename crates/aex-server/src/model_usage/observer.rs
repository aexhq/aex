use super::*;
use futures_util::{StreamExt, stream};
use std::{collections::HashMap, time::Duration};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;

pub async fn run(app: App, stop: CancellationToken) {
    let mut watchers: HashMap<String, JoinHandle<()>> = HashMap::new();
    let mut stopping: HashMap<String, JoinHandle<()>> = HashMap::new();
    let mut changes = app.changed.subscribe();
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! { _ = stop.cancelled() => break, _ = interval.tick() => {}, _ = changes.changed() => {} }
        watchers.retain(|_, task| !task.is_finished());
        let rows = match sqlx::query(
            "SELECT id,account FROM sessions WHERE state!='deleted' AND NOT model_complete",
        )
        .fetch_all(&app.store.0)
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(%error, "model observation discovery failed");
                continue;
            }
        };
        for row in rows {
            let session: String = row.get("id");
            if let std::collections::hash_map::Entry::Vacant(entry) = watchers.entry(session) {
                let app = app.clone();
                let id = entry.key().clone();
                entry.insert(tokio::spawn(async move {
                    if let Err(error) = watch(&app, &id).await {
                        tracing::warn!(session=%id, code=%error.1.code, "model usage stream will reconnect");
                    }
                }));
            }
        }
        match exhausted(&app).await {
            Ok(accounts) => {
                stopping.retain(|account, task| {
                    if accounts.contains(account) {
                        true
                    } else {
                        task.abort();
                        false
                    }
                });
                for account in accounts {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        stopping.entry(account)
                    {
                        let app = app.clone();
                        let id = entry.key().clone();
                        entry.insert(tokio::spawn(async move {
                        while let Err(error) = interrupt(&app, &id).await {
                            tracing::warn!(account=%id, code=%error.1.code, "spending-limit interruption will retry");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }));
                    }
                }
            }
            Err(error) => tracing::warn!(code=%error.1.code, "token spending check failed"),
        }
    }
    for (_, task) in watchers.into_iter().chain(stopping) {
        task.abort();
    }
}

async fn exhausted(app: &App) -> Result<Vec<String>> {
    let accounts: Vec<String> = sqlx::query_scalar("SELECT w.account FROM wallets w JOIN pricebooks p ON p.id=w.pricebook WHERE p.document::jsonb->'rates' ? 'model_tokens'")
        .fetch_all(&app.store.0).await?;
    let mut exhausted = Vec::new();
    for account in accounts {
        let mut tx = app.store.0.begin().await?;
        Store::lock_account(&mut tx, &account).await?;
        if let Err(error) = admit_in(&mut tx, &account).await {
            if error.0 != axum::http::StatusCode::PAYMENT_REQUIRED {
                return Err(error);
            }
            exhausted.push(account);
        }
        tx.commit().await?;
    }
    Ok(exhausted)
}

async fn interrupt(app: &App, account: &str) -> Result<()> {
    let ids = app.store.session_ids(account).await?;
    let results = stream::iter(ids)
        .map(|session| async move {
            let response = app
                .brain
                .request(
                    Method::POST,
                    &format!("/v1/sessions/{session}/cancel"),
                    &HeaderMap::new(),
                    Bytes::from_static(b"{}"),
                    Some(&crate::identity::random("spending-stop")),
                    None,
                )
                .await?;
            if !response.status().is_success() {
                return Err(Error::ambiguous());
            }
            app.brain.bytes(response).await?;
            Ok(())
        })
        .buffer_unordered(8)
        .collect::<Vec<Result<()>>>()
        .await;
    for result in results {
        result?;
    }
    Ok(())
}

struct Call {
    estimate: Estimate,
    rate: Option<Rate>,
}

async fn calls(app: &App, session: &str) -> Result<HashMap<u64, Call>> {
    let rows = sqlx::query("SELECT u.*,p.document FROM model_usage u LEFT JOIN pricebooks p ON p.id=u.pricebook WHERE session=$1 AND NOT terminal")
        .bind(session).fetch_all(&app.store.0).await?;
    let mut calls = HashMap::new();
    for row in rows {
        let book = row
            .get::<Option<String>, _>("document")
            .map(|document| serde_json::from_str::<Pricebook>(&document))
            .transpose()?;
        calls.insert(
            row.get::<i64, _>("sequence") as u64,
            Call {
                estimate: Estimate {
                    input_bytes: row.get::<i64, _>("input_bytes") as u64,
                    media_inputs: row.get::<i64, _>("media_inputs") as u64,
                    output_bytes: row.get::<i64, _>("output_bytes") as u64,
                    started_ms: row.get("started_ms"),
                    usage: serde_json::from_str(row.get("usage"))?,
                    previous_input: None,
                },
                rate: book.and_then(|book| book.rates.get(&Meter::ModelTokens).cloned()),
            },
        );
    }
    Ok(calls)
}

async fn flush(app: &App, session: &str, calls: &HashMap<u64, Call>) -> Result<()> {
    let mut pending = 0i64;
    let mut tx = app.store.0.begin().await?;
    let account: String = sqlx::query_scalar("SELECT account FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&mut *tx)
        .await?;
    Store::lock_account(&mut tx, &account).await?;
    for (sequence, call) in calls {
        if let Some(rate) = &call.rate {
            pending = pending
                .checked_add(call.estimate.cost(now_ms(), rate)?)
                .ok_or_else(Error::capacity)?;
        }
        sqlx::query("UPDATE model_usage SET usage=$1,input_bytes=$2,media_inputs=$3,output_bytes=$4 WHERE session=$5 AND sequence=$6 AND NOT terminal")
            .bind(serde_json::to_string(&call.estimate.usage)?).bind(quantity(Some(call.estimate.input_bytes))?).bind(quantity(Some(call.estimate.media_inputs))?).bind(quantity(Some(call.estimate.output_bytes))?)
            .bind(session).bind(quantity(Some(*sequence))?).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE sessions SET model_pending=$1,model_observed_at=$2 WHERE id=$3 AND NOT model_complete")
        .bind(pending).bind(now_ms()).bind(session).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn watch(app: &App, session: &str) -> Result<()> {
    let after: i64 = sqlx::query_scalar("SELECT model_cursor FROM sessions WHERE id=$1")
        .bind(session)
        .fetch_one(&app.store.0)
        .await?;
    let mut calls = calls(app, session).await?;
    let mut headers = HeaderMap::new();
    headers.insert("accept", "text/event-stream".parse().unwrap());
    let response = app
        .brain
        .request(
            Method::GET,
            &format!("/v1/sessions/{session}/events?after={after}"),
            &headers,
            Bytes::new(),
            None,
            None,
        )
        .await?;
    if !response.status().is_success() {
        return Err(Error::ambiguous());
    }
    let mut stream = response.bytes_stream();
    let mut decoder = Decoder::default();
    let mut timer = tokio::time::interval(Duration::from_millis(250));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut previous_input = None;
    let mut last_flush = 0;
    loop {
        tokio::select! {
            _ = timer.tick() => {
                if !calls.is_empty() || now_ms()-last_flush >= 1000 {
                    flush(app, session, &calls).await?;
                    last_flush = now_ms();
                }
            },
            chunk = stream.next() => {
                let Some(chunk) = chunk else { return Ok(()); };
                let chunk = chunk.map_err(|_| Error::ambiguous())?;
                for frame in decoder.feed(&chunk, app.config.limits.response_bytes)? {
                    if frame.id {
                        let event: Event = serde_json::from_str(&frame.data)?;
                        if matches!(event.event_type.as_str(), "model_call_ended" | "model_call_failed") {
                            flush(app, session, &calls).await?;
                            if event.origin.is_none() && let Some(sequence) = event.data["sequence"].as_u64()
                                && let Some(call) = calls.get(&sequence)
                                && let Some(input) = event.data["result"]["usage"]["total_input_tokens"].as_u64() {
                                    previous_input = Some((input,call.estimate.input_bytes));
                            }
                        }
                        record(app, session, &event).await?;
                        if event.event_type == "model_call_started" {
                            let mut refreshed = self::calls(app, session).await?;
                            if let Some(mut call) = refreshed.remove(&event.sequence) {
                                if event.origin.is_none() { call.estimate.previous_input = previous_input; }
                                calls.insert(event.sequence, call);
                            }
                        }
                        if matches!(event.event_type.as_str(), "model_call_ended" | "model_call_failed")
                            && let Some(sequence) = event.data["sequence"].as_u64() {
                            calls.remove(&sequence);
                        }
                        if event.origin.is_none() && matches!(event.event_type.as_str(), "session_ended" | "session_creation_failed") { return Ok(()); }
                    } else {
                        let data: serde_json::Value = serde_json::from_str(&frame.data)?;
                        let Some(call) = data["model_call_sequence"].as_u64().and_then(|sequence| calls.get_mut(&sequence)) else { continue; };
                        match frame.event.as_str() {
                            "model_request" => {
                                call.estimate.input_bytes = data["input_bytes"].as_u64().ok_or_else(Error::internal)?;
                                call.estimate.media_inputs = data["media_inputs"].as_u64().ok_or_else(Error::internal)?;
                            },
                            "model_usage" => call.estimate.usage.observe(&serde_json::from_value(data["usage"].clone())?).map_err(|_| Error::ambiguous())?,
                            "assistant_delta" | "refusal_delta" | "tool_call_delta" => {
                                let bytes = data["text"].as_str().or_else(|| data["partial_json"].as_str()).map_or(0, str::len);
                                call.estimate.output_bytes = call.estimate.output_bytes.saturating_add(bytes as u64);
                            },
                            "model_output" => call.estimate.output_bytes = call.estimate.output_bytes.saturating_add(data["output_bytes"].as_u64().ok_or_else(Error::internal)?),
                            _ => {},
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct Frame {
    event: String,
    id: bool,
    data: String,
}
#[derive(Default)]
struct Decoder {
    bytes: Vec<u8>,
    frame: Frame,
    size: usize,
}
impl Decoder {
    fn feed(&mut self, chunk: &[u8], limit: usize) -> Result<Vec<Frame>> {
        let mut frames = Vec::new();
        for byte in chunk {
            self.size += 1;
            if self.size > limit {
                return Err(Error::capacity());
            }
            if *byte != b'\n' {
                self.bytes.push(*byte);
                continue;
            }
            if self.bytes.last() == Some(&b'\r') {
                self.bytes.pop();
            }
            let line = std::str::from_utf8(&self.bytes).map_err(|_| Error::ambiguous())?;
            if line.is_empty() {
                if !self.frame.data.is_empty() {
                    self.frame.data.pop();
                    frames.push(std::mem::take(&mut self.frame));
                } else {
                    self.frame = Frame::default();
                }
                self.size = 0;
            } else if let Some((name, value)) = line.split_once(':') {
                let value = value.strip_prefix(' ').unwrap_or(value);
                match name {
                    "event" => self.frame.event = value.into(),
                    "id" => self.frame.id = !value.is_empty(),
                    "data" => {
                        self.frame.data.push_str(value);
                        self.frame.data.push('\n');
                    }
                    _ => {}
                }
            }
            self.bytes.clear();
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoding_handles_fragmentation_crlf_and_live_identity_without_advancing_a_cursor() {
        let raw = ": keepalive\r\n\r\nevent: model_usage\r\ndata: {\"text\":\"中文\",\r\ndata: \"model_call_sequence\":3}\r\n\r\nid: 4\nevent: model_call_ended\ndata: {}\n\n";
        for width in 1..raw.len() {
            let mut decoder = Decoder::default();
            let frames: Vec<_> = raw
                .as_bytes()
                .chunks(width)
                .flat_map(|chunk| decoder.feed(chunk, 1024).unwrap())
                .collect();
            assert_eq!(frames.len(), 2);
            assert!(!frames[0].id);
            assert!(frames[1].id);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&frames[0].data).unwrap()["model_call_sequence"],
                3
            );
        }
        assert!(Decoder::default().feed(b"data: unbounded", 5).is_err());
    }
}
