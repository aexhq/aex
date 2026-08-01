//! `aex-runtime-control` owns the pure Hands lifecycle and true-idle model, including the
//! exact 179999/180000 millisecond idle boundary and generation fencing.
//!
//! # Invariants
//!
//! - true idle requires a proven 180000 ms of inactivity; 179999 ms is not idle
//! - a generation is monotonic and a stale generation is always rejected
//! - lifecycle transitions are total functions of the recorded state
//!
//! # Not this crate's job
//!
//! - the compute provider (`aex-hands-control-aws`)
//! - activity storage (`aex-runtime-activity-dynamodb`)
//! - queues, schedules or wall-clock reads
//!
//! Every value here is a plain function of its inputs. There is no clock, no
//! `tokio`, no AWS client and no I/O, which is what makes the 180000 ms boundary
//! testable as an exhaustive table rather than as a timing observation.

pub mod clock;
pub mod generation;
pub mod idle;
pub mod lifecycle;
pub mod pressure;
pub mod shape;
pub mod store;
pub mod usage;

pub use clock::{millis_between, minus_millis, plus_millis};
pub use generation::{
    AdmissionRefused, Admitted, FenceVerdict, GUEST_ROOT, GenerationHead, GenerationState,
    HandsGeneration, ImageCapability, ImageIdentifier, ImagePin, ImageVersion, InvalidTransition,
    LimitsRevision, NetworkPolicy, Revision, TransportMode, evaluate_request_binding, guest_root,
    is_canonical_root, may_incorporate_result, next_fence, supersedes,
};
pub use idle::{
    IDLE_EVALUATION_JITTER_MS, IdleAssessment, KEEPALIVE_MAX_MS, KeepaliveRefused,
    TRUE_IDLE_THRESHOLD_MS, issue_keepalive, lease_holds_at,
};
pub use lifecycle::{
    IntentRecord, IntentState, LIFETIME_DRAIN_MARGIN_MS, LIFETIME_TERMINATE_MARGIN_MS,
    LifecycleAction, LifecycleIntentId, Lifetime, LifetimeVerdict, MicrovmId,
    MissingProviderEvidence, ORPHAN_GRACE_MS, PROVIDER_LIFETIME_MS, ProviderCall, ProviderQuotaId,
    ProviderState, RECONCILE_ATTEMPTS, ReconcileStep, RedactedDetail, ResumeRefused, RetryPolicy,
    TransientClass, UnmodelledProviderState, client_token, snapshot_lifecycle_id,
};
pub use pressure::{
    PRESSURE_HIGH_WATER, PRESSURE_LOW_WATER, PressureCandidate, PressurePlan, plan_release,
};
pub use shape::{
    MEMORY_GIB_MICRO_USD_PER_HOUR, ShapeCapacity, UnknownComputeSize, VCPU_MICRO_USD_PER_HOUR,
    parse_compute_size,
};
pub use store::{
    GenerationCommit, GenerationPlan, GenerationPointer, IdleProbe, LifecycleIntentPlan,
    LifecycleReceipt, LifecycleReceiptPlan, PageBudget, RuntimeActivityStore, RuntimeDuePage,
    RuntimeShard, RuntimeStoreError,
};
pub use usage::{
    DerivationError, FactContext, HandsUsage, SinkError, SnapshotIo, SnapshotResidence,
    UsageCategory, UsageFactSink, category_of, derive_facts, derive_usage,
};
