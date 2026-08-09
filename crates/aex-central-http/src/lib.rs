//! `aex-central-http` owns the generated central `HTTP` composition: route tables, request
//! limits, authentication extraction and the public error mapping.
//!
//! # Invariants
//!
//! - every route is generated from the contract bundle; a hand-added route is a build failure
//! - a composition that is not behind an authenticating gateway verifies the credential itself,
//!   in-process, and never trusts an ambient context map
//! - a body larger than the declared limit is rejected before it is buffered
//! - an internal error never leaks a database or vendor message to the public wire
//! - an unreadable account state **rejects** admission; there is no stale fallback
//! - a deployable cannot link a capability it did not declare, and finds out at start-up
//!
//! # Not this crate's job
//!
//! - business logic: handlers delegate to application crates
//! - regional routes (`aex-regional-http`)
//! - the wire types themselves (`aex-wire`)

pub mod admission;
pub mod authorizer;
pub mod capability;
pub mod config;
pub mod cursor;
pub mod error;
pub mod headers;
pub mod health;
pub mod router;
pub mod target;

pub use admission::{
    CentralAuthenticator, CredentialAdmission, CredentialPeppers, PurposedPeppers,
};
pub use authorizer::{CentralAuthorizerContext, ContextError, ContextPrincipalKind};
pub use capability::{CompositionError, CompositionManifest, ResolvedConfig, admit};
pub use config::{
    CentralHttpConfigError, CentralServiceId, DeploymentPlane, HttpConfig, central_groups,
};
pub use cursor::{PageBinding, next_cursor, page_request};
pub use error::EdgeError;
pub use headers::DeclaredHeaders;
pub use health::{Dependency, Readiness};
pub use router::{
    Admitted, EdgeStack, mount_api_keys_api, mount_auth_api, mount_billing_api,
    mount_bootstrap_api, mount_central_operations_api, mount_identity_api, mount_organizations_api,
    mount_workspaces_api, mounted_routes,
};
pub use target::{ControlStoreTargets, TargetPath, TargetResolver, admit_request};
