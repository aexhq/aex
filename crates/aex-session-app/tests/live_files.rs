//! The live workspace observation path: `list` and `stat` against a running
//! generation.
//!
//! The point of this file is what it does **not** assert. A live listing is
//! answered from the guest's `lstat`, so no case here reads content, asks for a
//! digest, or builds a persisted tree.

use aex_session_app::testing::{CountingIds, FixedClock, PortCall, ScriptedPorts};
use aex_session_app::{
    AppError, LiveEntry, LiveEntryKind, LiveListQuery, PortError, list_live_files, stat_live_file,
};
use aex_session_domain::testing::{moment, session_fixture};
use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};

fn clock() -> FixedClock {
    FixedClock(moment(1_000))
}

fn generation(seed: u8) -> GenerationId {
    GenerationId::from_uuid7(Uuid7::compose(1, [seed; 10]))
}

fn entry(path: &str, kind: LiveEntryKind, size_bytes: u64, mode: u32) -> LiveEntry {
    LiveEntry {
        path: path.to_owned(),
        kind,
        size_bytes,
        mode,
        mtime: moment(1_700),
        target: None,
    }
}

fn workspace_entries() -> Vec<LiveEntry> {
    vec![
        entry("/workspace/a.txt", LiveEntryKind::File, 12, 0o644),
        entry("/workspace/bin", LiveEntryKind::Directory, 0, 0o755),
        entry("/workspace/bin/run", LiveEntryKind::File, 40, 0o755),
        entry("/workspace/z.txt", LiveEntryKind::File, 3, 0o600),
    ]
}

fn query(limit: u16, after: Option<&str>) -> LiveListQuery {
    LiveListQuery {
        path: None,
        recursive: true,
        limit,
        after: after.map(ToOwned::to_owned),
    }
}

#[tokio::test]
async fn a_listing_reports_mode_mtime_and_size_and_never_reads_content() {
    let ports = ScriptedPorts::idle()
        .with_generation(generation(7))
        .with_live_entries(workspace_entries());
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let session = session_fixture();

    let read = list_live_files(
        &context,
        session.workspace,
        session.id,
        None,
        &query(100, None),
    )
    .await
    .expect("the listing succeeds");

    assert_eq!(read.generation, generation(7));
    let observed: Vec<&str> = read
        .observed
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(
        observed,
        vec![
            "/workspace/a.txt",
            "/workspace/bin",
            "/workspace/bin/run",
            "/workspace/z.txt",
        ]
    );
    let first = read.observed.entries.first().expect("an entry");
    assert_eq!(first.mode, 0o644);
    assert_eq!(first.size_bytes, 12);
    assert_eq!(first.mtime, moment(1_700));

    // The content authority was never consulted.
    let calls = ports.calls();
    assert!(
        !calls.contains(&PortCall::Read("content")) && !calls.contains(&PortCall::Read("page")),
        "answering `ls` must not touch the content authority: {calls:?}"
    );
    assert!(calls.contains(&PortCall::Read("live_list")));
    assert!(
        calls.iter().all(|call| !call.is_write()),
        "an observation writes nothing: {calls:?}"
    );
}

#[tokio::test]
async fn paging_a_listing_walks_every_entry_exactly_once() {
    let ports = ScriptedPorts::idle()
        .with_generation(generation(7))
        .with_live_entries(workspace_entries());
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let session = session_fixture();

    let mut walked = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let read = list_live_files(
            &context,
            session.workspace,
            session.id,
            None,
            &query(2, cursor.as_deref()),
        )
        .await
        .expect("the listing succeeds");
        walked.extend(read.observed.entries.iter().map(|entry| entry.path.clone()));
        match read.observed.next_after {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked,
        workspace_entries()
            .into_iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>(),
        "two-at-a-time paging reproduces the listing with no repeat and no gap"
    );
}

#[tokio::test]
async fn a_stat_reports_a_symlink_without_following_it() {
    let mut entries = workspace_entries();
    entries.push(LiveEntry {
        path: "/workspace/link".to_owned(),
        kind: LiveEntryKind::Symlink,
        size_bytes: 0,
        mode: 0o777,
        mtime: moment(1_700),
        target: Some("/etc/shadow".to_owned()),
    });
    let ports = ScriptedPorts::idle()
        .with_generation(generation(7))
        .with_live_entries(entries);
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let session = session_fixture();

    let read = stat_live_file(
        &context,
        session.workspace,
        session.id,
        None,
        "/workspace/link",
    )
    .await
    .expect("the stat succeeds");
    assert_eq!(read.observed.kind, LiveEntryKind::Symlink);
    assert_eq!(read.observed.target.as_deref(), Some("/etc/shadow"));
}

#[tokio::test]
async fn a_missing_entry_is_not_found_rather_than_an_empty_answer() {
    let ports = ScriptedPorts::idle()
        .with_generation(generation(7))
        .with_live_entries(workspace_entries());
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let session = session_fixture();

    let outcome = stat_live_file(
        &context,
        session.workspace,
        session.id,
        None,
        "/workspace/absent",
    )
    .await;
    assert!(matches!(
        outcome,
        Err(AppError::Port(PortError::NotFound { .. }))
    ));
}

#[tokio::test]
async fn a_session_with_no_generation_in_force_fails_rather_than_listing_nothing() {
    // An empty listing and "there is no live filesystem" are different answers,
    // and returning the first for the second is the invented-data failure the
    // whole port refusal discipline exists to prevent.
    let mut session = session_fixture();
    session.generation = None;
    let ports = ScriptedPorts::idle()
        .with_session(session.clone())
        .with_live_entries(workspace_entries());
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let outcome = list_live_files(
        &context,
        session.workspace,
        session.id,
        None,
        &query(100, None),
    )
    .await;
    assert!(matches!(
        outcome,
        Err(AppError::Port(PortError::NotFound {
            kind: "live workspace generation"
        }))
    ));
}

#[tokio::test]
async fn a_pinned_generation_that_is_no_longer_in_force_is_refused() {
    // A different generation is a different filesystem. Answering from it would
    // return a confident wrong answer to the question that was asked.
    let ports = ScriptedPorts::idle()
        .with_generation(generation(7))
        .with_live_entries(workspace_entries());
    let clock = clock();
    let ids = CountingIds::default();
    let context = ports.context(&clock, &ids);
    let session = session_fixture();

    let outcome = list_live_files(
        &context,
        session.workspace,
        session.id,
        Some(generation(9)),
        &query(100, None),
    )
    .await;
    assert!(matches!(
        outcome,
        Err(AppError::Port(PortError::NotFound {
            kind: "the pinned live workspace generation"
        }))
    ));
}
