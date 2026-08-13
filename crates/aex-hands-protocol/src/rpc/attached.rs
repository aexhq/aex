//! The typed answer carried by attached delivery.

use serde::{Deserialize, Serialize};

use crate::operation::{OperationFailure, TerminalMetadata};

use super::{CallHash, HandsOperationId, ResultChunk, StatusResponse};

/// What the guest answers an `attach` with.
///
/// This is deliberately the union of the start decision and the terminal result.
/// The body also stays pullable, so a lost connection loses no durable result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "response",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AttachResponse {
    /// The operation reached a terminal state inside this call.
    Terminal {
        /// Which operation.
        operation: HandsOperationId,
        /// Whether the guest found the operation already recorded.
        existing: bool,
        /// How it ended.
        terminal: TerminalMetadata,
        /// The body, absent only when the operation produced none.
        chunk: Option<ResultChunk>,
    },
    /// The operation was already open and has not finished.
    NotTerminal {
        /// Which operation.
        operation: HandsOperationId,
        /// What the guest does know.
        state: Box<StatusResponse>,
    },
    /// The operation exists under a different call hash.
    Conflict {
        /// Which operation.
        operation: HandsOperationId,
        /// The hash the guest already has.
        recorded_call_hash: CallHash,
    },
    /// The guest refused. Nothing was started.
    Rejected {
        /// Which operation.
        operation: HandsOperationId,
        /// Why.
        failure: OperationFailure,
    },
}
