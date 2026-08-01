//! Slice S-1.1 — the golden semantic history corpus.
//!
//! Each case names what it proves and is asserted against the *named* guard, not merely
//! against "an error happened". A test that only asserted `is_err()` would pass when the
//! wrong guard fired, which is exactly the regression this corpus exists to catch.

use aex_brain_domain::fold::fold;
use aex_brain_test_support::histories::{Expectation, all};

#[test]
fn every_golden_history_produces_exactly_its_declared_outcome() {
    let mut failures: Vec<String> = Vec::new();
    for case in all() {
        let outcome = fold(&case.history);
        match (&case.expectation, &outcome) {
            (Expectation::FoldsOpen, Ok(state)) => {
                if state.is_finished() {
                    failures.push(format!(
                        "{}: expected the agent to stay open, found terminal {:?}",
                        case.name, state.finish
                    ));
                }
            }
            (Expectation::FoldsTerminal(reason), Ok(state)) => {
                if state.finish != Some(*reason) {
                    failures.push(format!(
                        "{}: expected terminal {reason:?}, found {:?}",
                        case.name, state.finish
                    ));
                }
            }
            (Expectation::Rejected(rejection), Err(error)) => {
                if !rejection.matches(error) {
                    failures.push(format!(
                        "{}: expected {rejection:?}, found {error:?}",
                        case.name
                    ));
                }
            }
            (Expectation::Rejected(rejection), Ok(_)) => {
                failures.push(format!("{}: expected {rejection:?}, it folded", case.name));
            }
            (expected, Err(error)) => {
                failures.push(format!(
                    "{}: expected {expected:?}, folding failed with {error:?}",
                    case.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_corpus_covers_every_fold_guard_it_names() {
    use aex_brain_test_support::histories::Rejection;
    // Every rejection variant a case can name must actually be exercised by a case;
    // otherwise the enum grows discriminants nothing proves.
    let exercised: Vec<Rejection> = all()
        .into_iter()
        .filter_map(|case| match case.expectation {
            Expectation::Rejected(rejection) => Some(rejection),
            _ => None,
        })
        .collect();
    for required in [
        Rejection::JournalGap,
        Rejection::JournalForked,
        Rejection::TerminalAbsorbing,
        Rejection::NotStarted,
        Rejection::UnknownCall,
        Rejection::DuplicateToolResult,
        Rejection::UnknownEffect,
        Rejection::DuplicateEffect,
        Rejection::AlreadyStarted,
        Rejection::Budget,
        Rejection::CompactionOutOfRange,
        Rejection::EnvelopeHashMismatch,
        Rejection::UnknownWait,
        Rejection::UnknownChild,
        Rejection::UnprovenAssistantMessage,
    ] {
        assert!(
            exercised.contains(&required),
            "no golden history exercises {required:?}"
        );
    }
}
