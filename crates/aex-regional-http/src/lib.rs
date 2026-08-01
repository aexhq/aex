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
pub mod capability;
pub mod config;
pub mod context;
pub mod cursor;
pub mod edge;
pub mod envelope;
pub mod error;
pub mod health;
pub mod idempotency;
pub mod limits;
pub mod mount;
pub mod page;
pub mod router;
pub mod stream;

pub use capability::{CompositionManifest, admit};
pub use config::{ConfigError, Environment, Lookup};
pub use context::{EffectiveLimits, RegionalAuthorization, RequestContext};
pub use cursor::{CursorBinding, decode, encode};
pub use edge::{
    EdgeBinding, EdgeClock, ProjectedState, ProjectionError, ProjectionReader, RegionalEdge,
    SystemClock,
};
pub use idempotency::IdempotencyIdentity;
pub use mount::{
    AdmissionRequest, EdgeAdmission, MountError, Mounted, UnaryDispatch, mount_unary, not_served,
    render, render_error,
};
pub use router::{EdgeStack, RouteOwner, route_owner};
