//! `aex-regional-http` owns the generated finite regional `HTTP` composition: route tables,
//! region assertion extraction, body limits and the public error mapping.
//!
//! # Invariants
//!
//! - every route is generated from the contract bundle; the route set is finite and closed
//! - a request without a valid current region assertion is rejected before any handler runs
//! - the composition links no forbidden capability: a link-time check proves it
//!
//! # Not this crate's job
//!
//! - business logic: handlers delegate to application crates
//! - central routes (`aex-central-http`)
//! - the wire types themselves (`aex-wire`)

pub mod assertion;
pub mod assertion_flight;
pub mod authz;
pub mod capability;
pub mod config;
pub mod context;
pub mod cursor;
pub mod drain;
pub mod edge;
pub mod envelope;
pub mod error;
pub mod health;
pub mod idempotency;
pub mod limits;
pub mod mount;
pub mod page;
pub mod projection;
pub mod release_health;
pub mod router;
pub mod stream;

pub use authz::{
    LambdaAssertionSource, ParameterStore, RegionalProjection, TrustError, parse_trust_anchors,
};
pub use capability::{CompositionManifest, admit};
pub use config::{Environment, Lookup, RegionalHttpConfigError};
pub use context::{EffectiveLimits, RegionalAuthorization, RequestContext};
pub use cursor::{CursorBinding, decode, encode};
pub use drain::{
    DRAIN_EXIT_MARGIN_MS, FARGATE_STOP_TIMEOUT_S, MAX_DRAIN_DEADLINE_MS, MIN_DRAIN_DEADLINE_MS,
};
pub use edge::{
    EdgeBinding, EdgeClock, ProjectedState, ProjectionError, ProjectionReader, RegionalEdge,
    SystemClock,
};
pub use idempotency::IdempotencyIdentity;
pub use mount::{
    AdmissionRequest, EdgeAdmission, MountError, Mounted, UnaryDispatch, mount_unary, not_served,
    render, render_error,
};
pub use projection::ProjectionError as WireProjectionError;
pub use router::{EdgeStack, RouteOwner, route_owner};
