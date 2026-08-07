//! `regional-secret-api` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: the plaintext-bearing secret and provider-credential
//! admission routes. It mounts nothing else, and its configuration refuses a
//! session, content, work, registry or queue binding outright.
//!
//! The binary is a composition root only: it validates configuration, resolves
//! its trust anchors, builds the real adapters, assembles the router from the
//! generated route table, and hands it to `lambda_http`. Behaviour lives in the
//! library crates it composes.

use std::process::ExitCode;
use std::sync::Arc;

use aex_internal_contracts::assertion::AssertionAudience;
use aex_regional_http::assertion::AuthFailure;
use aex_regional_http::authz::{
    LambdaAssertionSource, ParameterStore, RegionalProjection, TrustError,
};
use aex_regional_http::config::RegionalHttpConfigError;
use aex_regional_http::edge::{EdgeBinding, RegionalEdge, SystemClock};
use aex_regional_http::health::Readiness;
use aex_regional_http::mount::{MountError, mount_unary};
use aex_wire::dispatch::RequestLimits;
use regional_secret_api::config::Config;
use regional_secret_api::handlers::{Dispatcher, Shared};

/// The one audience this deployable accepts.
///
/// This is the only process in the platform that ever holds secret plaintext, so
/// an assertion minted for any other regional role must never admit a request
/// here.
const AUDIENCE: AssertionAudience = AssertionAudience::RegionalSecret;

/// Why `regional-secret-api` stopped.
#[derive(Debug, thiserror::Error)]
enum RegionalSecretApiRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] RegionalHttpConfigError),
    /// Start-up key material was rejected.
    #[error(transparent)]
    Trust(#[from] TrustError),
    /// The edge could not be composed over its resolved inputs.
    #[error("the request edge could not be composed: {0}")]
    Edge(AuthFailure),
    /// The served route set could not be mounted.
    #[error(transparent)]
    Mount(#[from] MountError),
    /// The `HTTP` runtime stopped.
    #[error("the lambda runtime stopped: {0}")]
    Runtime(String),
}

#[tokio::main]
async fn main() -> ExitCode {
    // Telemetry first: a configuration refusal must reach the wire, or a
    // crash-looping deployment is visible only to whoever tails stderr.
    let settings = aex_platform_telemetry::Settings::default();
    let telemetry = aex_platform_telemetry::Handle::install(&settings, None);
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            telemetry.emit(
                aex_platform_telemetry::Record::event(
                    aex_telemetry_schema::generated::EVENT_AEX_PROCESS_CONFIGURATION_REJECTED,
                )
                .with(
                    aex_telemetry_schema::generated::AEX_DEPLOYABLE,
                    "regional-secret-api",
                ),
            );
            let _ = telemetry.flush(settings.flush_deadline);
            eprintln!("regional-secret-api: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = run(&config, &telemetry).await;
    if let aex_platform_telemetry::FlushOutcome::DeadlineExceeded { pending } =
        telemetry.flush(settings.flush_deadline)
    {
        eprintln!("regional-secret-api: telemetry flush left {pending} record(s) undelivered");
    }
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("regional-secret-api: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the real adapters, assembles the router and serves it.
async fn run(
    config: &Config,
    telemetry: &aex_platform_telemetry::Handle,
) -> Result<(), RegionalSecretApiRunError> {
    telemetry.emit(
        aex_platform_telemetry::Record::event(
            aex_telemetry_schema::generated::EVENT_AEX_PROCESS_STARTED,
        )
        .with(
            aex_telemetry_schema::generated::AEX_PLANE,
            config.plane.as_str().to_owned(),
        )
        .with(
            aex_telemetry_schema::generated::AEX_REGION,
            config.region.as_str().to_owned(),
        ),
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let kms = aws_sdk_kms::Client::new(&aws);
    let parameters = ParameterStore::new(aws_sdk_ssm::Client::new(&aws));

    // The two adapters this deployable is allowed to hold, and nothing else: the
    // custody authority and the envelope crypto over the *secret* KMS key.
    let custody = Arc::new(aex_secret_custody_dynamodb::CustodyStore::new(
        dynamodb.clone(),
        config.secret_custody_table.clone(),
    ));
    let crypto = Arc::new(aex_secret_aws::crypto::EnvelopeCrypto::new(
        Box::new(aex_secret_aws::keystore::KmsBranchKeys::new(
            kms,
            config.secret_kms_key.value.clone(),
        )),
        config.crypto_partition(),
    ));
    let edge = regional_secret_api::SecretEdge::new(Arc::clone(&custody), crypto);

    // Readiness is fail-closed and is resolved from the composition, not from a
    // constant: it reports `ready` only when every declared dependency of the
    // served route set is bound.
    let readiness = match edge.unresolved(config) {
        unresolved if unresolved.is_empty() => Readiness::ready(config.release_digest.clone()),
        unresolved => Readiness::not_ready(config.release_digest.clone(), unresolved)
            .map_err(|error| RegionalSecretApiRunError::Runtime(error.to_string()))?,
    };

    let anchors = parameters
        .trust_anchors(&config.authz_verify_keys_param)
        .await?;
    // This deployable serves no listing, so it resolves no cursor signing ring
    // and its configuration declares none. A key it cannot use is a key it
    // cannot leak.
    let admission = build_edge(config, &aws, &dynamodb, anchors)?;
    let dispatcher = Dispatcher::new(Arc::new(Shared {
        custody,
        custody_table: config.secret_custody_table.clone(),
    }));
    let mounted = mount_unary(Arc::new(dispatcher), Arc::new(admission), limits(config))?;

    lambda_http::run(
        mounted
            .router
            .merge(aex_regional_http::health::router(readiness)),
    )
    .await
    .map_err(|error| RegionalSecretApiRunError::Runtime(error.to_string()))
}

/// The shared regional edge over its three resolved inputs.
type Edge = RegionalEdge<
    LambdaAssertionSource,
    RegionalProjection<aex_session_dynamodb::projection::ProjectionReader>,
    SystemClock,
>;

fn build_edge(
    config: &Config,
    aws: &aws_config::SdkConfig,
    dynamodb: &aws_sdk_dynamodb::Client,
    anchors: aex_identity_domain::assertion::VerificationKeySet,
) -> Result<Edge, RegionalSecretApiRunError> {
    let projection = aex_session_dynamodb::projection::ProjectionReader::new(
        dynamodb.clone(),
        config.authz_projection_table.clone(),
    );
    RegionalEdge::new(
        LambdaAssertionSource::new(
            aws_sdk_lambda::Client::new(aws),
            config.authz_function.value.clone(),
            AUDIENCE,
            config.region,
        ),
        anchors,
        RegionalProjection::new(projection, config.region),
        SystemClock,
        EdgeBinding {
            plane: config.plane,
            audience: AUDIENCE,
            region: config.region,
            cache_budget_bytes: config.assertion_cache_bytes,
        },
    )
    .map_err(RegionalSecretApiRunError::Edge)
}

/// The decode bounds the generated dispatchers enforce.
fn limits(config: &Config) -> RequestLimits {
    RequestLimits {
        max_json_body_bytes: config.max_json_body_bytes,
        max_otlp_body_bytes: RequestLimits::DEFAULT_OTLP_BODY_BYTES,
    }
}
