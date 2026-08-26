//! Incremental metering reconciliation against Brain's authoritative tenant index.
//!
//! A per-account durable watermark bounds normal discovery to the changed window. Every sweep
//! queries each non-deleted state with an overlap, settles changed journals plus bounded due local
//! meters, then advances the watermark only after the whole account succeeds. Partial work is
//! idempotent and a failed sweep leaves the old watermark for the next process to retry.

use std::collections::{HashMap, HashSet, VecDeque};

use futures_util::StreamExt;

use crate::brain::{BrainClient, BrainSessionSnapshot, parse_session_snapshot};
use crate::rating::{self, Priced, RateCard};
use crate::store::{Db, SessionRow};
use crate::{Result, now_ms};

const BACKGROUND_STORAGE_SETTLEMENT_MS: i64 = 5 * 60 * 1_000;
const BACKGROUND_STORAGE_BATCH: usize = 1_000;
const BACKGROUND_ACCOUNT_CONCURRENCY: usize = 16;
const DELETION_JOB_CONCURRENCY: usize = 4;
const DELETION_JOB_BATCH: usize = 32;

/// A swept, priced line for one session.
#[derive(Debug, Clone)]
pub struct SweptLine {
    pub row: SessionRow,
    pub priced: Priced,
}

fn priced_line(card: &RateCard, row: SessionRow, now: i64) -> Result<SweptLine> {
    let priced = rating::price(card, &row.shape, &row.fold, now)?;
    Ok(SweptLine { row, priced })
}

async fn settle_snapshot(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    mut row: SessionRow,
    snapshot: Option<&BrainSessionSnapshot>,
    now: i64,
) -> Result<SweptLine> {
    if row.is_final {
        return priced_line(card, row, now);
    }

    let mut fold = row.fold.clone();
    if let Some(snapshot) = snapshot {
        // A stale concurrent HEAD may lag a newer fold already committed locally. Never regress
        // it. Conversely, replay may race ahead of the captured HEAD; defer later events so one
        // fold represents one coherent high-water.
        if snapshot.last_seq >= fold.folded_seq {
            if snapshot.last_seq > fold.folded_seq {
                brain
                    .fold_replay_events(
                        &row.account_id,
                        &row.id,
                        fold.folded_seq,
                        snapshot.last_seq,
                        &mut fold,
                    )
                    .await?;
            }
            // Brain's HEAD high-water includes durable records that may not be public SSE events.
            // Advancing to it prevents replaying the same intentional sequence gaps forever.
            fold.folded_seq = fold.folded_seq.max(snapshot.last_seq);
            // Journal transitions are the only historical source of truth. HEAD verifies the
            // post-transition gauge but never turns Aex wall time into a closed storage fact: a
            // transition may linearize just after this read with an `at` before `now`.
            rating::apply_snapshot(&mut fold, &snapshot.document)?;
        }
    }

    // This is only the wall-time through which the absolute ledger estimate was produced and an
    // optimistic-write fence. Durable storage remains closed only through storage_transition_ms.
    fold.metered_to_ms = fold.metered_to_ms.max(now);

    let priced = rating::price(card, &row.shape, &fold, now)?;
    db.apply_sweep(
        row.id.clone(),
        row.account_id.clone(),
        fold.clone(),
        false,
        priced.total_microusd,
        now,
    )
    .await?;
    row.fold = fold;
    Ok(SweptLine { row, priced })
}

async fn settle_deleted(
    db: &Db,
    card: &RateCard,
    mut row: SessionRow,
    now: i64,
) -> Result<SweptLine> {
    if row.is_final {
        return priced_line(card, row, now);
    }
    let mut fold = row.fold.clone();
    // A confirmed physical purge is the final authoritative zero boundary. Unlike an ordinary
    // HEAD read, no later pre-boundary storage transition can still be committed.
    if fold.storage_transition_ms > now {
        return Err(crate::Error::Upstream(
            "deletion completion timestamp regressed behind durable storage usage".into(),
        ));
    }
    rating::accrue_storage(&mut fold, now)?;
    fold.metered_to_ms = fold.metered_to_ms.max(now);
    fold.session_state = "deleted".into();
    fold.session_storage_bytes = 0;
    fold.upload_reserved_bytes = 0;
    let priced = rating::price(card, &row.shape, &fold, now)?;
    db.apply_sweep(
        row.id.clone(),
        row.account_id.clone(),
        fold.clone(),
        true,
        priced.total_microusd,
        now,
    )
    .await?;
    row.fold = fold;
    row.is_final = true;
    Ok(SweptLine { row, priced })
}

/// Finalize the already-settled local projection of a Brain-confirmed cascading deletion.
pub async fn finalize_deleted_subtree(
    db: &Db,
    card: &RateCard,
    account_id: &str,
    session_id: &str,
    completed_at_ms: i64,
) -> Result<()> {
    for row in db
        .subtree_sessions(account_id.to_owned(), session_id.to_owned())
        .await?
    {
        settle_deleted(db, card, row, completed_at_ms).await?;
    }
    db.finish_subtree_deletion(account_id.to_owned(), session_id.to_owned())
        .await
}

/// Targeted reconciliation for a known session. Ordinary account sweeps use the tenant index;
/// this GET path is reserved for deletion confirmation and recovery of a specifically addressed
/// session.
pub async fn sweep_session(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    row: SessionRow,
) -> Result<SweptLine> {
    let document = brain.get_session(&row.account_id, &row.id).await?;
    // The direct HEAD is an actor fence / Dynamo consistent read. Replay through exactly its
    // returned last_seq. The wall time below is only an ephemeral price boundary, not a durable
    // storage boundary.
    let now = now_ms();
    match document {
        Some(document) => {
            let snapshot = parse_session_snapshot(document)?;
            settle_snapshot(db, brain, card, row, Some(&snapshot), now).await
        }
        None => match brain.deletion_status(&row.account_id, &row.id).await? {
            Some(status) if status.state == "succeeded" => {
                let completed_at_ms = status.completed_at_ms.ok_or_else(|| {
                    crate::Error::Upstream(
                        "succeeded deletion has no authoritative completion boundary".into(),
                    )
                })?;
                settle_deleted(db, card, row, completed_at_ms).await
            }
            _ => Err(crate::Error::Upstream(
                "session HEAD disappeared without a completed deletion barrier".into(),
            )),
        },
    }
}

/// Strong destructive-settlement barrier. The caller has already requested Brain's recursive end
/// fence; this walks the base direct-child adjacency, installs unknown child identities, and folds
/// every journal through its authoritative HEAD high-water before physical purge is permitted.
pub async fn settle_fenced_subtree(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    account_id: &str,
    root_id: &str,
) -> Result<()> {
    let root_document = brain
        .get_session(account_id, root_id)
        .await?
        .ok_or_else(|| crate::Error::Upstream("fenced deletion root disappeared".into()))?;
    if !matches!(root_document["state"].as_str(), Some("ended" | "deleting")) {
        return Err(crate::Error::Upstream(
            "recursive session end fence is still running".into(),
        ));
    }
    let root = parse_session_snapshot(root_document)?;
    let expected_root = root.root_id.clone();
    let mut queue = VecDeque::from([root.id.clone()]);
    let mut seen = HashSet::from([root.id.clone()]);
    let mut snapshots = vec![root];

    while let Some(parent_id) = queue.pop_front() {
        let mut cursor = None;
        let mut cursors = HashSet::new();
        loop {
            let (children, next) = brain
                .list_direct_children(account_id, &parent_id, cursor.as_deref())
                .await?;
            for child in children {
                if child.parent_id.as_deref() != Some(parent_id.as_str())
                    || child.root_id != expected_root
                {
                    return Err(crate::Error::Upstream(format!(
                        "strong child adjacency returned inconsistent session {}",
                        child.id
                    )));
                }
                if !seen.insert(child.id.clone()) {
                    return Err(crate::Error::Upstream(format!(
                        "strong child adjacency repeated session {}",
                        child.id
                    )));
                }
                if seen.len() > brain.discovery_session_limit() {
                    return Err(crate::Error::Upstream(format!(
                        "deletion subtree exceeded the configured {}-session safety bound",
                        brain.discovery_session_limit()
                    )));
                }
                queue.push_back(child.id.clone());
                snapshots.push(child);
            }
            let Some(next) = next else { break };
            if !cursors.insert(next.clone()) {
                return Err(crate::Error::Upstream(
                    "strong child adjacency repeated a pagination cursor".into(),
                ));
            }
            cursor = Some(next);
        }
    }

    let discovered_rows = snapshots
        .iter()
        .map(|snapshot| {
            // A newly discovered child may have uploaded and deleted an object entirely between
            // tenant-index sweeps. Start from the protocol's zero-at-create invariant and let its
            // journal reconstruct every historical gauge transition before consulting HEAD.
            let fold = crate::rating::FoldState {
                storage_transition_ms: snapshot.created_ms,
                metered_to_ms: snapshot.created_ms,
                ..Default::default()
            };
            SessionRow {
                id: snapshot.id.clone(),
                account_id: account_id.to_owned(),
                key_id: "brain-discovered".into(),
                parent_id: snapshot.parent_id.clone(),
                root_id: snapshot.root_id.clone(),
                depth: snapshot.depth,
                shape: snapshot.shape.clone(),
                created_ms: snapshot.created_ms,
                is_final: false,
                fold,
            }
        })
        .collect();
    db.upsert_discovered_sessions(discovered_rows).await?;
    for snapshot in snapshots {
        let row = db
            .session(snapshot.id.clone())
            .await?
            .ok_or_else(|| crate::Error::Internal("fenced session was not persisted".into()))?;
        // Strong adjacency proves completeness, while each direct GET supplies that session's
        // authoritative journal high-water. GSI/listing projections never form a purge barrier.
        sweep_session(db, brain, card, row).await?;
    }
    Ok(())
}

/// Read and persist one changed remote window. The caller owns watermark advancement so any later
/// fold failure retries this exact window.
async fn discover_changed(
    db: &Db,
    brain: &BrainClient,
    account_id: &str,
) -> Result<(i64, HashMap<String, BrainSessionSnapshot>)> {
    let cutoff = now_ms();
    let watermark = db.discovery_watermark(account_id.to_owned()).await?;
    let snapshots = brain.discover_sessions(account_id, watermark).await?;

    let discovered_rows = snapshots
        .iter()
        .map(|snapshot| {
            let fold = crate::rating::FoldState {
                storage_transition_ms: snapshot.created_ms,
                metered_to_ms: snapshot.created_ms,
                ..Default::default()
            };
            SessionRow {
                id: snapshot.id.clone(),
                account_id: account_id.to_owned(),
                // The creating API key is not part of Brain's neutral child identity and is not
                // used for authorization; ownership is sealed to the tenant above.
                key_id: "brain-discovered".into(),
                parent_id: snapshot.parent_id.clone(),
                root_id: snapshot.root_id.clone(),
                depth: snapshot.depth,
                shape: snapshot.shape.clone(),
                created_ms: snapshot.created_ms,
                is_final: false,
                fold,
            }
        })
        .collect();
    db.upsert_discovered_sessions(discovered_rows).await?;

    let snapshots: HashMap<_, _> = snapshots
        .into_iter()
        .map(|snapshot| (snapshot.id.clone(), snapshot))
        .collect();
    Ok((cutoff, snapshots))
}

/// Discover and fully settle one account. Explicit balance/usage, low-balance admission, refunds,
/// and destructive reconciliation use this rare path because they need an exact local ledger at
/// the response boundary.
pub async fn sweep_account(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    account_id: &str,
) -> Result<Vec<SweptLine>> {
    let (cutoff, _snapshots) = discover_changed(db, brain, account_id).await?;
    for row in db.sessions_of(account_id.to_owned()).await? {
        if row.is_final {
            continue;
        }
        sweep_session(db, brain, card, row).await?;
    }
    db.advance_discovery_watermark(account_id.to_owned(), cutoff)
        .await?;
    db.sessions_of(account_id.to_owned())
        .await?
        .into_iter()
        .map(|row| priced_line(card, row, now_ms()))
        .collect()
}

/// Normal background reconciliation is proportional to changed sessions, live open turns, and a
/// bounded oldest-first byte-meter batch. It never loads every locally known session merely to
/// advance the tenant discovery watermark.
pub async fn sweep_account_incremental(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    account_id: &str,
) -> Result<()> {
    let (cutoff, snapshots) = discover_changed(db, brain, account_id).await?;
    let mut settled = HashSet::with_capacity(snapshots.len());
    let changed_rows = db
        .sessions_by_ids(account_id.to_owned(), snapshots.keys().cloned().collect())
        .await?;
    if changed_rows.len() != snapshots.len() {
        return Err(crate::Error::Internal(
            "one or more discovered sessions were not persisted".into(),
        ));
    }
    for row in changed_rows {
        let snapshot = snapshots.get(&row.id).ok_or_else(|| {
            crate::Error::Internal(format!("unexpected changed session {}", row.id))
        })?;
        sweep_session(db, brain, card, row).await?;
        settled.insert(snapshot.id.clone());
    }

    // A long-running turn may have no new HEAD update, but its debit estimate must continue to
    // move before in-memory action reservations expire.
    for row in db.open_turn_sessions(account_id.to_owned()).await? {
        if settled.insert(row.id.clone()) {
            settle_snapshot(db, brain, card, row, None, cutoff).await?;
        }
    }

    // Storage is much cheaper and can be settled less often. A due row with no changed tenant
    // projection needs only a new absolute local estimate: durable transition history is already
    // closed at `storage_transition_ms`, while the open interval remains replaceable when a later
    // changed-window replay arrives. Do not turn this timer into one Brain HEAD per idle session.
    let due_before = cutoff.saturating_sub(BACKGROUND_STORAGE_SETTLEMENT_MS);
    for row in db
        .due_storage_sessions(account_id.to_owned(), due_before, BACKGROUND_STORAGE_BATCH)
        .await?
    {
        if settled.insert(row.id.clone()) {
            settle_snapshot(db, brain, card, row, None, cutoff).await?;
        }
    }

    db.advance_discovery_watermark(account_id.to_owned(), cutoff)
        .await
}

/// Background reconciliation covers every account registered before its first create request.
/// Errors retain that account's watermark and are retried on the next delayed tick.
pub async fn run_sweeper(
    db: Db,
    brain: BrainClient,
    card: RateCard,
    interval_s: u64,
    admission: crate::admission::Admission,
) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval_s.max(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let accounts = match db.discovery_accounts().await {
            Ok(accounts) => accounts,
            Err(error) => {
                tracing::warn!("sweeper: list accounts: {error}");
                continue;
            }
        };
        futures_util::stream::iter(accounts)
            .for_each_concurrent(BACKGROUND_ACCOUNT_CONCURRENCY, |account_id| {
                let db = db.clone();
                let brain = brain.clone();
                let card = card.clone();
                let admission = admission.clone();
                async move {
                    // Settlement covers every durable account, but dormant tenants must not churn
                    // the bounded hot admission cache merely because the sweeper visited them.
                    let reconciliation = admission.begin_cached_reconciliation(&account_id);
                    match sweep_account_incremental(&db, &brain, &card, &account_id).await {
                        Ok(_) => {
                            // A strong changed-session fold may have moved a local root from a
                            // resource-bearing lifecycle to ended. Invalidate the separate
                            // root-count cache so its next use recomputes
                            // max(eventual GSI, durable local floor).
                            admission.invalidate_live_root_sessions(&account_id);
                            match db.balance(account_id.clone()).await {
                                Ok(balance) => {
                                    if let Some(fence) = reconciliation.as_ref() {
                                        admission.mark_reconciled(
                                            &account_id,
                                            balance,
                                            now_ms(),
                                            fence.generation(),
                                        );
                                    }
                                }
                                Err(error) => {
                                    admission.invalidate_balance(&account_id);
                                    tracing::warn!(
                                        "sweeper: account {account_id}: balance: {error}"
                                    );
                                }
                            }
                        }
                        Err(error) => tracing::warn!("sweeper: account {account_id}: {error}"),
                    }
                }
            })
            .await;
    }
}

async fn advance_deletion(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    job: crate::store::DeletionRow,
) -> Result<()> {
    match job.phase.as_str() {
        "ending" => {
            brain.request_end(&job.account_id, &job.session_id).await?;
            match brain.get_session(&job.account_id, &job.session_id).await? {
                Some(document)
                    if matches!(document["state"].as_str(), Some("ended" | "deleting")) =>
                {
                    db.set_deletion_phase(job.session_id, "settling".into(), now_ms())
                        .await
                }
                Some(_) => Ok(()),
                None => match brain
                    .deletion_status(&job.account_id, &job.session_id)
                    .await?
                {
                    Some(status) if status.state == "succeeded" => {
                        finalize_deleted_subtree(
                            db,
                            card,
                            &job.account_id,
                            &job.session_id,
                            status.completed_at_ms.ok_or_else(|| {
                                crate::Error::Upstream(
                                    "succeeded deletion has no completion timestamp".into(),
                                )
                            })?,
                        )
                        .await
                    }
                    _ => Err(crate::Error::Upstream(
                        "deletion root disappeared before settlement".into(),
                    )),
                },
            }
        }
        "settling" => {
            settle_fenced_subtree(db, brain, card, &job.account_id, &job.session_id).await?;
            // The durable local phase is the destructive barrier. A crash after this commit may
            // repeat DELETE, but can never purge before all strongly discovered journals folded.
            db.set_deletion_phase(job.session_id, "purging".into(), now_ms())
                .await
        }
        "purging" => {
            let complete = brain
                .accept_delete(&job.account_id, &job.session_id)
                .await?;
            if complete {
                let status = brain
                    .deletion_status(&job.account_id, &job.session_id)
                    .await?
                    .filter(|status| status.state == "succeeded")
                    .ok_or_else(|| {
                        crate::Error::Upstream(
                            "completed deletion has no readable status boundary".into(),
                        )
                    })?;
                finalize_deleted_subtree(
                    db,
                    card,
                    &job.account_id,
                    &job.session_id,
                    status.completed_at_ms.ok_or_else(|| {
                        crate::Error::Upstream(
                            "succeeded deletion has no completion timestamp".into(),
                        )
                    })?,
                )
                .await
            } else {
                db.set_deletion_phase(job.session_id, "awaiting_purge".into(), now_ms())
                    .await
            }
        }
        "awaiting_purge" => match brain
            .deletion_status(&job.account_id, &job.session_id)
            .await?
        {
            Some(status) if status.state == "succeeded" => {
                finalize_deleted_subtree(
                    db,
                    card,
                    &job.account_id,
                    &job.session_id,
                    status.completed_at_ms.ok_or_else(|| {
                        crate::Error::Upstream(
                            "succeeded deletion has no completion timestamp".into(),
                        )
                    })?,
                )
                .await
            }
            Some(_) => Ok(()),
            None => {
                // A lost/rolled-back Brain acceptance is safe to retry because Aex's settlement
                // barrier is already durable.
                db.set_deletion_phase(job.session_id, "purging".into(), now_ms())
                    .await
            }
        },
        "succeeded" => Ok(()),
        phase => Err(crate::Error::Internal(format!(
            "unknown deletion phase {phase}"
        ))),
    }
}

/// One retryable step of an account deletion. Acceptance already handed every session the account
/// owned to the per-session jobs above, so this only covers what discovery surfaces afterwards and
/// then closes the account out. The final strong sweep is the erasure barrier: nothing is erased
/// until a discovery that finds no session left to delete.
async fn advance_account_deletion(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    job: crate::store::AccountDeletionRow,
) -> Result<()> {
    if job.sessions_pending > 0 {
        db.accept_account_session_deletions(job.account_id, now_ms())
            .await?;
        return Ok(());
    }
    sweep_account(db, brain, card, &job.account_id).await?;
    db.accept_account_session_deletions(job.account_id, now_ms())
        .await?;
    // The close-out re-reads the session count under its own transaction, so a session this
    // sweep has just discovered postpones erasure rather than racing it.
    db.close_account_deletion(job.id, now_ms()).await?;
    Ok(())
}

/// Retryable, crash-safe destructive workflow. DELETE admission only inserts the small SQLite
/// anchor; this worker ends/fences, strongly settles, accepts Brain purge, and observes
/// completion. Account deletions ride the same loop because they complete only when the session
/// jobs they enqueued have.
pub async fn run_deletion_worker(db: Db, brain: BrainClient, card: RateCard) {
    if let Err(error) = db.clear_deletion_claims().await {
        tracing::error!("deletion worker could not recover scheduler claims: {error}");
        return;
    }
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let jobs = match db
            .claim_pending_deletions(DELETION_JOB_BATCH, now_ms())
            .await
        {
            Ok(jobs) => jobs,
            Err(error) => {
                tracing::warn!("deletion worker: list jobs: {error}");
                continue;
            }
        };
        futures_util::stream::iter(jobs)
            .for_each_concurrent(DELETION_JOB_CONCURRENCY, |job| {
                let db = db.clone();
                let brain = brain.clone();
                let card = card.clone();
                async move {
                    if let Err(error) = advance_deletion(&db, &brain, &card, job.clone()).await {
                        let message = error.to_string();
                        if let Err(store_error) = db
                            .record_deletion_error(job.session_id.clone(), message, now_ms())
                            .await
                        {
                            tracing::warn!(
                                session = %job.session_id,
                                error = %store_error,
                                "deletion worker could not persist retry error"
                            );
                        }
                        tracing::warn!(
                            session = %job.session_id,
                            phase = %job.phase,
                            error = %error,
                            "session deletion will retry"
                        );
                    }
                    if let Err(error) = db.release_deletion_claim(job.session_id.clone()).await {
                        tracing::warn!(
                            session = %job.session_id,
                            error = %error,
                            "deletion worker could not release its scheduler claim"
                        );
                    }
                }
            })
            .await;

        match db.pending_account_deletions(DELETION_JOB_BATCH).await {
            Ok(jobs) => {
                for job in jobs {
                    let id = job.id.clone();
                    if let Err(error) = advance_account_deletion(&db, &brain, &card, job).await {
                        tracing::warn!(
                            account_deletion = %id,
                            error = %error,
                            "account deletion will retry"
                        );
                    }
                }
            }
            Err(error) => tracing::warn!("deletion worker: list account deletions: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Uri, header};
    use axum::response::Response;
    use serde_json::json;

    use super::*;
    use crate::store::AccountRow;

    async fn failing_replay_brain(method: Method, uri: Uri) -> Response {
        let response = |status, body: serde_json::Value| {
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        if method == Method::GET && uri.path() == "/v1/session-changes" {
            let data = vec![json!({
                "id":"change-failure",
                "session":{
                    "id":"ses_failure00000000000001",
                    "root_id":"ses_failure00000000000001",
                    "depth":0,
                    "object":"session",
                    "state":"open",
                    "turn_state":"running",
                    "shape":"1gb",
                    "model":{"provider":"anthropic", "name":"model"},
                    "storage":{"session_storage_bytes":0, "upload_reserved_bytes":0},
                    "created_at":crate::rfc3339(crate::now_ms() - 2_000),
                    "updated_at":crate::rfc3339(crate::now_ms()),
                    "last_seq":2,
                    "turns":1,
                    "metadata":{}
                }
            })];
            return response(
                200,
                json!({
                    "object":"session.change.list",
                    "partition":0,
                    "partitions":1,
                    "watermark_ms":crate::now_ms(),
                    "data":data,
                    "has_more":false
                }),
            );
        }
        if method == Method::GET && uri.path() == "/v1/sessions/ses_failure00000000000001" {
            return response(
                200,
                json!({
                    "id":"ses_failure00000000000001",
                    "root_id":"ses_failure00000000000001",
                    "depth":0,
                    "object":"session",
                    "state":"open",
                    "turn_state":"running",
                    "shape":"1gb",
                    "model":{"provider":"anthropic", "name":"model"},
                    "storage":{"session_storage_bytes":0, "upload_reserved_bytes":0},
                    "created_at":crate::rfc3339(crate::now_ms() - 2_000),
                    "updated_at":crate::rfc3339(crate::now_ms()),
                    "last_seq":2,
                    "turns":1,
                    "metadata":{}
                }),
            );
        }
        if method == Method::GET && uri.path().ends_with("/events") {
            return response(
                503,
                json!({"error":{"code":"unavailable", "message":"retry"}}),
            );
        }
        response(
            404,
            json!({"error":{"code":"not_found", "message":"missing"}}),
        )
    }

    async fn unchanged_brain(method: Method, uri: Uri) -> Response {
        let response = |status, body: serde_json::Value| {
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        if method == Method::GET && uri.path() == "/v1/session-changes" {
            return response(
                200,
                json!({
                    "object":"session.change.list",
                    "partition":0,
                    "partitions":1,
                    "watermark_ms":crate::now_ms(),
                    "data":[],
                    "has_more":false
                }),
            );
        }
        if method == Method::GET && uri.path().starts_with("/v1/sessions/") {
            return response(
                500,
                json!({"error":{"code":"unexpected_head", "message":"unchanged rows must remain local"}}),
            );
        }
        response(
            404,
            json!({"error":{"code":"not_found", "message":"missing"}}),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_mid_sweep_never_advances_the_durable_watermark() {
        let app = axum::Router::new().fallback(failing_replay_brain);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let db = Db::open_memory().unwrap();
        db.create_account(
            AccountRow {
                id: "acc_failure".into(),
                email: "failure@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "failure@example.test".into(),
            "token-hash".into(),
        )
        .await
        .unwrap();
        db.enable_session_discovery("acc_failure".into())
            .await
            .unwrap();
        db.advance_discovery_watermark("acc_failure".into(), 123)
            .await
            .unwrap();

        let brain = BrainClient::new(base, "operator");
        let error = sweep_account(&db, &brain, &RateCard::default(), "acc_failure")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("GET events -> 503"), "{error}");
        assert_eq!(
            db.discovery_watermark("acc_failure".into()).await.unwrap(),
            123
        );
        assert_eq!(
            db.session("ses_failure00000000000001".into())
                .await
                .unwrap()
                .expect("discovered session")
                .shape,
            "1gb",
            "hidden sessions inherit the hosted root's authoritative physical shape",
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn background_work_is_changed_or_due_not_every_known_session() {
        let app = axum::Router::new().fallback(unchanged_brain);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let db = Db::open_memory().unwrap();
        db.create_account(
            AccountRow {
                id: "acc_incremental".into(),
                email: "incremental@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "incremental@example.test".into(),
            "token-hash".into(),
        )
        .await
        .unwrap();
        let now = now_ms();
        let row = |id: &str, fold: crate::rating::FoldState| SessionRow {
            id: id.into(),
            account_id: "acc_incremental".into(),
            key_id: "key".into(),
            parent_id: None,
            root_id: id.into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold,
        };
        db.insert_session(row(
            "ses_recent",
            crate::rating::FoldState {
                metered_to_ms: now,
                session_storage_bytes: 10,
                session_state: "ended".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
        db.insert_session(row(
            "ses_open",
            crate::rating::FoldState {
                metered_to_ms: now - 1_000,
                turn_open_ms: Some(now - 10_000),
                session_state: "open".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
        db.insert_session(row(
            "ses_due",
            crate::rating::FoldState {
                metered_to_ms: now - BACKGROUND_STORAGE_SETTLEMENT_MS - 1_000,
                session_storage_bytes: 10,
                session_state: "ended".into(),
                ..Default::default()
            },
        ))
        .await
        .unwrap();

        sweep_account_incremental(
            &db,
            &BrainClient::new(base, "operator"),
            &RateCard::default(),
            "acc_incremental",
        )
        .await
        .unwrap();

        assert_eq!(
            db.session("ses_recent".into())
                .await
                .unwrap()
                .unwrap()
                .fold
                .metered_to_ms,
            now,
            "a recent unchanged storage row must not be rewritten"
        );
        assert!(
            db.session("ses_open".into())
                .await
                .unwrap()
                .unwrap()
                .fold
                .metered_to_ms
                >= now
        );
        assert!(
            db.session("ses_due".into())
                .await
                .unwrap()
                .unwrap()
                .fold
                .metered_to_ms
                >= now
        );
    }

    #[tokio::test]
    async fn an_eventually_consistent_snapshot_never_regresses_a_newer_local_fold() {
        let db = Db::open_memory().unwrap();
        db.create_account(
            AccountRow {
                id: "acc_stale".into(),
                email: "stale@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "stale@example.test".into(),
            "token-hash".into(),
        )
        .await
        .unwrap();
        let now = now_ms();
        let row = SessionRow {
            id: "ses_stale".into(),
            account_id: "acc_stale".into(),
            key_id: "key".into(),
            parent_id: None,
            root_id: "ses_stale".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold: crate::rating::FoldState {
                folded_seq: 5,
                metered_to_ms: now,
                session_storage_bytes: 10,
                session_state: "ended".into(),
                ..Default::default()
            },
        };
        db.insert_session(row.clone()).await.unwrap();
        let stale = BrainSessionSnapshot {
            id: row.id.clone(),
            root_id: row.root_id.clone(),
            parent_id: None,
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            updated_ms: now - 1,
            last_seq: 4,
            document: json!({
                "state": "open",
                "turn_state": "running",
                "storage": {"session_storage_bytes": 999, "upload_reserved_bytes": 999}
            }),
        };

        let settled = settle_snapshot(
            &db,
            &BrainClient::new("http://127.0.0.1:9", "operator"),
            &RateCard::default(),
            row,
            Some(&stale),
            now + 1,
        )
        .await
        .unwrap();

        assert_eq!(settled.row.fold.folded_seq, 5);
        assert_eq!(settled.row.fold.session_state, "ended");
        assert_eq!(settled.row.fold.session_storage_bytes, 10);
        assert_eq!(settled.row.fold.upload_reserved_bytes, 0);
    }

    #[tokio::test]
    async fn physical_purge_cannot_close_storage_before_its_last_transition() {
        let db = Db::open_memory().unwrap();
        db.create_account(
            AccountRow {
                id: "acc_boundary".into(),
                email: "boundary@example.test".into(),
                created_ms: 1,
                max_concurrent_sessions: 10,
                session_creates_per_hour: 30,
            },
            "boundary@example.test".into(),
            "token-hash".into(),
        )
        .await
        .unwrap();
        let row = SessionRow {
            id: "ses_boundary".into(),
            account_id: "acc_boundary".into(),
            key_id: "key".into(),
            parent_id: None,
            root_id: "ses_boundary".into(),
            depth: 0,
            shape: "1gb".into(),
            created_ms: 1,
            is_final: false,
            fold: crate::rating::FoldState {
                storage_transition_ms: 20,
                session_storage_bytes: 10,
                ..Default::default()
            },
        };
        db.insert_session(row.clone()).await.unwrap();

        let error = settle_deleted(&db, &RateCard::default(), row, 19)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("regressed"), "{error}");
        assert!(
            !db.session("ses_boundary".into())
                .await
                .unwrap()
                .unwrap()
                .is_final
        );
    }
}
