//! The shared load and soak substrate.
//!
//! `D-11`: `tests/load/` holds this one package plus descriptors, tier profiles
//! and fixtures. The executors themselves are `[[test]] name = "load"` targets
//! inside the live companion that owns the subject, so a load run needs no
//! credential broader than that companion already has, and the executor sits
//! next to the system it measures.
//!
//! What lives here is everything two load owners would otherwise implement
//! twice and incomparably: the arrival process, the driver that walks it, the
//! recorder that produces the mandatory metric set, the resource sampler, and
//! the workload descriptor with its gate contract.
//!
//! # Invariants
//!
//! - a gate is a **budget** or a **diagnostic**; `kind = "slo"` is rejected at
//!   parse time, because `Q-PERFORMANCE` forbids a customer percentile promise
//!   as a prelaunch release gate;
//! - every descriptor reports the full mandatory metric set, so two campaigns
//!   are comparable;
//! - the arrival process is a pure function of `(descriptor, seed)`, so a
//!   campaign can be replayed exactly;
//! - a sampler that cannot read a counter on this platform returns an error, it
//!   never reports zero.

pub mod arrival;
pub mod driver;
pub mod gate;
pub mod recorder;
pub mod sampler;
pub mod workload;

pub use arrival::{Arrival, ArrivalSchedule, Mix, Offer, Rng};
pub use driver::{Driver, DriverError, OfferOutcome, Workload};
pub use gate::{GateOutcome, GateReport, GateVerdict};
pub use recorder::{Metric, Recorder, Summary};
pub use sampler::{ProcProbe, ResourceProbe, Sample, SamplerError};
pub use workload::{Gate, GateKind, Phase, WorkloadDescriptor, WorkloadError};
