//! Provider error normalization and the transition awaits.
//!
//! Every provider answer is classified into [`ProviderCall`] by **code**, never by
//! matching a message string: a message is a human-facing field the service is free
//! to reword, and a classifier that reads one turns a copy edit upstream into a
//! silent behaviour change here.
//!
//! An outcome that cannot be classified is [`ProviderCall::Fatal`]. There is no
//! optimistic default, because the two cheap defaults are both wrong: assuming
//! success strands a running `MicroVM` with no receipt, and assuming failure
//! retries an effect that already happened.

use core::time::Duration;

use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_runtime_control::generation::GenerationState;
use aex_runtime_control::lifecycle::{
    LifecycleAction, ProviderCall, ProviderQuotaId, ProviderState, RedactedDetail, TransientClass,
};

/// How long a launch may take to reach `RUNNING`.
pub const LAUNCH_AWAIT_MS: u64 = 30_000;

/// How often a launch is polled.
pub const LAUNCH_POLL_MS: u64 = 300;

/// How long a suspend may take to reach `SUSPENDED`.
pub const SUSPEND_AWAIT_MS: u64 = 30_000;

/// How often a suspend is polled.
pub const SUSPEND_POLL_MS: u64 = 250;

/// How long a resume may take to reach `RUNNING`.
pub const RESUME_AWAIT_MS: u64 = 30_000;

/// How often a resume is polled.
pub const RESUME_POLL_MS: u64 = 300;

/// Which side a Smithy error blames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fault {
    /// The caller.
    Client,
    /// The service.
    Server,
}

/// One raw provider answer, before classification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderAnswer {
    /// The modelled exception name, when the service returned one.
    pub error_code: Option<String>,
    /// The HTTP status, when there was one.
    pub http_status: Option<u16>,
    /// The Smithy `$fault` discriminator.
    pub fault: Option<Fault>,
    /// The advertised retry delay, in seconds.
    pub retry_after_seconds: Option<u64>,
    /// The field the service named as invalid.
    pub invalid_field: Option<String>,
    /// The quota the service named.
    pub quota: Option<String>,
    /// The request id, when one exists.
    pub request_id: Option<String>,
    /// Whether the transport failed in a way that leaves the outcome unknown: a
    /// timeout, a dropped connection, or an interrupted read.
    pub outcome_indeterminate: bool,
}

/// Classifies one provider answer.
///
/// The order matters and is asserted by test: an indeterminate transport outcome
/// wins over everything, because a timeout after the service has already acted is
/// exactly the case a blind retry corrupts.
#[must_use]
pub fn classify(answer: &ProviderAnswer) -> ProviderCall {
    if answer.outcome_indeterminate {
        return ProviderCall::Unknown {
            request: answer
                .request_id
                .as_ref()
                .map(|id| ProviderRequestId(id.clone())),
        };
    }
    if let Some(code) = answer.error_code.as_deref() {
        return classify_code(code, answer);
    }
    if let Some(status) = answer.http_status {
        if status == 429 {
            return ProviderCall::Throttled {
                retry_after: Duration::from_secs(answer.retry_after_seconds.unwrap_or(1)),
            };
        }
        if status == 404 {
            return ProviderCall::NotFound;
        }
        if (500..600).contains(&status) {
            return ProviderCall::Transient {
                class: TransientClass::ServerStatus,
            };
        }
        if (400..500).contains(&status) {
            return ProviderCall::Fatal {
                code: format!("http-{status}").into_boxed_str(),
            };
        }
    }
    if answer.fault == Some(Fault::Server) {
        return ProviderCall::Transient {
            class: TransientClass::ServerFault,
        };
    }
    ProviderCall::Fatal {
        code: "unclassified".into(),
    }
}

/// Classifies by the modelled exception name.
fn classify_code(code: &str, answer: &ProviderAnswer) -> ProviderCall {
    match code {
        "ThrottlingException" | "TooManyRequestsException" | "RequestThrottledException" => {
            ProviderCall::Throttled {
                retry_after: Duration::from_secs(answer.retry_after_seconds.unwrap_or(1)),
            }
        }
        "ServiceQuotaExceededException" | "LimitExceededException" => ProviderCall::Capacity {
            quota: ProviderQuotaId(
                answer
                    .quota
                    .clone()
                    .unwrap_or_else(|| "unnamed-quota".to_owned()),
            ),
        },
        "InternalServerException" => ProviderCall::Transient {
            class: TransientClass::InternalServer,
        },
        "ServiceUnavailableException" => ProviderCall::Transient {
            class: TransientClass::ServiceUnavailable,
        },
        "ResourceNotFoundException" => ProviderCall::NotFound,
        "ValidationException" | "InvalidParameterValueException" => ProviderCall::Invalid {
            field: answer
                .invalid_field
                .clone()
                .unwrap_or_else(|| "unnamed".to_owned())
                .into_boxed_str(),
            detail: RedactedDetail::new("the provider rejected the request shape"),
        },
        other => ProviderCall::Fatal { code: other.into() },
    }
}

/// What a transition await concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AwaitVerdict {
    /// The target state was reached.
    Reached(GenerationState),
    /// Keep polling.
    Poll,
    /// The generation is gone.
    Lost,
    /// The await ran out of time. Never assumed successful.
    TimedOut,
}

/// Decides what a transition await should do next.
///
/// `elapsed_ms` is an input, so the whole await matrix is deterministic. A real
/// clock in here would make the timeout case a timing race, and a timing race in a
/// lifecycle test is a test that eventually passes for the wrong reason.
#[must_use]
pub fn await_step(
    action: LifecycleAction,
    observed: ProviderState,
    recorded: GenerationState,
    elapsed_ms: u64,
) -> AwaitVerdict {
    let mapped = observed.map_onto(recorded);
    match action {
        LifecycleAction::Launch | LifecycleAction::Resume | LifecycleAction::NativeResume => {
            let budget = if action == LifecycleAction::Launch {
                LAUNCH_AWAIT_MS
            } else {
                RESUME_AWAIT_MS
            };
            match observed {
                ProviderState::Running => AwaitVerdict::Reached(mapped),
                ProviderState::Terminated | ProviderState::Terminating => AwaitVerdict::Lost,
                ProviderState::Pending | ProviderState::Suspended | ProviderState::Suspending => {
                    poll_or_time_out(elapsed_ms, budget)
                }
            }
        }
        LifecycleAction::Suspend => match observed {
            ProviderState::Suspended => AwaitVerdict::Reached(mapped),
            ProviderState::Suspending | ProviderState::Running => {
                poll_or_time_out(elapsed_ms, SUSPEND_AWAIT_MS)
            }
            // Anything but RUNNING or SUSPENDED during a suspend means the
            // generation is gone.
            ProviderState::Pending | ProviderState::Terminated | ProviderState::Terminating => {
                AwaitVerdict::Lost
            }
        },
        // A terminate is done when the provider says `TERMINATED`, and a
        // `NotFound` counts as done too; there is nothing to time out.
        LifecycleAction::Terminate => match observed {
            ProviderState::Terminated => AwaitVerdict::Reached(GenerationState::Terminated),
            _ => AwaitVerdict::Poll,
        },
    }
}

/// Poll while there is budget left, then time out.
const fn poll_or_time_out(elapsed_ms: u64, budget_ms: u64) -> AwaitVerdict {
    if elapsed_ms >= budget_ms {
        AwaitVerdict::TimedOut
    } else {
        AwaitVerdict::Poll
    }
}

/// The poll interval for one action.
#[must_use]
pub const fn poll_interval_ms(action: LifecycleAction) -> u64 {
    match action {
        LifecycleAction::Launch => LAUNCH_POLL_MS,
        LifecycleAction::Suspend => SUSPEND_POLL_MS,
        LifecycleAction::Resume | LifecycleAction::NativeResume | LifecycleAction::Terminate => {
            RESUME_POLL_MS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AwaitVerdict, Fault, LAUNCH_AWAIT_MS, ProviderAnswer, SUSPEND_AWAIT_MS, await_step,
        classify, poll_interval_ms,
    };
    use aex_runtime_control::generation::GenerationState;
    use aex_runtime_control::lifecycle::{
        LifecycleAction, ProviderCall, ProviderState, RetryPolicy, TransientClass,
    };
    use core::time::Duration;

    fn coded(code: &str) -> ProviderAnswer {
        ProviderAnswer {
            error_code: Some(code.to_owned()),
            ..ProviderAnswer::default()
        }
    }

    #[test]
    fn every_documented_exception_is_classified_by_code() {
        let cases: [(&str, ProviderCall); 8] = [
            (
                "ThrottlingException",
                ProviderCall::Throttled {
                    retry_after: Duration::from_secs(1),
                },
            ),
            (
                "TooManyRequestsException",
                ProviderCall::Throttled {
                    retry_after: Duration::from_secs(1),
                },
            ),
            (
                "InternalServerException",
                ProviderCall::Transient {
                    class: TransientClass::InternalServer,
                },
            ),
            (
                "ServiceUnavailableException",
                ProviderCall::Transient {
                    class: TransientClass::ServiceUnavailable,
                },
            ),
            ("ResourceNotFoundException", ProviderCall::NotFound),
            (
                "AccessDeniedException",
                ProviderCall::Fatal {
                    code: "AccessDeniedException".into(),
                },
            ),
            (
                "ConflictException",
                ProviderCall::Fatal {
                    code: "ConflictException".into(),
                },
            ),
            (
                "UnrecognizedClientException",
                ProviderCall::Fatal {
                    code: "UnrecognizedClientException".into(),
                },
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(classify(&coded(code)), expected, "{code}");
        }
    }

    #[test]
    fn capacity_is_classified_and_never_retried_in_adapter() {
        let answer = ProviderAnswer {
            error_code: Some("ServiceQuotaExceededException".to_owned()),
            quota: Some("microvm-memory-gib".to_owned()),
            ..ProviderAnswer::default()
        };
        let call = classify(&answer);
        assert!(matches!(call, ProviderCall::Capacity { .. }));
        assert_eq!(
            call.retry_policy(),
            RetryPolicy::Durable {
                min_ms: 15_000,
                max_ms: 60_000
            },
            "a full regional pool takes the long durable backoff at the worker"
        );
    }

    #[test]
    fn an_indeterminate_outcome_wins_over_every_other_signal() {
        let answer = ProviderAnswer {
            error_code: Some("ThrottlingException".to_owned()),
            http_status: Some(500),
            fault: Some(Fault::Server),
            request_id: Some("req-9".to_owned()),
            outcome_indeterminate: true,
            ..ProviderAnswer::default()
        };
        let call = classify(&answer);
        assert!(
            matches!(call, ProviderCall::Unknown { .. }),
            "a timeout after the service may already have acted is never a retry"
        );
        assert_eq!(call.retry_policy(), RetryPolicy::Reconcile);
    }

    #[test]
    fn a_status_class_is_used_only_when_no_code_arrived() {
        for (status, expected) in [
            (429u16, "throttled"),
            (404, "not_found"),
            (500, "transient"),
            (503, "transient"),
            (400, "fatal"),
        ] {
            let answer = ProviderAnswer {
                http_status: Some(status),
                ..ProviderAnswer::default()
            };
            let observed = match classify(&answer) {
                ProviderCall::Throttled { .. } => "throttled",
                ProviderCall::NotFound => "not_found",
                ProviderCall::Transient { .. } => "transient",
                ProviderCall::Fatal { .. } => "fatal",
                other => panic!("{status} classified as {other:?}"),
            };
            assert_eq!(observed, expected, "{status}");
        }
    }

    #[test]
    fn an_unclassifiable_answer_is_fatal_and_never_optimistic() {
        let call = classify(&ProviderAnswer::default());
        assert!(
            matches!(call, ProviderCall::Fatal { .. }),
            "an unclassifiable answer must not default to success or to a retry"
        );
    }

    #[test]
    fn a_server_fault_with_no_code_or_status_is_transient() {
        let answer = ProviderAnswer {
            fault: Some(Fault::Server),
            ..ProviderAnswer::default()
        };
        assert!(matches!(
            classify(&answer),
            ProviderCall::Transient {
                class: TransientClass::ServerFault
            }
        ));
    }

    #[test]
    fn a_launch_await_reaches_running_polls_pending_and_times_out() {
        use LifecycleAction::Launch;
        assert_eq!(
            await_step(
                Launch,
                ProviderState::Running,
                GenerationState::Launching,
                0
            ),
            AwaitVerdict::Reached(GenerationState::Running)
        );
        assert_eq!(
            await_step(
                Launch,
                ProviderState::Pending,
                GenerationState::Launching,
                0
            ),
            AwaitVerdict::Poll
        );
        assert_eq!(
            await_step(
                Launch,
                ProviderState::Pending,
                GenerationState::Launching,
                LAUNCH_AWAIT_MS - 1
            ),
            AwaitVerdict::Poll
        );
        assert_eq!(
            await_step(
                Launch,
                ProviderState::Pending,
                GenerationState::Launching,
                LAUNCH_AWAIT_MS
            ),
            AwaitVerdict::TimedOut,
            "a timeout is never assumed successful"
        );
        assert_eq!(
            await_step(
                Launch,
                ProviderState::Terminated,
                GenerationState::Launching,
                0
            ),
            AwaitVerdict::Lost
        );
    }

    #[test]
    fn a_suspend_await_treats_anything_but_running_or_suspended_as_lost() {
        use LifecycleAction::Suspend;
        assert_eq!(
            await_step(
                Suspend,
                ProviderState::Suspended,
                GenerationState::Suspending,
                0
            ),
            AwaitVerdict::Reached(GenerationState::Suspended)
        );
        assert_eq!(
            await_step(
                Suspend,
                ProviderState::Suspending,
                GenerationState::Suspending,
                0
            ),
            AwaitVerdict::Poll
        );
        assert_eq!(
            await_step(
                Suspend,
                ProviderState::Running,
                GenerationState::Suspending,
                SUSPEND_AWAIT_MS
            ),
            AwaitVerdict::TimedOut
        );
        for gone in [
            ProviderState::Pending,
            ProviderState::Terminated,
            ProviderState::Terminating,
        ] {
            assert_eq!(
                await_step(Suspend, gone, GenerationState::Suspending, 0),
                AwaitVerdict::Lost,
                "{gone:?}"
            );
        }
    }

    #[test]
    fn a_terminate_await_never_times_out_and_settles_on_terminated() {
        use LifecycleAction::Terminate;
        assert_eq!(
            await_step(
                Terminate,
                ProviderState::Terminated,
                GenerationState::Terminating,
                u64::MAX
            ),
            AwaitVerdict::Reached(GenerationState::Terminated)
        );
        assert_eq!(
            await_step(
                Terminate,
                ProviderState::Terminating,
                GenerationState::Terminating,
                u64::MAX
            ),
            AwaitVerdict::Poll
        );
    }

    #[test]
    fn a_running_generation_inside_the_drain_margin_stays_draining() {
        assert_eq!(
            await_step(
                LifecycleAction::Launch,
                ProviderState::Running,
                GenerationState::LifetimeDraining,
                0
            ),
            AwaitVerdict::Reached(GenerationState::LifetimeDraining),
            "observing RUNNING must not undo an AEX drain decision"
        );
    }

    #[test]
    fn every_action_has_a_poll_interval() {
        assert_eq!(poll_interval_ms(LifecycleAction::Launch), 300);
        assert_eq!(poll_interval_ms(LifecycleAction::Suspend), 250);
        assert_eq!(poll_interval_ms(LifecycleAction::Resume), 300);
        assert_eq!(poll_interval_ms(LifecycleAction::Terminate), 300);
    }
}
