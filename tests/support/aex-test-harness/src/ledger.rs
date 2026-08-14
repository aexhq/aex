//! The cleanup ledger.
//!
//! One type, here, re-exported by the domain-specific `*-test-support` crates. A resource
//! is recorded **before** the create call returns, so a process that dies
//! between the create and the record cannot hide residue; the janitor
//! cross-references the ledger against what it finds by tag and reports
//! anything in one and not the other.
//!
//! # `release` means released
//!
//! The ledger is a reclamation path, not a notebook. [`CleanupLedger::release`]
//! calls the installed [`Reclaimer`] and stamps `released_at` **only** when the
//! reclamation succeeded, so an entry can never be marked released by a test
//! that merely believes it deleted something. A ledger with no reclaimer
//! installed cannot release at all: every entry stays residue and [`Drop`] says
//! so. That is the deliberate default, because a lane that silently reports
//! success having deleted nothing is the exact defect this module exists to
//! stop.
//!
//! The out-of-band janitor remains the backstop for the run that is killed
//! before any of this executes. The ledger closes the normal path; only a sweep
//! by tag closes `SIGKILL`.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::run::{TestRunId, Ttl, rfc3339};

/// What kind of thing was created.
///
/// The variants are the rows of `[janitor.resource]` in
/// `release/policy/test-profiles.toml`, and a test asserts the two sets are
/// equal in both directions. A kind with no policy row has no reclamation rank
/// and no discovery route, which would let a stream record something the
/// janitor can never find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    /// A recurring off-session charge authorization at a payment provider.
    AutoRechargePolicy,
    /// A saved payment method at a payment provider.
    PaymentInstrument,
    /// A bearer API key and its epoch.
    ApiKey,
    /// A dashboard user row carrying a password hash.
    DashboardUser,
    /// A Hands generation (`MicroVM` lifetime).
    HandsGeneration,
    /// A started ECS task.
    EcsTask,
    /// A session.
    Session,
    /// A durable operation record.
    Operation,
    /// An export job and its output.
    Export,
    /// An in-flight S3 multipart upload.
    S3Multipart,
    /// One S3 object.
    S3Object,
    /// One generation of a custody secret.
    SecretGeneration,
    /// A KMS key created for the run.
    KmsKey,
    /// One `DynamoDB` item.
    DynamoItem,
    /// A workspace.
    Workspace,
    /// An organization.
    Organization,
    /// A customer record at a payment provider.
    PaymentCustomer,
    /// A message left on a queue.
    SqsMessage,
    /// Anything a model or payment provider created on the run's behalf that
    /// has no more specific kind.
    ProviderResource,
}

impl ResourceKind {
    /// Every kind, in declaration order.
    pub const ALL: [Self; 19] = [
        Self::AutoRechargePolicy,
        Self::PaymentInstrument,
        Self::ApiKey,
        Self::DashboardUser,
        Self::HandsGeneration,
        Self::EcsTask,
        Self::Session,
        Self::Operation,
        Self::Export,
        Self::S3Multipart,
        Self::S3Object,
        Self::SecretGeneration,
        Self::KmsKey,
        Self::DynamoItem,
        Self::Workspace,
        Self::Organization,
        Self::PaymentCustomer,
        Self::SqsMessage,
        Self::ProviderResource,
    ];

    /// The name this kind carries in policy, tags, ledger lines and sweep
    /// reports.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AutoRechargePolicy => "auto_recharge_policy",
            Self::PaymentInstrument => "payment_instrument",
            Self::ApiKey => "api_key",
            Self::DashboardUser => "dashboard_user",
            Self::HandsGeneration => "hands_generation",
            Self::EcsTask => "ecs_task",
            Self::Session => "session",
            Self::Operation => "operation",
            Self::Export => "export",
            Self::S3Multipart => "s3_multipart",
            Self::S3Object => "s3_object",
            Self::SecretGeneration => "secret_generation",
            Self::KmsKey => "kms_key",
            Self::DynamoItem => "dynamo_item",
            Self::Workspace => "workspace",
            Self::Organization => "organization",
            Self::PaymentCustomer => "payment_customer",
            Self::SqsMessage => "sqs_message",
            Self::ProviderResource => "provider_resource",
        }
    }

    /// The kind a policy or wire name denotes, if any.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// This kind's row in the embedded janitor policy.
    ///
    /// # Panics
    ///
    /// Panics when the embedded policy declares no row for the kind. The
    /// document is compiled in, so that is a build-time defect, and a kind with
    /// no reclamation rank is worse than a loud failure.
    #[must_use]
    pub fn policy(self) -> &'static ResourcePolicy {
        janitor_policy()
            .resources
            .get(self.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "release/policy/test-profiles.toml declares no [janitor.resource.{}] row",
                    self.as_str()
                )
            })
    }

    /// Where this kind sits in the reclamation order. Lower goes first.
    #[must_use]
    pub fn reclaim_rank(self) -> u8 {
        self.policy().rank
    }

    /// How a janitor with only a plane and a credential finds one.
    #[must_use]
    pub fn discovery(self) -> Discovery {
        self.policy().discovery
    }

    /// Whether AEX or the provider minted the identity.
    #[must_use]
    pub fn naming(self) -> Naming {
        self.policy().naming
    }

    /// Whether the janitor can reclaim this kind knowing only the tags.
    ///
    /// This is the predicate `prd` eligibility is gated on: a scenario that
    /// creates a kind for which this is false has produced residue no sweep can
    /// ever remove.
    #[must_use]
    pub fn reclaimable_from_tags(self) -> bool {
        self.discovery() != Discovery::None
    }

    /// Kinds that must be reclaimed before this one.
    #[must_use]
    pub fn requires_reclaim_first(self) -> Vec<Self> {
        self.policy()
            .requires_reclaim_first
            .iter()
            .filter_map(|name| Self::parse(name))
            .collect()
    }
}

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a janitor finds a resource of some kind knowing only a plane, a
/// credential and the tag scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Discovery {
    /// The provider's tag API lists it by tag.
    ResourceTag,
    /// A list call filtered by one of the minted name templates.
    NamePrefix,
    /// A payment provider's metadata search.
    ProviderMetadata,
    /// Not findable. Such a kind is never reclaimable and never `prd`-eligible.
    None,
}

/// Whether AEX minted a resource's identity or the provider did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Naming {
    /// AEX minted the name, so it carries the run prefix and the janitor
    /// requires that as well as the tag set.
    AexMinted,
    /// The provider minted the id, so the tag set is the only binding.
    ProviderMinted,
}

/// One `[janitor.resource.<kind>]` row.
#[derive(Debug, Clone, Deserialize)]
pub struct ResourcePolicy {
    /// Reclamation order, ascending.
    pub rank: u8,
    /// How the janitor finds one.
    pub discovery: Discovery,
    /// Who minted the identity.
    pub naming: Naming,
    /// Kinds that must be reclaimed before this one.
    #[serde(default)]
    pub requires_reclaim_first: Vec<String>,
    /// Kinds a `prd`-eligible scenario must also declare if it declares this
    /// one.
    #[serde(default)]
    pub requires_declared_with: Vec<String>,
    /// What reclaiming one actually does, and why the order matters.
    pub reclaim: String,
}

/// The `[janitor]` section of `release/policy/test-profiles.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct JanitorPolicy {
    /// The marker tag key.
    pub synthetic_tag: String,
    /// The marker tag's one legal value.
    pub synthetic_value: String,
    /// The run-id tag key.
    pub run_id_tag: String,
    /// The owner tag key.
    pub owner_tag: String,
    /// The lane tag key.
    pub lane_tag: String,
    /// The expiry tag key.
    pub expires_at_tag: String,
    /// The fixed prefix every run id carries.
    pub run_id_prefix: String,
    /// How many lowercase hex characters follow the prefix.
    pub run_id_hex_len: usize,
    /// Grace added to a run's expiry before a survivor is residue.
    pub residue_grace_minutes: i64,
    /// The name templates `TestRun` mints, with `{run_id}` unsubstituted.
    pub minted_name_templates: Vec<String>,
    /// The lanes permitted to run against `prd` at all.
    pub prd_lanes: Vec<String>,
    /// Reclaimable kinds by name.
    #[serde(default, rename = "resource")]
    pub resources: BTreeMap<String, ResourcePolicy>,
}

/// The embedded janitor policy.
///
/// # Panics
///
/// Panics when the embedded document does not parse or declares no `[janitor]`
/// section.
#[must_use]
pub fn janitor_policy() -> &'static JanitorPolicy {
    static POLICY: OnceLock<JanitorPolicy> = OnceLock::new();
    POLICY.get_or_init(|| {
        #[derive(Deserialize)]
        struct Document {
            janitor: JanitorPolicy,
        }
        let document: Document = toml::from_str(crate::TEST_PROFILES_TOML).expect(
            "release/policy/test-profiles.toml is embedded and must parse a [janitor] section",
        );
        document.janitor
    })
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
    ///
    /// The mechanism's own deadline is carried here rather than described in
    /// the reason, so the janitor can decide whether a surviving resource is
    /// still inside the window it was promised. A free-text reason alone made
    /// this an unchecked escape hatch.
    Retained {
        /// Why it may survive.
        reason: String,
        /// The deadline the named mechanism honours.
        within: Ttl,
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
    /// When the reclamation of this resource succeeded, if it did.
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

impl std::fmt::Display for Entry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.kind, self.identity)
    }
}

/// Deletes the remote resource an [`Entry`] names.
///
/// The lane implements this once over whatever client it already holds, and
/// every release goes through it. There is deliberately no "the test deleted it
/// itself" path: two reclamation routes is how one of them stops being
/// exercised.
pub trait Reclaimer: Send + Sync {
    /// Deletes the resource, or reports why it could not.
    ///
    /// Implementations must be idempotent: reclaiming something already gone is
    /// success, because the janitor and the ledger can race.
    ///
    /// # Errors
    ///
    /// Returns [`ReclaimError`] when the resource still exists and could not be
    /// removed.
    fn reclaim(&self, entry: &Entry) -> Result<(), ReclaimError>;
}

/// Why one resource could not be reclaimed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot reclaim {kind} `{identity}`: {reason}")]
pub struct ReclaimError {
    /// What could not be reclaimed.
    pub kind: ResourceKind,
    /// Its identity.
    pub identity: String,
    /// What the reclaimer reported.
    pub reason: String,
}

impl ReclaimError {
    /// A failure for one entry.
    #[must_use]
    pub fn new(entry: &Entry, reason: impl Into<String>) -> Self {
        Self {
            kind: entry.kind,
            identity: entry.identity.clone(),
            reason: reason.into(),
        }
    }
}

/// Why a release did not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseError {
    /// No reclaimer was installed, so nothing could have been deleted.
    #[error(
        "cannot release {kind} `{identity}`: no reclaimer is installed on this ledger, so \
         nothing was deleted; install one with `CleanupLedger::install_reclaimer`"
    )]
    NoReclaimer {
        /// What the caller tried to release.
        kind: ResourceKind,
        /// Its identity.
        identity: String,
    },
    /// The ledger never recorded this resource.
    #[error(
        "cannot release {kind} `{identity}`: the ledger holds no unreleased entry for it; a \
         resource is recorded before the create call returns"
    )]
    NotRecorded {
        /// What the caller tried to release.
        kind: ResourceKind,
        /// Its identity.
        identity: String,
    },
    /// The reclaimer refused or failed. The entry stays unreleased.
    #[error(transparent)]
    Reclaim(#[from] ReclaimError),
}

/// The outcome of reclaiming everything a ledger still holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReclaimSummary {
    /// Entries reclaimed, in the order they were reclaimed.
    pub reclaimed: Vec<Entry>,
    /// Entries that could not be reclaimed, with the reason.
    pub failed: Vec<ReclaimError>,
}

impl ReclaimSummary {
    /// Whether everything the ledger held was reclaimed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
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

/// How many residue reports were emitted while the thread was already
/// panicking.
static RESIDUE_REPORTS_DURING_PANIC: AtomicUsize = AtomicUsize::new(0);

/// The marker every residue report carries, so a log scan finds one whichever
/// channel it came out of.
pub const RESIDUE_MARKER: &str = "AEX-TEST-RESIDUE";

/// How many times a ledger reported residue while its thread was already
/// panicking.
///
/// Panicking again inside `Drop` during an unwind aborts the process and
/// destroys the original failure message, so a residue report raised at that
/// moment goes to stderr and to this counter instead. The check is not stood
/// down - a failing test is exactly when a leak matters - only its channel
/// changes.
#[must_use]
pub fn residue_reports_during_panic() -> usize {
    RESIDUE_REPORTS_DURING_PANIC.load(Ordering::SeqCst)
}

/// Reports residue in the loudest channel available at this instant.
///
/// Not panicking: this panics, and the leak fails the test.
///
/// Already unwinding: panicking again inside a `Drop` aborts the process and
/// destroys the original failure message, so the report goes to stderr and
/// increments [`residue_reports_during_panic`] instead. The check is never
/// stood down - a failing test is exactly when a leak matters most - only its
/// channel changes.
///
/// This is the one implementation. Domain-specific fixture ledgers call it
/// rather than each deciding for themselves what to do while a thread is
/// unwinding; duplicating that decision is how the guard came to be disabled.
///
/// # Panics
///
/// Panics with `report` when the current thread is not already panicking.
pub fn report_residue(report: &str) {
    if std::thread::panicking() {
        RESIDUE_REPORTS_DURING_PANIC.fetch_add(1, Ordering::SeqCst);
        eprintln!("{report}");
    } else {
        panic!("{report}");
    }
}

/// The append-only record of everything a run created, and the path that
/// reclaims it.
pub struct CleanupLedger {
    run_id: TestRunId,
    root: PathBuf,
    entries: Mutex<Vec<Entry>>,
    reclaimer: OnceLock<Arc<dyn Reclaimer>>,
    /// Whether a caller has taken the residue report and become responsible for
    /// it. Set only by [`CleanupLedger::take_residue_report`].
    reported: AtomicBool,
    /// How many entries the file already holds.
    ///
    /// A watermark rather than a `flushed` flag: the file is append-only, so a
    /// flush must write the tail it has not written yet. A boolean cannot
    /// express that, and made a mid-run flush both restate every earlier entry
    /// and silently discard every later one.
    written: Mutex<usize>,
}

impl std::fmt::Debug for CleanupLedger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CleanupLedger")
            .field("run_id", &self.run_id)
            .field("root", &self.root)
            .field("entries", &self.entries)
            .field("reclaimer_installed", &self.reclaimer.get().is_some())
            .finish_non_exhaustive()
    }
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
            reclaimer: OnceLock::new(),
            reported: AtomicBool::new(false),
            written: Mutex::new(0),
        }
    }

    /// Installs the reclamation path. Returns whether this call installed it.
    ///
    /// Installed once and never replaced: a lane that could swap the reclaimer
    /// mid-run could swap in one that deletes nothing.
    pub fn install_reclaimer(&self, reclaimer: Arc<dyn Reclaimer>) -> bool {
        self.reclaimer.set(reclaimer).is_ok()
    }

    /// Whether a reclamation path is installed.
    #[must_use]
    pub fn has_reclaimer(&self) -> bool {
        self.reclaimer.get().is_some()
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

    /// Reclaims the resource and, only if that succeeded, marks it released.
    ///
    /// # Errors
    ///
    /// Returns [`ReleaseError::NoReclaimer`] when no reclamation path is
    /// installed, [`ReleaseError::NotRecorded`] when the ledger holds no
    /// unreleased entry for this kind and identity, and
    /// [`ReleaseError::Reclaim`] when the reclaimer could not delete it. In the
    /// last case the entry stays unreleased and remains residue.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned.
    pub fn release(&self, kind: ResourceKind, identity: &str) -> Result<(), ReleaseError> {
        let Some(reclaimer) = self.reclaimer.get().cloned() else {
            return Err(ReleaseError::NoReclaimer {
                kind,
                identity: identity.to_owned(),
            });
        };
        let candidate = {
            let entries = self
                .entries
                .lock()
                .expect("the cleanup ledger mutex is not poisoned");
            entries
                .iter()
                .rev()
                .find(|entry| {
                    entry.kind == kind && entry.identity == identity && entry.released_at.is_none()
                })
                .cloned()
        };
        let Some(candidate) = candidate else {
            return Err(ReleaseError::NotRecorded {
                kind,
                identity: identity.to_owned(),
            });
        };
        // The reclaim call is made outside the lock: it talks to a remote plane
        // and holding the ledger mutex across it would serialize every other
        // record on one network round trip.
        reclaimer.reclaim(&candidate)?;
        self.stamp_released(kind, identity);
        Ok(())
    }

    fn stamp_released(&self, kind: ResourceKind, identity: &str) {
        let mut entries = self
            .entries
            .lock()
            .expect("the cleanup ledger mutex is not poisoned");
        if let Some(entry) = entries.iter_mut().rev().find(|entry| {
            entry.kind == kind && entry.identity == identity && entry.released_at.is_none()
        }) {
            entry.released_at = Some(OffsetDateTime::now_utc());
        }
    }

    /// Reclaims everything still outstanding, in dependency order.
    ///
    /// The order is [`ResourceKind::reclaim_rank`], ascending, and within one
    /// rank the reverse of record order, so a resource is removed before
    /// whatever it was created under. A failure does not stop the sweep: the
    /// remaining kinds are still attempted and every failure is reported.
    ///
    /// # Panics
    ///
    /// Panics when the ledger mutex was poisoned.
    pub fn reclaim_all(&self) -> ReclaimSummary {
        let mut summary = ReclaimSummary::default();
        let mut outstanding: Vec<(usize, Entry)> = self
            .entries()
            .into_iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.released_at.is_none()
                    && !matches!(entry.expected_terminal, Terminal::Retained { .. })
            })
            .collect();
        outstanding.sort_by(|(left_index, left), (right_index, right)| {
            left.kind
                .reclaim_rank()
                .cmp(&right.kind.reclaim_rank())
                .then_with(|| right_index.cmp(left_index))
        });
        for (_, entry) in outstanding {
            match self.release(entry.kind, &entry.identity) {
                Ok(()) => summary.reclaimed.push(entry),
                Err(ReleaseError::Reclaim(error)) => summary.failed.push(error),
                Err(ReleaseError::NoReclaimer { .. }) => summary.failed.push(ReclaimError::new(
                    &entry,
                    "no reclaimer is installed on this ledger",
                )),
                // Another thread released it between the snapshot and here.
                Err(ReleaseError::NotRecorded { .. }) => {}
            }
        }
        summary
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

    /// Entries that were recorded and never reclaimed.
    ///
    /// An entry whose expected terminal is [`Terminal::Retained`] is not
    /// residue: it is a declared survivor with a stated reason and a stated
    /// deadline.
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

    /// Takes the residue and accepts responsibility for reporting it.
    ///
    /// This is the one way past the loud [`Drop`], and it exists because the
    /// release lane must carry residue into the evidence receipt rather than
    /// abort the process holding it. Taking the report does not make the
    /// residue acceptable: the receipt gate refuses a lane that carries any,
    /// and the janitor still finds it by tag.
    #[must_use]
    pub fn take_residue_report(&self) -> Vec<Entry> {
        self.reported.store(true, Ordering::SeqCst);
        self.residue()
    }

    /// The human-readable residue report, naming every unreclaimed resource.
    #[must_use]
    pub fn residue_report(&self) -> String {
        let residue = self.residue();
        let names: Vec<String> = residue.iter().map(ToString::to_string).collect();
        format!(
            "{RESIDUE_MARKER} run {} left {} unreclaimed resource(s): {}",
            self.run_id,
            names.len(),
            names.join(", ")
        )
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
    /// The `finally`-equivalent half of the contract: write the tail, then say
    /// loudly what was not reclaimed.
    ///
    /// Residue is a failure, not a note in a file under `target/`. When the
    /// thread is not already unwinding this panics and names every leak. When
    /// it is, panicking again would abort the process and destroy the original
    /// failure message, so the same report goes to stderr and increments
    /// [`residue_reports_during_panic`]. The check itself never stands down:
    /// a failing test is exactly when a leak matters most.
    fn drop(&mut self) {
        let written = *self
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The condition is the unwritten tail, not "was flush ever called": a
        // ledger flushed at entry 3 and then given a fourth still owes one line.
        let owes_a_line = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
            > written;
        if owes_a_line && let Err(error) = self.flush() {
            eprintln!(
                "aex-test-harness: cleanup ledger for {} was not written: {error}",
                self.run_id
            );
        }
        if self.reported.load(Ordering::SeqCst) || self.residue().is_empty() {
            return;
        }
        report_residue(&self.residue_report());
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CleanupLedger, Discovery, Entry, Naming, ReclaimError, Reclaimer, ReleaseError,
        ResourceKind, Terminal, TestCaseId, janitor_policy, report_residue,
        residue_reports_during_panic,
    };
    use crate::run::{Lane, TestRunId};
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    /// A reclaimer that records what it was asked to delete, in order.
    #[derive(Debug, Default)]
    struct RecordingReclaimer {
        deleted: Mutex<Vec<String>>,
        refuse: BTreeSet<String>,
    }

    impl RecordingReclaimer {
        fn refusing(identities: &[&str]) -> Self {
            Self {
                deleted: Mutex::new(Vec::new()),
                refuse: identities.iter().map(|id| (*id).to_owned()).collect(),
            }
        }

        fn deleted(&self) -> Vec<String> {
            self.deleted.lock().expect("not poisoned").clone()
        }
    }

    impl Reclaimer for RecordingReclaimer {
        fn reclaim(&self, entry: &Entry) -> Result<(), ReclaimError> {
            if self.refuse.contains(&entry.identity) {
                return Err(ReclaimError::new(entry, "refused by the fixture"));
            }
            self.deleted
                .lock()
                .expect("not poisoned")
                .push(entry.to_string());
            Ok(())
        }
    }

    fn ledger(root: &std::path::Path) -> CleanupLedger {
        CleanupLedger::with_root(TestRunId::mint(), root.to_path_buf())
    }

    fn wired(root: &std::path::Path) -> (CleanupLedger, Arc<RecordingReclaimer>) {
        let ledger = ledger(root);
        let reclaimer = Arc::new(RecordingReclaimer::default());
        assert!(ledger.install_reclaimer(reclaimer.clone()));
        (ledger, reclaimer)
    }

    fn entry(identity: &str) -> Entry {
        kinded(ResourceKind::S3Object, identity)
    }

    fn kinded(kind: ResourceKind, identity: &str) -> Entry {
        Entry::new(
            kind,
            identity,
            Terminal::Deleted,
            TestCaseId("cleanup::demo".to_owned()),
        )
    }

    /// Reports at drop, so the report lands mid-unwind when the enclosing
    /// scope is already panicking.
    struct ReportOnDrop;

    impl Drop for ReportOnDrop {
        fn drop(&mut self) {
            report_residue("a leak during an unwind");
        }
    }

    /// The shared primitive the domain-specific `*-test-support` fixture ledgers call
    /// instead of each deciding for themselves what to do during an unwind.
    #[test]
    fn report_residue_panics_when_it_can_and_counts_when_it_cannot() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| report_residue("a plain leak"));
        std::panic::set_hook(previous);
        let payload = outcome.expect_err("not unwinding, so it panics");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("a plain leak")
        );

        let before = residue_reports_during_panic();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let _guard = ReportOnDrop;
            panic!("the original failure");
        });
        std::panic::set_hook(previous);
        let payload = outcome.expect_err("the original panic propagates");
        assert_eq!(
            payload.downcast_ref::<&str>().copied(),
            Some("the original failure"),
            "reporting must not replace the original failure"
        );
        assert_eq!(residue_reports_during_panic(), before + 1);
    }

    #[test]
    fn every_resource_kind_has_a_policy_row_and_every_row_has_a_kind() {
        let declared: BTreeSet<&str> = janitor_policy()
            .resources
            .keys()
            .map(String::as_str)
            .collect();
        let known: BTreeSet<&str> = ResourceKind::ALL
            .into_iter()
            .map(ResourceKind::as_str)
            .collect();
        assert_eq!(
            declared, known,
            "[janitor.resource] rows and ResourceKind must be the same set"
        );
    }

    #[test]
    fn a_kinds_serde_name_is_its_policy_name() {
        for kind in ResourceKind::ALL {
            let rendered = serde_json::to_string(&kind).expect("a kind serializes");
            assert_eq!(rendered, format!("\"{}\"", kind.as_str()));
            assert_eq!(ResourceKind::parse(kind.as_str()), Some(kind));
        }
    }

    /// The reclamation order is the dependency order or it is nothing: a
    /// prerequisite that outranks its dependent would be reclaimed second.
    #[test]
    fn every_declared_prerequisite_is_reclaimed_strictly_before_its_dependent() {
        for kind in ResourceKind::ALL {
            for prerequisite in kind.requires_reclaim_first() {
                assert!(
                    prerequisite.reclaim_rank() < kind.reclaim_rank(),
                    "{prerequisite} (rank {}) must outrank {kind} (rank {})",
                    prerequisite.reclaim_rank(),
                    kind.reclaim_rank()
                );
            }
            for name in &kind.policy().requires_reclaim_first {
                assert!(
                    ResourceKind::parse(name).is_some(),
                    "{kind} requires unknown kind `{name}` first"
                );
            }
        }
    }

    /// The recurring-charge tail is the one residue that keeps costing money
    /// after the run, so it is reclaimed before anything else can delay it.
    #[test]
    fn the_money_kinds_are_reclaimed_before_everything_else_they_depend_on() {
        assert_eq!(ResourceKind::AutoRechargePolicy.reclaim_rank(), 0);
        assert!(
            ResourceKind::AutoRechargePolicy.reclaim_rank()
                < ResourceKind::PaymentInstrument.reclaim_rank()
        );
        assert!(
            ResourceKind::PaymentInstrument.reclaim_rank()
                < ResourceKind::PaymentCustomer.reclaim_rank()
        );
        assert!(
            ResourceKind::ApiKey.reclaim_rank() < ResourceKind::Workspace.reclaim_rank(),
            "an API key must not outlive the workspace it authorises"
        );
        assert!(
            ResourceKind::Workspace.reclaim_rank() < ResourceKind::Organization.reclaim_rank(),
            "an organization must not go before the workspaces under it"
        );
    }

    #[test]
    fn a_kind_with_no_discovery_route_is_not_reclaimable() {
        assert_eq!(ResourceKind::SqsMessage.discovery(), Discovery::None);
        assert!(!ResourceKind::SqsMessage.reclaimable_from_tags());
        assert_eq!(ResourceKind::ProviderResource.discovery(), Discovery::None);
        assert!(!ResourceKind::ProviderResource.reclaimable_from_tags());
        assert!(ResourceKind::ApiKey.reclaimable_from_tags());
        assert_eq!(ResourceKind::ApiKey.naming(), Naming::AexMinted);
        assert_eq!(
            ResourceKind::PaymentCustomer.naming(),
            Naming::ProviderMinted
        );
    }

    #[test]
    fn releasing_without_a_reclaimer_fails_rather_than_marking_it_released() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        ledger.record(entry("a"));
        let error = ledger
            .release(ResourceKind::S3Object, "a")
            .expect_err("a ledger with no reclamation path cannot release anything");
        assert!(matches!(error, ReleaseError::NoReclaimer { .. }));
        assert_eq!(ledger.residue().len(), 1, "the entry is still residue");
        assert_eq!(ledger.take_residue_report().len(), 1);
    }

    #[test]
    fn release_deletes_the_resource_before_it_marks_the_entry_released() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let (ledger, reclaimer) = wired(root.path());
        ledger.record(entry("a"));
        ledger.record(entry("b"));
        ledger
            .release(ResourceKind::S3Object, "a")
            .expect("the reclaimer accepts");
        assert_eq!(reclaimer.deleted(), vec!["s3_object:a".to_owned()]);
        let residue = ledger.residue();
        assert_eq!(residue.len(), 1);
        assert_eq!(residue[0].identity, "b");
        ledger
            .release(ResourceKind::S3Object, "b")
            .expect("the reclaimer accepts");
    }

    /// The whole point of the change: a reclaimer that could not delete leaves
    /// the entry as residue instead of stamping it released.
    #[test]
    fn a_failed_reclamation_leaves_the_entry_as_residue() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        let reclaimer = Arc::new(RecordingReclaimer::refusing(&["stuck"]));
        assert!(ledger.install_reclaimer(reclaimer));
        ledger.record(entry("stuck"));
        let error = ledger
            .release(ResourceKind::S3Object, "stuck")
            .expect_err("the reclaimer refused");
        assert!(matches!(error, ReleaseError::Reclaim(_)));
        assert_eq!(ledger.residue().len(), 1);
        assert!(ledger.residue()[0].released_at.is_none());
        assert_eq!(ledger.take_residue_report().len(), 1);
    }

    #[test]
    fn releasing_something_never_recorded_is_reported_rather_than_ignored() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let (ledger, _) = wired(root.path());
        let error = ledger
            .release(ResourceKind::S3Object, "never-created")
            .expect_err("nothing was recorded under that identity");
        assert!(matches!(error, ReleaseError::NotRecorded { .. }));
    }

    #[test]
    fn reclaim_all_reclaims_in_dependency_order() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let (ledger, reclaimer) = wired(root.path());
        // Recorded in creation order, which is the opposite of reclaim order.
        ledger.record(kinded(ResourceKind::Organization, "org"));
        ledger.record(kinded(ResourceKind::Workspace, "ws"));
        ledger.record(kinded(ResourceKind::ApiKey, "key"));
        ledger.record(kinded(ResourceKind::PaymentCustomer, "cus"));
        ledger.record(kinded(ResourceKind::PaymentInstrument, "card"));
        ledger.record(kinded(ResourceKind::AutoRechargePolicy, "topup"));
        let summary = ledger.reclaim_all();
        assert!(summary.is_complete(), "{:?}", summary.failed);
        assert_eq!(
            reclaimer.deleted(),
            vec![
                "auto_recharge_policy:topup".to_owned(),
                "payment_instrument:card".to_owned(),
                "api_key:key".to_owned(),
                "workspace:ws".to_owned(),
                "organization:org".to_owned(),
                "payment_customer:cus".to_owned(),
            ]
        );
        assert!(ledger.residue().is_empty());
    }

    #[test]
    fn reclaim_all_reports_what_it_could_not_reclaim_and_still_attempts_the_rest() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let ledger = ledger(root.path());
        let reclaimer = Arc::new(RecordingReclaimer::refusing(&["key"]));
        assert!(ledger.install_reclaimer(reclaimer.clone()));
        ledger.record(kinded(ResourceKind::Organization, "org"));
        ledger.record(kinded(ResourceKind::ApiKey, "key"));
        let summary = ledger.reclaim_all();
        assert!(!summary.is_complete());
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].kind, ResourceKind::ApiKey);
        assert_eq!(
            reclaimer.deleted(),
            vec!["organization:org".to_owned()],
            "a failure in one kind does not stop the rest of the sweep"
        );
        assert_eq!(ledger.take_residue_report().len(), 1);
    }

    #[test]
    fn a_declared_survivor_is_not_residue() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let (ledger, _) = wired(root.path());
        ledger.record(Entry::new(
            ResourceKind::DynamoItem,
            "retained",
            Terminal::Retained {
                reason: "the authority item is removed by a verified 24 h TTL".to_owned(),
                within: Lane::Soak.default_ttl(),
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
        ledger
            .release(ResourceKind::DynamoItem, "expiring")
            .expect("the reclaimer accepts");
    }

    /// `Drop` is the last chance to say a run leaked, so it says it loudly
    /// rather than writing a file nobody reads.
    #[test]
    fn dropping_a_ledger_with_residue_panics_and_names_every_leak() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let (ledger, _) = wired(root.path());
            ledger.record(kinded(ResourceKind::ApiKey, "leaked-key"));
            ledger.record(kinded(ResourceKind::S3Object, "leaked-object"));
        });
        std::panic::set_hook(previous);
        let payload = outcome.expect_err("a ledger holding residue must fail at drop");
        let message = payload
            .downcast_ref::<String>()
            .map_or_else(|| String::from("<non-string panic>"), Clone::clone);
        assert!(message.contains("leaked-key"), "{message}");
        assert!(message.contains("leaked-object"), "{message}");
        assert!(message.contains("2 unreclaimed resource(s)"), "{message}");
    }

    /// F11: the domain-specific copies stood down while panicking, which
    /// disabled the leak check exactly when a test was failing. The check is
    /// kept; only its channel changes, because panicking during an unwind
    /// aborts the process and destroys the original message.
    #[test]
    fn residue_is_still_reported_when_the_thread_is_already_panicking() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let before = residue_reports_during_panic();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let outcome = std::panic::catch_unwind(|| {
            let (ledger, _) = wired(root.path());
            ledger.record(kinded(ResourceKind::ApiKey, "leaked-during-failure"));
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
            "the residue report must not replace the original failure"
        );
        assert_eq!(
            residue_reports_during_panic(),
            before + 1,
            "the leak must still be reported while the thread unwinds"
        );
    }

    /// A resource created after an explicit flush is exactly the resource a
    /// janitor would later find with no ledger row to explain it.
    #[test]
    fn an_entry_recorded_after_an_explicit_flush_still_reaches_the_file() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let path = {
            let (ledger, _) = wired(root.path());
            ledger.record(entry("before"));
            let path = ledger.flush().expect("the ledger flushes").0;
            ledger.record(entry("after"));
            ledger.reclaim_all();
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
        let (ledger, _) = wired(root.path());
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
        ledger.reclaim_all();
    }

    #[test]
    fn a_flushed_ledger_is_append_only_jsonl_that_round_trips() {
        let root = tempfile::tempdir().expect("a temporary directory");
        let (ledger, _) = wired(root.path());
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
        ledger.reclaim_all();
    }
}
