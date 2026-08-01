//! The fixture ledger.
//!
//! Every resource a fixture creates is recorded here and must be released
//! before the ledger is dropped. A ledger that still holds entries at drop
//! fails and names them, which turns a silent leak in a shared environment into
//! an actionable list.
//!
//! # Why this is not `CleanupLedger`
//!
//! `aex_test_harness::CleanupLedger` is the one cleanup ledger, and it owns
//! remote reclamation: it holds a `Reclaimer`, it deletes, and its entries are
//! the janitor's closed `ResourceKind` set. This type tracks *fixture* state -
//! an activation, a journal entry, a registry pointer - whose classes are
//! product-internal and have no discovery route a janitor could sweep by.
//! Folding these classes into the janitor's set would fill it with names no
//! sweep can ever find, which is worse than two types with two names. So there
//! is exactly one `CleanupLedger` in the workspace, and this is a
//! [`FixtureLedger`].
//!
//! The one thing the two share is what to do when a ledger holding entries is
//! dropped, and that is `aex_test_harness::ledger::report_residue`, called here
//! rather than reimplemented. Deciding it separately four times is how the
//! guard came to stand down while panicking in all four copies - disabled at
//! exactly the moment a leak matters most.

/// A resource class this crate's fixtures can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegionalResource {
    /// A row in the session-authority table.
    SessionItem,
    /// A content descriptor and its S3 object.
    ContentObject,
    /// A named registry pointer and its revision.
    RegistryEntry,
    /// A secret custody generation and its lineage.
    SecretGeneration,
    /// A durable operation and its claim.
    DurableOperation,
}

impl RegionalResource {
    /// The stable name used in ledger reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionItem => "session_item",
            Self::ContentObject => "content_object",
            Self::RegistryEntry => "registry_entry",
            Self::SecretGeneration => "secret_generation",
            Self::DurableOperation => "durable_operation",
        }
    }
}

impl std::fmt::Display for RegionalResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One resource awaiting release.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FixtureEntry {
    /// Which class of resource was created.
    pub kind: RegionalResource,
    /// The identifier the teardown path needs to delete it.
    pub id: String,
}

impl std::fmt::Display for FixtureEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.kind, self.id)
    }
}

/// Records every resource a test creates and refuses to be dropped while any
/// remain.
#[derive(Debug, Default)]
pub struct FixtureLedger {
    entries: Vec<FixtureEntry>,
}

impl FixtureLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a created resource.
    pub fn record(&mut self, kind: RegionalResource, id: impl Into<String>) {
        self.entries.push(FixtureEntry {
            kind,
            id: id.into(),
        });
    }

    /// Marks a resource released. Returns whether the ledger held it.
    pub fn release(&mut self, kind: RegionalResource, id: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.kind == kind && entry.id == id));
        before != self.entries.len()
    }

    /// Resources still awaiting release.
    #[must_use]
    pub fn pending(&self) -> &[FixtureEntry] {
        &self.entries
    }

    /// Whether every recorded resource has been released.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes and returns everything recorded, for a teardown path that will
    /// actually delete each entry.
    pub fn drain(&mut self) -> Vec<FixtureEntry> {
        std::mem::take(&mut self.entries)
    }

    /// Fails now rather than at drop.
    ///
    /// # Panics
    ///
    /// Panics when any recorded resource has not been released.
    pub fn assert_empty(&self) {
        assert!(self.entries.is_empty(), "{}", self.leak_report());
    }

    fn leak_report(&self) -> String {
        let names: Vec<String> = self
            .entries
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        format!(
            "{} aex-regional-test-support fixture ledger still holds {} resource(s): {}",
            aex_test_harness::ledger::RESIDUE_MARKER,
            names.len(),
            names.join(", ")
        )
    }
}

impl Drop for FixtureLedger {
    /// Reports every unreleased resource.
    ///
    /// There is deliberately no `std::thread::panicking()` stand-down here. The
    /// previous guard returned early while the thread was unwinding, which
    /// disabled the leak check for every failing test - and a failing test is
    /// the case where resources are most likely to have been left behind.
    /// `report_residue` keeps the check and changes only its channel: it panics
    /// when it safely can, and writes the same report to stderr when panicking
    /// again would abort the process and destroy the original failure.
    fn drop(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        aex_test_harness::ledger::report_residue(&self.leak_report());
    }
}

#[cfg(test)]
mod tests {
    use super::{FixtureLedger, RegionalResource};

    #[test]
    fn recording_then_releasing_leaves_the_ledger_empty() {
        let mut ledger = FixtureLedger::new();
        ledger.record(RegionalResource::SessionItem, "aex-test-fixture-0001");
        assert!(!ledger.is_empty());
        assert_eq!(ledger.pending().len(), 1);
        assert!(ledger.release(RegionalResource::SessionItem, "aex-test-fixture-0001"));
        assert!(ledger.is_empty());
        ledger.assert_empty();
    }

    #[test]
    fn releasing_an_unknown_resource_reports_false() {
        let mut ledger = FixtureLedger::new();
        ledger.record(RegionalResource::SessionItem, "held");
        assert!(!ledger.release(RegionalResource::SessionItem, "never-recorded"));
        assert_eq!(ledger.pending().len(), 1);
        ledger.drain();
    }

    #[test]
    fn draining_hands_every_entry_to_the_teardown_path() {
        let mut ledger = FixtureLedger::new();
        ledger.record(RegionalResource::SessionItem, "one");
        ledger.record(RegionalResource::ContentObject, "two");
        let drained = ledger.drain();
        assert_eq!(drained.len(), 2);
        assert!(ledger.is_empty());
    }

    #[test]
    fn dropping_an_empty_ledger_is_silent() {
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = FixtureLedger::new();
            ledger.record(RegionalResource::SessionItem, "one");
            ledger.drain();
        });
        assert!(outcome.is_ok());
    }

    #[test]
    fn dropping_a_non_empty_ledger_names_every_leak() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = FixtureLedger::new();
            ledger.record(RegionalResource::SessionItem, "leaked-one");
            ledger.record(RegionalResource::ContentObject, "leaked-two");
        });
        std::panic::set_hook(previous);

        let payload = outcome.expect_err("a leaked ledger must fail at drop");
        let message = payload
            .downcast_ref::<String>()
            .map_or_else(|| String::from("<non-string panic>"), Clone::clone);
        assert!(message.contains("leaked-one"), "{message}");
        assert!(message.contains("leaked-two"), "{message}");
        assert!(message.contains("2 resource(s)"), "{message}");
    }

    /// The guard used to stand down here, so a failing test leaked silently.
    /// It now still reports, through the channel an unwind permits, and the
    /// original failure still propagates unchanged.
    #[test]
    fn a_leak_is_still_reported_when_the_test_is_already_failing() {
        let before = aex_test_harness::residue_reports_during_panic();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = FixtureLedger::new();
            ledger.record(RegionalResource::SessionItem, "leaked-while-failing");
            panic!("the original failure");
        });
        std::panic::set_hook(previous);

        let payload = outcome.expect_err("the original panic propagates");
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .unwrap_or("<not a &str>");
        assert_eq!(
            message, "the original failure",
            "the leak report must not replace the original failure"
        );
        assert_eq!(
            aex_test_harness::residue_reports_during_panic(),
            before + 1,
            "the leak must still be reported while the thread unwinds"
        );
    }
}
