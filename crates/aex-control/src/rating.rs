//! Rated usage on the two-rate card (ARCHITECTURE-v1 D4).
//!
//! The brain's journal is the billing record. Compute time is a pure fold over a session's
//! event log: a turn runs from `turn.started` to `turn.completed`/`turn.failed`; those
//! intervals — and nothing else — are billed at the shape's baseline rate (the pre-suspend
//! idle window is absorbed, per D4). Storage is the brain-reported byte meters (session/v1
//! `StorageInfo`) integrated over wall time, piecewise-constant between meter readings.
//! Everything is integer arithmetic in micro-USD; division floors, in the customer's favor.

use serde_json::Value;

/// The active prices, in micro-USD. Defaults are the D4 card; every field can be overridden by
/// `AEX_RATE_*` env (the private `platform` repo owns the deployed pricing config).
#[derive(Debug, Clone)]
pub struct RateCard {
    pub vcpu_hour_microusd: i64,
    pub gb_hour_microusd: i64,
    pub suspended_gb_month_microusd: i64,
    pub workspace_gb_month_microusd: i64,
    pub web_search_query_microusd: i64,
    pub month_hours: i64,
}

impl Default for RateCard {
    fn default() -> Self {
        RateCard {
            vcpu_hour_microusd: 190_000,
            gb_hour_microusd: 25_000,
            suspended_gb_month_microusd: 100_000,
            workspace_gb_month_microusd: 30_000,
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
        Ok(RateCard {
            vcpu_hour_microusd: rate("AEX_RATE_VCPU_HOUR_MICROUSD", d.vcpu_hour_microusd)?,
            gb_hour_microusd: rate("AEX_RATE_GB_HOUR_MICROUSD", d.gb_hour_microusd)?,
            suspended_gb_month_microusd: rate(
                "AEX_RATE_SUSPENDED_GB_MONTH_MICROUSD",
                d.suspended_gb_month_microusd,
            )?,
            workspace_gb_month_microusd: rate(
                "AEX_RATE_WORKSPACE_GB_MONTH_MICROUSD",
                d.workspace_gb_month_microusd,
            )?,
            web_search_query_microusd: rate(
                "AEX_RATE_WEB_SEARCH_QUERY_MICROUSD",
                d.web_search_query_microusd,
            )?,
            month_hours: rate("AEX_RATE_MONTH_HOURS", d.month_hours)?,
        })
    }

    /// Micro-USD per hour running on a shape: baseline vCPU = memory/2, bursts free.
    /// 1gb -> 120,000 ($0.12/h), 2gb -> 240,000, 4gb -> 480,000, 8gb -> 960,000.
    pub fn hourly_microusd(&self, shape: &str) -> i64 {
        let gb = shape_gb(shape);
        self.vcpu_hour_microusd * gb / 2 + self.gb_hour_microusd * gb
    }

    /// The wire form (`contracts/control/v1` `RateCard`). The server responds in JSON validated
    /// against the schema by the e2e test; the generated Rust types are for consumers (SDK).
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "object": "rate_card",
            "vcpu_hour_microusd": self.vcpu_hour_microusd,
            "gb_hour_microusd": self.gb_hour_microusd,
            "suspended_gb_month_microusd": self.suspended_gb_month_microusd,
            "workspace_gb_month_microusd": self.workspace_gb_month_microusd,
            "web_search_query_microusd": self.web_search_query_microusd,
            "month_hours": self.month_hours,
        })
    }
}

/// Baseline memory GB of a session/v1 `HandShape`. Unknown shapes rate as 1gb — the smallest
/// bill we could be wrong by, and the contract's default shape.
pub fn shape_gb(shape: &str) -> i64 {
    match shape {
        "2gb" => 2,
        "4gb" => 4,
        "8gb" => 8,
        _ => 1,
    }
}

/// A session's meter state: the fold position plus the integrals. Persisted per session and
/// advanced by sweeps; deterministic given the event log and the meter readings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FoldState {
    /// Highest event seq folded. Replay resumes at `after=folded_seq`.
    pub folded_seq: i64,
    /// Closed turn intervals, exact from event timestamps.
    pub running_ms: i64,
    /// `at` of an unclosed `turn.started`, if a turn is open.
    pub turn_open_ms: Option<i64>,
    /// Storage integrals in byte-seconds.
    pub suspended_byte_seconds: i64,
    pub workspace_byte_seconds: i64,
    pub artifact_byte_seconds: i64,
    /// Successful managed `web_search` results committed to the journal.
    pub web_search_queries: i64,
    /// Storage integrated up to this wall time.
    pub metered_to_ms: i64,
    /// Latest meter readings (brain-reported bytes) and states.
    pub workspace_bytes: i64,
    pub suspended_bytes: i64,
    pub artifact_bytes: i64,
    pub hand_state: String,
    pub session_state: String,
}

/// Fold replayed events into the state: seq high-water, turn intervals, state transitions.
/// Live-only events (deltas, tool.output) never appear in replay; seq gaps are normal.
pub fn fold_events(state: &mut FoldState, events: &[Value]) {
    for ev in events {
        if let Some(seq) = ev["seq"].as_i64() {
            state.folded_seq = state.folded_seq.max(seq);
        }
        let at = ev["at"].as_str().and_then(crate::parse_rfc3339_ms);
        match ev["type"].as_str().unwrap_or("") {
            "turn.started" => state.turn_open_ms = at.or(state.turn_open_ms),
            "turn.completed" | "turn.failed" => {
                if let (Some(t0), Some(t1)) = (state.turn_open_ms.take(), at) {
                    state.running_ms += (t1 - t0).max(0);
                }
            }
            "session.updated" => {
                if let Some(s) = ev["state"].as_str() {
                    state.session_state = s.into();
                }
                if let Some(h) = ev["hand"]["state"].as_str() {
                    state.hand_state = h.into();
                }
            }
            "tool.result"
                if ev["name"].as_str() == Some("web_search")
                    && ev["outcome"].as_str() == Some("completed") =>
            {
                state.web_search_queries = state.web_search_queries.saturating_add(1);
            }
            _ => {}
        }
    }
}

/// Integrate the current byte meters from `metered_to_ms` to `now_ms`. Piecewise-constant:
/// call this BEFORE applying a fresh meter reading, so the elapsed window is charged at the
/// bytes that were actually reported for it.
pub fn accrue_storage(state: &mut FoldState, now_ms: i64) {
    let dt_ms = (now_ms - state.metered_to_ms).max(0);
    if state.metered_to_ms == 0 {
        // First sweep: nothing was metered before; start the clock, charge nothing.
        state.metered_to_ms = now_ms;
        return;
    }
    let secs = |bytes: i64| -> i64 {
        i64::try_from(i128::from(bytes) * i128::from(dt_ms) / 1000).unwrap_or(i64::MAX)
    };
    state.suspended_byte_seconds += secs(state.suspended_bytes);
    state.workspace_byte_seconds += secs(state.workspace_bytes);
    state.artifact_byte_seconds += secs(state.artifact_bytes);
    state.metered_to_ms = now_ms;
}

/// Apply a fresh brain snapshot of the session document (meters and states).
pub fn apply_snapshot(state: &mut FoldState, session: &Value) {
    if let Some(s) = session["state"].as_str() {
        state.session_state = s.into();
    }
    if let Some(h) = session["hand"]["state"].as_str() {
        state.hand_state = h.into();
    }
    let storage = &session["storage"];
    if storage.is_object() {
        state.workspace_bytes = storage["workspace_bytes"].as_i64().unwrap_or(0);
        state.suspended_bytes = storage["suspended_bytes"].as_i64().unwrap_or(0);
        state.artifact_bytes = storage["artifact_bytes"].as_i64().unwrap_or(0);
    }
}

/// A priced line, in micro-USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Priced {
    pub running_ms: i64,
    pub compute_microusd: i64,
    pub storage_microusd: i64,
    pub web_search_microusd: i64,
    pub total_microusd: i64,
}

/// Price the state on the card. While the brain reports the session `active` and a turn is
/// open, the open turn is included up to `now_ms` as an estimate — the closing event replaces
/// it with the exact interval, so the ledger converges; a long turn cannot run unbilled.
pub fn price(card: &RateCard, shape: &str, state: &FoldState, now_ms: i64) -> Priced {
    let mut running_ms = state.running_ms;
    if state.session_state == "active"
        && let Some(t0) = state.turn_open_ms
    {
        running_ms += (now_ms - t0).max(0);
    }
    let compute = i128::from(running_ms) * i128::from(card.hourly_microusd(shape)) / 3_600_000;
    let month_byte_seconds = 1_000_000_000i128 * i128::from(card.month_hours) * 3600;
    let storage = (i128::from(state.suspended_byte_seconds)
        * i128::from(card.suspended_gb_month_microusd)
        + i128::from(state.workspace_byte_seconds + state.artifact_byte_seconds)
            * i128::from(card.workspace_gb_month_microusd))
        / month_byte_seconds;
    let compute = i64::try_from(compute).unwrap_or(i64::MAX);
    let storage = i64::try_from(storage).unwrap_or(i64::MAX);
    let web_search = state
        .web_search_queries
        .saturating_mul(card.web_search_query_microusd);
    Priced {
        running_ms,
        compute_microusd: compute,
        storage_microusd: storage,
        web_search_microusd: web_search,
        total_microusd: compute.saturating_add(storage).saturating_add(web_search),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(seq: i64, t: &str, at: &str) -> Value {
        json!({"type": t, "seq": seq, "at": at, "session_id": "ses_x", "turn_id": "trn_x"})
    }

    #[test]
    fn card_matches_d4() {
        let c = RateCard::default();
        assert_eq!(c.hourly_microusd("1gb"), 120_000); // $0.12/h
        assert_eq!(c.hourly_microusd("2gb"), 240_000);
        assert_eq!(c.hourly_microusd("4gb"), 480_000);
        assert_eq!(c.hourly_microusd("8gb"), 960_000);
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
        );
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
            suspended_byte_seconds: 1_932_735_283_200, // 1 GiB * 1800 s
            workspace_byte_seconds: 48_129_638_400,    // 17,825,792 B * 2700 s
            artifact_byte_seconds: 552_960_000,        // 204,800 B * 2700 s
            web_search_queries: 2,
            session_state: "idle".into(),
            ..FoldState::default()
        };
        let p = price(&card, "1gb", &s, 0);
        assert_eq!(p.compute_microusd, 10_733);
        assert_eq!(p.storage_microusd, 74);
        assert_eq!(p.web_search_microusd, 6_000);
        assert_eq!(p.total_microusd, 16_807);
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
        );
        assert_eq!(s.web_search_queries, 1);
        let p = price(&RateCard::default(), "1gb", &s, 0);
        assert_eq!(p.web_search_microusd, 3_000);
        assert_eq!(p.total_microusd, 3_000);
    }

    #[test]
    fn open_turn_is_estimated_only_while_active() {
        let card = RateCard::default();
        let mut s = FoldState::default();
        fold_events(&mut s, &[ev(10, "turn.started", "2026-08-18T09:40:00Z")]);
        let t0 = crate::parse_rfc3339_ms("2026-08-18T09:40:00Z").unwrap();
        s.session_state = "active".into();
        assert_eq!(price(&card, "1gb", &s, t0 + 30_000).running_ms, 30_000);
        // Not active (the closing event just has not replayed yet): no estimate.
        s.session_state = "idle".into();
        assert_eq!(price(&card, "1gb", &s, t0 + 30_000).running_ms, 0);
    }

    #[test]
    fn storage_integrates_piecewise_constant() {
        let mut s = FoldState {
            workspace_bytes: 1_000_000_000, // 1 GB
            ..FoldState::default()
        };
        accrue_storage(&mut s, 1_000); // first sweep only starts the clock
        assert_eq!(s.workspace_byte_seconds, 0);
        accrue_storage(&mut s, 3_600_000 + 1_000); // one hour at 1 GB
        assert_eq!(s.workspace_byte_seconds, 3_600_000_000_000);
        // One GB-hour at $0.03/GB-month(730h) = 30000/730 = 41 microusd.
        let p = price(&RateCard::default(), "1gb", &s, 0);
        assert_eq!(p.storage_microusd, 41);
    }

    #[test]
    fn snapshot_applies_meters_and_states() {
        let mut s = FoldState::default();
        apply_snapshot(
            &mut s,
            &json!({"state": "idle", "hand": {"state": "suspended"},
                    "storage": {"workspace_bytes": 5, "suspended_bytes": 7, "artifact_bytes": 9}}),
        );
        assert_eq!(
            (s.workspace_bytes, s.suspended_bytes, s.artifact_bytes),
            (5, 7, 9)
        );
        assert_eq!(s.hand_state, "suspended");
        assert_eq!(s.session_state, "idle");
    }
}
