//! Measured memory pressure, and the receive gate over it.
//!
//! The task's memory envelope is *reserved* rather than measured: [`crate::compose::Envelope`]
//! promises each pool in full, leaves headroom reserved to nobody, and admission refuses an
//! activation whose reservation does not fit. That accounting is exact about the bytes it
//! knows and blind to every byte it does not — allocator fragmentation, a provider client's
//! TLS buffers, an SDK's own arenas. This module closes the gap from the other side: it reads
//! what the kernel says the task is actually using, and stops the task *receiving* before the
//! kernel is the thing that stops it.
//!
//! Three rules shape everything here.
//!
//! **A percentage is never fabricated.** A percentage needs a usage *and* a limit from the
//! same interface. Where no limit is declared — no cgroup mount, `memory.max = max`, the
//! `/proc` fallback, which reports a resident set and nothing that could bound it — the
//! reading says so by name and the gate holds its state. The conservative per-task activation
//! cap is what bounds an unmeasurable task, and it is in force whether or not this file can
//! read anything.
//!
//! **Recovery thresholds sit below entry thresholds.** A single threshold compared twice is a
//! task that oscillates between receiving and not receiving as one activation finishes, which
//! is worse for the queue than either state.
//!
//! **Nothing here kills the process.** Not at 90 %, not ever. A Brain task under memory
//! pressure is usually a task that has already dispatched a provider call it cannot prove was
//! not sent; terminating it converts a recoverable overload into an ambiguous external effect
//! nobody can settle. Pressure removes the task from *new* work and leaves drain and
//! settlement to the rules that already own them.

use crate::control::HealthState;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// How often the task's memory usage is re-measured.
///
/// One second is the reactor window's publication cadence, and a sample costs two small
/// virtual-file reads. The dependency probe's ten seconds is far too slow for this signal: at
/// the launch profile a burst of restores can commit hundreds of mebibytes well inside ten
/// seconds, and those ten seconds are the entire window in which stopping receipt still helps.
pub const SAMPLE_INTERVAL: core::time::Duration = core::time::Duration::from_secs(1);

/// Current usage under the unified cgroup v2 hierarchy.
pub const CGROUP_V2_CURRENT_PATH: &str = "/sys/fs/cgroup/memory.current";

/// Memory ceiling under the unified cgroup v2 hierarchy. Holds `max` when there is none.
pub const CGROUP_V2_MAX_PATH: &str = "/sys/fs/cgroup/memory.max";

/// Current usage under the cgroup v1 memory controller.
pub const CGROUP_V1_CURRENT_PATH: &str = "/sys/fs/cgroup/memory/memory.usage_in_bytes";

/// Memory ceiling under the cgroup v1 memory controller.
pub const CGROUP_V1_MAX_PATH: &str = "/sys/fs/cgroup/memory/memory.limit_in_bytes";

/// The per-process status file the diagnostic fallback reads.
pub const PROC_STATUS_PATH: &str = "/proc/self/status";

/// The field naming the resident set in [`PROC_STATUS_PATH`].
const VM_RSS_FIELD: &str = "VmRSS:";

/// At or above this, a declared limit is a "no limit" sentinel rather than a ceiling.
///
/// cgroup v1 has no textual `max`: an unlimited memory controller reports `PAGE_COUNTER_MAX`,
/// roughly eight exbibytes, and treating that as a ceiling would compute a percentage of zero
/// for any real usage — a fabricated measurement that reads as perfect health. One pebibyte is
/// six orders of magnitude above the largest Fargate task shape that exists, so nothing below
/// it is a sentinel and nothing above it is a task envelope.
const UNLIMITED_LIMIT_FLOOR: u64 = 1 << 50;

/// The interface a reading came from.
///
/// Named rather than inferred from which path answered, because the three interfaces do not
/// mean the same thing: two of them can bound the task and the third can only describe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    /// The unified cgroup v2 hierarchy.
    CgroupV2,
    /// The cgroup v1 memory controller.
    CgroupV1,
    /// `/proc/self/status`. Diagnostic only: it reports the process's resident set and
    /// nothing that could bound it, so it can never produce a percentage.
    ProcStatus,
}

impl MemorySource {
    /// The name this source is reported under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CgroupV2 => "cgroup-v2",
            Self::CgroupV1 => "cgroup-v1",
            Self::ProcStatus => "proc-status",
        }
    }
}

/// The name reported when no interface produced a usable reading.
const UNAVAILABLE_SOURCE: &str = "unavailable";

/// What one sample established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryReading {
    /// Usage and a usable limit, both from `source`. The only reading a percentage exists for.
    Measured {
        /// Which interface answered.
        source: MemorySource,
        /// Bytes the task is using.
        current_bytes: u64,
        /// Bytes the task may use.
        limit_bytes: u64,
    },
    /// Usage is known and nothing declares a limit, so there is no percentage to compute.
    LimitUnavailable {
        /// Which interface answered.
        source: MemorySource,
        /// Bytes the task is using.
        current_bytes: u64,
    },
    /// No interface answered at all.
    Unreadable,
}

impl MemoryReading {
    /// The measured share of the limit in use, rounded down.
    ///
    /// `None` is the whole point of this type: a task with no declared limit has no percentage,
    /// and inventing one — from the release envelope, from a default, from the largest number
    /// seen so far — would make the pressure gate act on a number nothing measured. A value
    /// above 100 is returned as measured rather than clamped: usage genuinely exceeds the limit
    /// for as long as reclaim takes, and hiding that would hide the worst moment.
    #[must_use]
    pub fn used_percent(self) -> Option<u32> {
        match self {
            Self::Measured {
                current_bytes,
                limit_bytes,
                ..
            } => {
                if limit_bytes == 0 {
                    // A zero limit describes nothing. `read_memory` never produces one; a
                    // hand-built reading that does gets the same answer as no limit at all.
                    return None;
                }
                let percent = u128::from(current_bytes) * 100 / u128::from(limit_bytes);
                Some(u32::try_from(percent).unwrap_or(u32::MAX))
            }
            Self::LimitUnavailable { .. } | Self::Unreadable => None,
        }
    }

    /// The source this reading is reported under.
    #[must_use]
    pub const fn source_label(self) -> &'static str {
        match self {
            Self::Measured { source, .. } | Self::LimitUnavailable { source, .. } => {
                source.as_str()
            }
            Self::Unreadable => UNAVAILABLE_SOURCE,
        }
    }
}

/// Whether the last sample produced a percentage.
///
/// Three states rather than a `bool`, because "nothing has been sampled yet" and "sampled, and
/// no limit is declared" are different operational facts: the first is a process that has just
/// started, the second is a process whose gate is blind for as long as it lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureReadability {
    /// No sample has been taken yet.
    Unsampled,
    /// The last sample produced a percentage.
    Measured,
    /// The last sample could not: no interface declared a limit.
    Unavailable,
}

impl PressureReadability {
    const fn code(self) -> u8 {
        match self {
            Self::Unsampled => 0,
            Self::Measured => 1,
            Self::Unavailable => 2,
        }
    }

    const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Unsampled,
            1 => Self::Measured,
            2 => Self::Unavailable,
            // A literal panic rather than `unreachable!`, which is not callable in a
            // `const fn`. One publisher writes this byte and it writes only the codes above.
            _ => panic!("the gate stores only declared readability codes"),
        }
    }

    /// The name this readability is reported under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsampled => "unsampled",
            Self::Measured => "measured",
            Self::Unavailable => UNAVAILABLE_SOURCE,
        }
    }
}

/// The measured memory-pressure state of one task, in increasing severity.
///
/// The thresholds are the accepted first-launch profile's watermarks. What each state *does*
/// is deliberately small for the first alpha: the warning state is an observation, the two
/// above it stop new receipt, and none of them touches work that is already admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PressureState {
    /// Below every watermark. Normal admission.
    Normal,
    /// At or above 70 %. Observed and nothing more: the fold cache is disabled for the first
    /// alpha, so there is no optional retention for this state to release.
    Warning,
    /// At or above 80 %. The task stops receiving and readiness fails.
    AdmissionStop,
    /// At or above 90 %. As [`PressureState::AdmissionStop`], reported separately so an
    /// operator can tell "stopped taking work" from "close to the ceiling". Admitted work
    /// still settles, and the process is never terminated for being here.
    Critical,
}

impl PressureState {
    /// Every state, in increasing severity.
    pub const ALL: [Self; 4] = [
        Self::Normal,
        Self::Warning,
        Self::AdmissionStop,
        Self::Critical,
    ];

    /// The measured percentage at or above which a task enters this state.
    #[must_use]
    pub const fn entry_percent(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Warning => 70,
            Self::AdmissionStop => 80,
            Self::Critical => 90,
        }
    }

    /// The measured percentage below which a task leaves this state.
    ///
    /// Five points under each entry threshold. At the launch task shape that band is roughly
    /// 205 MiB, which is more than the 64 MiB worst-case context restore one activation
    /// reserves: a single activation finishing therefore cannot reopen receipt into the same
    /// pressure it just closed it under. A shared threshold would do exactly that, once per
    /// completion, for as long as the load lasted.
    #[must_use]
    pub const fn recovery_percent(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Warning => 65,
            Self::AdmissionStop => 75,
            Self::Critical => 85,
        }
    }

    /// Whether a task in this state stops taking new deliveries.
    #[must_use]
    pub const fn stops_receiving(self) -> bool {
        matches!(self, Self::AdmissionStop | Self::Critical)
    }

    /// The name this state is reported under.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::AdmissionStop => "admission-stop",
            Self::Critical => "critical",
        }
    }

    /// The most severe state `percent` is high enough to *enter*.
    #[must_use]
    pub fn entered_at(percent: u32) -> Self {
        Self::most_severe(|state| percent >= state.entry_percent())
    }

    /// The most severe state `percent` is high enough to *remain in*.
    #[must_use]
    pub fn retained_at(percent: u32) -> Self {
        Self::most_severe(|state| percent >= state.recovery_percent())
    }

    /// The state a task in `self` moves to when the next sample reads `percent`.
    ///
    /// Between the two thresholds the state is held, which is the hysteresis. Escalation is
    /// not stepwise — a sample at 95 % from [`PressureState::Normal`] lands on
    /// [`PressureState::Critical`] immediately, because pressure that arrives in one sample
    /// must be acted on in one sample. Recovery is not stepwise either: it is bounded by the
    /// recovery thresholds rather than by how many samples have passed, so a task that is
    /// genuinely idle is receiving again on the next sample instead of three later.
    #[must_use]
    pub fn after(self, percent: u32) -> Self {
        // `entered_at <= retained_at` for every percentage, because each recovery threshold is
        // below its entry threshold, so this clamp is total.
        self.clamp(Self::entered_at(percent), Self::retained_at(percent))
    }

    fn most_severe(admits: impl Fn(Self) -> bool) -> Self {
        Self::ALL
            .into_iter()
            .rev()
            .find(|state| admits(*state))
            .unwrap_or(Self::Normal)
    }

    /// The byte the health responder and the gate publish this state as.
    pub(crate) const fn code(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Warning => 1,
            Self::AdmissionStop => 2,
            Self::Critical => 3,
        }
    }

    /// The state `code` was published as.
    pub(crate) const fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Normal,
            1 => Self::Warning,
            2 => Self::AdmissionStop,
            3 => Self::Critical,
            _ => panic!("the gate stores only declared pressure-state codes"),
        }
    }
}

/// One observed change in what the gate publishes.
///
/// A change in whether pressure can be measured is a transition in its own right: a task whose
/// cgroup files stopped answering is not in the state it was last measured in, it is in a state
/// nobody can see, and an operator has to be told once when that starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressureTransition {
    /// The state the task was in.
    pub previous: PressureState,
    /// The state it is in now.
    pub current: PressureState,
    /// Whether the sample that caused this could be measured at all.
    pub readability: PressureReadability,
    /// The sample itself.
    pub reading: MemoryReading,
}

/// The measured pressure state admission and readiness read.
///
/// One publisher — [`PressureSampler`] — and any number of readers. Atomics rather than a lock
/// for the same reason the health responder uses them: a reader must never wait on anything the
/// main runtime holds, least of all when the main runtime is under the load being described.
#[derive(Debug, Default)]
pub struct PressureGate {
    state: AtomicU8,
    readability: AtomicU8,
    samples: AtomicU64,
    transitions: AtomicU64,
}

impl PressureGate {
    /// A gate that has never been sampled.
    ///
    /// It starts at [`PressureState::Normal`]: an unsampled task is not a task under pressure,
    /// and refusing work before anything has been measured would make every start-up a refusal.
    /// The per-task activation cap bounds it in the meantime.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The published state.
    #[must_use]
    pub fn state(&self) -> PressureState {
        PressureState::from_code(self.state.load(Ordering::SeqCst))
    }

    /// Whether the last sample produced a percentage.
    #[must_use]
    pub fn readability(&self) -> PressureReadability {
        PressureReadability::from_code(self.readability.load(Ordering::SeqCst))
    }

    /// Whether measured pressure currently stops the task taking new deliveries.
    #[must_use]
    pub fn stops_receiving(&self) -> bool {
        self.state().stops_receiving()
    }

    /// How many samples have been observed.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::SeqCst)
    }

    /// How many transitions have been published.
    ///
    /// The denominator of the rate bound: diagnostics are emitted once per transition, so this
    /// stays flat while [`PressureGate::samples`] climbs.
    #[must_use]
    pub fn transitions(&self) -> u64 {
        self.transitions.load(Ordering::SeqCst)
    }

    /// Records one sample, returning the transition it caused.
    ///
    /// `None` means the sample changed nothing, which is the ordinary case and the reason a
    /// transition observation is not one per sample.
    pub fn observe(&self, reading: MemoryReading) -> Option<PressureTransition> {
        self.samples.fetch_add(1, Ordering::SeqCst);
        let previous = self.state();
        let (current, readability) = match reading.used_percent() {
            Some(percent) => (previous.after(percent), PressureReadability::Measured),
            // An unmeasurable sample is neither evidence of pressure nor evidence of recovery,
            // so the state is left exactly where the last measured sample put it. Moving it to
            // Normal would silently reopen receipt on no evidence; moving it up would refuse
            // work on no evidence.
            None => (previous, PressureReadability::Unavailable),
        };
        let previous_readability = self.readability();
        self.state.store(current.code(), Ordering::SeqCst);
        self.readability.store(readability.code(), Ordering::SeqCst);
        if current == previous && readability == previous_readability {
            return None;
        }
        self.transitions.fetch_add(1, Ordering::SeqCst);
        Some(PressureTransition {
            previous,
            current,
            readability,
            reading,
        })
    }
}

/// Reads the virtual files a memory sample comes from.
///
/// A trait rather than `std::fs` directly. The deployment target is Linux under Fargate and no
/// development host here is: a suite that read the real filesystem would assert nothing on the
/// machine the code is written on, something different in CI, and neither would be the
/// behaviour that ships.
pub trait MemoryFiles: Send + Sync + core::fmt::Debug {
    /// The contents of `path`, or `None` when this host cannot supply it.
    fn read(&self, path: &str) -> Option<String>;
}

/// The host's own files.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostMemoryFiles;

impl MemoryFiles for HostMemoryFiles {
    fn read(&self, path: &str) -> Option<String> {
        // An absent interface and an unreadable one lead to the same named place — a reading
        // that says so, published to health and emitted once — so they do not need to be told
        // apart here. Nothing is swallowed: the terminal outcome is reported, not defaulted.
        std::fs::read_to_string(path).ok()
    }
}

/// Reads the task's memory usage from the first interface that answers.
///
/// The order is cgroup v2, then cgroup v1, then `/proc/self/status`. It is an order of
/// authority rather than of preference: only the first two can state what the task is allowed
/// to use, and the third is reached only to report a resident set nothing bounds.
#[must_use]
pub fn read_memory(files: &dyn MemoryFiles) -> MemoryReading {
    if let Some(reading) = cgroup_reading(
        files,
        MemorySource::CgroupV2,
        CGROUP_V2_CURRENT_PATH,
        CGROUP_V2_MAX_PATH,
    ) {
        return reading;
    }
    if let Some(reading) = cgroup_reading(
        files,
        MemorySource::CgroupV1,
        CGROUP_V1_CURRENT_PATH,
        CGROUP_V1_MAX_PATH,
    ) {
        return reading;
    }
    match resident_bytes(files) {
        Some(current_bytes) => MemoryReading::LimitUnavailable {
            source: MemorySource::ProcStatus,
            current_bytes,
        },
        None => MemoryReading::Unreadable,
    }
}

/// One cgroup interface's answer, or `None` when this host does not have that interface.
///
/// A usage file that answers is what makes the interface present. Its limit file is read
/// second and is allowed to say there is no limit, which is a different answer from the
/// interface being absent and must not fall through to the next one: a v2 host with
/// `memory.max = max` has no v1 controller to consult.
fn cgroup_reading(
    files: &dyn MemoryFiles,
    source: MemorySource,
    current_path: &str,
    limit_path: &str,
) -> Option<MemoryReading> {
    let current_bytes = files.read(current_path).as_deref().and_then(parse_bytes)?;
    let limit_bytes = files.read(limit_path).as_deref().and_then(usable_limit);
    Some(match limit_bytes {
        Some(limit_bytes) => MemoryReading::Measured {
            source,
            current_bytes,
            limit_bytes,
        },
        None => MemoryReading::LimitUnavailable {
            source,
            current_bytes,
        },
    })
}

fn parse_bytes(raw: &str) -> Option<u64> {
    raw.trim().parse::<u64>().ok()
}

/// A declared limit that can bound the task, or `None` for every value that cannot.
///
/// cgroup v2 writes the literal `max` when there is no limit and cgroup v1 writes a sentinel
/// near `u64::MAX`; a zero limit would permit nothing at all and is not a shape any live task
/// has. All three are the same answer to this module: there is no ceiling to be a percentage of.
fn usable_limit(raw: &str) -> Option<u64> {
    match parse_bytes(raw) {
        Some(0) | None => None,
        Some(limit) if limit >= UNLIMITED_LIMIT_FLOOR => None,
        Some(limit) => Some(limit),
    }
}

/// The process's resident set, from `/proc/self/status`.
///
/// The file reports `VmRSS` in kibibytes, spelled `kB`.
fn resident_bytes(files: &dyn MemoryFiles) -> Option<u64> {
    let status = files.read(PROC_STATUS_PATH)?;
    let field = status
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(VM_RSS_FIELD))?;
    let kibibytes = field.split_whitespace().next()?.parse::<u64>().ok()?;
    kibibytes.checked_mul(1_024)
}

/// The plane and region an emitted transition is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// Deployment plane this process belongs to.
    pub plane: String,
    /// `AWS` region this process is bound to.
    pub region: String,
}

/// Samples memory pressure and publishes it to admission, readiness and diagnostics.
#[derive(Debug)]
pub struct PressureSampler {
    health: Arc<HealthState>,
    gate: Arc<PressureGate>,
    files: Arc<dyn MemoryFiles>,
    identity: ProcessIdentity,
    interval: core::time::Duration,
}

/// Everything one sampler publishes to.
///
/// A struct rather than five positional arguments, so a caller cannot transpose the gate
/// admission reads with one nothing reads.
#[derive(Debug)]
pub struct PressureBindings {
    /// Readiness, which fails while receipt is stopped.
    pub health: Arc<HealthState>,
    /// The gate admission reads. It must be the one the admission controller minted.
    pub gate: Arc<PressureGate>,
    /// Where a sample is read from.
    pub files: Arc<dyn MemoryFiles>,
    /// What a transition is attributed to.
    pub identity: ProcessIdentity,
}

impl PressureSampler {
    /// Binds a sampler at the production cadence.
    #[must_use]
    pub fn new(bindings: PressureBindings) -> Self {
        Self::with_interval(bindings, SAMPLE_INTERVAL)
    }

    /// As [`PressureSampler::new`], at a cadence the caller chooses.
    ///
    /// Only this module's own assertions call this. Production uses [`SAMPLE_INTERVAL`], which
    /// is why it is a constant rather than configuration.
    #[must_use]
    pub fn with_interval(bindings: PressureBindings, interval: core::time::Duration) -> Self {
        Self {
            health: bindings.health,
            gate: bindings.gate,
            files: bindings.files,
            identity: bindings.identity,
            interval,
        }
    }

    /// The gate this sampler publishes to.
    #[must_use]
    pub fn gate(&self) -> &Arc<PressureGate> {
        &self.gate
    }

    /// Takes one sample, publishes it, and reports the transition it caused.
    ///
    /// Readiness is republished on every sample rather than only on a transition. It is one
    /// atomic store, and it means the answer a load balancer gets never depends on a
    /// diagnostic event.
    #[must_use]
    pub fn round(&self) -> Option<PressureTransition> {
        let reading = read_memory(self.files.as_ref());
        let transition = self.gate.observe(reading);
        self.health.observe_pressure(self.gate.state());
        if let Some(transition) = transition {
            self.emit(transition);
        }
        transition
    }

    /// Samples now, then every interval until aborted.
    ///
    /// The first sample runs before the first sleep, so the state a probe reads is measured
    /// rather than assumed. The loop never returns: it is a child of the composition root's
    /// structured shutdown scope, which aborts and joins it as part of drain, so it cannot
    /// still be publishing after the process has reported it stopped answering.
    pub async fn run(self: Arc<Self>) {
        loop {
            let _ = self.round();
            tokio::time::sleep(self.interval).await;
        }
    }

    /// Reports one transition. Exactly one record per transition, never one per sample.
    fn emit(&self, transition: PressureTransition) {
        tracing::info!(
            target: "aex::diagnostics",
            event = "brain_memory_pressure_transitioned",
            plane = %self.identity.plane,
            region = %self.identity.region,
            deployable = crate::DEPLOYABLE,
            previous_state = transition.previous.as_str(),
            state = transition.current.as_str(),
            memory_source = transition.reading.source_label(),
            memory_used_percent = transition.reading.used_percent(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CGROUP_V1_CURRENT_PATH, CGROUP_V1_MAX_PATH, CGROUP_V2_CURRENT_PATH, CGROUP_V2_MAX_PATH,
        MemoryFiles, MemoryReading, MemorySource, PROC_STATUS_PATH, PressureBindings, PressureGate,
        PressureReadability, PressureSampler, PressureState, ProcessIdentity, SAMPLE_INTERVAL,
        read_memory,
    };
    use crate::control::HealthState;
    use crate::health::{LIVE_PATH, READY_PATH};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// The exact set of virtual files a host exposes, and nothing else.
    #[derive(Debug, Default)]
    struct FakeFiles {
        contents: std::sync::Mutex<BTreeMap<String, String>>,
    }

    impl FakeFiles {
        fn with(entries: &[(&str, &str)]) -> Arc<Self> {
            let files = Self::default();
            for (path, body) in entries {
                files.set(path, body);
            }
            Arc::new(files)
        }

        fn set(&self, path: &str, body: &str) {
            let mut contents = self
                .contents
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            contents.insert((*path).to_owned(), (*body).to_owned());
        }
    }

    impl MemoryFiles for FakeFiles {
        fn read(&self, path: &str) -> Option<String> {
            self.contents
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(path)
                .cloned()
        }
    }

    /// The launch task's memory limit, which is what a deployed cgroup declares.
    const LIMIT_BYTES: u64 = 4 * 1_024 * 1_024 * 1_024;

    /// The fewest bytes that measure as `percent` of [`LIMIT_BYTES`].
    ///
    /// Rounded up, because the measurement itself rounds down: the largest byte count below
    /// the exact share measures as one point less, and a threshold test that asked for 80 %
    /// and got 79 % would be asserting the rounding rather than the watermark.
    fn bytes_at(percent: u64) -> u64 {
        (LIMIT_BYTES * percent).div_ceil(100)
    }

    /// A cgroup v2 host at `percent` of the launch task's memory limit.
    fn cgroup_v2_at(percent: u64) -> Arc<FakeFiles> {
        FakeFiles::with(&[
            (CGROUP_V2_CURRENT_PATH, &bytes_at(percent).to_string()),
            (CGROUP_V2_MAX_PATH, &LIMIT_BYTES.to_string()),
        ])
    }

    fn sampler(files: &Arc<FakeFiles>) -> (Arc<HealthState>, PressureSampler) {
        let health = HealthState::starting(50, 32);
        health.bindings_validated();
        health.schema_matched();
        health.store_reachable(true);
        let sampler = PressureSampler::new(PressureBindings {
            health: Arc::clone(&health),
            gate: Arc::new(PressureGate::new()),
            files: Arc::clone(files) as Arc<dyn MemoryFiles>,
            identity: ProcessIdentity {
                plane: "dev".to_owned(),
                region: "eu-west-1".to_owned(),
            },
        });
        (health, sampler)
    }

    /// The accepted first-launch watermarks. They are what the guardrail *is*, so they are
    /// pinned here rather than left to be inferred from the transition table.
    #[test]
    fn the_watermarks_are_the_approved_first_launch_profile() {
        assert_eq!(PressureState::Normal.entry_percent(), 0);
        assert_eq!(PressureState::Warning.entry_percent(), 70);
        assert_eq!(PressureState::AdmissionStop.entry_percent(), 80);
        assert_eq!(PressureState::Critical.entry_percent(), 90);
    }

    /// Every recovery threshold is below its own entry threshold, and both ladders climb with
    /// severity. This is the invariant that makes the hysteresis clamp total: violate it and a
    /// state could be entered and left at the same percentage.
    #[test]
    fn every_recovery_threshold_sits_below_the_entry_threshold_it_recovers_from() {
        for state in PressureState::ALL {
            if state == PressureState::Normal {
                continue;
            }
            assert!(
                state.recovery_percent() < state.entry_percent(),
                "{state:?} recovers at or above the percentage it is entered at"
            );
        }
        for pair in PressureState::ALL.windows(2) {
            assert!(pair[0].entry_percent() < pair[1].entry_percent());
            assert!(pair[0].recovery_percent() < pair[1].recovery_percent());
            assert!(pair[0] < pair[1], "severity must order the states");
        }
    }

    /// The exact boundary in each direction. One below an entry threshold is not the state, and
    /// exactly at it is.
    #[test]
    fn each_state_is_entered_at_its_threshold_and_not_one_point_below() {
        assert_eq!(PressureState::Normal.after(69), PressureState::Normal);
        assert_eq!(PressureState::Normal.after(70), PressureState::Warning);
        assert_eq!(PressureState::Warning.after(79), PressureState::Warning);
        assert_eq!(
            PressureState::Warning.after(80),
            PressureState::AdmissionStop
        );
        assert_eq!(
            PressureState::AdmissionStop.after(89),
            PressureState::AdmissionStop
        );
        assert_eq!(
            PressureState::AdmissionStop.after(90),
            PressureState::Critical
        );
    }

    /// Recovery uses the lower ladder. Between the two thresholds the state is held, which is
    /// the whole reason two ladders exist: at 78 % a task that was stopped stays stopped, while
    /// a task that never stopped does not start stopping.
    #[test]
    fn recovery_uses_the_lower_thresholds_so_the_state_cannot_flap() {
        assert_eq!(
            PressureState::AdmissionStop.after(78),
            PressureState::AdmissionStop,
            "78 % is above the 75 % recovery threshold, so receipt stays stopped"
        );
        assert_eq!(
            PressureState::Warning.after(78),
            PressureState::Warning,
            "the same 78 % does not stop a task that was only warned"
        );
        assert_eq!(
            PressureState::AdmissionStop.after(74),
            PressureState::Warning
        );
        assert_eq!(PressureState::Warning.after(65), PressureState::Warning);
        assert_eq!(PressureState::Warning.after(64), PressureState::Normal);
        assert_eq!(PressureState::Critical.after(85), PressureState::Critical);
        assert_eq!(
            PressureState::Critical.after(84),
            PressureState::AdmissionStop
        );
    }

    /// Pressure that arrives in one sample is acted on in one sample, and a task that is
    /// genuinely idle again is receiving on the next one rather than three later.
    #[test]
    fn escalation_and_recovery_are_both_immediate_rather_than_stepwise() {
        assert_eq!(PressureState::Normal.after(95), PressureState::Critical);
        assert_eq!(PressureState::Critical.after(10), PressureState::Normal);
    }

    /// The two upper states stop receipt; the warning state is an observation only.
    #[test]
    fn only_the_two_upper_states_stop_receipt() {
        assert!(!PressureState::Normal.stops_receiving());
        assert!(!PressureState::Warning.stops_receiving());
        assert!(PressureState::AdmissionStop.stops_receiving());
        assert!(PressureState::Critical.stops_receiving());
    }

    #[test]
    fn a_percentage_is_the_measured_share_of_the_declared_limit() {
        assert_eq!(
            MemoryReading::Measured {
                source: MemorySource::CgroupV2,
                current_bytes: 800,
                limit_bytes: 1_000,
            }
            .used_percent(),
            Some(80)
        );
        assert_eq!(
            MemoryReading::Measured {
                source: MemorySource::CgroupV2,
                current_bytes: 1_100,
                limit_bytes: 1_000,
            }
            .used_percent(),
            Some(110),
            "usage over the limit is reported rather than clamped"
        );
    }

    /// The rule the whole module is built around. No limit means no percentage: not zero, not
    /// the release envelope, not the last number seen.
    #[test]
    fn a_reading_without_a_usable_limit_has_no_percentage_at_all() {
        for reading in [
            MemoryReading::LimitUnavailable {
                source: MemorySource::ProcStatus,
                current_bytes: 4_000,
            },
            MemoryReading::Unreadable,
            MemoryReading::Measured {
                source: MemorySource::CgroupV1,
                current_bytes: 4_000,
                limit_bytes: 0,
            },
        ] {
            assert_eq!(reading.used_percent(), None, "{reading:?}");
        }
    }

    #[test]
    fn cgroup_v2_is_read_before_cgroup_v1() {
        let files = FakeFiles::with(&[
            (CGROUP_V2_CURRENT_PATH, "512"),
            (CGROUP_V2_MAX_PATH, "1024"),
            (CGROUP_V1_CURRENT_PATH, "1"),
            (CGROUP_V1_MAX_PATH, "1024"),
        ]);
        assert_eq!(
            read_memory(files.as_ref()),
            MemoryReading::Measured {
                source: MemorySource::CgroupV2,
                current_bytes: 512,
                limit_bytes: 1_024,
            }
        );
    }

    #[test]
    fn cgroup_v1_is_read_where_the_unified_hierarchy_is_absent() {
        let files = FakeFiles::with(&[
            (CGROUP_V1_CURRENT_PATH, "2048"),
            (CGROUP_V1_MAX_PATH, "4096"),
        ]);
        assert_eq!(
            read_memory(files.as_ref()),
            MemoryReading::Measured {
                source: MemorySource::CgroupV1,
                current_bytes: 2_048,
                limit_bytes: 4_096,
            }
        );
    }

    /// `/proc/self/status` is the diagnostic fallback: it reports a resident set and nothing
    /// that could bound it, so it is reached last and never produces a percentage.
    #[test]
    fn proc_status_reports_a_resident_set_and_never_a_limit() {
        let files = FakeFiles::with(&[(
            PROC_STATUS_PATH,
            "Name:\tbrain-mux\nVmHWM:\t  900 kB\nVmRSS:\t  512 kB\nThreads:\t8\n",
        )]);
        let reading = read_memory(files.as_ref());
        assert_eq!(
            reading,
            MemoryReading::LimitUnavailable {
                source: MemorySource::ProcStatus,
                current_bytes: 512 * 1_024,
            }
        );
        assert_eq!(reading.used_percent(), None);
    }

    /// An unlimited cgroup declares no ceiling, in either of the two spellings the kernel uses.
    /// Treating the v1 sentinel as a ceiling would compute a percentage near zero for any real
    /// usage, which is a fabricated measurement that reads as perfect health.
    #[test]
    fn an_unlimited_cgroup_declares_no_ceiling_in_either_spelling() {
        let unified =
            FakeFiles::with(&[(CGROUP_V2_CURRENT_PATH, "700"), (CGROUP_V2_MAX_PATH, "max")]);
        assert_eq!(
            read_memory(unified.as_ref()),
            MemoryReading::LimitUnavailable {
                source: MemorySource::CgroupV2,
                current_bytes: 700,
            }
        );

        let legacy = FakeFiles::with(&[
            (CGROUP_V1_CURRENT_PATH, "700"),
            (CGROUP_V1_MAX_PATH, "9223372036854771712"),
        ]);
        assert_eq!(
            read_memory(legacy.as_ref()),
            MemoryReading::LimitUnavailable {
                source: MemorySource::CgroupV1,
                current_bytes: 700,
            }
        );
    }

    /// A host with none of the three interfaces — every development machine here — reads as
    /// unreadable rather than as a healthy zero.
    #[test]
    fn a_host_without_any_of_the_interfaces_reads_as_unreadable() {
        let files = FakeFiles::with(&[]);
        assert_eq!(read_memory(files.as_ref()), MemoryReading::Unreadable);
        assert_eq!(read_memory(files.as_ref()).source_label(), "unavailable");
    }

    /// An unmeasurable sample is not evidence of recovery. A gate that had measured pressure
    /// and then went blind holds what it measured; the activation cap bounds it either way.
    #[test]
    fn an_unmeasurable_sample_holds_the_state_the_last_measured_one_left() {
        let gate = PressureGate::new();
        let _ = gate.observe(MemoryReading::Measured {
            source: MemorySource::CgroupV2,
            current_bytes: 85,
            limit_bytes: 100,
        });
        assert_eq!(gate.state(), PressureState::AdmissionStop);

        let transition = gate
            .observe(MemoryReading::Unreadable)
            .expect("losing measurability is itself a transition");
        assert_eq!(transition.readability, PressureReadability::Unavailable);
        assert_eq!(transition.current, PressureState::AdmissionStop);
        assert_eq!(
            gate.state(),
            PressureState::AdmissionStop,
            "no evidence is not evidence of recovery"
        );
        assert!(gate.stops_receiving());
    }

    /// One record per transition, not one per sample. A gate that emitted every sample would
    /// bury the transition it exists to report under sixty records a minute.
    #[test]
    fn a_transition_is_published_once_however_many_samples_repeat_it() {
        let gate = PressureGate::new();
        let steady = MemoryReading::Measured {
            source: MemorySource::CgroupV2,
            current_bytes: 82,
            limit_bytes: 100,
        };
        assert!(
            gate.observe(steady).is_some(),
            "the first sample transitions"
        );
        for _ in 0..20 {
            assert_eq!(gate.observe(steady), None);
        }
        assert_eq!(gate.samples(), 21);
        assert_eq!(gate.transitions(), 1);
    }

    /// The whole loop, on a host that can measure: one transition observation at 70 %, one more
    /// when receipt stops at 80 %, one more at 90 %, and one on the way back down.
    #[tokio::test(flavor = "current_thread")]
    async fn each_watermark_crossing_emits_exactly_one_observation() {
        let files = cgroup_v2_at(10);
        let (_health, sampler) = sampler(&files);
        assert!(
            sampler.round().is_some(),
            "the first sample is a transition"
        );

        for (percent, expected) in [
            (72, PressureState::Warning),
            (72, PressureState::Warning),
            (81, PressureState::AdmissionStop),
            (95, PressureState::Critical),
            (10, PressureState::Normal),
        ] {
            files.set(CGROUP_V2_CURRENT_PATH, &bytes_at(percent).to_string());
            let _ = sampler.round();
            assert_eq!(sampler.gate().state(), expected, "at {percent}%");
        }

        assert_eq!(sampler.gate().samples(), 6);
        assert_eq!(
            sampler.gate().transitions(),
            5,
            "the repeated 72 % sample is not a transition"
        );
    }

    /// A transition into an unmeasurable state carries no percentage. A zero would read as an
    /// idle task, which is the opposite of what "we cannot see" means.
    #[tokio::test(flavor = "current_thread")]
    async fn an_unmeasurable_transition_reports_its_source_and_no_percentage() {
        let files = FakeFiles::with(&[]);
        let (_health, sampler) = sampler(&files);
        let transition = sampler.round().expect("unreadable is a transition");
        assert_eq!(transition.reading.source_label(), "unavailable");
        assert_eq!(transition.reading.used_percent(), None);
        assert_eq!(
            sampler.gate().state(),
            PressureState::Normal,
            "an unmeasurable host is not a task under pressure"
        );
        assert_eq!(
            sampler.gate().readability(),
            PressureReadability::Unavailable
        );
    }

    /// The measured stop reaches the load balancer through readiness and nothing else. Liveness
    /// holds throughout: the task is settling effects it owns, and killing it for the memory
    /// those effects need is the one outcome this guardrail exists to prevent.
    #[tokio::test(flavor = "current_thread")]
    async fn a_measured_stop_takes_the_task_out_of_service_and_recovery_puts_it_back() {
        let files = cgroup_v2_at(10);
        let (health, sampler) = sampler(&files);
        let _ = sampler.round();
        assert_eq!(health.respond("GET", READY_PATH).status, 200);

        for percent in [72, 81, 95] {
            files.set(CGROUP_V2_CURRENT_PATH, &bytes_at(percent).to_string());
            let _ = sampler.round();
            let expected = if percent < 80 { 200 } else { 503 };
            assert_eq!(
                health.respond("GET", READY_PATH).status,
                expected,
                "readiness at {percent}%"
            );
            assert_eq!(
                health.respond("GET", LIVE_PATH).status,
                200,
                "liveness at {percent}%"
            );
        }
        assert!(
            health
                .respond("GET", READY_PATH)
                .body
                .contains("memory pressure critical")
        );

        // 76 % is above the 75 % recovery threshold: still stopped, on the same measurement
        // that would not have stopped a task which never crossed 80 %.
        files.set(CGROUP_V2_CURRENT_PATH, &bytes_at(76).to_string());
        let _ = sampler.round();
        assert_eq!(health.respond("GET", READY_PATH).status, 503);

        files.set(CGROUP_V2_CURRENT_PATH, &bytes_at(74).to_string());
        let _ = sampler.round();
        assert_eq!(health.respond("GET", READY_PATH).status, 200);
        assert_eq!(sampler.gate().state(), PressureState::Warning);
    }

    /// The sampler is a child of the shutdown scope, not a detached background task: once it is
    /// aborted and joined it can never publish again, so nothing is still moving readiness after
    /// the process has reported it stopped answering.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn an_aborted_and_joined_sampler_never_publishes_again() {
        let files = cgroup_v2_at(10);
        let (_health, sampler) = sampler(&files);
        let sampler = Arc::new(sampler);
        let running = tokio::spawn(Arc::clone(&sampler).run());

        tokio::task::yield_now().await;
        assert_eq!(
            sampler.gate().samples(),
            1,
            "the first sample runs before the first sleep"
        );
        tokio::time::advance(SAMPLE_INTERVAL).await;
        tokio::task::yield_now().await;
        assert!(sampler.gate().samples() >= 2);

        running.abort();
        let _ = running.await;
        let settled = sampler.gate().samples();
        tokio::time::advance(SAMPLE_INTERVAL * 5).await;
        tokio::task::yield_now().await;
        assert_eq!(sampler.gate().samples(), settled);
    }

    #[test]
    fn the_alpha_cadence_is_a_background_schedule_rather_than_a_request_path_check() {
        assert_eq!(SAMPLE_INTERVAL.as_secs(), 1);
    }
}
