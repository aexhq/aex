//! Snapshot equivalence and hostile-byte tests.

use aex_brain_domain::fold::{apply, fold};
use aex_brain_domain::ids::{AgentId, AgentKey, SessionId};
use aex_brain_domain::snapshot::{
    FOLD_SNAPSHOT_SCHEMA, FoldSnapshotArtifact, FoldSnapshotError, MAX_FOLD_SNAPSHOT_BYTES,
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
    FoldSnapshotArtifact::capture(key(), &fold(&history).expect("fixture folds"))
        .expect("fixture snapshots")
}

fn recanonicalize(
    artifact: &mut FoldSnapshotArtifact,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let mut value: serde_json::Value =
        serde_json::from_slice(&artifact.body).expect("snapshot is JSON");
    mutate(&mut value);
    artifact.body = aex_wire::to_jcs_bytes(&value).expect("mutated JSON canonicalizes");
    artifact.pointer.body_bytes = u64::try_from(artifact.body.len()).expect("fixture fits");
    artifact.pointer.body_digest = BodyDigest::of(&artifact.body);
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
        let prefix = fold(&history[..cut]).expect("every prefix folds");
        let artifact = FoldSnapshotArtifact::capture(key(), &prefix)
            .expect("every non-empty valid prefix snapshots");
        let mut restored = artifact.pointer
            .verify(key(), &artifact.body, artifact.body.len())
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
    let mut artifact = artifact();
    artifact.body[0] ^= 1;
    assert_eq!(
        artifact
            .pointer
            .verify(key(), &artifact.body, artifact.body.len()),
        Err(FoldSnapshotError::BodyDigestMismatch)
    );
}

#[test]
fn schema_agent_tail_hash_and_config_are_independently_bound() {
    let base = artifact();

    let mut wrong_schema = base.clone();
    wrong_schema.pointer.schema = "aex.brain.fold.v0".to_owned();
    assert!(matches!(
        wrong_schema
            .pointer
            .verify(key(), &wrong_schema.body, wrong_schema.body.len()),
        Err(FoldSnapshotError::Schema { .. })
    ));

    let other = AgentKey::new(key().session, AgentId(Uuid::from_u128(10)));
    assert_eq!(
        base.pointer.verify(other, &base.body, base.body.len()),
        Err(FoldSnapshotError::AgentMismatch)
    );

    let mut wrong_tail = base.clone();
    recanonicalize(&mut wrong_tail, |value| value["state"]["tail"] = 99.into());
    assert_eq!(
        wrong_tail
            .pointer
            .verify(key(), &wrong_tail.body, wrong_tail.body.len()),
        Err(FoldSnapshotError::StateTailMismatch)
    );

    let mut wrong_hash = base.clone();
    recanonicalize(&mut wrong_hash, |value| {
        value["state"]["hashes"][1][0] = 255.into();
    });
    assert_eq!(
        wrong_hash
            .pointer
            .verify(key(), &wrong_hash.body, wrong_hash.body.len()),
        Err(FoldSnapshotError::StateTailHashMismatch)
    );

    let mut wrong_config = base;
    recanonicalize(&mut wrong_config, |value| {
        value["state"]["config"]["model"] = "substituted-model".into();
    });
    assert_eq!(
        wrong_config
            .pointer
            .verify(key(), &wrong_config.body, wrong_config.body.len()),
        Err(FoldSnapshotError::ConfigDigestMismatch)
    );
}

#[test]
fn noncanonical_json_is_refused_even_when_the_pointer_digest_matches() {
    let mut artifact = artifact();
    let value: serde_json::Value = serde_json::from_slice(&artifact.body).expect("snapshot JSON");
    artifact.body = serde_json::to_vec_pretty(&value).expect("pretty JSON");
    artifact.pointer.body_bytes = u64::try_from(artifact.body.len()).expect("fixture fits");
    artifact.pointer.body_digest = BodyDigest::of(&artifact.body);
    assert_eq!(
        artifact
            .pointer
            .verify(key(), &artifact.body, artifact.body.len()),
        Err(FoldSnapshotError::NonCanonical)
    );
}

#[test]
fn exact_schema_is_stable() {
    assert_eq!(artifact().pointer.schema, FOLD_SNAPSHOT_SCHEMA);
}

#[test]
fn the_domain_hard_ceiling_cannot_be_disabled_by_a_larger_caller_limit() {
    let mut artifact = artifact();
    artifact.pointer.body_bytes =
        u64::try_from(MAX_FOLD_SNAPSHOT_BYTES + 1).expect("the ceiling fits u64");
    assert_eq!(
        artifact.pointer.verify(key(), &[], usize::MAX),
        Err(FoldSnapshotError::BodyTooLarge {
            declared: artifact.pointer.body_bytes,
            max: MAX_FOLD_SNAPSHOT_BYTES,
        })
    );
}
