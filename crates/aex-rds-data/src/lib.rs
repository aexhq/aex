//! `aex-rds-data` owns the focused Aurora Data `API` transport: request and parameter
//! mapping, paging, transaction lifecycle and error classification.
//!
//! # Invariants
//!
//! - a commit whose outcome is unknown is reported as ambiguous, never as success
//! - parameters are typed and bound; no statement is assembled by string concatenation
//! - retry classification is explicit per error family, with no blanket retry
//! - floating point cannot cross this boundary in either direction: [`SqlValue`] has
//!   no `f64` variant and a `doubleValue` arriving in any column is a hard decode
//!   failure rather than a lossy conversion
//! - a response is size-checked before it is decoded, so a paging bug surfaces as
//!   [`DataApiError::ResultTooLarge`] rather than as a truncated read
//!
//! # Not this crate's job
//!
//! - schema knowledge: table and column names belong to the owning `*-aurora` adapter
//! - native `PostgreSQL` connections (`central-schema-admin` owns the `sqlx` path)
//! - business transactions or authority decisions
//! - retry policy: this crate classifies, the application decides

pub mod aws;
pub mod client;
pub mod config;
pub mod error;
pub mod params;
pub mod record;
pub mod transaction;

pub use aws::AwsTransport;
pub use client::{DataApiClient, ExecuteResponse, Transport, TransportError};
pub use config::{RdsDataConfigError, DataApiConfig, DatabaseName, ResourceArn, SecretArn};
pub use error::ExceptionKind;
pub use error::{DataApiError, DecodeError, SqlState};
pub use params::{SqlValue, Statement, sql};
pub use record::{Record, Row};
pub use transaction::{CommitFailure, Committed, Isolation, Transaction, TransactionId};
