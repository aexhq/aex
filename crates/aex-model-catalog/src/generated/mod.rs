//! The generated models.dev admit table.
//!
//! `admit.rs` is emitted by `scripts/gen-models.ts` from the vendored
//! `release/models-dev/api.json` snapshot and is never edited by hand. CI
//! regenerates it and asserts the committed bytes match.

pub use admit::{
    AdmittedModel, GENERATED_FROM_SHA256, MODELS, PROVIDERS, ProviderMeta, SNAPSHOT_DIGEST,
};

mod admit;
