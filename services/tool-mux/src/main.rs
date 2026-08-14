//! `tool-mux` distributed composition root.

use std::future::Future;
use std::net::{Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use aex_identity_domain::assertion::{
    Plane as AssertionPlane, VerificationKey, VerificationKeySet,
};
use aex_runtime_control::store::{OpenEffectCounter, PageBudget, RuntimeActivityStore};
use aex_runtime_control_aws::{Pace, RuntimeControl, RuntimePorts, RuntimeSettings};
use aex_tool_mux::{GuestPort, McpPort, ResultRetentionPort, RuntimePort, StoragePersistPort};
use tool_mux::auth::AssertionAuthorizer;
use tool_mux::mcp::{McpSecretReader, RemoteMcpAdapter};
use tool_mux::production_hands::{ProductionGuestAdapter, ProductionRuntimeAdapter};
use tool_mux::production_mcp::{ProductionMcpSecrets, ProductionQualifiedMcpClients};
use tool_mux::production_storage::{ProductionGuestFileStream, ProductionLatestFileAuthority};
use tool_mux::retention::ProductionResultRetention;
use tool_mux::storage::{GuestFileStreamPort, StorageAdapter};

const DEPLOYABLE: &str = "tool-mux";

#[derive(Debug, Clone)]
struct Config {
    plane: AssertionPlane,
    region: aex_wire::types::Region,
    runtime_activity_table: String,
    session_table: String,
    rating_queue_url: String,
    due_shards: u16,
    due_page: PageBudget,
    pricing_version: String,
    file_authority_table: String,
    content_bucket: String,
    content_bucket_owner: String,
    content_kms_key_arn: String,
    secret_kms_key_arn: String,
    telemetry_bucket: String,
    telemetry_bucket_owner: String,
    telemetry_kms_key_arn: String,
    assertion_keys: VerificationKeySet,
    listen: SocketAddr,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let required = |name: &'static str| {
            lookup(name)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| format!("required environment variable `{name}` is missing"))
        };
        let plane_text = required("AEX_PLANE")?;
        let plane = AssertionPlane::parse(&plane_text)
            .ok_or_else(|| "AEX_PLANE must be dev or prd".to_owned())?;
        let region_text = required("AEX_REGION")?;
        let region = aex_wire::types::Region::from_name(&region_text)
            .ok_or_else(|| "AEX_REGION is not a supported placement region".to_owned())?;
        let assertion_keys = VerificationKeySet::new(
            aex_tool_mux::parse_assertion_trust_anchors(&required("AEX_ASSERTION_TRUST_ANCHORS")?)
                .map_err(|_| "AEX_ASSERTION_TRUST_ANCHORS is invalid".to_owned())?
                .into_iter()
                .map(|(kid, public_key)| VerificationKey {
                    kid: aex_identity_domain::assertion::KeyId::new(kid),
                    public_key,
                    not_after_ms: u64::MAX,
                })
                .collect(),
        )
        .map_err(|_| "AEX_ASSERTION_TRUST_ANCHORS exceeds the verifier bound".to_owned())?;
        let due_shards = parse(&required, "AEX_RUNTIME_DUE_SHARDS")?;
        let due_items = parse(&required, "AEX_RUNTIME_DUE_PAGE_ITEMS")?;
        let due_reads = parse(&required, "AEX_RUNTIME_DUE_PAGE_READS")?;
        let listen = lookup("AEX_TOOL_MUX_LISTEN")
            .unwrap_or_else(|| SocketAddr::from((Ipv6Addr::UNSPECIFIED, 8080)).to_string())
            .parse()
            .map_err(|_| "AEX_TOOL_MUX_LISTEN is not a socket address".to_owned())?;
        Ok(Self {
            plane,
            region,
            runtime_activity_table: required("AEX_RUNTIME_ACTIVITY_TABLE")?,
            session_table: required("AEX_SESSION_AUTHORITY_TABLE")?,
            rating_queue_url: {
                let value = required("AEX_USAGE_RATING_QUEUE_URL")?;
                if !value.ends_with(".fifo") {
                    return Err("AEX_USAGE_RATING_QUEUE_URL must name a FIFO queue".to_owned());
                }
                value
            },
            due_shards,
            due_page: PageBudget {
                max_items: due_items,
                max_reads: due_reads,
            },
            pricing_version: required("AEX_PRICING_VERSION")?,
            file_authority_table: required("AEX_FILE_AUTHORITY_TABLE")?,
            content_bucket: required("AEX_CONTENT_BUCKET")?,
            content_bucket_owner: required("AEX_CONTENT_BUCKET_OWNER")?,
            content_kms_key_arn: required("AEX_CONTENT_KMS_KEY_ARN")?,
            secret_kms_key_arn: required("AEX_SECRET_KMS_KEY_ARN")?,
            telemetry_bucket: required("AEX_SESSION_TELEMETRY_BUCKET")?,
            telemetry_bucket_owner: required("AEX_SESSION_TELEMETRY_BUCKET_OWNER")?,
            telemetry_kms_key_arn: required("AEX_SESSION_TELEMETRY_KMS_KEY_ARN")?,
            assertion_keys,
            listen,
        })
    }

    fn secret_plane(&self) -> aex_secret_domain::context::Plane {
        match self.plane {
            AssertionPlane::Dev => aex_secret_domain::context::Plane::Dev,
            AssertionPlane::Prd => aex_secret_domain::context::Plane::Prd,
        }
    }
}

fn parse<T: std::str::FromStr>(
    required: &impl Fn(&'static str) -> Result<String, String>,
    name: &'static str,
) -> Result<T, String> {
    required(name)?
        .parse()
        .map_err(|_| format!("{name} is invalid"))
}

#[derive(Debug)]
struct TokioPace;

impl Pace for TokioPace {
    fn sleep(&self, millis: u64) -> core::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(Duration::from_millis(millis)))
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics failed: {error}");
        return ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(target: "aex::diagnostics", event = "process_configuration_rejected", deployable = DEPLOYABLE, error = %error);
            return ExitCode::FAILURE;
        }
    };
    match run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(target: "aex::diagnostics", event = "process_stopped", deployable = DEPLOYABLE, error = %error);
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> Result<(), String> {
    let sdk = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_sdk_dynamodb::config::Region::new(
            config.region.as_str(),
        ))
        .load()
        .await;
    let dynamo = aws_sdk_dynamodb::Client::new(&sdk);
    let sqs = aws_sdk_sqs::Client::new(&sdk);
    let store: Arc<dyn RuntimeActivityStore> = Arc::new(
        aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore::new(
            dynamo.clone(),
            config.runtime_activity_table.clone(),
        ),
    );
    let effects: Arc<dyn OpenEffectCounter> = Arc::new(
        aex_session_dynamodb::runtime_effects::OpenHandsEffectCounter::new(
            dynamo.clone(),
            config.session_table.clone(),
        ),
    );
    let provider: Arc<dyn aex_hands_control_aws::MicrovmControlApi> =
        Arc::new(aex_hands_control_aws::AwsMicrovmControl::new(
            aws_sdk_lambdamicrovms::Client::new(&sdk),
            config.region.as_str(),
        ));
    let runtime = Arc::new(RuntimeControl::new(
        RuntimePorts {
            store: Arc::clone(&store),
            effects,
            provider: Arc::clone(&provider),
            compute: Arc::new(
                aex_runtime_control_aws::usage_ingress::SqsFactDraftSink::new(
                    sqs.clone(),
                    config.rating_queue_url.clone(),
                    aex_usage_domain::meter::Category::Compute,
                ),
            ),
            storage: Arc::new(
                aex_runtime_control_aws::usage_ingress::SqsFactDraftSink::new(
                    sqs,
                    config.rating_queue_url.clone(),
                    aex_usage_domain::meter::Category::Storage,
                ),
            ),
            pace: Arc::new(TokioPace),
        },
        RuntimeSettings {
            region: config.region,
            pricing_version: aex_internal_contracts::PricingVersion(config.pricing_version.clone()),
            shards: config.due_shards,
            page: config.due_page,
            schedule_jitter_ms: 0,
        },
    ));
    let production_hands = Arc::new(
        aex_brain_hands::ProductionHandsBackend::new(store, provider, runtime)
            .map_err(|error| format!("Hands composition failed: {error}"))?,
    );
    let backend: Arc<dyn aex_brain_hands::HandsBackend> = production_hands.clone();
    let live: Arc<dyn aex_brain_hands::LiveFileBackend> = production_hands;
    let hands: Arc<dyn aex_brain_app::ports::HandsPort> =
        Arc::new(aex_brain_hands::HandsAdapter::new(backend));

    let custody = Arc::new(aex_secret_custody_dynamodb::CustodyStore::new(
        dynamo.clone(),
        config.session_table.clone(),
    ));
    let crypto = Arc::new(aex_secret_aws::EnvelopeCrypto::new(
        Box::new(aex_secret_aws::KmsBranchKeys::new(
            aws_sdk_kms::Client::new(&sdk),
            config.secret_kms_key_arn.clone(),
        )),
        format!("tool-mux:{}:{}", config.plane.as_str(), config.region),
    ));
    let secrets: Arc<dyn McpSecretReader> = Arc::new(ProductionMcpSecrets::new(
        custody,
        crypto,
        config.secret_plane(),
        config.region,
    ));
    let staging = std::env::temp_dir().join("aex-tool-mux");
    let guest_stream: Arc<dyn GuestFileStreamPort> = Arc::new(ProductionGuestFileStream::new(
        Arc::clone(&live),
        staging.clone(),
    ));
    let storage: Arc<dyn StoragePersistPort> = Arc::new(StorageAdapter::new(
        Arc::clone(&guest_stream),
        ProductionLatestFileAuthority::new(
            aex_registry_dynamodb::store::RegistryDynamoStore::new(
                dynamo.clone(),
                config.file_authority_table.clone(),
            ),
            aex_content_dynamodb::store::ContentStore::new(
                dynamo.clone(),
                config.file_authority_table.clone(),
            ),
            aex_content_aws::S3ContentObjects::new(
                aws_sdk_s3::Client::new(&sdk),
                aex_content_aws::BucketBinding {
                    bucket: config.content_bucket.clone(),
                    expected_owner: config.content_bucket_owner.clone(),
                    kms_key_id: config.content_kms_key_arn.clone(),
                },
            ),
            config.content_kms_key_arn.clone(),
            staging,
        ),
    ));
    let results: Arc<dyn ResultRetentionPort> = Arc::new(ProductionResultRetention::new(
        aws_sdk_s3::Client::new(&sdk),
        config.telemetry_bucket.clone(),
        config.telemetry_bucket_owner.clone(),
        config.telemetry_kms_key_arn.clone(),
        guest_stream,
    ));
    let runtime_adapter: Arc<dyn RuntimePort> =
        Arc::new(ProductionRuntimeAdapter::new(Arc::clone(&live)));
    let guest: Arc<dyn GuestPort> =
        Arc::new(ProductionGuestAdapter::new(hands, Arc::clone(&secrets)));
    let mcp: Arc<dyn McpPort> = Arc::new(RemoteMcpAdapter::new(
        Arc::new(ProductionQualifiedMcpClients::new()),
        secrets,
    ));
    let (telemetry, telemetry_receiver) = tool_mux::telemetry::BoundedTelemetryIngress::new(1_024);
    let exporter = tokio::spawn(tool_mux::telemetry::export(
        telemetry_receiver,
        dynamo,
        config.session_table.clone(),
        aex_session_telemetry_aws::SessionTelemetryWriter::new(
            aws_sdk_s3::Client::new(&sdk),
            config.telemetry_bucket.clone(),
            config.telemetry_kms_key_arn.clone(),
        ),
    ));
    let mux = Arc::new(aex_tool_mux::ToolMux::new(
        runtime_adapter,
        guest,
        mcp,
        storage,
        results,
        Arc::new(telemetry),
    ));
    let app = tool_mux::App::new(
        mux,
        Arc::new(AssertionAuthorizer::new(
            config.assertion_keys,
            config.plane,
            config.region,
        )),
    );
    let drain = app.clone();
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| format!("listener bind failed: {error}"))?;
    axum::serve(listener, tool_mux::router(app))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            drain.begin_drain();
        })
        .await
        .map_err(|error| format!("listener failed: {error}"))?;
    let _ = tokio::time::timeout(Duration::from_secs(5), exporter).await;
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = terminate.recv() => {},
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(name: &str) -> Option<String> {
        Some(
            match name {
                "AEX_PLANE" => "dev",
                "AEX_REGION" => "eu-west-1",
                "AEX_ASSERTION_TRUST_ANCHORS" => {
                    "018f47a2-65ee-7c61-a1d2-65097d0d8b11:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc"
                }
                "AEX_RUNTIME_DUE_SHARDS" => "8",
                "AEX_RUNTIME_DUE_PAGE_ITEMS" => "32",
                "AEX_RUNTIME_DUE_PAGE_READS" => "100",
                "AEX_RUNTIME_ACTIVITY_TABLE" => "runtime-activity",
                "AEX_SESSION_AUTHORITY_TABLE" => "session-authority",
                "AEX_USAGE_RATING_QUEUE_URL" => {
                    "https://sqs.eu-west-1.amazonaws.com/1/rating.fifo"
                }
                "AEX_PRICING_VERSION" => "launch",
                "AEX_FILE_AUTHORITY_TABLE" => "regional-file-authority",
                "AEX_CONTENT_BUCKET" => "content",
                "AEX_CONTENT_BUCKET_OWNER" => "123456789012",
                "AEX_CONTENT_KMS_KEY_ARN" => "arn:aws:kms:eu-west-1:1:key/content",
                "AEX_SECRET_KMS_KEY_ARN" => "arn:aws:kms:eu-west-1:1:key/secret",
                "AEX_SESSION_TELEMETRY_BUCKET" => "telemetry",
                "AEX_SESSION_TELEMETRY_BUCKET_OWNER" => "123456789012",
                "AEX_SESSION_TELEMETRY_KMS_KEY_ARN" => {
                    "arn:aws:kms:eu-west-1:1:key/telemetry"
                }
                "AEX_TOOL_MUX_LISTEN" => "127.0.0.1:8080",
                _ => return None,
            }
            .to_owned(),
        )
    }

    #[test]
    fn startup_refuses_missing_resource_identity_before_aws() {
        let result = Config::from_lookup(|name| match name {
            "AEX_PLANE" => Some("dev".to_owned()),
            "AEX_REGION" => Some("eu-west-1".to_owned()),
            _ => None,
        });
        assert!(
            result
                .expect_err("incomplete config is refused")
                .contains("missing")
        );
    }

    #[test]
    fn exact_five_table_binding_uses_session_and_file_authorities() {
        let config = Config::from_lookup(complete).expect("exact hosted binding parses");
        assert_eq!(config.session_table, "session-authority");
        assert_eq!(config.file_authority_table, "regional-file-authority");
        assert_eq!(config.assertion_keys.len(), 1);
        assert!(
            Config::from_lookup(|name| match name {
                "AEX_SESSION_AUTHORITY_TABLE" | "AEX_FILE_AUTHORITY_TABLE" => None,
                _ => complete(name),
            })
            .is_err()
        );
    }
}
