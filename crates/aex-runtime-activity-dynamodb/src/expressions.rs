//! The verbatim `runtime-activity` condition and update expressions.
//!
//! Every lifecycle write is conditional on the **exact** generation fence and
//! revision. That is what makes a late reply from a superseded provider call
//! harmless: it loses its condition and is discarded, rather than reviving a
//! generation the system has already moved past.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_session_dynamodb::attr::{n, n_i64, s, stamp};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{IMMUTABLE, key};
use aex_wire::ids::{GenerationId, SessionId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::builders::{PutBuilder, UpdateBuilder};
use aws_sdk_dynamodb::types::{Put, Update};

use crate::codec::{self, GenerationRow, LifecycleIntent, LifecycleReceipt};
use crate::keys;

/// Writes a generation head that must not already exist.
///
/// # Errors
///
/// [`StoreError::Invalid`] when the row could not be encoded.
pub fn create_generation(table: &str, row: &GenerationRow) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_generation(row).map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Moves a generation under its exact fence and revision.
///
/// A terminal target removes the due index attributes in the same write, so the
/// reaper's ordered scan can never return a generation that is already gone.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn transition(
    table: &str,
    row: &GenerationRow,
    from_state: GenerationState,
    from_fence: Fence,
    from_revision: Revision,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let target = keys::head(row.session, row.generation);
    let evaluable = keys::is_evaluable(row.state);
    let update = if evaluable {
        "SET #state = :toState, fence = :toFence, revision = :toRevision, \
         nextEvaluateAt = :nextEvaluateAt, updatedAt = :now, \
         rtDuePk = :duePk, rtDueSk = :dueSk"
    } else {
        "SET #state = :toState, fence = :toFence, revision = :toRevision, \
         updatedAt = :now REMOVE rtDuePk, rtDueSk"
    };
    let mut builder = Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(
            "attribute_exists(pk) AND #state = :fromState AND fence = :fromFence \
             AND revision = :fromRevision",
        )
        .update_expression(update)
        .expression_attribute_names("#state", "state")
        .expression_attribute_values(":fromState", s(keys::state_str(from_state)))
        .expression_attribute_values(":toState", s(keys::state_str(row.state)))
        .expression_attribute_values(":fromFence", n(from_fence.0))
        .expression_attribute_values(":toFence", n(row.fence.0))
        .expression_attribute_values(":fromRevision", n(from_revision.value()))
        .expression_attribute_values(":toRevision", n(row.revision.value()))
        .expression_attribute_values(":now", stamp(now));
    if evaluable {
        builder = builder
            .expression_attribute_values(":nextEvaluateAt", stamp(row.next_evaluate_at))
            .expression_attribute_values(":duePk", s(keys::due_partition(row.generation)))
            .expression_attribute_values(
                ":dueSk",
                s(keys::due_sort(row.next_evaluate_at, row.generation)),
            );
    }
    Ok(builder)
}

/// Advances only the evaluation time, without touching the fence.
///
/// Rescheduling is not a lifecycle change: advancing the fence here would
/// invalidate an in-flight lifecycle call that is still legitimately outstanding.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn reschedule(
    table: &str,
    session: SessionId,
    generation: GenerationId,
    fence: Fence,
    revision: Revision,
    next_evaluate_at: Timestamp,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let target = keys::head(session, generation);
    Ok(Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression("fence = :fence AND revision = :revision")
        .update_expression(
            "SET nextEvaluateAt = :nextEvaluateAt, rtDueSk = :dueSk, \
             revision = :nextRevision, updatedAt = :now",
        )
        .expression_attribute_values(":fence", n(fence.0))
        .expression_attribute_values(":revision", n(revision.value()))
        .expression_attribute_values(":nextRevision", n(revision.next().value()))
        .expression_attribute_values(":nextEvaluateAt", stamp(next_evaluate_at))
        .expression_attribute_values(":dueSk", s(keys::due_sort(next_evaluate_at, generation)))
        .expression_attribute_values(":now", stamp(now)))
}

/// Records a lifecycle intent, which must not already exist.
///
/// # Errors
///
/// [`StoreError`] when the intent identity could not enter a key.
pub fn record_intent(table: &str, intent: &LifecycleIntent) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_intent(intent).map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Settles a lifecycle intent with an immutable receipt.
///
/// The receipt is written with `attribute_not_exists`, so a second settlement of
/// the same intent loses rather than overwriting lifecycle evidence a usage fact
/// already references.
///
/// # Errors
///
/// As [`record_intent`].
pub fn settle_intent(table: &str, receipt: &LifecycleReceipt) -> Result<PutBuilder, StoreError> {
    let item = codec::encode_receipt(receipt).map_err(|error| StoreError::Invalid {
        detail: error.to_string(),
    })?;
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(item))
        .condition_expression(IMMUTABLE))
}

/// Records one true-idle probe, with the only TTL on this table.
///
/// # Errors
///
/// [`StoreError`] when the row could not be encoded.
pub fn record_probe(table: &str, probe: &codec::IdleProbe) -> Result<PutBuilder, StoreError> {
    Ok(Put::builder()
        .table_name(table)
        .set_item(Some(codec::encode_probe(probe)))
        // A probe at the same instant is the same probe.
        .condition_expression(IMMUTABLE))
}

/// Points a session at a generation, under the fence the caller observed.
///
/// # Errors
///
/// [`StoreError`] when the update could not be built.
pub fn point_current(
    table: &str,
    session: SessionId,
    generation: GenerationId,
    fence: Fence,
    from_revision: Option<Revision>,
    now: Timestamp,
) -> Result<UpdateBuilder, StoreError> {
    let target = keys::current(session);
    let condition = if from_revision.is_some() {
        "revision = :fromRevision"
    } else {
        "attribute_not_exists(pk)"
    };
    let next = from_revision.map_or(Revision::ZERO, Revision::next);
    let mut builder = Update::builder()
        .table_name(table)
        .set_key(Some(key(&target.pk, &target.sk)))
        .condition_expression(condition)
        .update_expression(
            "SET #itemType = :itemType, sessionId = :session, generationId = :generation, \
             fence = :fence, revision = :nextRevision, updatedAt = :now",
        )
        .expression_attribute_names("#itemType", "itemType")
        .expression_attribute_values(":itemType", s(codec::CURRENT_GENERATION))
        .expression_attribute_values(":session", s(session.to_string()))
        .expression_attribute_values(":generation", s(generation.to_string()))
        .expression_attribute_values(":fence", n(fence.0))
        .expression_attribute_values(":nextRevision", n(next.value()))
        .expression_attribute_values(":now", stamp(now));
    if let Some(revision) = from_revision {
        builder = builder.expression_attribute_values(":fromRevision", n(revision.value()));
    }
    Ok(builder)
}

/// The TTL value an idle probe carries.
#[must_use]
pub fn probe_ttl(observed_at: Timestamp) -> aws_sdk_dynamodb::types::AttributeValue {
    n_i64(observed_at.unix_millis().div_euclid(1_000) + keys::PROBE_TTL_SECONDS)
}

#[cfg(test)]
mod tests {
    use aex_hands_protocol::rpc::Fence;
    use aex_internal_contracts::SchemaVersion;
    use aex_runtime_control::generation::{
        GenerationState, HandsGeneration, ImageIdentifier, ImagePin, ImageVersion, LimitsRevision,
        NetworkPolicy, Revision, guest_root,
    };
    use aex_wire::ids::{
        ContentHash, GenerationId, OrganizationId, PrefixedId, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::{ComputeSize, Timestamp};

    use super::{reschedule, transition};
    use crate::codec::GenerationRow;

    const TABLE: &str = "dev-eu-west-1-runtime-activity";

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10]))
    }

    fn row(state: GenerationState) -> GenerationRow {
        let workspace = WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let definition = HandsGeneration {
            generation: generation(),
            session: session(),
            workspace,
            organization,
            size: ComputeSize::ALL[0],
            image: ImagePin {
                identifier: ImageIdentifier("hands:test".to_owned()),
                version: ImageVersion("1".to_owned()),
                artifact_digest: ContentHash::from_bytes([7; 32]),
                capabilities: Vec::new(),
            },
            network: NetworkPolicy::None,
            protocol_version: SchemaVersion::V1,
            limits_revision: LimitsRevision(1),
            root: guest_root(),
        };
        GenerationRow {
            definition,
            session: session(),
            workspace,
            organization,
            generation: generation(),
            size: ComputeSize::ALL[0],
            state,
            fence: Fence(3),
            revision: Revision::new(5),
            provider_vm_id: None,
            open_operations: 0,
            last_busy_at: now(),
            idle_since: None,
            keepalive_lease_until: None,
            provider_lifetime_expires_at: None,
            microvm: None,
            lifetime: None,
            accounted_from: now(),
            open_intent: None,
            suspended_at: None,
            snapshot_ordinal: 0,
            snapshot_bytes: 445_000_000,
            suspend_lock_expires_at: None,
            keepalive_lease: None,
            transport_mode: None,
            next_evaluate_at: now(),
            updated_at: now(),
        }
    }

    fn built(builder: aws_sdk_dynamodb::types::builders::UpdateBuilder) -> (String, String) {
        let built = builder.build().expect("a complete update");
        (
            built
                .condition_expression()
                .expect("conditional")
                .to_owned(),
            built.update_expression().to_owned(),
        )
    }

    #[test]
    fn every_lifecycle_write_is_fenced_by_the_exact_generation() {
        let (condition, _) = built(
            transition(
                TABLE,
                &row(GenerationState::Running),
                GenerationState::Launching,
                Fence(2),
                Revision::new(4),
                now(),
            )
            .expect("builds"),
        );
        assert!(condition.contains("fence = :fromFence"));
        assert!(condition.contains("revision = :fromRevision"));
        assert!(condition.contains("#state = :fromState"));
    }

    #[test]
    fn a_terminal_transition_leaves_the_due_index_in_the_same_write() {
        let (_, update) = built(
            transition(
                TABLE,
                &row(GenerationState::Terminated),
                GenerationState::Terminating,
                Fence(2),
                Revision::new(4),
                now(),
            )
            .expect("builds"),
        );
        assert!(
            update.contains("REMOVE rtDuePk, rtDueSk"),
            "the reaper must never see a generation that is already gone: {update}"
        );
    }

    #[test]
    fn a_non_terminal_transition_keeps_the_generation_evaluable() {
        let (_, update) = built(
            transition(
                TABLE,
                &row(GenerationState::Running),
                GenerationState::Launching,
                Fence(2),
                Revision::new(4),
                now(),
            )
            .expect("builds"),
        );
        assert!(update.contains("rtDuePk = :duePk"));
        assert!(!update.contains("REMOVE"));
    }

    #[test]
    fn rescheduling_never_advances_the_fence() {
        let (condition, update) = built(
            reschedule(
                TABLE,
                session(),
                generation(),
                Fence(3),
                Revision::new(5),
                now(),
                now(),
            )
            .expect("builds"),
        );
        assert!(condition.contains("fence = :fence"));
        assert!(
            !update.contains("fence ="),
            "rescheduling an evaluation is not a lifecycle change: {update}"
        );
    }
}
