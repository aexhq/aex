//! The reconciler's typed path to the payment command edge.

use aex_payment_contracts::{PaymentCommandEnvelope, PaymentResult};
use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;

use crate::sweep::SweepError;

/// Executes a previously admitted command or a read-only outcome lookup.
#[async_trait::async_trait]
pub trait EffectRecoveryGateway: Send + Sync + 'static {
    /// Invokes the edge and decodes its exact public payment result.
    async fn execute(&self, envelope: &PaymentCommandEnvelope)
    -> Result<PaymentResult, SweepError>;
}

/// AWS Lambda implementation of the recovery edge.
#[derive(Debug, Clone)]
pub struct LambdaEffectRecoveryGateway {
    client: aws_sdk_lambda::Client,
    function_arn: String,
}

impl LambdaEffectRecoveryGateway {
    /// Binds the gateway to one immutable function ARN.
    #[must_use]
    pub const fn new(client: aws_sdk_lambda::Client, function_arn: String) -> Self {
        Self {
            client,
            function_arn,
        }
    }
}

#[async_trait::async_trait]
impl EffectRecoveryGateway for LambdaEffectRecoveryGateway {
    async fn execute(
        &self,
        envelope: &PaymentCommandEnvelope,
    ) -> Result<PaymentResult, SweepError> {
        let payload =
            serde_json::to_vec(envelope).map_err(|error| SweepError::Decode(error.to_string()))?;
        let answer = self
            .client
            .invoke()
            .function_name(&self.function_arn)
            .invocation_type(InvocationType::RequestResponse)
            .payload(Blob::new(payload))
            .send()
            .await
            .map_err(|error| {
                SweepError::RecoveryUnknown(format!(
                    "invoking the payment command edge failed: {}",
                    aws_sdk_lambda::error::DisplayErrorContext(&error)
                ))
            })?;
        if let Some(function_error) = answer.function_error() {
            return Err(SweepError::RecoveryUnknown(format!(
                "the payment command edge failed: {function_error}"
            )));
        }
        let body = answer
            .payload()
            .ok_or_else(|| SweepError::Decode("the payment edge returned no payload".to_owned()))?;
        serde_json::from_slice(body.as_ref()).map_err(|error| SweepError::Decode(error.to_string()))
    }
}
