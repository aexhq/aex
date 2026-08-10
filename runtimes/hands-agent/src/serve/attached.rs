//! Guest-side attached serving: frame/fence decision, unlocked execution, and answer.

use super::{
    Arc, AttachResponse, Bound, Bytes, DeliveryMode, Dispatch, FrameError, FrameExpectation, Guest,
    HandsOperationId, IntoResponse, JournalError, OperationMeta, Response, ResponseStatus,
    ResultResponse, StartDecision, StartInput, StartRequest, State, StatusCode, StatusResponse,
    Verb, decode_request, frame_response, journal_error, now, protocol_error,
};

/// How many terminal bytes an attached answer carries on the connection itself.
///
/// Base64 keeps this inside the shared 1 MiB frame ceiling. A larger body is not
/// truncated; it remains resumably pullable from the same journal and digest.
pub const ATTACH_CHUNK_BYTES: u64 = 180_000;

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
        let expectation = FrameExpectation {
            generation: state.supervisor.generation(),
            min_fence: state.supervisor.fence_floor(),
            schema_version: aex_hands_agent::wire::PROTOCOL_V1,
            max_frame_bytes: 1_048_576,
        };
        let decoded = match decode_request(&body, &expectation) {
            Ok(frame) => frame,
            Err(error) => return protocol_error(guest.as_ref(), state, Verb::Attach, &error),
        };
        if decoded.preamble.verb != Verb::Attach {
            return protocol_error(
                guest.as_ref(),
                state,
                Verb::Attach,
                &FrameError::Malformed {
                    at: "verb",
                    reason: format!(
                        "the frame declares {:?} but was posted to {}",
                        decoded.preamble.verb,
                        Verb::Attach.path()
                    ),
                },
            );
        }
        state.supervisor.adopt_fence(decoded.preamble.fence);
        match decide_attach(guest.as_ref(), state, decoded.payload) {
            Ok((AttachStep::Answered(response), _)) => {
                return encode_attach(guest.as_ref(), state, &response);
            }
            Ok(settled) => settled,
            Err(error) => return protocol_error(guest.as_ref(), state, Verb::Attach, &error),
        }
    };

    let existing = matches!(step, AttachStep::Recorded);
    if let AttachStep::Run(meta) = step
        && let Err(error) = run_attached(&guest, meta).await
    {
        let bound = guest.bound.read().await;
        return match bound.as_ref() {
            Some(state) => protocol_error(guest.as_ref(), state, Verb::Attach, &error),
            None => (StatusCode::SERVICE_UNAVAILABLE, "the binding was replaced").into_response(),
        };
    }

    answer_attached(&guest, operation, existing).await
}

/// The fenced decision half of an attach, before work starts.
fn decide_attach(
    guest: &Guest,
    state: &mut Bound,
    payload: &[u8],
) -> Result<(AttachStep, HandsOperationId), FrameError> {
    let request: StartRequest =
        serde_json::from_slice(payload).map_err(|_| FrameError::Malformed {
            at: "payload",
            reason: "the payload is not the request this verb takes".to_owned(),
        })?;
    if request.delivery != DeliveryMode::Attached {
        return Err(FrameError::Malformed {
            at: "delivery",
            reason: "attach is the attached delivery mode; a detached start is posted to /start"
                .to_owned(),
        });
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
async fn run_attached(guest: &Arc<Guest>, meta: Box<OperationMeta>) -> Result<(), FrameError> {
    let operation = meta.operation;
    let runner = Arc::clone(guest);
    let dispatched = tokio::task::spawn_blocking(move || {
        runner
            .executor
            .dispatch(&runner.journal, &meta, DeliveryMode::Attached, now())
    })
    .await
    .map_err(|error| FrameError::Malformed {
        at: "attach",
        reason: format!("the attached operation did not join: {error}"),
    })?
    .map_err(|error| journal_error(&error))?;
    if let Dispatch::Terminal(terminal) = dispatched {
        match guest.journal.record_terminal(operation, &terminal) {
            Ok(()) | Err(JournalError::AlreadyTerminal { .. }) => {}
            Err(error) => return Err(journal_error(&error)),
        }
    }
    Ok(())
}

/// Reads the authoritative terminal record and frames it for the held connection.
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
    let revision = state.supervisor.guest_revision();
    let response = match state.supervisor.result(operation, 0, ATTACH_CHUNK_BYTES) {
        Ok(ResultResponse::Terminal { terminal, chunk }) => AttachResponse::Terminal {
            operation,
            guest_revision: revision,
            existing,
            terminal,
            chunk,
        },
        Ok(ResultResponse::NotTerminal {
            operation: found,
            state: observed,
        }) => AttachResponse::NotTerminal {
            operation: found,
            guest_revision: revision,
            state: observed,
        },
        Ok(ResultResponse::Unknown { operation: found }) => AttachResponse::NotTerminal {
            operation: found,
            guest_revision: revision,
            state: Box::new(StatusResponse::Unknown { operation: found }),
        },
        Err(error) => {
            return protocol_error(guest.as_ref(), state, Verb::Attach, &journal_error(&error));
        }
    };
    encode_attach(guest.as_ref(), state, &response)
}

/// Frames one attach answer.
fn encode_attach(guest: &Guest, state: &Bound, response: &AttachResponse) -> Response {
    match serde_json::to_vec(response) {
        Ok(payload) => frame_response(
            guest,
            state,
            Verb::Attach,
            ResponseStatus::Payload,
            &payload,
        ),
        Err(_) => protocol_error(
            guest,
            state,
            Verb::Attach,
            &FrameError::Malformed {
                at: "response",
                reason: "the attach answer could not be encoded".to_owned(),
            },
        ),
    }
}
