//! Rated usage on the public compute and storage rate card.
//!
//! The brain's journal is the billing record. Compute time is a pure fold over a session's
//! event log: a turn runs from `turn.started` to `turn.completed`/`turn.failed`; those
//! intervals — and nothing else — are billed at the hosted alpha's fixed baseline rate. The
//! pre-suspend idle window is absorbed by the public rate policy. Session-storage history comes
//! from durable journal gauge transitions; HEAD `StorageInfo` gauges are reconciliation evidence.
//! Integrals retain the sub-second byte-millisecond remainder. The public usage projection adds the
//! derived open interval through its response cutoff, so it can reproduce the charge exactly.
//! Everything is integer arithmetic in micro-USD; division floors in the customer's favor.

use serde_json::Value;

use crate::{Error, Result};

fn storage_gauges(storage: &serde_json::Map<String, Value>, context: &str) -> Result<(i64, i64)> {
    let gauge = |name: &str| {
        storage
            .get(name)
            .and_then(Value::as_u64)
            .filter(|bytes| *bytes <= crate::MAX_HOSTED_SESSION_STORAGE_BYTES)
            .and_then(|bytes| i64::try_from(bytes).ok())
            .ok_or_else(|| Error::Upstream(format!("{context} has invalid {name}")))
    };
    let published = gauge("session_storage_bytes")?;
    let reserved = gauge("upload_reserved_bytes")?;
    let total = u64::try_from(published)
        .ok()
        .and_then(|value| value.checked_add(u64::try_from(reserved).ok()?));
    if total.is_none_or(|value| value > crate::MAX_HOSTED_SESSION_STORAGE_BYTES) {
        return Err(Error::Upstream(format!(
            "{context} exceeds the hosted session storage ceiling"
        )));
    }
    Ok((published, reserved))
}

/// The active prices, in micro-USD. Defaults match the public rate card; every field can be overridden by
/// `AEX_RATE_*` env (the private `platform` repo owns the deployed pricing config).
#[derive(Debug, Clone)]
pub struct RateCard {
    pub vcpu_hour_microusd: i64,
    pub gb_hour_microusd: i64,
    pub session_storage_gb_month_microusd: i64,
    pub web_search_query_microusd: i64,
    pub month_hours: i64,
}

impl Default for RateCard {
    fn default() -> Self {
        RateCard {
            vcpu_hour_microusd: 190_000,
            gb_hour_microusd: 25_000,
            session_storage_gb_month_microusd: 30_000,
            web_search_query_microusd: 3_000,
            month_hours: 730,
        }
    }
}

impl RateCard {
    pub fn from_env() -> anyhow::Result<Self> {
        fn rate(name: &str, default: i64) -> anyhow::Result<i64> {
            match std::env::var(name) {
                Ok(v) => v.parse().map_err(|e| anyhow::anyhow!("{name}={v}: {e}")),
                Err(_) => Ok(default),
            }
        }
        let d = RateCard::default();
        RateCard {
            vcpu_hour_microusd: rate("AEX_RATE_VCPU_HOUR_MICROUSD", d.vcpu_hour_microusd)?,
            gb_hour_microusd: rate("AEX_RATE_GB_HOUR_MICROUSD", d.gb_hour_microusd)?,
            session_storage_gb_month_microusd: rate(
                "AEX_RATE_SESSION_STORAGE_GB_MONTH_MICROUSD",
                d.session_storage_gb_month_microusd,
            )?,
            web_search_query_microusd: rate(
                "AEX_RATE_WEB_SEARCH_QUERY_MICROUSD",
                d.web_search_query_microusd,
            )?,
            month_hours: rate("AEX_RATE_MONTH_HOURS", d.month_hours)?,
        }
        .validate()
    }

    pub fn validate(self) -> anyhow::Result<Self> {
        if self.vcpu_hour_microusd < 0
            || self.gb_hour_microusd < 0
            || self.session_storage_gb_month_microusd < 0
            || self.web_search_query_microusd < 0
        {
            anyhow::bail!("Aex rate-card prices must be non-negative integer micro-USD");
        }
        if !(1..=8_760).contains(&self.month_hours) {
            anyhow::bail!("AEX_RATE_MONTH_HOURS must be between 1 and 8760");
        }
        if (self.vcpu_hour_microusd / 2)
            .checked_add(self.gb_hour_microusd)
            .is_none()
        {
            anyhow::bail!("Aex hosted hourly rate exceeds the exact billing range");
        }
        Ok(self)
    }

    /// Micro-USD per hour running on the alpha's sealed 1 GiB / 0.5 vCPU baseline.
    /// Transient provider burst capacity is neither separately metered nor an entitlement.
    /// The hosted boundary rejects every other neutral Brain shape before creation.
    pub fn hourly_microusd(&self, shape: &str) -> Result<i64> {
        if shape != "1gb" {
            return Err(Error::Internal(format!(
                "cannot price unsupported hosted managed-compute shape {shape}"
            )));
        }
        let vcpu = self.vcpu_hour_microusd / 2;
        vcpu.checked_add(self.gb_hour_microusd).ok_or_else(|| {
            Error::Internal("hosted hourly rate exceeds the exact billing range".into())
        })
    }

    /// The wire form (`contracts/control/v1` `RateCard`). The server responds in JSON validated
    /// against the schema by the e2e test; the generated Rust types are for consumers (SDK).
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "object": "rate_card",
            "vcpu_hour_microusd": self.vcpu_hour_microusd.to_string(),
            "gb_hour_microusd": self.gb_hour_microusd.to_string(),
            "session_storage_gb_month_microusd": self.session_storage_gb_month_microusd.to_string(),
            "web_search_query_microusd": self.web_search_query_microusd.to_string(),
            "month_hours": self.month_hours,
        })
    }
}

/// A session's meter state: the fold position plus the integrals. Persisted per session and
/// advanced by sweeps; deterministic given journal transitions, with HEAD gauges used only for
/// reconciliation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FoldState {
    /// Highest event seq folded. Replay resumes at `after=folded_seq`.
    pub folded_seq: i64,
    /// Closed turn intervals, exact from event timestamps.
    pub running_ms: i64,
    /// `at` of an unclosed `turn.started`, if a turn is open.
    pub turn_open_ms: Option<i64>,
    /// Durable storage integral, split into byte-seconds plus its subsecond remainder for compact
    /// SQLite persistence.
    pub session_storage_byte_seconds: i64,
    /// Exact persisted remainder for the internal byte-millisecond integral. Keeping it separate
    /// preserves every short journal interval without rounding at each transition. The public
    /// projection recombines this pair as byte-milliseconds.
    pub session_storage_byte_millisecond_remainder: i64,
    /// Successful managed `web_search` results committed to the journal.
    pub web_search_queries: i64,
    /// Last durable storage transition integrated into the closed byte-time total.
    pub storage_transition_ms: i64,
    /// Latest wall-time at which the absolute usage ledger projection was priced. This is a
    /// scheduler/concurrency fence, never a claimed durable storage transition.
    pub metered_to_ms: i64,
    /// Latest meter readings (Brain-reported bytes) and session state.
    pub session_storage_bytes: i64,
    pub upload_reserved_bytes: i64,
    pub session_state: String,
}

/// Fold replayed events into the state: seq high-water, turn intervals, state transitions.
/// Live-only events (deltas, tool.output) never appear in replay; seq gaps are normal.
pub fn fold_events(state: &mut FoldState, events: &[Value]) -> Result<()> {
    for ev in events {
        let seq = ev["seq"]
            .as_u64()
            .and_then(|seq| i64::try_from(seq).ok())
            .ok_or_else(|| Error::Upstream("Brain replay event has invalid sequence".into()))?;
        // `after=folded_seq` is the ordinary exact-once boundary. Keep the fold idempotent too:
        // an overlap, retry, or malformed upstream page must never rate an already committed
        // event twice.
        if seq <= state.folded_seq {
            continue;
        }
        let at = ev["at"].as_str().and_then(crate::parse_rfc3339_ms);
        match ev["type"].as_str().unwrap_or("") {
            "turn.started" => {
                state.turn_open_ms = Some(at.ok_or_else(|| {
                    Error::Upstream("turn.started has invalid event timestamp".into())
                })?);
            }
            "turn.completed" | "turn.failed" => {
                let t1 = at.ok_or_else(|| {
                    Error::Upstream("terminal turn event has invalid timestamp".into())
                })?;
                if let Some(t0) = state.turn_open_ms {
                    let elapsed = t1.checked_sub(t0).ok_or_else(|| {
                        Error::Upstream("terminal turn timestamp overflowed".into())
                    })?;
                    state.running_ms = state
                        .running_ms
                        .checked_add(elapsed.max(0))
                        .ok_or_else(|| Error::Internal("running meter overflowed".into()))?;
                    state.turn_open_ms = None;
                }
            }
            "storage.usage" => {
                let transition_ms = at.ok_or_else(|| {
                    Error::Upstream("storage.usage has invalid event timestamp".into())
                })?;
                if state.storage_transition_ms > transition_ms {
                    return Err(Error::Upstream(
                        "storage.usage timestamp regressed behind the durable meter".into(),
                    ));
                }
                let storage = ev["storage"]
                    .as_object()
                    .ok_or_else(|| Error::Upstream("storage.usage has no storage gauge".into()))?;
                let (session_storage_bytes, upload_reserved_bytes) =
                    storage_gauges(storage, "storage.usage")?;
                accrue_storage(state, transition_ms)?;
                state.session_storage_bytes = session_storage_bytes;
                state.upload_reserved_bytes = upload_reserved_bytes;
            }
            "session.updated" => {
                let lifecycle = ev["state"]
                    .as_str()
                    .filter(|value| {
                        matches!(
                            *value,
                            "open" | "ending" | "ended" | "deleting" | "deleted" | "failed"
                        )
                    })
                    .ok_or_else(|| {
                        Error::Upstream("session.updated has invalid lifecycle state".into())
                    })?;
                ev["turn_state"]
                    .as_str()
                    .filter(|value| matches!(*value, "idle" | "running"))
                    .ok_or_else(|| {
                        Error::Upstream("session.updated has invalid turn state".into())
                    })?;
                state.session_state = lifecycle.into();
            }
            "tool.result"
                if ev["name"].as_str() == Some("web_search")
                    && ev["outcome"].as_str() == Some("completed") =>
            {
                state.web_search_queries = state
                    .web_search_queries
                    .checked_add(1)
                    .filter(|value| *value <= 134_217_728)
                    .ok_or_else(|| {
                        Error::Internal(
                            "web search meter exceeds the hosted journal ceiling".into(),
                        )
                    })?;
            }
            _ => {}
        }
        state.folded_seq = seq;
    }
    Ok(())
}

/// Integrate the current byte meters through a durable transition timestamp. Piecewise-constant:
/// call this immediately before applying the transition's post-state gauge, or once at a confirmed
/// physical-purge boundary. Ordinary HEAD reads must never call this with Aex wall time.
pub fn accrue_storage(state: &mut FoldState, transition_ms: i64) -> Result<()> {
    if state.storage_transition_ms == 0 {
        // The protocol defines an implicit zero gauge at create. Older unit fixtures may not
        // carry that origin, so the first transition starts the clock without inventing history.
        state.storage_transition_ms = transition_ms;
        return Ok(());
    }
    if transition_ms <= state.storage_transition_ms {
        return Ok(());
    }
    let dt_ms = transition_ms
        .checked_sub(state.storage_transition_ms)
        .ok_or_else(|| Error::Internal("storage transition interval overflowed".into()))?;
    let bytes = state
        .session_storage_bytes
        .checked_add(state.upload_reserved_bytes)
        .filter(|value| {
            (0..=i64::try_from(crate::MAX_HOSTED_SESSION_STORAGE_BYTES).unwrap_or(i64::MAX))
                .contains(value)
        })
        .ok_or_else(|| {
            Error::Internal("persisted storage gauge is outside its exact range".into())
        })?;
    accrue_byte_milliseconds(
        &mut state.session_storage_byte_seconds,
        &mut state.session_storage_byte_millisecond_remainder,
        bytes,
        dt_ms,
    )?;
    state.storage_transition_ms = transition_ms;
    Ok(())
}

fn accrue_byte_milliseconds(
    seconds: &mut i64,
    millisecond_remainder: &mut i64,
    bytes: i64,
    dt_ms: i64,
) -> Result<()> {
    if *seconds < 0 || !(0..=999).contains(millisecond_remainder) || bytes < 0 || dt_ms < 0 {
        return Err(Error::Internal(
            "storage meter contains a negative or non-canonical component".into(),
        ));
    }
    let maximum = i128::from(i64::MAX) * 1_000 + 999;
    let delta = i128::from(bytes)
        .checked_mul(i128::from(dt_ms))
        .ok_or_else(|| Error::Internal("storage byte-millisecond delta overflowed".into()))?;
    let accumulated = byte_milliseconds(*seconds, *millisecond_remainder)?
        .checked_add(delta)
        .filter(|value| *value <= maximum)
        .ok_or_else(|| Error::Internal("storage byte-millisecond meter overflowed".into()))?;
    *seconds = i64::try_from(accumulated / 1_000)
        .map_err(|_| Error::Internal("storage second meter overflowed".into()))?;
    *millisecond_remainder = i64::try_from(accumulated % 1_000)
        .map_err(|_| Error::Internal("storage millisecond remainder overflowed".into()))?;
    Ok(())
}

fn byte_milliseconds(seconds: i64, millisecond_remainder: i64) -> Result<i128> {
    if seconds < 0 || !(0..=999).contains(&millisecond_remainder) {
        return Err(Error::Internal(
            "storage meter contains a negative or non-canonical component".into(),
        ));
    }
    Ok(i128::from(seconds) * 1_000 + i128::from(millisecond_remainder))
}

/// Apply a fresh Brain HEAD snapshot after journal replay. The projection is reconciliation
/// evidence only: an absent/malformed/mismatched gauge fails closed instead of sampling history.
pub fn apply_snapshot(state: &mut FoldState, session: &Value) -> Result<()> {
    let storage = session["storage"]
        .as_object()
        .ok_or_else(|| Error::Upstream("session HEAD has no valid storage gauge".into()))?;
    let (published, reserved) = storage_gauges(storage, "session HEAD")?;
    if (published, reserved) != (state.session_storage_bytes, state.upload_reserved_bytes) {
        return Err(Error::Upstream(
            "session HEAD storage gauge disagrees with durable storage.usage replay".into(),
        ));
    }
    let lifecycle = session["state"]
        .as_str()
        .filter(|value| {
            matches!(
                *value,
                "open" | "ending" | "ended" | "deleting" | "deleted" | "failed"
            )
        })
        .ok_or_else(|| Error::Upstream("session HEAD has invalid lifecycle state".into()))?;
    let turn_state = session["turn_state"]
        .as_str()
        .filter(|value| matches!(*value, "idle" | "running"))
        .ok_or_else(|| Error::Upstream("session HEAD has invalid turn state".into()))?;
    if (turn_state == "running") != state.turn_open_ms.is_some() {
        return Err(Error::Upstream(
            "session HEAD turn state disagrees with durable turn replay".into(),
        ));
    }
    state.session_state = lifecycle.into();
    Ok(())
}

/// A priced line, in micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Priced {
    pub running_ms: i64,
    pub session_storage_byte_milliseconds: i128,
    pub compute_microusd: i64,
    pub storage_microusd: i64,
    pub web_search_microusd: i64,
    pub total_microusd: i64,
}

/// Price the state on the card. Open compute/storage intervals are derived ephemerally through
/// `now_ms`; their durable integrals never advance until their closing journal transition. A later
/// transition can therefore replace an earlier absolute ledger estimate in either direction.
pub fn price(card: &RateCard, shape: &str, state: &FoldState, now_ms: i64) -> Result<Priced> {
    let mut running_ms = state.running_ms;
    if running_ms < 0 {
        return Err(Error::Internal("running meter is negative".into()));
    }
    if let Some(t0) = state.turn_open_ms {
        let elapsed = now_ms
            .checked_sub(t0)
            .ok_or_else(|| Error::Internal("open turn interval overflowed".into()))?;
        running_ms = running_ms
            .checked_add(elapsed.max(0))
            .ok_or_else(|| Error::Internal("running meter overflowed".into()))?;
    }
    let compute = i128::from(running_ms)
        .checked_mul(i128::from(card.hourly_microusd(shape)?))
        .ok_or_else(|| Error::Internal("compute charge overflowed".into()))?
        / 3_600_000;
    let month_byte_milliseconds = 1_000_000_000i128
        .checked_mul(i128::from(card.month_hours))
        .and_then(|value| value.checked_mul(3_600_000))
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::Internal("storage pricing denominator is invalid".into()))?;
    let current_bytes = state
        .session_storage_bytes
        .checked_add(state.upload_reserved_bytes)
        .filter(|value| {
            (0..=i64::try_from(crate::MAX_HOSTED_SESSION_STORAGE_BYTES).unwrap_or(i64::MAX))
                .contains(value)
        })
        .ok_or_else(|| {
            Error::Internal("persisted storage gauge is outside its exact range".into())
        })?;
    let open_storage_ms = if state.storage_transition_ms > 0 {
        let elapsed = now_ms
            .checked_sub(state.storage_transition_ms)
            .ok_or_else(|| Error::Internal("open storage interval overflowed".into()))?;
        i128::from(current_bytes)
            .checked_mul(i128::from(elapsed.max(0)))
            .ok_or_else(|| Error::Internal("open storage byte-milliseconds overflowed".into()))?
    } else {
        0
    };
    let session_storage_byte_milliseconds = byte_milliseconds(
        state.session_storage_byte_seconds,
        state.session_storage_byte_millisecond_remainder,
    )?
    .checked_add(open_storage_ms)
    .ok_or_else(|| Error::Internal("storage byte-millisecond projection overflowed".into()))?;
    let storage = session_storage_byte_milliseconds
        .checked_mul(i128::from(card.session_storage_gb_month_microusd))
        .ok_or_else(|| Error::Internal("storage charge overflowed".into()))?
        / month_byte_milliseconds;
    let compute = i64::try_from(compute)
        .map_err(|_| Error::Internal("compute charge exceeds the exact ledger range".into()))?;
    let storage = i64::try_from(storage)
        .map_err(|_| Error::Internal("storage charge exceeds the exact ledger range".into()))?;
    if !(0..=134_217_728).contains(&state.web_search_queries) {
        return Err(Error::Internal(
            "web search meter exceeds the hosted journal ceiling".into(),
        ));
    }
    let web_search = state
        .web_search_queries
        .checked_mul(card.web_search_query_microusd)
        .ok_or_else(|| {
            Error::Internal("web search charge exceeds the exact ledger range".into())
        })?;
    let total = compute
        .checked_add(storage)
        .and_then(|value| value.checked_add(web_search))
        .ok_or_else(|| Error::Internal("total charge exceeds the exact ledger range".into()))?;
    Ok(Priced {
        running_ms,
        session_storage_byte_milliseconds,
        compute_microusd: compute,
        storage_microusd: storage,
        web_search_microusd: web_search,
        total_microusd: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(seq: i64, t: &str, at: &str) -> Value {
        json!({"type": t, "seq": seq, "at": at, "session_id": "ses_x", "turn_id": "trn_x"})
    }

    fn priced(card: &RateCard, shape: &str, state: &FoldState, now_ms: i64) -> Priced {
        price(card, shape, state, now_ms).unwrap()
    }

    #[test]
    fn card_matches_public_rate_card() {
        let c = RateCard::default();
        assert_eq!(c.hourly_microusd("1gb").unwrap(), 120_000); // $0.12/h
        assert!(c.hourly_microusd("2gb").is_err());
        assert!(price(&c, "unknown", &FoldState::default(), 0).is_err());
    }

    #[test]
    fn invalid_or_extreme_rate_cards_fail_closed_without_arithmetic_panics() {
        assert!(
            RateCard {
                vcpu_hour_microusd: -1,
                ..RateCard::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RateCard {
                month_hours: 0,
                ..RateCard::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RateCard {
                month_hours: 8_760,
                ..RateCard::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            RateCard {
                month_hours: 8_761,
                ..RateCard::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            RateCard {
                vcpu_hour_microusd: i64::MAX,
                gb_hour_microusd: i64::MAX,
                session_storage_gb_month_microusd: i64::MAX,
                web_search_query_microusd: i64::MAX,
                month_hours: 1,
            }
            .validate()
            .is_err()
        );
        let extreme = RateCard {
            gb_hour_microusd: 0,
            ..RateCard::default()
        };
        assert!(
            price(
                &extreme,
                "1gb",
                &FoldState {
                    session_storage_byte_seconds: i64::MAX,
                    web_search_queries: i64::MAX,
                    ..FoldState::default()
                },
                0,
            )
            .is_err()
        );
    }

    #[test]
    fn turn_intervals_fold_exactly() {
        let mut s = FoldState::default();
        fold_events(
            &mut s,
            &[
                ev(10, "turn.started", "2026-08-18T09:40:00Z"),
                ev(15, "turn.completed", "2026-08-18T09:40:01.500Z"),
                ev(20, "turn.started", "2026-08-18T09:41:00Z"),
                ev(27, "turn.failed", "2026-08-18T09:41:02Z"),
            ],
        )
        .unwrap();
        assert_eq!(s.running_ms, 3_500);
        assert_eq!(s.folded_seq, 27);
        assert_eq!(s.turn_open_ms, None);
    }

    #[test]
    fn worked_example_matches_the_contract_example() {
        // The arithmetic committed in contracts/examples/control/SessionUsage.example.json.
        let card = RateCard::default();
        let s = FoldState {
            running_ms: 322_000,
            session_storage_byte_seconds: 3_600_000_000_000, // 1 GB * 1 hour
            web_search_queries: 2,
            session_state: "open".into(),
            ..FoldState::default()
        };
        let p = priced(&card, "1gb", &s, 0);
        assert_eq!(p.session_storage_byte_milliseconds, 3_600_000_000_000_000);
        assert_eq!(p.compute_microusd, 10_733);
        assert_eq!(p.storage_microusd, 41);
        assert_eq!(p.web_search_microusd, 6_000);
        assert_eq!(p.total_microusd, 16_774);
    }

    #[test]
    fn only_successful_committed_web_search_results_are_counted() {
        let mut s = FoldState::default();
        fold_events(
            &mut s,
            &[
                json!({"type":"tool.result", "seq":1, "name":"web_search", "outcome":"completed"}),
                json!({"type":"tool.result", "seq":2, "name":"web_search", "outcome":"failed"}),
                json!({"type":"tool.result", "seq":3, "name":"web_fetch", "outcome":"completed"}),
            ],
        )
        .unwrap();
        assert_eq!(s.web_search_queries, 1);
        let p = priced(&RateCard::default(), "1gb", &s, 0);
        assert_eq!(p.web_search_microusd, 3_000);
        assert_eq!(p.total_microusd, 3_000);
    }

    #[test]
    fn compaction_provider_usage_advances_once_without_a_provider_charge() {
        let started = ev(10, "turn.started", "2026-08-18T09:40:00Z");
        let usage = json!({
            "type": "model.usage",
            "seq": 12,
            "purpose": "compaction",
            "provider": "openai",
            "model": "model",
            "usage": {"input_tokens": 5_000, "output_tokens": 500}
        });
        let completed = ev(15, "turn.completed", "2026-08-18T09:40:03.600Z");
        let events = [started, usage, completed];
        let mut state = FoldState::default();
        fold_events(&mut state, &events).unwrap();
        let first = state.clone();
        fold_events(&mut state, &events).unwrap();

        assert_eq!(
            state, first,
            "a replayed usage record is folded exactly once"
        );
        assert_eq!(state.folded_seq, 15);
        assert_eq!(state.running_ms, 3_600, "compaction stays inside its turn");
        assert_eq!(
            priced(&RateCard::default(), "1gb", &state, 0).total_microusd,
            120,
            "only the continuous managed-compute interval is charged"
        );
    }

    #[test]
    fn open_turn_estimate_follows_the_durable_turn_axis() {
        let card = RateCard::default();
        let mut s = FoldState::default();
        fold_events(&mut s, &[ev(10, "turn.started", "2026-08-18T09:40:00Z")]).unwrap();
        let t0 = crate::parse_rfc3339_ms("2026-08-18T09:40:00Z").unwrap();
        s.session_state = "open".into();
        assert_eq!(priced(&card, "1gb", &s, t0 + 30_000).running_ms, 30_000);
        // Lifecycle and current-turn activity are independent. A recursive end fence can move
        // the lifecycle to ending while the terminal turn event is still pending.
        s.session_state = "ending".into();
        assert_eq!(priced(&card, "1gb", &s, t0 + 30_000).running_ms, 30_000);
        fold_events(&mut s, &[ev(11, "turn.completed", "2026-08-18T09:40:30Z")]).unwrap();
        assert_eq!(priced(&card, "1gb", &s, t0 + 60_000).running_ms, 30_000);
    }

    #[test]
    fn storage_integrates_piecewise_constant() {
        let mut s = FoldState {
            session_storage_bytes: 1_000_000_000, // 1 GB
            upload_reserved_bytes: 500_000_000,
            ..FoldState::default()
        };
        accrue_storage(&mut s, 1_000).unwrap(); // first sweep only starts the clock
        assert_eq!(s.session_storage_byte_seconds, 0);
        accrue_storage(&mut s, 3_600_000 + 1_000).unwrap(); // one hour at 1 GB
        assert_eq!(s.session_storage_byte_seconds, 5_400_000_000_000);
        assert_eq!(s.session_storage_byte_millisecond_remainder, 0);
        // 1.5 GB-hours at $0.03/GB-month(730h) floors to 61 micro-USD.
        let p = priced(&RateCard::default(), "1gb", &s, 0);
        assert_eq!(p.session_storage_byte_milliseconds, 5_400_000_000_000_000);
        assert_eq!(p.storage_microusd, 61);
    }

    #[test]
    fn storage_keeps_subsecond_byte_millisecond_remainders_across_restarts() {
        let mut state = FoldState {
            storage_transition_ms: 1_000,
            metered_to_ms: 1_000,
            session_storage_bytes: 3,
            ..FoldState::default()
        };
        accrue_storage(&mut state, 1_001).unwrap();
        assert_eq!(state.session_storage_byte_seconds, 0);
        assert_eq!(state.session_storage_byte_millisecond_remainder, 3);
        assert_eq!(
            priced(&RateCard::default(), "1gb", &state, 1_001).session_storage_byte_milliseconds,
            3
        );

        // The persisted seconds+remainder pair is sufficient to resume without rounding each
        // short interval away.
        let mut resumed = state.clone();
        accrue_storage(&mut resumed, 1_334).unwrap();
        assert_eq!(resumed.session_storage_byte_seconds, 1);
        assert_eq!(resumed.session_storage_byte_millisecond_remainder, 2);
        assert_eq!(
            priced(&RateCard::default(), "1gb", &resumed, 1_334).session_storage_byte_milliseconds,
            1_002
        );
    }

    #[test]
    fn durable_storage_transitions_reconstruct_history_between_sweeps_exactly_once() {
        let created_ms = crate::parse_rfc3339_ms("2026-08-18T09:00:00Z").unwrap();
        let transition = |seq: i64, offset_ms: i64, published: u64, reserved: u64| {
            json!({
                "type": "storage.usage",
                "seq": seq,
                "at": crate::rfc3339(created_ms + offset_ms),
                "session_id": "ses_01HZX8Y2K3M4N5P6Q7R8S9T0",
                "storage": {
                    "session_storage_bytes": published,
                    "upload_reserved_bytes": reserved
                }
            })
        };
        let events = [
            transition(1, 10_000, 0, 500_000_000),
            transition(2, 11_000, 500_000_000, 0),
            transition(3, 1_811_000, 0, 0),
        ];
        let mut state = FoldState {
            storage_transition_ms: created_ms,
            metered_to_ms: created_ms,
            ..FoldState::default()
        };

        fold_events(&mut state, &events).unwrap();
        assert_eq!(state.folded_seq, 3);
        assert_eq!(state.session_storage_byte_seconds, 900_500_000_000);
        assert_eq!(state.session_storage_byte_millisecond_remainder, 0);
        assert_eq!(state.session_storage_bytes, 0);
        assert_eq!(state.upload_reserved_bytes, 0);
        let priced = priced(&RateCard::default(), "1gb", &state, 0);
        assert_eq!(
            priced.session_storage_byte_milliseconds,
            900_500_000_000_000
        );
        assert_eq!(priced.storage_microusd, 10);

        let first = state.clone();
        fold_events(&mut state, &events).unwrap();
        assert_eq!(state, first, "journal replay cannot double-rate storage");
    }

    #[test]
    fn malformed_or_time_regressing_storage_transition_does_not_advance_high_water() {
        let at = "2026-08-18T09:00:00Z";
        let max = crate::MAX_HOSTED_SESSION_STORAGE_BYTES;
        for event in [
            json!({"type":"storage.usage", "seq":1, "at":at,
                   "storage":{"session_storage_bytes":-1, "upload_reserved_bytes":0}}),
            json!({"type":"storage.usage", "seq":1, "at":at,
                   "storage":{"session_storage_bytes":max + 1, "upload_reserved_bytes":0}}),
            json!({"type":"storage.usage", "seq":1, "at":at,
                   "storage":{"session_storage_bytes":max, "upload_reserved_bytes":1}}),
            json!({"type":"storage.usage", "seq":1, "at":"not-a-time",
                   "storage":{"session_storage_bytes":0, "upload_reserved_bytes":0}}),
        ] {
            let mut state = FoldState::default();
            assert!(fold_events(&mut state, &[event]).is_err());
            assert_eq!(state.folded_seq, 0);
        }

        let mut at_limit = FoldState::default();
        fold_events(
            &mut at_limit,
            &[json!({"type":"storage.usage", "seq":1, "at":at,
                "storage":{"session_storage_bytes":max, "upload_reserved_bytes":0}})],
        )
        .unwrap();
        assert_eq!(at_limit.session_storage_bytes, i64::try_from(max).unwrap());

        let mut state = FoldState {
            storage_transition_ms: crate::parse_rfc3339_ms(at).unwrap() + 1,
            ..FoldState::default()
        };
        let regressed = json!({"type":"storage.usage", "seq":1, "at":at,
            "storage":{"session_storage_bytes":0, "upload_reserved_bytes":0}});
        assert!(fold_events(&mut state, &[regressed]).is_err());
        assert_eq!(state.folded_seq, 0);
    }

    #[test]
    fn snapshot_reconciles_but_never_repairs_session_storage_gauges() {
        let mut s = FoldState {
            session_storage_bytes: 5,
            upload_reserved_bytes: 3,
            ..FoldState::default()
        };
        apply_snapshot(
            &mut s,
            &json!({"state": "open", "turn_state": "idle",
                    "storage": {"session_storage_bytes": 5, "upload_reserved_bytes": 3}}),
        )
        .unwrap();
        assert_eq!((s.session_storage_bytes, s.upload_reserved_bytes), (5, 3));
        assert_eq!(s.session_state, "open");

        let before = s.clone();
        assert!(
            apply_snapshot(
                &mut s,
                &json!({"state": "open", "turn_state": "idle",
                        "storage": {"session_storage_bytes": 6, "upload_reserved_bytes": 3}}),
            )
            .is_err()
        );
        assert_eq!(
            s, before,
            "HEAD mismatch cannot silently sample or mutate state"
        );
        assert!(apply_snapshot(&mut s, &json!({"state": "open", "turn_state": "idle"})).is_err());
        assert_eq!(s, before, "a missing HEAD gauge must fail without mutation");

        assert!(
            apply_snapshot(
                &mut s,
                &json!({"state":"open", "turn_state":"running", "storage":{
                    "session_storage_bytes":5, "upload_reserved_bytes":3
                }}),
            )
            .is_err(),
            "a running HEAD cannot disagree with the replayed turn axis"
        );
        assert_eq!(s, before);
    }

    fn test_storage_card() -> RateCard {
        RateCard {
            session_storage_gb_month_microusd: 3_600_000,
            month_hours: 1,
            ..RateCard::default()
        }
    }

    #[test]
    fn delayed_transition_after_head_replaces_prior_estimate_in_both_directions() {
        let base = crate::parse_rfc3339_ms("2026-08-18T09:00:00Z").unwrap();
        let transition = |seq: i64, at_ms: i64, bytes: u64| {
            json!({
                "type":"storage.usage",
                "seq":seq,
                "at":crate::rfc3339(at_ms),
                "session_id":"ses_race",
                "storage":{"session_storage_bytes":bytes,"upload_reserved_bytes":0}
            })
        };
        let card = test_storage_card();

        // HEAD linearizes while the old 1 GB gauge is still authoritative. Pricing the open
        // interval to t=2s estimates 1,000 micro-USD but does not close the integral.
        let mut decrease = FoldState {
            folded_seq: 1,
            storage_transition_ms: base + 1_000,
            metered_to_ms: base + 2_000,
            session_storage_bytes: 1_000_000_000,
            session_state: "open".into(),
            ..FoldState::default()
        };
        apply_snapshot(
            &mut decrease,
            &json!({"state":"open","turn_state":"idle","storage":{
                "session_storage_bytes":1_000_000_000u64,"upload_reserved_bytes":0}}),
        )
        .unwrap();
        assert_eq!(
            priced(&card, "1gb", &decrease, base + 2_000).storage_microusd,
            1_000
        );
        assert_eq!(
            priced(&card, "1gb", &decrease, base + 2_000).session_storage_byte_milliseconds,
            1_000_000_000_000
        );

        // The delete transition commits just after that HEAD but is timestamped at t=1.5s.
        // It remains foldable even though the prior absolute estimate was rated through t=2s,
        // and replaces that estimate with the exact lower amount.
        fold_events(&mut decrease, &[transition(2, base + 1_500, 0)]).unwrap();
        assert_eq!(decrease.metered_to_ms, base + 2_000);
        assert_eq!(decrease.storage_transition_ms, base + 1_500);
        assert_eq!(
            priced(&card, "1gb", &decrease, base + 2_000).storage_microusd,
            500
        );
        assert_eq!(
            priced(&card, "1gb", &decrease, base + 2_000).session_storage_byte_milliseconds,
            500_000_000_000
        );
        let exact_decrease = decrease.clone();
        fold_events(&mut decrease, &[transition(2, base + 1_500, 0)]).unwrap();
        assert_eq!(
            decrease, exact_decrease,
            "replay cannot double-close the interval"
        );

        // The inverse race begins at zero. A delayed upload transition inside the already-rated
        // wall interval raises the replacement estimate to the exact 0.5 GB-second charge.
        let mut increase = FoldState {
            folded_seq: 1,
            storage_transition_ms: base + 1_000,
            metered_to_ms: base + 2_000,
            session_state: "open".into(),
            ..FoldState::default()
        };
        assert_eq!(
            priced(&card, "1gb", &increase, base + 2_000).storage_microusd,
            0
        );
        fold_events(&mut increase, &[transition(2, base + 1_500, 1_000_000_000)]).unwrap();
        assert_eq!(increase.metered_to_ms, base + 2_000);
        assert_eq!(
            priced(&card, "1gb", &increase, base + 2_000).storage_microusd,
            500
        );
        assert_eq!(
            priced(&card, "1gb", &increase, base + 2_000).session_storage_byte_milliseconds,
            500_000_000_000
        );
    }

    #[test]
    fn public_byte_milliseconds_reproduce_the_exact_storage_charge() {
        let card = RateCard::default();
        let state = FoldState {
            session_storage_byte_seconds: 9_007_199_254_740_992,
            session_storage_byte_millisecond_remainder: 731,
            storage_transition_ms: 10_000,
            session_storage_bytes: 17,
            upload_reserved_bytes: 5,
            ..FoldState::default()
        };
        let priced = priced(&card, "1gb", &state, 10_019);
        assert_eq!(
            priced.session_storage_byte_milliseconds,
            9_007_199_254_740_992_000i128 + 731 + 22 * 19
        );
        let denominator = 1_000_000_000i128 * i128::from(card.month_hours) * 3_600_000;
        assert_eq!(
            i128::from(priced.storage_microusd),
            priced.session_storage_byte_milliseconds
                * i128::from(card.session_storage_gb_month_microusd)
                / denominator
        );
    }
}
