//! The cleanup ledger.
//!
//! One type, here, re-exported by the four `*-test-support` crates. A resource
//! is recorded **before** the create call returns, so a process that dies
//! between the create and the record cannot hide residue; the janitor
//! cross-references the ledger against what it finds by tag and reports
//! anything in one and not the other.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::run::{TestRunId, Ttl, rfc3339};

/// What kind of thing was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// One `DynamoDB` item.
    DynamoItem,
    /// One S3 object.
    S3Object,
    /// An in-flight S3 multipart upload.
    S3Multipart,
    /// A message left on a queue.
    SqsMessage,
    /// A KMS key created for the run.
    KmsKey,
    /// One generation of a custody secret.
    SecretGeneration,
    /// A started ECS task.
    EcsTask,
    /// A Hands generation (`MicroVM` lifetime).
    HandsGeneration,
    /// Anything a model or payment provider created on the run's behalf.
    ProviderResource,
    /// A workspace.
    Workspace,
    /// An organization.
    Organization,
    /// A session.
    Session,
    /// A durable operation record.
    Operation,
    /// An export job and its output.
    Export,
}

/// How the resource is expected to end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "terminal", rename_all = "snake_case")]
pub enum Terminal {
    /// The test deletes it explicitly.
    Deleted,
    /// A verified TTL mechanism removes it.
    Expired {
        /// The TTL the mechanism honours.
        by: Ttl,
    },
    /// It legitimately survives the run. The only legal survivor, and only with
    /// a verified TTL mechanism behind it.
    Retained {
        /// Why it may survive.
        reason: String,
    },
}

/// Which test case created the resource.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TestCaseId(pub String);

impl std::fmt::Display for TestCaseId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One recorded resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// What was created.
    pub kind: ResourceKind,
    /// Its identity: table key, object key, ARN, task id, provider id.
    pub identity: String,
    /// When it was recorded.
    #[serde(with = "rfc3339")]
    pub created_at: OffsetDateTime,
    /// How it is expected to end.
    pub expected_terminal: Terminal,
    /// Which test case owns it.
    pub owner_test: TestCaseId,
    /// When the test released it, if it did.
    #[serde(with = "rfc3339::option", default)]
    pub released_at: Option<OffsetDateTime>,
}

impl Entry {
    /// A new entry, recorded now.
    #[must_use]
    pub fn new(
        kind: ResourceKind,
        identity: impl Into<String>,
        expected_terminal: Terminal,
        owner_test: TestCaseId,
    ) -> Self {
        Self {
            kind,
            identity: identity.into(),
            created_at: OffsetDateTime::now_utc(),
            expected_terminal,
            owner_test,
            released_at: None,
        }
    }
}

/// Where a flushed ledger was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerPath(pub PathBuf);

/// Why a ledger could not be written.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The ledger directory or file could not be created or appended to.
    #[error("cannot write cleanup ledger `{path}`: {source}")]
    Write {
        /// The path that failed.
        path: String,
        /// The underlying I/O failure.
        source: std::io::Error,
    },
    /// An entry could not be rendered.
    #[error("cannot render cleanup ledger entry `{identity}`: {source}")]
    Render {
        /// The entry that failed.
        identity: String,
        /// The underlying serialization failure.
        source: serde_json::Error,
    },
}

/// The append-only record of everything a run created.
#[derive(Debug)]
pub struct CleanupLedger {
    run_id: TestRunId,
    root: PathBuf,
    entries: Mutex<Vec<Entry>>,
    /// How many entries the file already holds.
    ///
    /// A watermark rather than a `flushed` flag: the file is append-only, so a
    /// flush must write the tail it has not written yet. A boolean cannot
    /// express that, and made a mid-run flush both restate every earlier entry
    /// and silently discard every later one.
    written: Mutex<usize>,
}

impl CleanupLedger {
    /// A ledger for `run_id` under the default `target/aex-test` root.
    #[must_use]
    pub fn new(run_id: TestRunId) -> Self {
        Self::with_root(run_id, PathBuf::from("target").join("aex-test"))
    }

    /// A ledger for `run_id` under an explicit root.
    #[must_use]
    pub fn with_root(run_id: TestRunId, root: PathBuf) -> Self {
        Self {
            run_id,
            root,
            entries: Mutex::new(Vec::new()),
            written: Mutex::new(0),
        }
    }

    /// The file this ledger flushes to.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.root
            .join(self.run_id.as_str())
            .join("cleanup-ledger.jsonl")
    }

    /// Records a created resource. Call this **before** the create call
    /// returns.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned by a panicking test, because
    /// continuing would silently stop recording resources.
    pub fn record(&self, entry: Entry) {
        self.entries
            .lock()
            .expect("the cleanup ledger mutex is not poisoned")
            .push(entry);
    }

    /// Marks the most recent unreleased entry with this kind and identity as
    /// released. Returns whether one was found.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned.
    pub fn release(&self, kind: ResourceKind, identity: &str) -> bool {
        let mut entries = self
            .entries
            .lock()
            .expect("the cleanup ledger mutex is not poisoned");
        let found = entries.iter_mut().rev().find(|entry| {
            entry.kind == kind && entry.identity == identity && entry.released_at.is_none()
        });
        match found {
            Some(entry) => {
                entry.released_at = Some(OffsetDateTime::now_utc());
                true
            }
            None => false,
        }
    }

    /// Every entry, in record order.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        self.entries
            .lock()
            .expect("the cleanup ledger mutex is not poisoned")
            .clone()
    }

    /// Entries that were recorded and never released.
    ///
    /// An entry whose expected terminal is [`Terminal::Retained`] is not
    /// residue: it is a declared survivor with a stated reason.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned.
    #[must_use]
    pub fn residue(&self) -> Vec<Entry> {
        self.entries
            .lock()
            .expect("the cleanup ledger mutex is not poisoned")
            .iter()
            .filter(|entry| {
                entry.released_at.is_none()
                    && !matches!(entry.expected_terminal, Terminal::Retained { .. })
            })
            .cloned()
            .collect()
    }

    /// Writes the ledger as append-only JSONL and returns where it landed.
    ///
    /// # Errors
    ///
    /// Returns [`LedgerError`] when the directory or file cannot be written, or
    /// when an entry cannot be rendered.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned.
    pub fn flush(&self) -> Result<LedgerPath, LedgerError> {
        let path = self.path();
        let directory: &Path = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(directory).map_err(|source| LedgerError::Write {
            path: directory.display().to_string(),
            source,
        })?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| LedgerError::Write {
                path: path.display().to_string(),
                source,
            })?;
        // The watermark is held across the write so two threads flushing at once
        // cannot both decide the same tail is theirs to append.
        let mut written = self
            .written
            .lock()
            .expect("the cleanup ledger mutex is not poisoned");
        let entries = self.entries();
        for entry in entries.iter().skip(*written) {
            let line = serde_json::to_string(entry).map_err(|source| LedgerError::Render {
                identity: entry.identity.clone(),
                source,
            })?;
            writeln!(file, "{line}").map_err(|source| LedgerError::Write {
                path: path.display().to_string(),
                source,
            })?;
            *written += 1;
        }
        Ok(LedgerPath(path))
    }
}

impl Drop for CleanupLedger {
    /// The `finally`-equivalent half of the flush contract.
    ///
    /// An explicit end-of-run [`CleanupLedger::flush`] is the normal path; this
    /// catches the run that panicked before reaching it, and the run that
    /// flushed mid-way and then created more. A failure here is reported, never
    /// swallowed, because an unwritten ledger turns real residue into an
    /// unexplained tagged resource.
    fn drop(&mut self) {
        let written = *self
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The condition is the unwritten tail, not "was flush ever called": a
        // ledger flushed at entry 3 and then given a fourth still owes one line.
        if self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            <= written
        {
            return;
        }
        if let Err(error) = self.flush() {
            eprintln!(
                "aex-test-harness: cleanup ledger for {} was not written: {error}",
                self.run_id
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CleanupLedger, Entry, ResourceKind, Terminal, TestCaseId};
    use crate::run::{Lane, TestRunId};

    fn ledger(root: &std::path::Path) -> CleanupLedger {
        CleanupLedger::with_root(TestRunId::mint(), root.to_path_buf())
    }

    fn entry(identity: &str) -> Entry {
        Entry::new(
            ResourceKind::S3Object,
            identity,
            Terminal::Deleted,
            TestCaseId("cleanup::demo".to_owned()),
        )
    }

    #[test]
    fn an_unreleased_entry_is_residue_and_a_released_one_is_not() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        ledger.record(entry("a"));
        ledger.record(entry("b"));
        assert!(ledger.release(ResourceKind::S3Object, "a"));
        let residue = ledger.residue();
        assert_eq!(residue.len(), 1);
        assert_eq!(residue[0].identity, "b");
    }

    #[test]
    fn releasing_something_never_recorded_is_reported_rather_than_ignored() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        assert!(!ledger.release(ResourceKind::S3Object, "never-created"));
    }

    #[test]
    fn a_declared_survivor_is_not_residue() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        ledger.record(Entry::new(
            ResourceKind::DynamoItem,
            "retained",
            Terminal::Retained {
                reason: "the authority item is removed by a verified 24 h TTL".to_owned(),
            },
            TestCaseId("cleanup::retained".to_owned()),
        ));
        ledger.record(Entry::new(
            ResourceKind::DynamoItem,
            "expiring",
            Terminal::Expired {
                by: Lane::E2e.default_ttl(),
            },
            TestCaseId("cleanup::expiring".to_owned()),
        ));
        let residue = ledger.residue();
        assert_eq!(
            residue.len(),
            1,
            "only the TTL-expiring item is residue until released"
        );
        assert_eq!(residue[0].identity, "expiring");
    }

    /// A resource created after an explicit flush is exactly the resource a
    /// janitor would later find with no ledger row to explain it.
    #[test]
    fn an_entry_recorded_after_an_explicit_flush_still_reaches_the_file() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let path = {
            let ledger = ledger(root.path());
            ledger.record(entry("before"));
            let path = ledger.flush().expect("the ledger flushes").0;
            ledger.record(entry("after"));
            path
        };
        let text = std::fs::read_to_string(&path).expect("the ledger is readable");
        let identities: Vec<String> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Entry>(line)
                    .expect("each line is one entry")
                    .identity
            })
            .collect();
        assert_eq!(
            identities,
            vec!["before".to_owned(), "after".to_owned()],
            "an entry recorded after a flush must still be written when the ledger drops"
        );
    }

    /// The file is append-only, so a second flush that restated the first
    /// flush's entries would make one resource look like two to the janitor.
    #[test]
    fn flushing_twice_does_not_restate_an_entry_the_first_flush_wrote() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        ledger.record(entry("a"));
        let path = ledger.flush().expect("the first flush").0;
        ledger.record(entry("b"));
        ledger.flush().expect("the second flush");
        let text = std::fs::read_to_string(&path).expect("the ledger is readable");
        let identities: Vec<String> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Entry>(line)
                    .expect("each line is one entry")
                    .identity
            })
            .collect();
        assert_eq!(identities, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn a_flushed_ledger_is_append_only_jsonl_that_round_trips() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        ledger.record(entry("a"));
        ledger.record(entry("b"));
        let path = ledger.flush().expect("the ledger flushes").0;
        let text = std::fs::read_to_string(&path).expect("the ledger is readable");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let parsed: Entry = serde_json::from_str(line).expect("each line is one entry");
            assert_eq!(parsed.kind, ResourceKind::S3Object);
        }
    }
}
