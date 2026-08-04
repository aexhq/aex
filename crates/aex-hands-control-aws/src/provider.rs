//! The trusted provider adapter's seam and its request shapes.
//!
//! # The seam exists on purpose
//!
//! [`MicrovmControlApi`] is the one place the `MicroVM` control plane is reached.
//! [`AwsMicrovmControl`] binds the official generated `aws-sdk-lambdamicrovms`
//! client to that seam. Provider enums are decoded strictly and effect calls keep
//! ambiguous transport outcomes ambiguous for reconciliation.
//!
//! # H-BOUNDARY B1
//!
//! [`RunRequest`] has **no** `execution_role_arn` field. That is the control: a role
//! cannot be threaded through, defaulted in a config, or spread from a caller's
//! struct, because there is nowhere to put it.

use core::future::Future;
use core::pin::Pin;

use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_runtime_control::generation::{ImageIdentifier, ImageVersion, NetworkPolicy};
use aex_runtime_control::lifecycle::{MicrovmId, ProviderCall, ProviderState};
use aex_wire::ids::{ContentHash, GenerationId};
use aex_wire::types::{ComputeSize, Timestamp};
use serde::{Deserialize, Serialize};

/// Provider-hard maximum lifetime, in seconds, across running and suspended time.
pub const MAX_DURATION_SECONDS: u64 = 28_800;

/// Traffic-idle backstop, in seconds.
///
/// Moved from 300 to the provider maximum deliberately. The provider measures idle
/// only by inbound endpoint traffic, so a 300-second backstop can suspend an
/// authority-open background job that is busy computing and simply not being talked
/// to. H-IDLE makes AEX the only thing that decides idleness.
pub const MAX_IDLE_DURATION_SECONDS: u64 = 28_800;

/// How long a suspended generation may stay suspended, in seconds.
///
/// Also the provider maximum: auto-termination of a suspended generation is a
/// lifecycle decision AEX owns, and letting the provider make it would retire a
/// customer's snapshot with no AEX intent record and no receipt.
pub const SUSPENDED_DURATION_SECONDS: u64 = 28_800;

/// The only port a `MicroVM` endpoint exposes.
pub const AGENT_PORT: u16 = 8_080;

/// Endpoint token lifetime, in seconds.
pub const TOKEN_TTL_SECONDS: u64 = 1_800;

/// Token age, in seconds, at which a refresh is due.
pub const TOKEN_REFRESH_SECONDS: u64 = 1_500;

/// The largest run-hook payload, in UTF-8 bytes.
pub const MAX_RUN_HOOK_PAYLOAD_BYTES: usize = 4_096;

/// The AWS-managed HTTPS proxy ingress connector.
pub const ALL_INGRESS: &str = "ALL_INGRESS";

/// The managed egress connector.
pub const INTERNET_EGRESS: &str = "INTERNET_EGRESS";

/// The exact IAM action set a runtime deployable holds.
///
/// Image mutation lives in a separate release role. `CreateMicrovmShellAuthToken`
/// is absent, which is what makes "no shell ingress" an IAM fact rather than a
/// promise not to call something. `PassNetworkConnector` is a permission-only
/// dependency of `RunMicrovm` when the request uses AWS-managed ingress or
/// egress connectors; it does not grant connector mutation or network control.
pub const RUNTIME_IAM_ACTIONS: [&str; 8] = [
    "lambda:RunMicrovm",
    "lambda:PassNetworkConnector",
    "lambda:GetMicrovm",
    "lambda:CreateMicrovmAuthToken",
    "lambda:SuspendMicrovm",
    "lambda:ResumeMicrovm",
    "lambda:TerminateMicrovm",
    "lambda:ListMicrovms",
];

/// Actions no runtime deployable may hold.
pub const FORBIDDEN_IAM_ACTIONS: [&str; 5] = [
    "lambda:CreateMicrovmShellAuthToken",
    "lambda:CreateMicrovmImage",
    "lambda:UpdateMicrovmImage",
    "lambda:DeleteMicrovmImage",
    "iam:PassRole",
];

/// The provider idle policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IdlePolicy {
    /// Always `false`: every billable state transition must have an AEX intent
    /// record and a provider request identity **before** the effect. An implicit
    /// resume produces a state change and a charge with no intent.
    pub auto_resume_enabled: bool,
    /// Traffic-idle backstop.
    pub max_idle_duration_seconds: u64,
    /// Suspended lifetime.
    pub suspended_duration_seconds: u64,
}

impl Default for IdlePolicy {
    fn default() -> Self {
        Self {
            auto_resume_enabled: false,
            max_idle_duration_seconds: MAX_IDLE_DURATION_SECONDS,
            suspended_duration_seconds: SUSPENDED_DURATION_SECONDS,
        }
    }
}

/// The bounds carried in the run-hook payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunHookBounds {
    /// Largest terminal body.
    pub max_output_bytes: u64,
    /// Largest single frame.
    pub max_frame_bytes: u32,
    /// Wall-clock ceiling.
    pub max_wall_ms: u64,
    /// How many operations may be open at once.
    pub max_concurrent_operations: u16,
}

/// The run-hook payload: identity only.
///
/// There is no launch ticket and no launch-authority callback. A credential-free
/// guest can call no authority, so there is nothing for a ticket to protect.
/// Forging this payload requires launching a `MicroVM` in AEX's own account;
/// rewriting it from inside the guest breaks only that customer's own session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunHookPayload {
    /// Payload version.
    pub v: u8,
    /// The exact generation.
    pub generation: GenerationId,
    /// The exact protocol version.
    pub protocol_version: u32,
    /// The guest root.
    pub root: String,
    /// The compute shape.
    pub size: ComputeSize,
    /// The image's code-artifact digest.
    pub image_digest: ContentHash,
    /// The image's capability layers.
    pub capabilities: Vec<String>,
    /// Bounds the guest may lower but never raise.
    pub bounds: RunHookBounds,
}

/// The sorted, closed key set of a run-hook payload.
///
/// Published so the boundary test can assert it exactly: B4 is "no managed secret
/// in the guest", and the way that stays true is that the payload's field set is
/// closed and checked, not reviewed.
pub const RUN_HOOK_PAYLOAD_KEYS: [&str; 8] = [
    "bounds",
    "capabilities",
    "generation",
    "imageDigest",
    "protocolVersion",
    "root",
    "size",
    "v",
];

/// Why a launch request could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchError {
    /// The run-hook payload exceeds the AEX hold.
    #[error(
        "the run-hook payload is {bytes} UTF-8 bytes, the hold is {MAX_RUN_HOOK_PAYLOAD_BYTES}"
    )]
    PayloadTooLarge {
        /// How large it was.
        bytes: usize,
    },
    /// The payload could not be encoded.
    #[error("the run-hook payload could not be encoded: {reason}")]
    PayloadEncoding {
        /// Why.
        reason: String,
    },
}

/// A `RunMicrovm` request.
///
/// **There is deliberately no `execution_role_arn` field.** See the module
/// documentation: the absence is the control.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunRequest {
    /// Which image.
    pub image_identifier: ImageIdentifier,
    /// Which image version.
    pub image_version: ImageVersion,
    /// Ingress connectors. Exactly the managed HTTP connector.
    pub ingress_network_connectors: Vec<String>,
    /// Egress connectors: the managed Internet connector, or none.
    pub egress_network_connectors: Vec<String>,
    /// The idle policy.
    pub idle_policy: IdlePolicy,
    /// Provider-hard maximum lifetime.
    pub maximum_duration_in_seconds: u64,
    /// The canonical run-hook payload.
    pub run_hook_payload: String,
    /// The deterministic replay identity, `aexgen-{generation}`.
    pub client_token: String,
}

impl RunRequest {
    /// Builds a launch request.
    ///
    /// # Errors
    ///
    /// Returns [`LaunchError`] when the run-hook payload cannot be encoded or
    /// exceeds the 4096-UTF-8-byte AEX hold. The hold is asserted **before**
    /// dispatch, so an oversized payload fails a build rather than a launch.
    pub fn build(
        image_identifier: ImageIdentifier,
        image_version: ImageVersion,
        network: NetworkPolicy,
        payload: &RunHookPayload,
        client_token: String,
    ) -> Result<Self, LaunchError> {
        let encoded =
            serde_json::to_string(payload).map_err(|error| LaunchError::PayloadEncoding {
                reason: error.to_string(),
            })?;
        if encoded.len() > MAX_RUN_HOOK_PAYLOAD_BYTES {
            return Err(LaunchError::PayloadTooLarge {
                bytes: encoded.len(),
            });
        }
        Ok(Self {
            image_identifier,
            image_version,
            ingress_network_connectors: vec![ALL_INGRESS.to_owned()],
            egress_network_connectors: match network {
                NetworkPolicy::PublicInternet => vec![INTERNET_EGRESS.to_owned()],
                NetworkPolicy::None => Vec::new(),
            },
            idle_policy: IdlePolicy::default(),
            maximum_duration_in_seconds: MAX_DURATION_SECONDS,
            run_hook_payload: encoded,
            client_token,
        })
    }
}

/// What the provider reports about one `MicroVM`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrovmDescription {
    /// The provider's identity.
    pub microvm: MicrovmId,
    /// Its verbatim state.
    pub state: ProviderState,
    /// The AWS-authenticated endpoint, once it exists.
    pub endpoint: Option<String>,
    /// When the provider says it launched.
    pub launched_at: Option<Timestamp>,
    /// The request identity on an effect response. `GetMicrovm` and list reads
    /// carry none; `RunMicrovm` must preserve it for the durable launch receipt.
    pub request_id: Option<ProviderRequestId>,
}

/// An endpoint authorization token.
///
/// Never `Display`, never `Serialize`, and its `Debug` renders a redaction. The
/// token is memory-only in a trusted process: it is never persisted, journalled,
/// traced or exported, and the proxy strips the header before the guest could
/// observe it.
#[derive(Clone, PartialEq, Eq)]
pub struct EndpointToken {
    /// The secret.
    secret: String,
    /// When it stops working.
    pub expires_at: Timestamp,
    /// The ports it is scoped to. Always exactly the agent port.
    pub allowed_ports: Vec<u16>,
}

impl EndpointToken {
    /// Wraps a provider-issued token.
    #[must_use]
    pub fn new(secret: impl Into<String>, expires_at: Timestamp) -> Self {
        Self {
            secret: secret.into(),
            expires_at,
            allowed_ports: vec![AGENT_PORT],
        }
    }

    /// The secret, for the one place that sets the header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.secret
    }

    /// Whether the token should be refreshed at `now`, given the TTL it was issued
    /// with.
    #[must_use]
    pub fn needs_refresh(&self, now: Timestamp, ttl_seconds: u64) -> bool {
        let ttl_ms = i64::try_from(ttl_seconds.saturating_mul(1_000)).unwrap_or(i64::MAX);
        let issued = self.expires_at.unix_millis().saturating_sub(ttl_ms);
        let age_ms = now.unix_millis().saturating_sub(issued);
        let refresh_ms =
            i64::try_from(TOKEN_REFRESH_SECONDS.saturating_mul(1_000)).unwrap_or(i64::MAX);
        age_ms >= refresh_ms
    }
}

impl core::fmt::Debug for EndpointToken {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EndpointToken")
            .field("secret", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("allowed_ports", &self.allowed_ports)
            .finish()
    }
}

/// A page of `ListMicrovms` results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MicrovmPage {
    /// The `MicroVM`s on this page.
    pub microvms: Vec<MicrovmDescription>,
    /// The next page token, when there is one.
    pub next: Option<String>,
}

/// A boxed provider future.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ProviderCall>> + Send + 'a>>;

/// The `MicroVM` control plane.
///
/// Boxed futures rather than `async fn` so the trait stays object-safe: the worker
/// holds one `Arc<dyn MicrovmControlApi>` and the tests hold a recorded-protocol
/// fake behind the same type.
pub trait MicrovmControlApi: Send + Sync + 'static {
    /// `RunMicrovm`.
    fn run<'a>(&'a self, request: &'a RunRequest) -> ProviderFuture<'a, MicrovmDescription>;

    /// `GetMicrovm`.
    fn get<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, MicrovmDescription>;

    /// `SuspendMicrovm`. The request id is the only evidence the empty body carries.
    fn suspend<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId>;

    /// `ResumeMicrovm`.
    fn resume<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId>;

    /// `TerminateMicrovm`.
    fn terminate<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId>;

    /// `CreateMicrovmAuthToken`, scoped to exactly the agent port.
    fn auth_token<'a>(
        &'a self,
        microvm: &'a MicrovmId,
        ttl_seconds: u64,
        ports: &'a [u16],
    ) -> ProviderFuture<'a, EndpointToken>;

    /// `ListMicrovms`. Never on a request path; used only by bounded startup
    /// validation and background reconciliation.
    fn list<'a>(
        &'a self,
        image: Option<&'a ImageIdentifier>,
        page: Option<&'a str>,
    ) -> ProviderFuture<'a, MicrovmPage>;
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_PORT, ALL_INGRESS, EndpointToken, FORBIDDEN_IAM_ACTIONS, INTERNET_EGRESS, IdlePolicy,
        LaunchError, MAX_DURATION_SECONDS, MAX_RUN_HOOK_PAYLOAD_BYTES, RUN_HOOK_PAYLOAD_KEYS,
        RUNTIME_IAM_ACTIONS, RunHookBounds, RunHookPayload, RunRequest, TOKEN_TTL_SECONDS,
    };
    use aex_runtime_control::generation::{ImageIdentifier, ImageVersion, NetworkPolicy};
    use aex_runtime_control::lifecycle::client_token;
    use aex_wire::ids::{ContentHash, GenerationId, PrefixedId as _, Uuid7};
    use aex_wire::types::{ComputeSize, Timestamp};

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn generation() -> GenerationId {
        GenerationId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    fn payload() -> RunHookPayload {
        RunHookPayload {
            v: 1,
            generation: generation(),
            protocol_version: 1,
            root: "/workspace".to_owned(),
            size: ComputeSize::Gb1,
            image_digest: ContentHash::from_bytes([7; 32]),
            capabilities: Vec::new(),
            bounds: RunHookBounds {
                max_output_bytes: 1_000_000,
                max_frame_bytes: 1_048_576,
                max_wall_ms: 600_000,
                max_concurrent_operations: 32,
            },
        }
    }

    fn request(network: NetworkPolicy) -> RunRequest {
        RunRequest::build(
            ImageIdentifier("aex-hands-1gb".to_owned()),
            ImageVersion("7".to_owned()),
            network,
            &payload(),
            client_token(generation()),
        )
        .expect("the request builds")
    }

    /// The `hands-run-request-golden` evidence class.
    #[test]
    fn the_launch_request_shape_is_exact() {
        let built = request(NetworkPolicy::PublicInternet);
        assert_eq!(built.ingress_network_connectors, vec![ALL_INGRESS]);
        assert_eq!(built.egress_network_connectors, vec![INTERNET_EGRESS]);
        assert_eq!(
            built.idle_policy,
            IdlePolicy {
                auto_resume_enabled: false,
                max_idle_duration_seconds: 28_800,
                suspended_duration_seconds: 28_800,
            }
        );
        assert_eq!(built.maximum_duration_in_seconds, MAX_DURATION_SECONDS);
        assert_eq!(built.client_token, format!("aexgen-{}", generation()));

        let isolated = request(NetworkPolicy::None);
        assert!(isolated.egress_network_connectors.is_empty());
    }

    #[test]
    fn no_execution_role_can_be_threaded_through_a_launch() {
        // B1: the struct has no such field, so there is nowhere to put one. A
        // serialized request therefore cannot carry one either, and
        // `deny_unknown_fields` refuses one that arrives from outside.
        let json = serde_json::to_value(request(NetworkPolicy::PublicInternet))
            .expect("the request serializes");
        let object = json.as_object().expect("an object").clone();
        for forbidden in [
            "executionRoleArn",
            "execution_role_arn",
            "roleArn",
            "credentials",
        ] {
            assert!(!object.contains_key(forbidden), "{forbidden} is present");
        }
        let mut smuggled = object;
        smuggled.insert("executionRoleArn".to_owned(), serde_json::json!("arn:x"));
        assert!(
            serde_json::from_value::<RunRequest>(serde_json::Value::Object(smuggled)).is_err(),
            "an unknown field must be refused, not ignored"
        );
    }

    #[test]
    fn no_shell_ingress_action_is_in_the_runtime_role() {
        assert!(RUNTIME_IAM_ACTIONS.contains(&"lambda:RunMicrovm"));
        assert!(RUNTIME_IAM_ACTIONS.contains(&"lambda:PassNetworkConnector"));
        for forbidden in FORBIDDEN_IAM_ACTIONS {
            assert!(
                !RUNTIME_IAM_ACTIONS.contains(&forbidden),
                "{forbidden} must not be in the runtime action set"
            );
        }
        assert!(
            !RUNTIME_IAM_ACTIONS
                .iter()
                .any(|action| action.contains("Shell")),
            "no shell-token action exists in the runtime role"
        );
    }

    #[test]
    fn the_run_hook_payload_key_set_is_closed_and_sorted() {
        let json = serde_json::to_value(payload()).expect("the payload serializes");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, RUN_HOOK_PAYLOAD_KEYS);

        // B4: nothing secret-shaped is in the key set, and an added key is refused.
        for secret in ["launchTicket", "token", "apiKey", "credentials", "pepper"] {
            assert!(!RUN_HOOK_PAYLOAD_KEYS.contains(&secret), "{secret}");
        }
        let mut smuggled = json.as_object().expect("an object").clone();
        smuggled.insert("launchTicket".to_owned(), serde_json::json!("opaque"));
        assert!(
            serde_json::from_value::<RunHookPayload>(serde_json::Value::Object(smuggled)).is_err(),
            "the deleted launch ticket has no way back in"
        );
    }

    #[test]
    fn an_oversized_run_hook_payload_fails_the_build_not_the_launch() {
        let mut huge = payload();
        huge.capabilities = (0..1_000).map(|n| format!("capability-{n}")).collect();
        let outcome = RunRequest::build(
            ImageIdentifier("aex-hands-1gb".to_owned()),
            ImageVersion("7".to_owned()),
            NetworkPolicy::PublicInternet,
            &huge,
            client_token(generation()),
        );
        assert!(matches!(outcome, Err(LaunchError::PayloadTooLarge { .. })));

        let built = request(NetworkPolicy::PublicInternet);
        assert!(built.run_hook_payload.len() <= MAX_RUN_HOOK_PAYLOAD_BYTES);
    }

    #[test]
    fn the_replay_identity_is_stable_across_two_builds() {
        assert_eq!(
            request(NetworkPolicy::PublicInternet).client_token,
            request(NetworkPolicy::PublicInternet).client_token,
            "a repeated RunMicrovm with the same token returns the same MicroVM"
        );
    }

    #[test]
    fn an_endpoint_token_never_renders_its_secret() {
        let token = EndpointToken::new("super-secret-value", at(1_800_000));
        let rendered = format!("{token:?}");
        assert!(
            !rendered.contains("super-secret-value"),
            "the Debug rendering leaked the token: {rendered}"
        );
        assert!(rendered.contains("<redacted>"));
        assert_eq!(token.expose(), "super-secret-value");
        assert_eq!(token.allowed_ports, vec![AGENT_PORT]);
        assert_eq!(
            AGENT_PORT, 8_080,
            "the endpoint is scoped to the agent port and never to allPorts"
        );
    }

    #[test]
    fn a_token_is_refreshed_at_twenty_five_minutes_and_not_before() {
        // Issued at zero, expiring at the full TTL.
        let expiry = i64::try_from(TOKEN_TTL_SECONDS).expect("the TTL fits") * 1_000;
        let token = EndpointToken::new("x", at(expiry));
        assert!(!token.needs_refresh(at(1_499_999), TOKEN_TTL_SECONDS));
        assert!(token.needs_refresh(at(1_500_000), TOKEN_TTL_SECONDS));
    }
    #[test]
    fn the_trusted_and_guest_payload_key_sets_are_identical() {
        // The guest cannot depend on this crate, so the two declarations are
        // separate types. A drift between them would mean the trusted side sends
        // a field the guest's `deny_unknown_fields` decoder refuses, which fails
        // every launch — and the first place anyone would look is the provider.
        assert_eq!(
            RUN_HOOK_PAYLOAD_KEYS,
            aex_hands_agent::boot::RUN_HOOK_KEYS,
            "the run-hook payload has one shape, declared twice"
        );
    }
}
