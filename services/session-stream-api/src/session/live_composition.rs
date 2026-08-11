//! Production composition for exact-generation live-file transport.

use std::future::Future;
use std::sync::Arc;

use aex_brain_hands::{LiveFileBackend, ProductionHandsBackend};
use aex_runtime_control::store::{OpenEffectCounter, RuntimeActivityStore};
use aex_runtime_control::usage::UsageFactSink;
use aex_runtime_control_aws::worker::{Pace, RuntimeControl, RuntimePorts, RuntimeSettings};

use crate::Config;

#[derive(Debug)]
struct TokioPace;

impl Pace for TokioPace {
    fn sleep(&self, millis: u64) -> core::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(core::time::Duration::from_millis(
            millis,
        )))
    }
}

/// Builds the authenticated guest backend over the same runtime authorities as Brain.
///
/// # Errors
///
/// Returns a typed Hands construction error when the bounded HTTPS transport
/// cannot be constructed.
pub fn production_live_files(
    aws: &aws_config::SdkConfig,
    config: &Config,
) -> Result<Arc<dyn LiveFileBackend>, aex_brain_app::ports::HandsError> {
    let dynamo = aws_sdk_dynamodb::Client::new(aws);
    let sqs = aws_sdk_sqs::Client::new(aws);
    let store: Arc<dyn RuntimeActivityStore> = Arc::new(
        aex_runtime_activity_dynamodb::RuntimeActivityDynamoStore::new(
            dynamo.clone(),
            config.runtime_activity_table.clone(),
        ),
    );
    let effects: Arc<dyn OpenEffectCounter> = Arc::new(
        aex_session_dynamodb::runtime_effects::OpenHandsEffectCounter::new(
            dynamo,
            config.session_table.clone(),
        ),
    );
    let provider: Arc<dyn aex_hands_control_aws::MicrovmControlApi> =
        Arc::new(aex_hands_control_aws::AwsMicrovmControl::new(
            aws_sdk_lambdamicrovms::Client::new(aws),
            config.region.as_str(),
        ));
    let compute: Arc<dyn UsageFactSink> = Arc::new(
        aex_runtime_control_aws::usage_ingress::SqsFactDraftSink::new(
            sqs.clone(),
            config.usage_compute_queue_url.clone(),
            aex_usage_domain::meter::Category::Compute,
        ),
    );
    let storage: Arc<dyn UsageFactSink> = Arc::new(
        aex_runtime_control_aws::usage_ingress::SqsFactDraftSink::new(
            sqs,
            config.usage_storage_queue_url.clone(),
            aex_usage_domain::meter::Category::Storage,
        ),
    );
    let runtime = Arc::new(RuntimeControl::new(
        RuntimePorts {
            store: Arc::clone(&store),
            effects,
            provider: Arc::clone(&provider),
            compute,
            storage,
            pace: Arc::new(TokioPace),
        },
        RuntimeSettings {
            region: config.region,
            pricing_version: aex_internal_contracts::PricingVersion(config.pricing_version.clone()),
            shards: config.runtime_due_shards,
            page: config.runtime_due_page,
            schedule_jitter_ms: 0,
        },
    ));
    ProductionHandsBackend::new(store, provider, runtime)
        .map(|backend| Arc::new(backend) as Arc<dyn LiveFileBackend>)
}
