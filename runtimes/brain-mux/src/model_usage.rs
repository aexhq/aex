//! Zero-dollar BYOK model-usage handoff to the central billing inbox.

use aex_brain_app::ports::{
    BoxFuture, ModelUsageError, ModelUsageObservation, ModelUsagePort, ModelUsagePublication,
};
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::usage::{
    FactIdempotency, ModelTokenClass, ModelUsageFact, ModelUsageFactId, ModelUsageRatingRequest,
    RatingMessage,
};
use aex_model_catalog::canonical::UsageField;
use aex_wire::PrefixedId as _;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::SessionId;
use aex_wire::types::{DecimalU128, Region, Timestamp};
use sha2::Digest as _;

/// FIFO publisher composed by Brain Mux.
#[derive(Debug, Clone)]
pub struct SqsModelUsagePort {
    client: aws_sdk_sqs::Client,
    queue_url: String,
    region: Region,
}

impl SqsModelUsagePort {
    /// Binds the one regional-to-central usage FIFO.
    #[must_use]
    pub fn new(client: aws_sdk_sqs::Client, queue_url: String, region: Region) -> Self {
        Self {
            client,
            queue_url,
            region,
        }
    }
}

impl ModelUsagePort for SqsModelUsagePort {
    fn publish<'a>(
        &'a self,
        observation: &'a ModelUsageObservation,
    ) -> BoxFuture<'a, Result<ModelUsagePublication, ModelUsageError>> {
        Box::pin(async move {
            let group = observation.organization.encode().as_str().to_owned();
            for message in rating_messages(self.region, observation)? {
                let RatingMessage::Model(request) = &message else {
                    unreachable!("the model-usage adapter never creates a priced fact")
                };
                let deduplication_id = request.model_usage.idempotency.deduplication_id.to_string();
                let body = serde_json::to_string(&message)
                    .map_err(|error| ModelUsageError::Unavailable(error.to_string()))?;
                self.client
                    .send_message()
                    .queue_url(&self.queue_url)
                    .message_group_id(&group)
                    .message_deduplication_id(deduplication_id)
                    .message_body(body)
                    .send()
                    .await
                    .map_err(|error| ModelUsageError::Unavailable(error.to_string()))?;
            }
            Ok(ModelUsagePublication::Accepted)
        })
    }
}

/// Builds the exact FIFO bodies for fields the provider actually reported.
pub(crate) fn rating_messages(
    region: Region,
    observation: &ModelUsageObservation,
) -> Result<Vec<RatingMessage>, ModelUsageError> {
    let session_uuid = aex_wire::Uuid7::from_bytes(*observation.session.0.as_bytes())
        .map_err(|error| ModelUsageError::Unavailable(error.to_string()))?;
    let session = SessionId::from_uuid7(session_uuid);
    let observed_at = Timestamp::from_unix_millis(observation.observed_at.0)
        .map_err(|error| ModelUsageError::Unavailable(error.to_string()))?;
    let effect = observation.effect.to_hex();
    let fields = [
        (
            UsageField::InputTokens,
            ModelTokenClass::Input,
            observation.usage.input_tokens,
        ),
        (
            UsageField::OutputTokens,
            ModelTokenClass::Output,
            observation.usage.output_tokens,
        ),
        (
            UsageField::CacheReadInputTokens,
            ModelTokenClass::CacheRead,
            observation.usage.cache_read_input_tokens,
        ),
        (
            UsageField::CacheWriteInputTokens,
            ModelTokenClass::CacheWrite,
            observation.usage.cache_write_input_tokens,
        ),
        (
            UsageField::ReasoningTokens,
            ModelTokenClass::Reasoning,
            observation.usage.reasoning_tokens,
        ),
    ];
    fields
        .into_iter()
        .filter(|(field, _, _)| observation.usage.completeness.reports(*field))
        .map(|(_, token_class, quantity)| {
            let fact_id = ModelUsageFactId::derive(region, session, &effect, token_class);
            let deduplication_id = format!("{}:model:{fact_id}", region.as_str());
            let fact = ModelUsageFact {
                schema_version: SchemaVersion::V1,
                fact_id,
                organization: observation.organization,
                workspace: observation.workspace,
                region,
                session,
                assistant_effect: effect.clone().into(),
                provider: observation.provider,
                model: observation.model.as_str().into(),
                token_class,
                quantity: DecimalU128::new(u128::from(quantity)),
                observed_at,
                idempotency: FactIdempotency {
                    deduplication_id: deduplication_id.clone().into(),
                    business_key: format!("model:{}:{fact_id}", region.as_str()).into(),
                },
            };
            let bytes = serde_json::to_vec(&fact)
                .map_err(|error| ModelUsageError::Unavailable(error.to_string()))?;
            let intent_hash = IntentDigest::from_bytes(sha2::Sha256::digest(bytes).into());
            Ok(RatingMessage::Model(ModelUsageRatingRequest {
                model_usage: fact,
                intent_hash,
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use aex_brain_app::ports::ModelUsageObservation;
    use aex_brain_domain::ids::{EffectId, ModelSlug, SessionId, Timestamp};
    use aex_brain_domain::wire_pending::{NormalizedUsage, ProviderId};
    use aex_internal_contracts::usage::RatingMessage;
    use aex_model_catalog::canonical::UsageCompleteness;
    use aex_wire::PrefixedId as _;
    use aex_wire::ids::{OrganizationId, WorkspaceId};
    use aex_wire::types::Region;
    use uuid::Uuid;

    use super::rating_messages;

    fn observation() -> ModelUsageObservation {
        let identity = aex_wire::Uuid7::from_bytes([
            0x01, 0x93, 0x3f, 0x2a, 0x1c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, 1,
        ])
        .expect("uuid7");
        ModelUsageObservation {
            organization: OrganizationId::from_uuid7(identity),
            workspace: WorkspaceId::from_uuid7(identity),
            session: SessionId(Uuid::from_bytes(*identity.as_bytes())),
            effect: EffectId([7; 16]),
            provider: ProviderId::Openai,
            model: ModelSlug::new("gpt-5").expect("model"),
            usage: NormalizedUsage {
                input_tokens: 11,
                cache_read_input_tokens: 2,
                cache_write_input_tokens: 3,
                output_tokens: 7,
                reasoning_tokens: 4,
                tool_use_prompt_tokens: 0,
                provider_total_tokens: Some(23),
                completeness: UsageCompleteness::Exact,
            },
            observed_at: Timestamp(1_800_000_000_000),
        }
    }

    #[test]
    fn one_deterministic_zero_dollar_message_is_built_per_reported_class() {
        let region = Region::from_name("eu-west-1").expect("region");
        let first = rating_messages(region, &observation()).expect("facts");
        let replay = rating_messages(region, &observation()).expect("same facts");
        assert_eq!(first, replay);
        assert_eq!(first.len(), 5);
        for message in first {
            let RatingMessage::Model(request) = message else {
                panic!("model usage must never become priced usage")
            };
            assert!(
                request
                    .model_usage
                    .idempotency
                    .deduplication_id
                    .contains(":model:")
            );
        }
    }
}
