//! Manual process-RSS probe for a one-million-token-shaped snapshot.
//!
//! Run `prepare <path>` outside the measurement, then compare OS peak working set for
//! `baseline` and `verify <path>`. The shape uses four 1,000,000-byte text blocks (roughly
//! one million tokens at the common four-bytes-per-token planning approximation); it is a
//! memory shape, not tokenizer truth.

use aex_brain_domain::ids::{AgentId, AgentKey, SessionId};
use aex_brain_domain::snapshot::{
    FoldSnapshotArtifact, FoldSnapshotPointer, JournalPoint, SnapshotReplay,
};
use aex_brain_domain::wire_pending::ContentBlockRef;
use aex_brain_test_support::journal_gen::{HistoryBuilder, grant, started, user_text};
use aex_model_catalog::canonical::CanonicalBlock;
use std::io::{Read as _, Write as _};
use uuid::Uuid;

const BLOCK_BYTES: usize = 1_000_000;
const BLOCKS: usize = 4;

fn key() -> AgentKey {
    AgentKey::new(SessionId(Uuid::from_u128(7)), AgentId(Uuid::from_u128(9)))
}

fn shaped_artifact() -> FoldSnapshotArtifact {
    let block = "x".repeat(BLOCK_BYTES);
    let mut history = HistoryBuilder::new().push(started(grant(100)));
    for _ in 0..BLOCKS {
        history = history.push(user_text(&block));
    }
    let history = history.build();
    let mut replay = SnapshotReplay::from_sequence_zero();
    for entry in &history {
        replay.apply(entry).expect("the shaped history folds");
    }
    let tail = history.last().expect("the shaped history is non-empty");
    replay
        .finish_exact(
            key(),
            JournalPoint {
                seq: tail.envelope.seq,
                hash: tail.envelope.content_hash,
            },
        )
        .expect("the shaped replay captures")
}

fn prepare(path: &std::path::Path) {
    let artifact = shaped_artifact();
    let pointer = serde_json::to_vec(artifact.pointer()).expect("the pointer encodes");
    let mut output = std::fs::File::create(path).expect("the probe bundle opens");
    output
        .write_all(
            &u64::try_from(pointer.len())
                .expect("pointer fits")
                .to_be_bytes(),
        )
        .expect("the pointer length writes");
    output.write_all(&pointer).expect("the pointer writes");
    output
        .write_all(artifact.body())
        .expect("the snapshot body writes");
    println!(
        "SNAPSHOT_MEMORY_PREPARED body_bytes={}",
        artifact.body().len()
    );
}

fn verify(path: &std::path::Path) {
    let mut input = std::fs::File::open(path).expect("the probe bundle opens");
    let mut length = [0_u8; 8];
    input
        .read_exact(&mut length)
        .expect("the pointer length reads");
    let pointer_bytes = usize::try_from(u64::from_be_bytes(length)).expect("pointer length fits");
    assert!(pointer_bytes <= 64 * 1_024, "the pointer remains bounded");
    let mut encoded_pointer = vec![0_u8; pointer_bytes];
    input
        .read_exact(&mut encoded_pointer)
        .expect("the pointer reads");
    let pointer: FoldSnapshotPointer =
        serde_json::from_slice(&encoded_pointer).expect("the pointer decodes");
    let mut body = Vec::new();
    input.read_to_end(&mut body).expect("the body reads");
    let verified = pointer
        .verify(key(), &body, body.len())
        .expect("the shaped body verifies");
    let restored_text_bytes = verified
        .state
        .open_user
        .iter()
        .map(|reference| match reference {
            ContentBlockRef::Inline {
                block: CanonicalBlock::Text { text, .. },
            } => text.as_str().len(),
            ContentBlockRef::Inline { .. } | ContentBlockRef::Placed { .. } => 0,
        })
        .sum::<usize>();
    assert_eq!(verified.state.open_user.len(), BLOCKS);
    assert_eq!(restored_text_bytes, BLOCK_BYTES * BLOCKS);
    println!(
        "SNAPSHOT_MEMORY_WORKLOAD body_bytes={} text_bytes={} restored_open_blocks={}",
        body.len(),
        restored_text_bytes,
        verified.state.open_user.len()
    );
    std::hint::black_box((&body, &verified));
    std::thread::sleep(core::time::Duration::from_millis(500));
}

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    match arguments.next().as_deref() {
        Some(mode) if mode == "baseline" => {
            std::thread::sleep(core::time::Duration::from_millis(500));
            println!("SNAPSHOT_MEMORY_BASELINE_READY");
        }
        Some(mode) if mode == "prepare" => {
            let path = arguments.next().expect("prepare requires one bundle path");
            prepare(std::path::Path::new(&path));
        }
        Some(mode) if mode == "verify" => {
            let path = arguments.next().expect("verify requires one bundle path");
            verify(std::path::Path::new(&path));
        }
        _ => panic!("usage: snapshot_memory baseline | prepare <path> | verify <path>"),
    }
}
