//! Guest-side attached serving: bounded fence decision, unlocked execution, and answer.

use super::{
    Arc, AttachResponse, Bound, Bytes, DeliveryMode, Dispatch, Guest, GuestError, HandsOperationId,
    IntoResponse, JournalError, MAX_RESULT_CHUNK_BYTES, OperationMeta, Response, ResultResponse,
    StartDecision, StartInput, StartRequest, State, StatusCode, StatusResponse,
    decode_guest_request, encode_guest_response, journal_error, json_bytes_response, now,
    protocol_error,
};

/// How many terminal bytes an attached answer carries on the connection itself.
///
/// Base64 keeps this inside the shared 1 MiB JSON ceiling. A larger body is not
/// truncated; it remains resumably pullable from the same journal and digest.
pub const ATTACH_CHUNK_BYTES: u64 = MAX_RESULT_CHUNK_BYTES;

/// What the fenced decision phase settled before anything runs.
enum AttachStep {
    /// This call owns the recorded operation and must run it.
    Run(Box<OperationMeta>),
    /// The operation was already recorded; answer from the journal.
    Recorded,
    /// The guest conflicted or refused. Nothing was started.
    Answered(Box<AttachResponse>),
}

/// Holds the caller's connection while an attached operation reaches its result.
///
/// The binding lock is held only for the fenced decision. Execution happens on
/// the blocking pool without the lock, then the answer is read from the journal.
pub(super) async fn attach_handler(State(guest): State<Arc<Guest>>, body: Bytes) -> Response {
    let (step, operation) = {
        let mut bound = guest.bound.write().await;
        let Some(state) = bound.as_mut() else {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "no generation is bound; the run hook has not completed",
            )
                .into_response();
        };
        let request = match decode_guest_request::<StartRequest>(&body, state) {
            Ok(request) => request,
            Err(error) => return protocol_error(&error),
        };
        match decide_attach(guest.as_ref(), state, &request) {
            Ok((AttachStep::Answered(response), _)) => {
                return encode_attach(&response, state);
            }
            Ok(settled) => settled,
            Err(error) => return protocol_error(&error),
        }
    };

    let existing = matches!(step, AttachStep::Recorded);
    if let AttachStep::Run(meta) = step
        && let Err(error) = run_attached(&guest, meta).await
    {
        let bound = guest.bound.read().await;
        return match bound.as_ref() {
            Some(_) => protocol_error(&error),
            None => (StatusCode::SERVICE_UNAVAILABLE, "the binding was replaced").into_response(),
        };
    }

    answer_attached(&guest, operation, existing).await
}

/// The fenced decision half of an attach, before work starts.
fn decide_attach(
    guest: &Guest,
    state: &mut Bound,
    request: &StartRequest,
) -> Result<(AttachStep, HandsOperationId), GuestError> {
    if request.delivery != DeliveryMode::Attached {
        return Err(GuestError::invalid(
            "attach is the attached delivery mode; a detached start is posted to /start",
        ));
    }
    let operation = request.operation;
    let input = StartInput {
        operation,
        call_hash: request.call_hash,
        request: request.request.clone(),
        bounds: request.bounds,
        deadline: request.deadline,
        open_operations: 0,
        browser_available: state.browser,
    };
    let decision = state
        .supervisor
        .decide_start(&input, now())
        .map_err(|error| journal_error(&error))?;
    let step = match decision {
        StartDecision::Spawn { meta } => {
            guest
                .journal
                .record_start(&meta)
                .map_err(|error| journal_error(&error))?;
            AttachStep::Run(meta)
        }
        StartDecision::AlreadyStarted | StartDecision::AlreadyTerminal { .. } => {
            AttachStep::Recorded
        }
        StartDecision::Conflict { recorded_call_hash } => {
            AttachStep::Answered(Box::new(AttachResponse::Conflict {
                operation,
                recorded_call_hash,
            }))
        }
        StartDecision::Refused(refusal) => {
            AttachStep::Answered(Box::new(AttachResponse::Rejected {
                operation,
                failure: refusal.failure(),
            }))
        }
    };
    Ok((step, operation))
}

/// Runs one attached operation to its terminal record, holding no binding lock.
async fn run_attached(guest: &Arc<Guest>, meta: Box<OperationMeta>) -> Result<(), GuestError> {
    let operation = meta.operation;
    let runner = Arc::clone(guest);
    let dispatched = tokio::task::spawn_blocking(move || {
        runner
            .executor
            .dispatch(&runner.journal, &meta, DeliveryMode::Attached, now())
    })
    .await
    .map_err(|error| GuestError::internal(format!("the attached operation did not join: {error}")))?
    .map_err(|error| journal_error(&error))?;
    if let Dispatch::Terminal(terminal) = dispatched {
        match guest.journal.record_terminal(operation, &terminal) {
            Ok(()) | Err(JournalError::AlreadyTerminal { .. }) => {}
            Err(error) => return Err(journal_error(&error)),
        }
    }
    Ok(())
}

/// Reads the authoritative terminal record for the held connection.
async fn answer_attached(
    guest: &Arc<Guest>,
    operation: HandsOperationId,
    existing: bool,
) -> Response {
    let bound = guest.bound.read().await;
    let Some(state) = bound.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the binding was replaced while the attached operation ran",
        )
            .into_response();
    };
    let response = match state.supervisor.result(operation, 0, ATTACH_CHUNK_BYTES) {
        Ok(ResultResponse::Terminal { terminal, chunk }) => AttachResponse::Terminal {
            operation,
            existing,
            terminal,
            chunk,
        },
        Ok(ResultResponse::NotTerminal {
            operation: found,
            state: observed,
        }) => AttachResponse::NotTerminal {
            operation: found,
            state: observed,
        },
        Ok(ResultResponse::Unknown { operation: found }) => AttachResponse::NotTerminal {
            operation: found,
            state: Box::new(StatusResponse::Unknown { operation: found }),
        },
        Err(error) => {
            return protocol_error(&journal_error(&error));
        }
    };
    encode_attach(&response, state)
}

/// Encodes one attached answer as bounded typed JSON.
fn encode_attach(response: &AttachResponse, state: &Bound) -> Response {
    match encode_guest_response(response, state) {
        Ok(payload) => json_bytes_response(payload),
        Err(error) => protocol_error(&error),
    }
}
