//! The batched frontier bundle one stream cycle reads.
//!
//! A follow socket re-reads the same small set of point rows on every cycle: the
//! scope's deletion fence, the accepted frontier of each selected signal, and
//! the workspace's gap-change hint. Reading them one key at a time makes one
//! socket's idle cost proportional to its signal count and the process's idle
//! cost proportional to sockets times signals. Every one of them is a point read
//! of a key this process already knows, so they are one `BatchGetItem`.
//!
//! # Fail-closed decoding
//!
//! `DynamoDB` omits an absent row and a throttled row from `Responses` the same
//! way. Absence is therefore only evidence of absence once `UnprocessedKeys` has
//! drained, so a bundle is decoded only after the whole batch completed. A row
//! that was requested, returned and cannot be decoded is an error; it never
//! becomes a default frontier, which would silently widen a snapshot.

use std::collections::{BTreeMap, HashMap};

use aex_observation_domain::keys::{self, ScopeKey};
use aex_observation_domain::signal::Signal;
use aex_observation_query::plan::NormalizedQuery;
use aex_observation_store_dynamodb::expressions::{PK, SK};
use aex_observation_store_dynamodb::gap_hint::GapAppendCount;
use aex_wire::ids::WorkspaceId;
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::{AttributeValue, KeysAndAttributes};

use crate::reader::{BATCH_GET_MAX_KEYS, ReadError, drain_batch_get, primary_key_pair, timestamp};

/// The safe complete and retained frontiers of one query selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Frontier {
    /// Every selected signal is complete through this instant.
    pub accepted_at: Timestamp,
    /// Every selected signal is retained from this instant, which is `earliestReplay`.
    pub earliest_accepted_at: Timestamp,
    /// The authoritative deletion epoch bound into every cursor.
    pub deletion_epoch: u64,
    /// Whether the scope remains queryable at that epoch.
    pub deletion_state: ScopeDeletionState,
}

/// The query-visible state of the authoritative scope deletion fence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScopeDeletionState {
    /// Reads may proceed.
    Open,
    /// A deletion is fenced and still in progress.
    Deleting,
    /// Deletion has been proven complete (or the session head is gone).
    Deleted,
}

/// One signal's own completeness and retention facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalFrontier {
    /// Which signal the row belongs to.
    pub signal: Signal,
    /// The signal is complete through this instant.
    pub accepted_at: Timestamp,
    /// The signal is retained from this instant.
    pub earliest_accepted_at: Timestamp,
}

/// What the batched gap-change hint row says.
///
/// Absence and an unreadable value are the same answer — *unknown* — and only
/// [`GapChange::Counted`] can ever suppress a gap-history read. They are
/// separate variants because absence is the normal state of a workspace that has
/// never had a gap, while an unreadable row is a defect worth counting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GapChange {
    /// The workspace has published this many gap appends.
    Counted(GapAppendCount),
    /// The workspace has never published a hint, so it has no hint row.
    Unpublished,
    /// A row was returned at the hint key and is not a readable hint.
    Unreadable,
}

/// Every frontier fact one cycle read, in one provider request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontierBundle {
    /// The authoritative deletion epoch of the scope.
    pub deletion_epoch: u64,
    /// Whether the scope remains queryable at that epoch.
    pub deletion_state: ScopeDeletionState,
    /// One entry per selected signal that has an accepted frontier row.
    ///
    /// A selected signal nothing has ever been admitted for has no row and so no
    /// entry. That is absence, not a defaulted frontier: it constrains nothing,
    /// exactly as a per-signal point read that found nothing did.
    pub signals: Vec<SignalFrontier>,
    /// Whether gap history has been appended to since the last cycle read it.
    ///
    /// This is a scheduling fact, never a gap fact: it says only whether the
    /// gap ledger is worth reading. Gap state itself comes from the ledger.
    pub gap_change: GapChange,
}

impl FrontierBundle {
    /// Narrows every selected signal's facts into the one frontier a query pins.
    ///
    /// Completeness is the minimum of the selected frontiers, because a query is
    /// only complete where its least complete signal is. Replay safety is the
    /// maximum retained floor, because a replay is only safe where its most
    /// recently trimmed signal still holds data.
    #[must_use]
    pub fn combine(&self, query: &NormalizedQuery) -> Frontier {
        let mut combined = combine_signals(query, &self.signals);
        combined.deletion_epoch = self.deletion_epoch;
        combined.deletion_state = self.deletion_state;
        combined
    }
}

/// Which authority row one bundle key addresses.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum BundleRow {
    /// The scope's deletion fence: a workspace deletion row or a session head.
    DeletionFence,
    /// One selected signal's accepted frontier.
    SignalFrontier(Signal),
    /// The workspace's monotonic gap-change hint.
    ///
    /// It rides this batch rather than a read of its own precisely because the
    /// answer is almost always "unchanged": learning that must cost nothing, or
    /// the optimisation pays for itself in the round trip it was meant to save.
    GapChange,
}

/// The exact key of one bundle row, in the table that owns it.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RowKey {
    table: String,
    pk: String,
    sk: String,
}

/// The keys one frontier bundle reads.
#[derive(Clone, Debug)]
pub(crate) struct BundleRequest {
    rows: BTreeMap<BundleRow, RowKey>,
}

/// The rows one bundle found, addressed by the role that asked for them.
#[derive(Clone, Debug)]
pub(crate) struct Bundle {
    rows: BTreeMap<BundleRow, HashMap<String, AttributeValue>>,
}

impl BundleRequest {
    /// The bundle one scope and signal selection needs.
    ///
    /// The deletion fence lives in the session authority for a session scope and
    /// in the observation authority for a workspace scope, so a session bundle
    /// spans two tables. `BatchGetItem` takes both in one request.
    pub(crate) fn plan(
        scope: &ScopeKey,
        workspace: WorkspaceId,
        query: &NormalizedQuery,
        observation_table: &str,
        session_table: &str,
    ) -> Self {
        let fence = match scope {
            ScopeKey::Session { session, .. } => {
                let (pk, sk) = aex_session_dynamodb::stream_keys::head(*session);
                RowKey {
                    table: session_table.to_owned(),
                    pk,
                    sk: sk.to_owned(),
                }
            }
            ScopeKey::Workspace(_) => RowKey {
                table: observation_table.to_owned(),
                pk: keys::frontier_pk(scope),
                sk: keys::DELETION_SK.to_owned(),
            },
        };
        let mut rows = BTreeMap::from([
            (BundleRow::DeletionFence, fence),
            (
                BundleRow::GapChange,
                RowKey {
                    table: observation_table.to_owned(),
                    pk: keys::gap_hint_pk(workspace),
                    sk: keys::GAP_HINT_SK.to_owned(),
                },
            ),
        ]);
        for signal in query.signals.in_authority().iter() {
            rows.insert(
                BundleRow::SignalFrontier(signal),
                RowKey {
                    table: observation_table.to_owned(),
                    pk: keys::frontier_pk(scope),
                    sk: keys::frontier_sk(signal),
                },
            );
        }
        Self { rows }
    }

    /// Reads every row of the bundle in exactly one `BatchGetItem`.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Provider`] when the batch cannot be completed, when
    /// the provider returns a row nobody requested, and when it returns the same
    /// row twice.
    pub(crate) async fn read(
        &self,
        dynamodb: &aws_sdk_dynamodb::Client,
    ) -> Result<Bundle, ReadError> {
        let returned = drain_batch_get(
            dynamodb,
            self.request_items()?,
            "provider repeatedly returned unprocessed frontier keys",
        )
        .await?;
        let mut found = BTreeMap::new();
        for (table, item) in returned {
            let (pk, sk) = primary_key_pair(&item)?;
            let returned_key = RowKey { table, pk, sk };
            let Some((row, _)) = self
                .rows
                .iter()
                .find(|(_, requested)| **requested == returned_key)
            else {
                return Err(ReadError::Provider {
                    operation: "BatchGetItem",
                    reason: "the frontier bundle returned a row it did not request".to_owned(),
                });
            };
            if found.insert(*row, item).is_some() {
                return Err(ReadError::Provider {
                    operation: "BatchGetItem",
                    reason: "the frontier bundle returned one row twice".to_owned(),
                });
            }
        }
        Ok(Bundle { rows: found })
    }

    /// One `KeysAndAttributes` per addressed table.
    fn request_items(&self) -> Result<HashMap<String, KeysAndAttributes>, ReadError> {
        if self.rows.len() > BATCH_GET_MAX_KEYS {
            return Err(ReadError::Provider {
                operation: "BatchGetItem",
                reason: format!(
                    "a frontier bundle of {} keys exceeds one request",
                    self.rows.len()
                ),
            });
        }
        let mut grouped = BTreeMap::<&str, Vec<HashMap<String, AttributeValue>>>::new();
        for key in self.rows.values() {
            grouped
                .entry(key.table.as_str())
                .or_default()
                .push(HashMap::from([
                    (PK.to_owned(), AttributeValue::S(key.pk.clone())),
                    (SK.to_owned(), AttributeValue::S(key.sk.clone())),
                ]));
        }
        grouped
            .into_iter()
            .map(|(table, keys)| {
                // Consistency is declared per table request. A strongly read
                // observation table beside an eventually read session table
                // would weaken the deletion fence without saying so.
                let request = KeysAndAttributes::builder()
                    .set_keys(Some(keys))
                    .consistent_read(true)
                    .build()
                    .map_err(|error| ReadError::provider("BatchGetItem", error))?;
                Ok((table.to_owned(), request))
            })
            .collect()
    }
}

impl Bundle {
    /// Decodes the whole bundle, after every requested key was accounted for.
    ///
    /// # Errors
    ///
    /// Returns [`ReadError::Malformed`] when a returned row is not the shape its
    /// role requires.
    pub(crate) fn decode(
        &self,
        scope: &ScopeKey,
        workspace: WorkspaceId,
    ) -> Result<FrontierBundle, ReadError> {
        let fence = self.rows.get(&BundleRow::DeletionFence);
        let (deletion_epoch, deletion_state) = match scope {
            ScopeKey::Session { .. } => decode_session_fence(fence, workspace)?,
            ScopeKey::Workspace(_) => decode_workspace_fence(fence)?,
        };
        let signals = self
            .rows
            .iter()
            .filter_map(|(row, item)| match row {
                BundleRow::DeletionFence | BundleRow::GapChange => None,
                BundleRow::SignalFrontier(signal) => Some(decode_signal_frontier(*signal, item)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FrontierBundle {
            deletion_epoch,
            deletion_state,
            signals,
            gap_change: decode_gap_change(self.rows.get(&BundleRow::GapChange)),
        })
    }
}

/// Decodes the gap-change hint row, which can only ever schedule a read.
///
/// A hint that cannot be read is [`GapChange::Unreadable`] rather than an error,
/// because failing the bundle would let one corrupt optimisation row take down
/// every socket in the workspace. The reader treats it as unknown, reads gap
/// history, and counts the defect — the same outcome as no hint at all, plus a
/// number somebody can alarm on.
fn decode_gap_change(item: Option<&HashMap<String, AttributeValue>>) -> GapChange {
    // Absence here is proven absence: the batch drained its unprocessed keys
    // before anything was decoded, so a throttled hint row never arrives as one
    // that does not exist.
    let Some(item) = item else {
        return GapChange::Unpublished;
    };
    aex_observation_store_dynamodb::gap_hint::decode(item)
        .map_or(GapChange::Unreadable, GapChange::Counted)
}

/// Decodes the deletion fence a session head carries.
///
/// # Errors
///
/// Returns [`ReadError::Malformed`] when a present head is not a session head of
/// the asserted workspace or carries an unmodelled lifecycle.
pub(crate) fn decode_session_fence(
    item: Option<&HashMap<String, AttributeValue>>,
    workspace: WorkspaceId,
) -> Result<(u64, ScopeDeletionState), ReadError> {
    use aex_session_dynamodb::stream_keys::SessionReadState;

    // A session with no head at all has been purged. That is the fail-closed
    // answer, not a default: there is no epoch left to read.
    let Some(item) = item else {
        return Ok((0, ScopeDeletionState::Deleted));
    };
    let state = aex_session_dynamodb::stream_keys::session_read_state(item, workspace)
        .map_err(|attribute| ReadError::Malformed { attribute })?;
    let epoch = aex_session_dynamodb::stream_keys::session_deletion_epoch(item, workspace)
        .map_err(|attribute| ReadError::Malformed { attribute })?;
    Ok((
        epoch,
        match state {
            SessionReadState::Active => ScopeDeletionState::Open,
            SessionReadState::Deleting => ScopeDeletionState::Deleting,
            SessionReadState::Deleted => ScopeDeletionState::Deleted,
        },
    ))
}

/// Decodes the deletion fence a workspace scope carries.
///
/// # Errors
///
/// Returns [`ReadError::Malformed`] when a present deletion row has no epoch or
/// names a state outside the fence vocabulary.
pub(crate) fn decode_workspace_fence(
    item: Option<&HashMap<String, AttributeValue>>,
) -> Result<(u64, ScopeDeletionState), ReadError> {
    // A workspace that was never deleted has no deletion row. Epoch zero and an
    // open scope is what that fact means, not a substituted default.
    let Some(item) = item else {
        return Ok((0, ScopeDeletionState::Open));
    };
    let epoch = crate::reader::number(item, "deletionEpoch").ok_or(ReadError::Malformed {
        attribute: "deletionEpoch",
    })?;
    let state =
        crate::reader::string(item, "state").ok_or(ReadError::Malformed { attribute: "state" })?;
    let state = match state.as_str() {
        "none" => ScopeDeletionState::Open,
        "fencing" | "deleting" | "verifying" => ScopeDeletionState::Deleting,
        "complete" => ScopeDeletionState::Deleted,
        _ => return Err(ReadError::Malformed { attribute: "state" }),
    };
    Ok((epoch, state))
}

/// Decodes one returned signal-frontier row.
fn decode_signal_frontier(
    signal: Signal,
    item: &HashMap<String, AttributeValue>,
) -> Result<SignalFrontier, ReadError> {
    let accepted_at = timestamp(item, "acceptedAt").ok_or(ReadError::Malformed {
        attribute: "acceptedAt",
    })?;
    Ok(SignalFrontier {
        signal,
        accepted_at,
        // A frontier row written before the retention floor existed retains
        // everything it has accepted, which is the narrowest honest window.
        earliest_accepted_at: timestamp(item, "earliestAcceptedAt").unwrap_or(accepted_at),
    })
}

/// Combines per-signal completeness and retention facts for one query.
///
/// Native session events are synchronous authority rows rather than observation
/// frontier rows. For the requested half-open range they therefore contribute
/// its upper bound as the complete point and its lower bound as the retained
/// floor. Observation signals narrow those facts.
fn combine_signals(query: &NormalizedQuery, signals: &[SignalFrontier]) -> Frontier {
    let unconstrained = Frontier {
        accepted_at: query.time_lt,
        earliest_accepted_at: query.time_gte,
        deletion_epoch: 0,
        deletion_state: ScopeDeletionState::Open,
    };
    let mut combined = query
        .signals
        .contains(Signal::Events)
        .then_some(unconstrained);
    for frontier in signals {
        combined = Some(combined.map_or(
            Frontier {
                accepted_at: frontier.accepted_at,
                earliest_accepted_at: frontier.earliest_accepted_at,
                deletion_epoch: 0,
                deletion_state: ScopeDeletionState::Open,
            },
            |current| {
                Frontier {
                    accepted_at: current.accepted_at.min(frontier.accepted_at),
                    earliest_accepted_at: current
                        .earliest_accepted_at
                        .max(frontier.earliest_accepted_at),
                    ..current
                }
            },
        ));
    }
    combined.unwrap_or(unconstrained)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_observation_domain::keys::{self, ScopeKey};
    use aex_observation_domain::order::{Direction, OrderBy};
    use aex_observation_domain::signal::{Signal, SignalSet};
    use aex_observation_query::plan::{NormalizedQuery, ScopeAxis};
    use aex_wire::ids::{PrefixedId as _, SessionId, WorkspaceId};
    use aex_wire::types::Timestamp;
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{
        BundleRequest, BundleRow, Frontier, FrontierBundle, GapChange, ScopeDeletionState,
        SignalFrontier, combine_signals, decode_gap_change, decode_session_fence,
        decode_workspace_fence,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10]))
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(aex_wire::Uuid7::compose(1, [4; 10]))
    }

    /// The session scope inside the workspace every fixture is authorized for.
    fn session_scope() -> ScopeKey {
        ScopeKey::Session {
            workspace: workspace(),
            session: session(),
        }
    }

    fn query(signals: SignalSet) -> NormalizedQuery {
        NormalizedQuery {
            axis: ScopeAxis::Workspace,
            signals,
            predicate: None,
            time_gte: Timestamp::from_unix_millis(0).expect("bounded"),
            time_lt: Timestamp::from_unix_millis(1_000).expect("bounded"),
            order_by: OrderBy::Accepted,
            direction: Direction::Ascending,
            limit: 100,
            trace_id: None,
            metric_name: None,
        }
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("bounded")
    }

    fn key_count(items: &HashMap<String, aws_sdk_dynamodb::types::KeysAndAttributes>) -> usize {
        items.values().map(|request| request.keys().len()).sum()
    }

    fn signal_frontier(signal: Signal, accepted: i64, earliest: i64) -> SignalFrontier {
        SignalFrontier {
            signal,
            accepted_at: at(accepted),
            earliest_accepted_at: at(earliest),
        }
    }

    #[test]
    fn a_workspace_bundle_addresses_the_deletion_row_and_every_selected_signal() {
        let scope = ScopeKey::Workspace(workspace());
        let selection = SignalSet::from_signal(Signal::Logs).with(Signal::Spans);

        let request = BundleRequest::plan(
            &scope,
            workspace(),
            &query(selection),
            "observation-authority",
            "session-authority",
        );

        let items = request.request_items().expect("one table request");
        assert_eq!(
            items.keys().collect::<Vec<_>>(),
            vec!["observation-authority"],
            "a workspace bundle never touches the session authority"
        );
        assert_eq!(
            key_count(&items),
            4,
            "one fence row, two signal rows and the gap-change hint"
        );
        assert_eq!(items["observation-authority"].consistent_read(), Some(true));
    }

    #[test]
    fn a_session_bundle_spans_both_authorities_and_reads_both_strongly() {
        let scope = session_scope();
        let selection = SignalSet::from_signal(Signal::Logs)
            .with(Signal::Spans)
            .with(Signal::Metrics)
            .with(Signal::Traces);

        let request = BundleRequest::plan(
            &scope,
            workspace(),
            &query(selection),
            "observation-authority",
            "session-authority",
        );
        let items = request.request_items().expect("two table requests");

        assert_eq!(items.len(), 2, "one request covers both authorities");
        assert_eq!(items["session-authority"].keys().len(), 1);
        assert_eq!(
            items["observation-authority"].keys().len(),
            5,
            "four signal frontiers and the gap-change hint"
        );
        for request in items.values() {
            assert_eq!(
                request.consistent_read(),
                Some(true),
                "consistency is declared per table request, so both must declare it"
            );
        }
    }

    #[test]
    fn a_selection_with_no_authority_signal_still_reads_its_fence_and_hint() {
        let scope = session_scope();

        let request = BundleRequest::plan(
            &scope,
            workspace(),
            &query(SignalSet::from_signal(Signal::Events)),
            "observation-authority",
            "session-authority",
        );
        let items = request.request_items().expect("two table requests");

        assert_eq!(key_count(&items), 2);
        assert_eq!(
            items["session-authority"].keys().len(),
            1,
            "an empty key list is not a legal table request"
        );
        assert_eq!(
            items["observation-authority"].keys().len(),
            1,
            "a native-event selection still follows gap history"
        );
    }

    #[test]
    fn signal_order_does_not_change_the_combined_frontier() {
        let selection = SignalSet::from_signal(Signal::Logs)
            .with(Signal::Spans)
            .with(Signal::Metrics);
        let query = query(selection);
        let signals = vec![
            signal_frontier(Signal::Logs, 800, 100),
            signal_frontier(Signal::Spans, 600, 300),
            signal_frontier(Signal::Metrics, 900, 200),
        ];
        let expected = Frontier {
            accepted_at: at(600),
            earliest_accepted_at: at(300),
            deletion_epoch: 0,
            deletion_state: ScopeDeletionState::Open,
        };

        for rotation in 0..signals.len() {
            let mut rotated = signals.clone();
            rotated.rotate_left(rotation);
            assert_eq!(combine_signals(&query, &rotated), expected, "{rotation}");
            rotated.reverse();
            assert_eq!(combine_signals(&query, &rotated), expected, "{rotation}");
        }
    }

    #[test]
    fn events_seed_a_complete_frontier_for_the_requested_range() {
        let query = query(SignalSet::from_signal(Signal::Events));

        assert_eq!(
            combine_signals(&query, &[]),
            Frontier {
                accepted_at: query.time_lt,
                earliest_accepted_at: query.time_gte,
                deletion_epoch: 0,
                deletion_state: ScopeDeletionState::Open,
            }
        );
    }

    #[test]
    fn mixed_frontiers_use_the_minimum_complete_point_and_maximum_retained_floor() {
        let query = query(SignalSet::all());
        let complete_later = signal_frontier(Signal::Logs, 800, 100);
        let retained_later = signal_frontier(Signal::Spans, 600, 300);

        assert_eq!(
            combine_signals(&query, &[complete_later, retained_later]),
            Frontier {
                accepted_at: retained_later.accepted_at,
                earliest_accepted_at: retained_later.earliest_accepted_at,
                deletion_epoch: 0,
                deletion_state: ScopeDeletionState::Open,
            }
        );
    }

    #[test]
    fn the_combined_frontier_carries_the_bundle_fence_rather_than_a_signal_default() {
        let bundle = FrontierBundle {
            deletion_epoch: 9,
            deletion_state: ScopeDeletionState::Deleting,
            signals: vec![signal_frontier(Signal::Logs, 700, 50)],
            gap_change: GapChange::Unpublished,
        };

        let combined = bundle.combine(&query(SignalSet::from_signal(Signal::Logs)));

        assert_eq!(combined.deletion_epoch, 9);
        assert_eq!(combined.deletion_state, ScopeDeletionState::Deleting);
        assert_eq!(combined.accepted_at, at(700));
    }

    #[test]
    fn an_absent_fence_row_keeps_the_meaning_its_authority_gives_absence() {
        assert_eq!(
            decode_workspace_fence(None).expect("a never-deleted workspace is open"),
            (0, ScopeDeletionState::Open)
        );
        assert_eq!(
            decode_session_fence(None, workspace()).expect("a missing head is a purged session"),
            (0, ScopeDeletionState::Deleted)
        );
    }

    #[test]
    fn a_malformed_fence_row_is_refused_rather_than_defaulted() {
        let no_epoch = HashMap::from([("state".to_owned(), AttributeValue::S("none".to_owned()))]);
        assert!(matches!(
            decode_workspace_fence(Some(&no_epoch)),
            Err(crate::reader::ReadError::Malformed {
                attribute: "deletionEpoch"
            })
        ));

        let unknown_state = HashMap::from([
            (
                "deletionEpoch".to_owned(),
                AttributeValue::N("3".to_owned()),
            ),
            (
                "state".to_owned(),
                AttributeValue::S("half-deleted".to_owned()),
            ),
        ]);
        assert!(matches!(
            decode_workspace_fence(Some(&unknown_state)),
            Err(crate::reader::ReadError::Malformed { attribute: "state" })
        ));

        let foreign_head = HashMap::from([
            (
                "itemType".to_owned(),
                AttributeValue::S("session_head".to_owned()),
            ),
            (
                "workspaceId".to_owned(),
                AttributeValue::S(
                    WorkspaceId::from_uuid7(aex_wire::Uuid7::compose(9, [9; 10])).to_string(),
                ),
            ),
            (
                "lifecycle".to_owned(),
                AttributeValue::S("active".to_owned()),
            ),
        ]);
        assert!(matches!(
            decode_session_fence(Some(&foreign_head), workspace()),
            Err(crate::reader::ReadError::Malformed {
                attribute: "workspaceId"
            })
        ));
    }

    #[test]
    fn the_hint_row_of_the_bundle_is_the_row_the_gap_writers_publish() {
        let scope = session_scope();

        let request = BundleRequest::plan(
            &scope,
            workspace(),
            &query(SignalSet::from_signal(Signal::Logs)),
            "observation-authority",
            "session-authority",
        );

        let hint = request.rows.get(&BundleRow::GapChange).expect("planned");
        assert_eq!(hint.table, "observation-authority");
        assert_eq!(hint.pk, keys::gap_hint_pk(workspace()));
        assert_eq!(hint.sk, keys::GAP_HINT_SK);
        assert_ne!(
            hint.pk,
            keys::gap_pk(&scope),
            "the hint may not sit in the partition a session gap query decodes whole"
        );
    }

    #[test]
    fn an_absent_hint_row_is_the_workspaces_own_answer_and_an_unreadable_one_is_not() {
        assert_eq!(decode_gap_change(None), GapChange::Unpublished);

        let published = HashMap::from([
            (
                "itemType".to_owned(),
                AttributeValue::S("gap_change_hint".to_owned()),
            ),
            ("gapAppends".to_owned(), AttributeValue::N("4".to_owned())),
        ]);
        assert!(matches!(
            decode_gap_change(Some(&published)),
            GapChange::Counted(count) if count.get() == 4
        ));

        let foreign = HashMap::from([(
            "itemType".to_owned(),
            AttributeValue::S("telemetry_gap".to_owned()),
        )]);
        assert_eq!(
            decode_gap_change(Some(&foreign)),
            GapChange::Unreadable,
            "a row that is not a hint never decodes into a count"
        );
        assert_eq!(
            decode_gap_change(Some(&HashMap::new())),
            GapChange::Unreadable
        );
    }

    #[test]
    fn a_signal_frontier_without_an_accepted_point_is_refused() {
        let scope = ScopeKey::Workspace(workspace());
        let bundle = super::Bundle {
            rows: std::collections::BTreeMap::from([(
                BundleRow::SignalFrontier(Signal::Logs),
                HashMap::from([(
                    "pk".to_owned(),
                    AttributeValue::S(keys::frontier_pk(&scope)),
                )]),
            )]),
        };

        assert!(matches!(
            bundle.decode(&scope, workspace()),
            Err(crate::reader::ReadError::Malformed {
                attribute: "acceptedAt"
            })
        ));
    }
}
