//! The metering sweep: advance one session's fold from the brain's journal, integrate storage,
//! price on the card, post the debit.
//!
//! Runs incrementally (replay after the fold position) and is safe to run concurrently — the
//! store fences it. Called opportunistically (message admission, balance/usage reads) and by
//! the background loop, so the bill converges even for an account nobody is looking at.

use crate::brain::BrainClient;
use crate::rating::{self, Priced, RateCard};
use crate::store::{Db, SessionRow};
use crate::{Result, now_ms};

/// A swept, priced line for one session.
#[derive(Debug, Clone)]
pub struct SweptLine {
    pub row: SessionRow,
    pub priced: Priced,
}

/// Sweep one session. Deleted-at-the-brain sessions go final: their meters stop, their rated
/// total stays on the ledger.
pub async fn sweep_session(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    row: SessionRow,
) -> Result<SweptLine> {
    if row.is_final {
        let priced = rating::price(card, &row.shape, &row.fold, row.fold.metered_to_ms);
        return Ok(SweptLine { row, priced });
    }
    let mut fold = row.fold.clone();
    let events = brain.replay_events(&row.id, fold.folded_seq).await?;
    rating::fold_events(&mut fold, &events);

    let session = brain.get_session(&row.id).await?;
    let now = now_ms();
    // Integrate the window we just lived through at the bytes reported for it, THEN apply the
    // fresh reading for the next window.
    rating::accrue_storage(&mut fold, now);
    let is_final = match &session {
        Some(doc) => {
            rating::apply_snapshot(&mut fold, doc);
            false
        }
        None => {
            // Deleted: everything is purged; nothing accrues from here on.
            fold.session_state = "deleted".into();
            fold.hand_state = "released".into();
            fold.workspace_bytes = 0;
            fold.suspended_bytes = 0;
            fold.artifact_bytes = 0;
            true
        }
    };

    let priced = rating::price(card, &row.shape, &fold, now);
    db.apply_sweep(
        row.id.clone(),
        row.account_id.clone(),
        fold.clone(),
        is_final,
        priced.total_microusd,
        now,
    )
    .await?;
    let mut row = row;
    row.fold = fold;
    row.is_final = is_final;
    Ok(SweptLine { row, priced })
}

/// Sweep every session of one account (the balance/usage path).
pub async fn sweep_account(
    db: &Db,
    brain: &BrainClient,
    card: &RateCard,
    account_id: &str,
) -> Result<Vec<SweptLine>> {
    let mut lines = Vec::new();
    for row in db.sessions_of(account_id.to_string()).await? {
        lines.push(sweep_session(db, brain, card, row).await?);
    }
    Ok(lines)
}

/// The background loop: every non-final session, every `interval`; errors are logged, not fatal
/// (the brain may be briefly unreachable — the fold resumes where it left off).
pub async fn run_sweeper(db: Db, brain: BrainClient, card: RateCard, interval_s: u64) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval_s.max(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let rows = match db.sessions_to_sweep().await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("sweeper: list: {e}");
                continue;
            }
        };
        for row in rows {
            let id = row.id.clone();
            if let Err(e) = sweep_session(&db, &brain, &card, row).await {
                tracing::warn!("sweeper: {id}: {e}");
            }
        }
    }
}
