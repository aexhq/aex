//! Production Lambda MicroVM control adapter over the official AWS Rust SDK.

use std::str::FromStr as _;

use aex_hands_protocol::lifecycle::ProviderRequestId;
use aex_runtime_control::generation::ImageIdentifier;
use aex_runtime_control::lifecycle::{MicrovmId, ProviderCall, ProviderState, TransientClass};
use aex_wire::types::Timestamp;
use aws_sdk_lambdamicrovms::Client;
use aws_sdk_lambdamicrovms::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_lambdamicrovms::types::{MicrovmItem, MicrovmState, PortSpecification};
use aws_smithy_types::DateTime;

use crate::lifecycle::{ProviderAnswer, classify};
use crate::provider::{
    EndpointToken, MicrovmControlApi, MicrovmDescription, MicrovmPage, ProviderFuture, RunRequest,
};

const AUTH_HEADER: &str = "X-aws-proxy-auth";

/// Official AWS Lambda MicroVM control-plane adapter.
#[derive(Debug, Clone)]
pub struct AwsMicrovmControl {
    client: Client,
    region: String,
}

impl AwsMicrovmControl {
    /// Binds a generated client and the AWS region used in managed connector ARNs.
    #[must_use]
    pub fn new(client: Client, region: impl Into<String>) -> Self {
        Self {
            client,
            region: region.into(),
        }
    }

    fn connector_arn(&self, connector: &str) -> String {
        if connector.starts_with("arn:") {
            connector.to_owned()
        } else {
            format!(
                "arn:aws:lambda:{}:aws:network-connector:aws-network-connector:{connector}",
                self.region
            )
        }
    }
}

impl MicrovmControlApi for AwsMicrovmControl {
    fn run<'a>(&'a self, request: &'a RunRequest) -> ProviderFuture<'a, MicrovmDescription> {
        Box::pin(async move {
            let idle = aws_sdk_lambdamicrovms::types::IdlePolicy::builder()
                .max_idle_duration_seconds(as_i32(
                    request.idle_policy.max_idle_duration_seconds,
                    "idlePolicy.maxIdleDurationSeconds",
                )?)
                .suspended_duration_seconds(as_i32(
                    request.idle_policy.suspended_duration_seconds,
                    "idlePolicy.suspendedDurationSeconds",
                )?)
                .auto_resume_enabled(request.idle_policy.auto_resume_enabled)
                .build()
                .map_err(|_| fatal("invalid-idle-policy"))?;
            let output = self
                .client
                .run_microvm()
                .image_identifier(&request.image_identifier.0)
                .image_version(&request.image_version.0)
                .set_ingress_network_connectors(Some(
                    request
                        .ingress_network_connectors
                        .iter()
                        .map(|connector| self.connector_arn(connector))
                        .collect(),
                ))
                .set_egress_network_connectors(Some(
                    request
                        .egress_network_connectors
                        .iter()
                        .map(|connector| self.connector_arn(connector))
                        .collect(),
                ))
                .idle_policy(idle)
                .maximum_duration_in_seconds(as_i32(
                    request.maximum_duration_in_seconds,
                    "maximumDurationInSeconds",
                )?)
                .run_hook_payload(&request.run_hook_payload)
                .client_token(&request.client_token)
                .send()
                .await
                .map_err(|error| effect_error(&error))?;
            description(
                output.microvm_id(),
                output.state(),
                Some(output.endpoint()),
                Some(output.started_at()),
            )
        })
    }

    fn get<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, MicrovmDescription> {
        Box::pin(async move {
            let output = self
                .client
                .get_microvm()
                .microvm_identifier(&microvm.0)
                .send()
                .await
                .map_err(|error| read_error(&error))?;
            description(
                output.microvm_id(),
                output.state(),
                Some(output.endpoint()),
                Some(output.started_at()),
            )
        })
    }

    fn suspend<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId> {
        Box::pin(async move {
            let output = self
                .client
                .suspend_microvm()
                .microvm_identifier(&microvm.0)
                .send()
                .await
                .map_err(|error| effect_error(&error))?;
            request_id(&output)
        })
    }

    fn resume<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId> {
        Box::pin(async move {
            let output = self
                .client
                .resume_microvm()
                .microvm_identifier(&microvm.0)
                .send()
                .await
                .map_err(|error| effect_error(&error))?;
            request_id(&output)
        })
    }

    fn terminate<'a>(&'a self, microvm: &'a MicrovmId) -> ProviderFuture<'a, ProviderRequestId> {
        Box::pin(async move {
            let output = self
                .client
                .terminate_microvm()
                .microvm_identifier(&microvm.0)
                .send()
                .await
                .map_err(|error| effect_error(&error))?;
            request_id(&output)
        })
    }

    fn auth_token<'a>(
        &'a self,
        microvm: &'a MicrovmId,
        ttl_seconds: u64,
        ports: &'a [u16],
    ) -> ProviderFuture<'a, EndpointToken> {
        Box::pin(async move {
            if ttl_seconds == 0 || !ttl_seconds.is_multiple_of(60) || ttl_seconds > 3_600 {
                return Err(invalid("expirationInMinutes"));
            }
            if ports != [crate::provider::AGENT_PORT] {
                return Err(invalid("allowedPorts"));
            }
            let expiration = as_i32(ttl_seconds / 60, "expirationInMinutes")?;
            let output = self
                .client
                .create_microvm_auth_token()
                .microvm_identifier(&microvm.0)
                .expiration_in_minutes(expiration)
                .allowed_ports(PortSpecification::Port(i32::from(ports[0])))
                .send()
                .await
                .map_err(|error| effect_error(&error))?;
            let secret = output
                .auth_token()
                .get(AUTH_HEADER)
                .filter(|token| !token.is_empty())
                .cloned()
                .ok_or_else(|| fatal("missing-auth-token"))?;
            let expires_at = now_plus(ttl_seconds)?;
            Ok(EndpointToken::new(secret, expires_at))
        })
    }

    fn list<'a>(
        &'a self,
        image: Option<&'a ImageIdentifier>,
        page: Option<&'a str>,
    ) -> ProviderFuture<'a, MicrovmPage> {
        Box::pin(async move {
            let output = self
                .client
                .list_microvms()
                .set_image_identifier(image.map(|value| value.0.clone()))
                .set_next_token(page.map(str::to_owned))
                .send()
                .await
                .map_err(|error| read_error(&error))?;
            let microvms = output
                .items()
                .iter()
                .map(item_description)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(MicrovmPage {
                microvms,
                next: output.next_token().map(str::to_owned),
            })
        })
    }
}

fn description(
    id: &str,
    state: &MicrovmState,
    endpoint: Option<&str>,
    started_at: Option<&DateTime>,
) -> Result<MicrovmDescription, ProviderCall> {
    if id.is_empty() {
        return Err(fatal("missing-microvm-id"));
    }
    let state =
        ProviderState::from_str(state.as_str()).map_err(|_| fatal("unmodelled-provider-state"))?;
    let endpoint = endpoint
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let launched_at = started_at.map(timestamp).transpose()?;
    Ok(MicrovmDescription {
        microvm: MicrovmId(id.to_owned()),
        state,
        endpoint,
        launched_at,
    })
}

fn item_description(item: &MicrovmItem) -> Result<MicrovmDescription, ProviderCall> {
    description(
        item.microvm_id(),
        item.state(),
        None,
        Some(item.started_at()),
    )
}

fn timestamp(value: &DateTime) -> Result<Timestamp, ProviderCall> {
    let millis = value
        .to_millis()
        .map_err(|_| fatal("unrepresentable-provider-timestamp"))?;
    Timestamp::from_unix_millis(millis).map_err(|_| fatal("unrepresentable-provider-timestamp"))
}

fn now_plus(ttl_seconds: u64) -> Result<Timestamp, ProviderCall> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| fatal("unreadable-host-clock"))?;
    let millis = elapsed
        .as_millis()
        .saturating_add(u128::from(ttl_seconds) * 1_000);
    let millis = i64::try_from(millis).map_err(|_| fatal("unrepresentable-token-expiry"))?;
    Timestamp::from_unix_millis(millis).map_err(|_| fatal("unrepresentable-token-expiry"))
}

fn as_i32(value: u64, field: &'static str) -> Result<i32, ProviderCall> {
    i32::try_from(value).map_err(|_| invalid(field))
}

fn request_id<T: aws_types::request_id::RequestId>(
    output: &T,
) -> Result<ProviderRequestId, ProviderCall> {
    output
        .request_id()
        .filter(|request| !request.is_empty())
        .map(|request| ProviderRequestId(request.to_owned()))
        .ok_or_else(|| fatal("missing-provider-request-id"))
}

fn effect_error<E, R>(error: &SdkError<E, R>) -> ProviderCall
where
    E: ProvideErrorMetadata,
{
    sdk_error(error, true)
}

fn read_error<E, R>(error: &SdkError<E, R>) -> ProviderCall
where
    E: ProvideErrorMetadata,
{
    sdk_error(error, false)
}

fn sdk_error<E, R>(error: &SdkError<E, R>, effect: bool) -> ProviderCall
where
    E: ProvideErrorMetadata,
{
    match error {
        SdkError::ServiceError(service) => classify(&ProviderAnswer {
            error_code: service.err().code().map(str::to_owned),
            ..ProviderAnswer::default()
        }),
        SdkError::ConstructionFailure(_) => fatal("request-construction"),
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_)
            if effect =>
        {
            ProviderCall::Unknown { request: None }
        }
        SdkError::TimeoutError(_) | SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => {
            ProviderCall::Transient {
                class: TransientClass::ServerStatus,
            }
        }
        _ if effect => ProviderCall::Unknown { request: None },
        _ => ProviderCall::Transient {
            class: TransientClass::ServerStatus,
        },
    }
}

fn invalid(field: &'static str) -> ProviderCall {
    ProviderCall::Invalid {
        field: field.into(),
        detail: aex_runtime_control::lifecycle::RedactedDetail::new(
            "the provider request violates an adapter invariant",
        ),
    }
}

fn fatal(code: &'static str) -> ProviderCall {
    ProviderCall::Fatal { code: code.into() }
}

#[cfg(test)]
mod tests {
    use super::{AwsMicrovmControl, description, now_plus};
    use aex_runtime_control::lifecycle::{ProviderCall, ProviderState};
    use aws_sdk_lambdamicrovms::types::MicrovmState;

    #[test]
    fn managed_connector_names_expand_to_the_documented_aws_arn() {
        let adapter = AwsMicrovmControl::new(
            aws_sdk_lambdamicrovms::Client::from_conf(
                aws_sdk_lambdamicrovms::Config::builder().build(),
            ),
            "eu-west-1",
        );
        assert_eq!(
            adapter.connector_arn("ALL_INGRESS"),
            "arn:aws:lambda:eu-west-1:aws:network-connector:aws-network-connector:ALL_INGRESS"
        );
        let custom = "arn:aws:lambda:eu-west-1:123456789012:network-connector/custom";
        assert_eq!(adapter.connector_arn(custom), custom);
    }

    #[test]
    fn every_current_provider_state_maps_exactly_and_unknown_fails_closed() {
        for (provider, expected) in [
            (MicrovmState::Pending, ProviderState::Pending),
            (MicrovmState::Running, ProviderState::Running),
            (MicrovmState::Suspending, ProviderState::Suspending),
            (MicrovmState::Suspended, ProviderState::Suspended),
            (MicrovmState::Terminating, ProviderState::Terminating),
            (MicrovmState::Terminated, ProviderState::Terminated),
        ] {
            assert_eq!(
                description("mvm-1", &provider, None, None)
                    .expect("the state is modelled")
                    .state,
                expected
            );
        }
        assert!(matches!(
            description("mvm-1", &MicrovmState::from("NEW_STATE"), None, None),
            Err(ProviderCall::Fatal { .. })
        ));
    }

    #[test]
    fn token_expiry_uses_the_response_clock_and_never_defaults_to_epoch() {
        assert!(now_plus(1_800).expect("the host clock works").unix_millis() > 1_700_000_000_000);
    }
}
