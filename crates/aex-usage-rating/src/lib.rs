//! `aex-usage-rating` owns the pure exact rating policy: the immutable rate-card schema,
//! exact rational arithmetic and deterministic settlement rounding.
//!
//! # Invariants
//!
//! - arithmetic is exact rational; rounding happens once, at the declared settlement boundary
//! - rating a segmented interval equals rating the whole interval (segmentation invariance)
//! - the rating context is immutable: the same inputs always produce the same rated amount
//!
//! # Not this crate's job
//!
//! - account mutation or balance posting (`aex-finance-domain`)
//! - provider credentials, invoices or negotiated cost values
//! - regional storage, queues or the usage authority tables

pub mod exact;
pub mod rate_card;
