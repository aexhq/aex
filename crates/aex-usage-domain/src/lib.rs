//! `aex-usage-domain` owns the pure immutable usage fact and frontier model: facts,
//! corrections, evidence, identities, closure vectors and frontier order.
//!
//! # Invariants
//!
//! - a fact is immutable; an amendment is a correction fact that names its target
//! - a frontier advances only over a contiguous accepted sequence
//! - the four launch meters are the only representable meters
//!
//! # Not this crate's job
//!
//! - prices, rate cards or money (`aex-usage-rating`, `aex-finance-domain`)
//! - `AWS` or `SQL`
//! - customer `HTTP`

pub mod closure;
pub mod correction;
pub mod fact;
pub mod frontier;
pub mod identity;
pub mod intent;
pub mod interval;
pub mod keys;
pub mod measurement;
pub mod meter;
pub mod projection;
pub mod quantity;
pub mod shape;
pub mod wire_pending;

pub use closure::{ClosureAuthority, ClosureDeclaration, ClosureError, ClosureVector};
pub use correction::{Correction, CorrectionError, CorrectionHead, CorrectionReason};
pub use fact::{FactDraft, FactKind, ObservabilityMeasurement, SchemaVersion, UsageFact};
pub use frontier::{AcceptedSequence, Frontier, FrontierError, FrontierState, PoisonReason};
pub use identity::{
    AuthorityId, AuthorityKey, AuthorityKind, FactId, IdentityError, SegmentOrdinal,
};
pub use intent::{Blake3Digest, IntentHash};
pub use keys::{AuthorityKeys, Item, ItemKey, ItemType, ItemValue, KeyError};
pub use measurement::{
    BoundaryId, Evidence, FactBasis, Measurement, MeasurementError, ReceiptKind, ServiceTime,
    SourceReceipt,
};
pub use meter::{BaseUnit, Category, Meter, ObservabilityMeter, PublicCategory, TokenClass};
pub use projection::{
    Generation, MAX_GENERATION, ProjectionKey, ProjectionKeyError, ProjectionKeys,
};
pub use quantity::{MAX_QUANTITY, Quantity, QuantityError};
pub use shape::{ComputeShape, HandsShape, ShapeUnit};
pub use wire_pending::{ActorRef, Timestamp};
