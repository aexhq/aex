//! Snapshot equivalence and hostile-byte tests.

use aex_brain_domain::fold::{apply, fold};
use aex_brain_domain::ids::{AgentId, AgentKey, SessionId};
use aex_brain_domain::journal::JournalEntry;
use aex_brain_domain::snapshot::{
    FOLD_SNAPSHOT_SCHEMA, FoldSnapshotArtifact, FoldSnapshotError, FoldSnapshotPointer,
    JournalPoint, MAX_FOLD_SNAPSHOT_BYTES, SnapshotReplay,
};
use aex_brain_test_support::journal_gen::{agent, arb_history};
use aex_wire::ids::ContentHash as BodyDigest;
use proptest::prelude::*;
use uuid::Uuid;

fn key() -> AgentKey {
    AgentKey::new(SessionId(Uuid::from_u128(7)), AgentId(Uuid::from_u128(9)))
}

fn artifact() -> FoldSnapshotArtifact {
    let history = aex_brain_test_support::journal_gen::HistoryBuilder::new()
        .push(aex_brain_test_support::journal_gen::started(
            aex_brain_test_support::journal_gen::grant(100),
        ))
        .push(aex_brain_test_support::journal_gen::user_text("hello"))
        .build();
    replay(&history)
}

fn replay(history: &[JournalEntry]) -> FoldSnapshotArtifact {
    let mut replay = SnapshotReplay::from_sequence_zero();
    for entry in history {
        replay.apply(entry).expect("fixture folds");
    }
    let tail = history.last().expect("snapshot history is non-empty");
    replay
        .finish_exact(
            key(),
            JournalPoint {
                seq: tail.envelope.seq,
                hash: tail.envelope.content_hash,
            },
        )
        .expect("fixture snapshots")
}

fn recanonicalize(
    artifact: &FoldSnapshotArtifact,
    mutate: impl FnOnce(&mut serde_json::Value),
) -> (FoldSnapshotPointer, Vec<u8>) {
    let mut value: serde_json::Value =
        serde_json::from_slice(artifact.body()).expect("snapshot is JSON");
    mutate(&mut value);
    let body = aex_wire::to_jcs_bytes(&value).expect("mutated JSON canonicalizes");
    let mut pointer = artifact.pointer().clone();
    pointer.body_bytes = u64::try_from(body.len()).expect("fixture fits");
    pointer.body_digest = BodyDigest::of(&body);
    (pointer, body)
}

proptest! {
    /// Every valid cut produces the same state as replay from sequence zero.
    #[test]
    fn snapshot_plus_suffix_equals_full_fold(
        history in arb_history(agent(44)),
        cut_seed in any::<usize>(),
    ) {
        prop_assume!(!history.is_empty());
        let cut = cut_seed % history.len() + 1;
        let expected = fold(&history).expect("the admissible generator folds");
        let artifact = replay(&history[..cut]);
        let mut restored = artifact.pointer()
            .verify(key(), artifact.body(), artifact.body().len())
            .expect("the captured body verifies")
            .state;
        for entry in &history[cut..] {
            apply(&mut restored, entry).expect("the suffix applies");
        }
        prop_assert_eq!(restored, expected);
    }
}

#[test]
fn body_corruption_and_substitution_are_refused_before_state_use() {
    let artifact = artifact();
    let mut corrupt = artifact.body().to_vec();
    corrupt[0] ^= 1;
    assert_eq!(
        artifact.pointer().verify(key(), &corrupt, corrupt.len()),
        Err(FoldSnapshotError::BodyDigestMismatch)
    );
}

#[test]
fn schema_agent_tail_hash_and_config_are_independently_bound() {
    let base = artifact();

    let mut wrong_schema = base.pointer().clone();
    wrong_schema.schema = "aex.brain.fold.v0".to_owned();
    assert!(matches!(
        wrong_schema.verify(key(), base.body(), base.body().len()),
        Err(FoldSnapshotError::Schema { .. })
    ));

    let other = AgentKey::new(key().session, AgentId(Uuid::from_u128(10)));
    assert_eq!(
        base.pointer().verify(other, base.body(), base.body().len()),
        Err(FoldSnapshotError::AgentMismatch)
    );

    let (wrong_tail, wrong_tail_body) =
        recanonicalize(&base, |value| value["state"]["tail"] = 99.into());
    assert_eq!(
        wrong_tail.verify(key(), &wrong_tail_body, wrong_tail_body.len()),
        Err(FoldSnapshotError::StateTailMismatch)
    );

    let (wrong_hash, wrong_hash_body) = recanonicalize(&base, |value| {
        value["state"]["hashes"][1][0] = 255.into();
    });
    assert_eq!(
        wrong_hash.verify(key(), &wrong_hash_body, wrong_hash_body.len()),
        Err(FoldSnapshotError::StateTailHashMismatch)
    );

    let (wrong_config, wrong_config_body) = recanonicalize(&base, |value| {
        value["state"]["config"]["model"] = "substituted-model".into();
    });
    assert_eq!(
        wrong_config.verify(key(), &wrong_config_body, wrong_config_body.len()),
        Err(FoldSnapshotError::ConfigDigestMismatch)
    );
}

#[test]
fn noncanonical_json_is_refused_even_when_the_pointer_digest_matches() {
    let artifact = artifact();
    let value: serde_json::Value = serde_json::from_slice(artifact.body()).expect("snapshot JSON");
    let body = serde_json::to_vec_pretty(&value).expect("pretty JSON");
    let mut pointer = artifact.pointer().clone();
    pointer.body_bytes = u64::try_from(body.len()).expect("fixture fits");
    pointer.body_digest = BodyDigest::of(&body);
    assert_eq!(
        pointer.verify(key(), &body, body.len()),
        Err(FoldSnapshotError::NonCanonical)
    );
}

#[test]
fn exact_schema_is_stable() {
    assert_eq!(artifact().pointer().schema, FOLD_SNAPSHOT_SCHEMA);
}

#[test]
fn the_domain_hard_ceiling_cannot_be_disabled_by_a_larger_caller_limit() {
    let artifact = artifact();
    let mut pointer = artifact.pointer().clone();
    pointer.body_bytes = u64::try_from(MAX_FOLD_SNAPSHOT_BYTES + 1).expect("the ceiling fits u64");
    assert_eq!(
        pointer.verify(key(), &[], usize::MAX),
        Err(FoldSnapshotError::BodyTooLarge {
            declared: pointer.body_bytes,
            max: MAX_FOLD_SNAPSHOT_BYTES,
        })
    );
}
