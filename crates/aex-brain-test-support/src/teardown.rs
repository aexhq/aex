//! The cleanup ledger.
//!
//! Every resource a fixture creates is recorded here and must be released before
//! the ledger is dropped. A ledger that still holds entries at drop panics and
//! names them, which turns a silent leak in a shared environment into a failing
//! test with an actionable list. The drop check stands down while the thread is
//! already panicking so it never masks the original failure.

/// A resource class this crate's fixtures can create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BrainResource {
    /// A Brain activation and its lease.
    Activation,
    /// A Brain journal entry and its effect record.
    JournalEntry,
    /// A registered tool route for one activation.
    ToolRoute,
    /// An MCP client session and its task identities.
    McpSession,
    /// An exact Hands generation reserved by a fixture.
    HandsGeneration,
}

impl BrainResource {
    /// The stable name used in ledger reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activation => "activation",
            Self::JournalEntry => "journal_entry",
            Self::ToolRoute => "tool_route",
            Self::McpSession => "mcp_session",
            Self::HandsGeneration => "hands_generation",
        }
    }
}

impl std::fmt::Display for BrainResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One resource awaiting release.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CleanupEntry {
    /// Which class of resource was created.
    pub kind: BrainResource,
    /// The identifier the teardown path needs to delete it.
    pub id: String,
}

impl std::fmt::Display for CleanupEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.kind, self.id)
    }
}

/// Records every resource a test creates and refuses to be dropped while any
/// remain.
#[derive(Debug, Default)]
pub struct CleanupLedger {
    entries: Vec<CleanupEntry>,
}

impl CleanupLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a created resource.
    pub fn record(&mut self, kind: BrainResource, id: impl Into<String>) {
        self.entries.push(CleanupEntry {
            kind,
            id: id.into(),
        });
    }

    /// Marks a resource released. Returns whether the ledger held it.
    pub fn release(&mut self, kind: BrainResource, id: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.kind == kind && entry.id == id));
        before != self.entries.len()
    }

    /// Resources still awaiting release.
    #[must_use]
    pub fn pending(&self) -> &[CleanupEntry] {
        &self.entries
    }

    /// Whether every recorded resource has been released.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes and returns everything recorded, for a teardown path that will
    /// actually delete each entry.
    pub fn drain(&mut self) -> Vec<CleanupEntry> {
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
            "aex-brain-test-support cleanup ledger still holds {} resource(s): {}",
            names.len(),
            names.join(", ")
        )
    }
}

impl Drop for CleanupLedger {
    fn drop(&mut self) {
        if self.entries.is_empty() || std::thread::panicking() {
            return;
        }
        panic!("{}", self.leak_report());
    }
}

#[cfg(test)]
mod tests {
    use super::{BrainResource, CleanupLedger};

    #[test]
    fn recording_then_releasing_leaves_the_ledger_empty() {
        let mut ledger = CleanupLedger::new();
        ledger.record(BrainResource::Activation, "aex-test-fixture-0001");
        assert!(!ledger.is_empty());
        assert_eq!(ledger.pending().len(), 1);
        assert!(ledger.release(BrainResource::Activation, "aex-test-fixture-0001"));
        assert!(ledger.is_empty());
        ledger.assert_empty();
    }

    #[test]
    fn releasing_an_unknown_resource_reports_false() {
        let mut ledger = CleanupLedger::new();
        ledger.record(BrainResource::Activation, "held");
        assert!(!ledger.release(BrainResource::Activation, "never-recorded"));
        assert_eq!(ledger.pending().len(), 1);
        ledger.drain();
    }

    #[test]
    fn draining_hands_every_entry_to_the_teardown_path() {
        let mut ledger = CleanupLedger::new();
        ledger.record(BrainResource::Activation, "one");
        ledger.record(BrainResource::JournalEntry, "two");
        let drained = ledger.drain();
        assert_eq!(drained.len(), 2);
        assert!(ledger.is_empty());
    }

    #[test]
    fn dropping_an_empty_ledger_is_silent() {
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = CleanupLedger::new();
            ledger.record(BrainResource::Activation, "one");
            ledger.drain();
        });
        assert!(outcome.is_ok());
    }

    #[test]
    fn dropping_a_non_empty_ledger_names_every_leak() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = CleanupLedger::new();
            ledger.record(BrainResource::Activation, "leaked-one");
            ledger.record(BrainResource::JournalEntry, "leaked-two");
        });
        std::panic::set_hook(previous);

        let payload = outcome.expect_err("a leaked ledger must panic at drop");
        let message = payload
            .downcast_ref::<String>()
            .map_or_else(|| String::from("<non-string panic>"), Clone::clone);
        assert!(message.contains("leaked-one"), "{message}");
        assert!(message.contains("leaked-two"), "{message}");
        assert!(message.contains("2 resource(s)"), "{message}");
    }

    #[test]
    fn the_drop_check_never_masks_an_original_failure() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let mut ledger = CleanupLedger::new();
            ledger.record(BrainResource::Activation, "leaked");
            panic!("the original failure");
        });
        std::panic::set_hook(previous);

        let payload = outcome.expect_err("the original panic propagates");
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .unwrap_or("<not a &str>");
        assert_eq!(message, "the original failure");
    }
}
