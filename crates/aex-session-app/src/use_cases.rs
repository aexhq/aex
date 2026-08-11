//! The use cases.
//!
//! Every one of them reads through ports, decides in the domain, and returns
//! exactly one [`SessionTransaction`] plus the projection the caller is told.
//! None of them commits: there is no committer in [`AppContext`], so "one
//! command = one transaction, no external call inside it" is enforced by the
//! type of the context rather than by review.
//!
//! Replayable admissions strongly read their receipt before mutable planning
//! dependencies. Fresh work then passes authorization and the pause gate.

use std::collections::BTreeSet;

use aex_internal_contracts::RunId;
use aex_operation_domain::DeletionState;
use aex_session_domain::{
    CommandClass, Message, MessageRole, MessageState, QueueRun, ReceiptKey, ReceiptOutcome,
    ReplayDecision, ResourceId, ResourceKind, ResponseBody, Run, Session, SessionDomainRunError,
    SessionStatus, TerminalAttempt, WorkAdmission, claim_terminal, pause_gate, queue, replay,
    resolve_message_bounds, start as start_run_domain,
};
use aex_wire::PrefixedId as _;
use aex_wire::error::ErrorCode;
use aex_wire::ids::{AgentId, GenerationId, MessageId, SessionId, WorkspaceId};
use sha2::Digest as _;

use crate::error::AppError;
use crate::plan::{Condition, Hint, Planned, SessionTransaction, TransactionIntent, Write};
use crate::ports::{AppContext, PortError};

/// Largest public text message admitted inline for MVP.
///
/// Brain journal entries are limited to 32,768 bytes; 24 KiB leaves bounded
/// room for the canonical record envelope and `DynamoDB` item metadata.
pub const MESSAGE_TEXT_MAX_BYTES: usize = 24_576;

fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// Admit a message and the run it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct SendMessage {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// Canonical idempotency envelope.
    pub identity: aex_session_domain::IdempotencyIdentity,
    /// Text-only public request.
    pub request: aex_wire::models::MessageSendRequest,
}

/// Either a new atomic admission plan or exact stored response bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageAdmissionOutcome {
    /// A new user message and session activity were planned together.
    Planned(Box<Planned<(Message, Session)>>),
    /// Exact replay before any mutable session/account/credential read.
    Replayed {
        /// Stored wire response.
        response: Box<aex_wire::models::MessageSendResult>,
        /// Exact canonical bytes sent by the winner.
        canonical_response: Vec<u8>,
    },
}

/// Start an admitted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartRun {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// Which run.
    pub run: RunId,
}

/// Settle a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitTerminal {
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which session.
    pub session: SessionId,
    /// The run and its outcome.
    pub attempt: TerminalAttempt,
    /// The agent that produced it.
    pub agent: AgentId,
    /// The open messages of that run.
    pub open_messages: Vec<Message>,
}

async fn gate(
    context: &AppContext<'_>,
    session: &Session,
    class: CommandClass,
) -> Result<aex_session_domain::AccountProjection, AppError> {
    // Fresh admission is gated after the strong receipt-first replay read.
    let projection = context.accounts.projection(session.organization).await?;
    pause_gate(class, &projection)?;
    Ok(projection)
}

fn live_conditions(session: &Session) -> Vec<Condition> {
    vec![
        Condition::SessionRevision {
            session: session.id,
            expected: session.revision,
        },
        Condition::DeletionState {
            session: session.id,
            expected: DeletionState::Live,
            epoch: session.deletion.epoch,
        },
    ]
}

/// Admits a message and the run it starts.
///
/// The plan carries the message, the run, the session advance, the root wake,
/// the idempotency receipt and the outbox event **together**: no history yields
/// a durable message without a wake.
///
/// # Errors
///
/// Returns [`AppError`] when the account is paused, a port read fails, or the
/// domain refuses the admission.
#[expect(
    clippy::too_many_lines,
    reason = "the receipt-first admission and its fixed eleven-action authority are one atomic product invariant"
)]
pub async fn admit_message(
    context: &AppContext<'_>,
    command: &SendMessage,
) -> Result<MessageAdmissionOutcome, AppError> {
    if command.request.text.is_empty() || command.request.text.len() > MESSAGE_TEXT_MAX_BYTES {
        return Err(AppError::Conflict(ErrorCode::InvalidRequest));
    }

    let receipt_scope = format!("session.message:{}", command.session);
    let now = context.clock.now();
    if let Some(stored) = context
        .sessions
        .load_receipt(command.workspace, &receipt_scope, &command.identity, now)
        .await?
    {
        return match replay(&stored, &command.identity.intent()) {
            ReplayDecision::Conflict(code) => Err(AppError::Conflict(code)),
            ReplayDecision::ReturnOriginal(ReceiptOutcome::Resource {
                kind: ResourceKind::Message,
                id,
                response,
            }) => {
                let bytes = response.inline().ok_or(AppError::Port(PortError::Corrupt {
                    kind: "message admission receipt",
                    reason: "the stored message response is not inline",
                }))?;
                let response: aex_wire::models::MessageSendResult = serde_json::from_slice(bytes)
                    .map_err(|_| {
                    AppError::Port(PortError::Corrupt {
                        kind: "message admission receipt",
                        reason: "the stored message response is malformed",
                    })
                })?;
                if id.0 != response.message.id.to_string() || response.session.id != command.session
                {
                    return Err(AppError::Port(PortError::Corrupt {
                        kind: "message admission receipt",
                        reason: "the stored resource identity disagrees with its response",
                    }));
                }
                Ok(MessageAdmissionOutcome::Replayed {
                    response: Box::new(response),
                    canonical_response: bytes.to_vec(),
                })
            }
            ReplayDecision::ReturnOriginal(_) => Err(AppError::Port(PortError::Corrupt {
                kind: "message admission receipt",
                reason: "the stored receipt does not name a message resource",
            })),
        };
    }
    let materialized = context
        .sessions
        .load_message_snapshot(command.workspace, command.session)
        .await?;
    if !materialized.root.idle {
        return Err(AppError::Conflict(ErrorCode::SessionNotIdle));
    }
    let account = gate(
        context,
        &materialized.session,
        CommandClass::PausableMutation,
    )
    .await?;

    let credential = context
        .credentials
        .read_provider_credential(
            command.workspace,
            materialized.session.provider_credential.credential,
        )
        .await?
        .ok_or(AppError::Conflict(ErrorCode::ProviderCredentialNotFound))?;
    if credential.state == crate::ports::CredentialState::Revoked
        || credential.credential != materialized.session.provider_credential.credential
        || credential.provider != materialized.session.provider_credential.provider
        || credential.source_generation
            != materialized.session.provider_credential.source_generation
        || credential.revision != materialized.session.provider_credential.revision
    {
        return Err(AppError::Conflict(ErrorCode::ProviderCredentialRevoked));
    }

    let bounds = resolve_message_bounds(
        command
            .request
            .max_spend_cents
            .map(aex_wire::types::Cents::get),
        command.request.deadline,
        now,
        materialized.session.lifecycle.expires_at,
    )
    .map_err(|_| AppError::Conflict(ErrorCode::InvalidRequest))?;
    let message_id = MessageId::from_uuid7(context.ids.next_uuid_v7());
    let run_id = RunId::from_uuid7(context.ids.next_uuid_v7());
    let commit = queue(
        run_id,
        &QueueRun {
            message: message_id,
            max_spend_cents: bounds.max_spend_cents,
            deadline: bounds.deadline,
        },
        &materialized.session,
        now,
    )?;

    let message = Message {
        id: message_id,
        session: command.session,
        run: Some(run_id),
        agent: materialized.root.agent,
        role: MessageRole::User,
        // A user message is born sealed.
        state: MessageState::Sealed,
        parts: vec![aex_session_domain::MessagePart::Text {
            text: command.request.text.clone(),
        }],
        created_at: now,
        sealed_at: Some(now),
    };

    let canonical_text = aex_model_catalog::BoundedString::<
        { aex_model_catalog::canonical::TEXT_MAX },
    >::new(command.request.text.clone())
    .map_err(|_| AppError::Conflict(ErrorCode::InvalidRequest))?;
    let run_admitted = aex_brain_domain::JournalRecord::RunAdmitted {
        run: run_id,
        message: message_id,
        content: vec![aex_brain_domain::wire_pending::ContentBlockRef::Inline {
            block: aex_brain_domain::wire_pending::CanonicalBlock::Text {
                text: canonical_text,
                annotations: Vec::new(),
            },
        }],
        max_spend_cents: bounds.max_spend_cents.get(),
        deadline: aex_brain_domain::Timestamp::from_millis(bounds.deadline.unix_millis()),
    };
    let journal_body = run_admitted
        .canonical_bytes()
        .map_err(|_| AppError::Conflict(ErrorCode::InvalidRequest))?;
    if journal_body.len() > aex_brain_domain::journal::INLINE_BODY_BYTES {
        return Err(AppError::Conflict(ErrorCode::InvalidRequest));
    }
    let entry_hash = run_admitted
        .content_hash()
        .map_err(|_| AppError::Conflict(ErrorCode::InvalidRequest))?;
    let entry_identity = aex_session_domain::EntryIdentity::from_bytes(entry_hash.0);
    let journal_entry = aex_session_domain::JournalEntry {
        agent: materialized.root.agent,
        seq: materialized.root.journal_tail.next(),
        kind: aex_internal_contracts::journal::JournalEntryKind::RunAdmitted,
        identity: entry_identity,
        body: aex_session_domain::JournalBody::Inline(journal_body),
        fact: aex_session_domain::AuthorityFact::None,
        recorded_at: now,
    };
    let wake_suffix = run_id.uuid7().encode_suffix();
    let wake_suffix = std::str::from_utf8(&wake_suffix).map_err(|_| {
        AppError::Port(PortError::Corrupt {
            kind: "root wake identity",
            reason: "an internal UUIDv7 did not render as Crockford ASCII",
        })
    })?;
    let mut wake_digest = sha2::Sha256::new();
    wake_digest.update(b"aex.agent.wake.v1\0");
    wake_digest.update(command.session.to_string().as_bytes());
    wake_digest.update(b"\0");
    wake_digest.update(materialized.root.agent.to_string().as_bytes());
    let wake = crate::plan::AgentWake {
        work_id: format!("wrk_{wake_suffix}"),
        dedupe_key: lowercase_hex(&wake_digest.finalize()),
        session: command.session,
        agent: materialized.root.agent,
        from: journal_entry.seq,
        cancellation: materialized.session.cancellation,
        at: now,
    };
    let event_body = aex_wire::canonical::to_jcs_bytes(&serde_json::json!({
        "deadline": bounds.deadline,
        "maxSpendCents": bounds.max_spend_cents.get(),
        "messageId": message_id,
        "sessionId": command.session,
        "sessionRevision": materialized.session.revision.next().0,
    }))?;
    let admitted_event = crate::plan::MessageAdmittedEvent {
        id: aex_wire::ids::ObservationId::from_uuid7(context.ids.next_uuid_v7()),
        session: command.session,
        sequence: journal_entry.seq.0.saturating_mul(1_024),
        body: event_body,
        at: now,
    };

    let mut head = materialized.session.clone();
    head.lifecycle
        .admit_message(message_id, run_id, bounds, now)?;
    head.status = head.lifecycle.status;
    head.active_run = Some(run_id);
    head.revision = materialized.session.revision.next();
    head.updated_at = now;

    let response = aex_wire::models::MessageSendResult {
        message: crate::projection::public_message(&message)?,
        session: crate::projection::public_session(&head)?,
    };
    let canonical_response = aex_wire::canonical::to_jcs_bytes(&response)?;
    let receipt = aex_session_domain::IdempotencyReceipt {
        key: ReceiptKey::of(&receipt_scope, &command.identity).map_err(|_| {
            AppError::Port(PortError::Corrupt {
                kind: "message idempotency scope",
                reason: "the session message scope is not a usable receipt key",
            })
        })?,
        identity: command.identity.clone(),
        intent: command.identity.intent(),
        outcome: ReceiptOutcome::Resource {
            kind: ResourceKind::Message,
            id: ResourceId(message.id.to_string()),
            response: ResponseBody::of(&canonical_response),
        },
        created_at: now,
        expires_at: None,
    };

    let mut conditions = live_conditions(&materialized.session);
    conditions.push(Condition::SessionActiveRun {
        session: command.session,
        expected: None,
    });
    conditions.push(Condition::WorkAdmission {
        session: command.session,
        expected: WorkAdmission::Open,
    });
    conditions.push(Condition::MutationGuardFree {
        session: command.session,
    });
    conditions.push(Condition::CancellationEpoch {
        session: command.session,
        expected: materialized.session.cancellation,
    });
    conditions.push(Condition::AccountRevisionAtLeast {
        workspace: command.workspace,
        organization: materialized.session.organization,
        at_least: account.revision,
    });
    conditions.push(Condition::AgentRevision {
        session: command.session,
        agent: materialized.root.agent,
        expected: materialized.root.revision,
    });
    conditions.push(Condition::JournalTail {
        session: command.session,
        agent: materialized.root.agent,
        expected: materialized.root.journal_tail,
    });
    conditions.push(Condition::RootAgentIdle {
        session: command.session,
        agent: materialized.root.agent,
    });
    conditions.push(Condition::ItemAbsent(
        Write::PutAgentWake(Box::new(wake.clone())).target(),
    ));
    conditions.push(Condition::ItemAbsent(
        Write::PutAgentWakeDedupe(Box::new(wake.clone())).target(),
    ));
    let plan = SessionTransaction {
        intent: TransactionIntent::AdmitMessage,
        conditions,
        writes: vec![
            Write::PutMessage(Box::new(message.clone())),
            Write::PutSealedMessage(Box::new(message.clone())),
            Write::PutRun(Box::new(commit.run.clone())),
            Write::PutSessionHead(Box::new(head.clone())),
            Write::AdmitRootRun {
                session: command.session,
                agent: materialized.root.agent,
                from_revision: materialized.root.revision,
                to_revision: materialized.root.revision.next(),
                from_tail: materialized.root.journal_tail,
                to_tail: materialized.root.journal_tail.next(),
                entry_identity,
                at: now,
            },
            Write::AppendJournalPage {
                session: command.session,
                page: Box::new(aex_session_domain::JournalPage {
                    agent: materialized.root.agent,
                    first: journal_entry.seq,
                    entries: vec![journal_entry],
                }),
            },
            Write::PutAgentWakeDedupe(Box::new(wake.clone())),
            Write::PutAgentWake(Box::new(wake)),
            Write::PutMessageAdmittedEvent(Box::new(admitted_event)),
            Write::PutIdempotencyReceipt(Box::new(receipt)),
        ],
        after_commit: vec![Hint::WakeAgent {
            agent: materialized.root.agent,
            reason: aex_internal_contracts::wake::WakeHint::SessionWork {
                session: command.session,
            },
        }],
    };
    plan.validate()?;

    Ok(MessageAdmissionOutcome::Planned(Box::new(Planned {
        plan,
        projected: (message, head),
    })))
}

/// Starts an admitted run.
///
/// # Errors
///
/// Returns [`AppError`] when the account is paused, a port read fails, or the
/// run cannot start.
pub async fn start_run(
    context: &AppContext<'_>,
    command: &StartRun,
) -> Result<Planned<Run>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    gate(context, &snapshot, CommandClass::PausableMutation).await?;

    let stored = context
        .sessions
        .load_run(command.session, command.run)
        .await?;
    if snapshot.active_run != Some(command.run) {
        return Err(AppError::Run(SessionDomainRunError::SessionBusy {
            active: snapshot.active_run.unwrap_or(command.run),
        }));
    }

    let commit = start_run_domain(&stored, &snapshot, context.clock.now())?;

    let mut conditions = live_conditions(&snapshot);
    conditions.push(Condition::RunNonTerminal {
        session: command.session,
        run: command.run,
    });
    conditions.push(Condition::SessionActiveRun {
        session: command.session,
        expected: Some(command.run),
    });

    let plan = SessionTransaction {
        intent: TransactionIntent::StartRun,
        conditions,
        // An idempotent start writes nothing and emits no `run.started` fact.
        writes: if commit.changed {
            vec![Write::PutRun(Box::new(commit.run.clone()))]
        } else {
            Vec::new()
        },
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: commit.run,
    })
}

/// Settles a run through the terminal barrier.
///
/// # Errors
///
/// Returns [`AppError`] when a port read fails or this attempt is not the
/// winner. A losing attempt produces no plan at all.
pub async fn commit_terminal(
    context: &AppContext<'_>,
    command: &CommitTerminal,
) -> Result<Planned<Run>, AppError> {
    let snapshot = context
        .sessions
        .load_session(command.workspace, command.session)
        .await?;
    let agent = context
        .sessions
        .load_agent(command.session, command.agent)
        .await?;
    let stored = context
        .sessions
        .load_run(command.session, command.attempt.run)
        .await?;

    let commit = claim_terminal(
        &stored,
        &snapshot,
        agent.id,
        agent.fence(),
        &command.open_messages,
        &command.attempt,
    )?;

    let mut writes = vec![
        Write::PutRun(Box::new(commit.run.clone())),
        Write::PutAgentControl(Box::new(agent.clone())),
        Write::PutSessionHead(Box::new(commit.session.clone())),
        Write::PutOutboxEvent(Box::new(commit.outbox.clone())),
    ];
    writes.extend(commit.sealed_messages.iter().flat_map(|message| {
        [
            Write::PutMessage(Box::new(message.clone())),
            Write::PutSealedMessage(Box::new(message.clone())),
        ]
    }));

    let plan = SessionTransaction {
        intent: TransactionIntent::CommitTerminal,
        conditions: vec![
            Condition::RunNonTerminal {
                session: command.session,
                run: command.attempt.run,
            },
            Condition::SessionActiveRun {
                session: command.session,
                expected: Some(command.attempt.run),
            },
            Condition::SessionRevision {
                session: command.session,
                expected: snapshot.revision,
            },
            Condition::CancellationEpoch {
                session: command.session,
                expected: snapshot.cancellation,
            },
            Condition::DeletionState {
                session: command.session,
                expected: snapshot.deletion.state,
                epoch: snapshot.deletion.epoch,
            },
            Condition::AgentFence {
                session: command.session,
                agent: agent.id,
                at_least: agent.fence(),
            },
        ],
        writes,
        after_commit: Vec::new(),
    };
    plan.validate()?;

    Ok(Planned {
        plan,
        projected: commit.run,
    })
}

/// The largest number of agent rows one stop step examines.
///
/// Derived from the transaction budget rather than chosen (D-4): a step also
/// writes the operation row and the session head, so its agent batch may never
/// exceed `MAX_ACTIONS - 2`. The landed use case read a flat 100 and could
/// therefore build a 101-action plan that `validate` rejected as an internal
/// fault.
#[allow(
    clippy::cast_possible_truncation,
    reason = "`MAX_ACTIONS` is `DynamoDB`'s hard ceiling of 100 and the static assertion below               refuses any value a `u16` could not hold"
)]
/// The statuses a read command accepts. Present so a condition table can name
/// one value instead of building a set inline at each call site.
#[must_use]
pub fn live_statuses() -> BTreeSet<SessionStatus> {
    [SessionStatus::Idle, SessionStatus::Running]
        .into_iter()
        .collect()
}

/// The stable public code a rejection maps onto.
#[must_use]
pub const fn rejection_code(error: &AppError) -> ErrorCode {
    error.code()
}

/// What a live workspace read observed, and what it cost the workspace.
///
/// The generation is carried back so the caller can report exactly which
/// incarnation answered. Two pages of one listing that name different
/// generations are two different filesystems, and a caller that cannot see that
/// would stitch them together silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRead<T> {
    /// What was observed.
    pub observed: T,
    /// The generation that answered.
    pub generation: GenerationId,
}

/// Resolves the exact generation a live read must be answered by.
///
/// `pinned` is the caller's `ifGenerationId`: when it names a generation other
/// than the one in force, the read fails rather than silently answering from a
/// successor. A successor is a *different filesystem*, so answering from it
/// would return a confident wrong answer to the question that was asked.
fn live_generation(
    session: &Session,
    pinned: Option<GenerationId>,
) -> Result<GenerationId, AppError> {
    let Some(generation) = session.generation else {
        // No generation is in force, so there is no live filesystem to read.
        // Reporting an empty listing here would be indistinguishable from an
        // empty workspace, which is the one answer that must never be invented.
        return Err(AppError::Port(PortError::NotFound {
            kind: "live workspace generation",
        }));
    };
    if pinned.is_some_and(|pinned| pinned != generation) {
        return Err(AppError::Port(PortError::NotFound {
            kind: "the pinned live workspace generation",
        }));
    }
    Ok(generation)
}

/// Lists one page of a live workspace directory.
///
/// A pass-through observation: the running `MicroVM` answers it from `lstat`,
/// and nothing here hashes a file, builds a Merkle page, or consults the
/// content authority. No filesystem snapshot or persisted root exists on this path.
///
/// # Errors
///
/// [`AppError`] when the account is paused, when no generation is in force,
/// when a pinned generation is no longer the one in force, or when the port
/// refuses.
pub async fn list_live_files(
    context: &AppContext<'_>,
    workspace: WorkspaceId,
    session: SessionId,
    pinned: Option<GenerationId>,
    query: &crate::ports::LiveListQuery,
) -> Result<LiveRead<crate::ports::LiveListing>, AppError> {
    let session = context.sessions.load_session(workspace, session).await?;
    gate(context, &session, CommandClass::PausableRead).await?;
    let generation = live_generation(&session, pinned)?;
    let observed = context.live.list(session.id, generation, query).await?;
    Ok(LiveRead {
        observed,
        generation,
    })
}

/// Stats one live workspace entry.
///
/// The symlink is reported, never followed, because the guest's own `lstat`
/// answer is carried through unchanged.
///
/// # Errors
///
/// [`AppError`] when the account is paused, when no generation is in force,
/// when a pinned generation is no longer the one in force, or when the entry
/// does not exist.
pub async fn stat_live_file(
    context: &AppContext<'_>,
    workspace: WorkspaceId,
    session: SessionId,
    pinned: Option<GenerationId>,
    path: &str,
) -> Result<LiveRead<crate::ports::LiveEntry>, AppError> {
    let session = context.sessions.load_session(workspace, session).await?;
    gate(context, &session, CommandClass::PausableRead).await?;
    let generation = live_generation(&session, pinned)?;
    let observed = context.live.stat(session.id, generation, path).await?;
    Ok(LiveRead {
        observed,
        generation,
    })
}
