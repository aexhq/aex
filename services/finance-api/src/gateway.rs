//! The one Rust-to-Stripe path: a synchronous invoke of `stripe-command-edge`.
//!
//! Rust never holds a Stripe secret and never dials `api.stripe.com`. It hands
//! an already-admitted [`PaymentCommandEnvelope`] to the edge and reads back a
//! [`PaymentResult`]. Every failure this module can observe is a failure to
//! *deliver the command*, and it is reported as
//! [`GatewayError::OutcomeUnknown`] whenever the call may have reached the edge
//! — a lost invoke response is not a refusal.

use aex_payment_contracts::{PaymentCommandEnvelope, PaymentResult};
use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;

use crate::authority::{GatewayError, PaymentGateway};

/// The command edge, invoked over the AWS Lambda control plane.
#[derive(Debug, Clone)]
pub struct CommandEdgeGateway {
    client: aws_sdk_lambda::Client,
    function_arn: String,
}

impl CommandEdgeGateway {
    /// Builds the gateway for one function.
    #[must_use]
    pub const fn new(client: aws_sdk_lambda::Client, function_arn: String) -> Self {
        Self {
            client,
            function_arn,
        }
    }

    /// The function this gateway is bound to.
    #[must_use]
    pub fn function_arn(&self) -> &str {
        &self.function_arn
    }
}

#[async_trait::async_trait]
impl PaymentGateway for CommandEdgeGateway {
    async fn execute(
        &self,
        envelope: &PaymentCommandEnvelope,
    ) -> Result<PaymentResult, GatewayError> {
        let payload = serde_json::to_vec(envelope)
            .map_err(|error| GatewayError::OffContract(error.to_string()))?;
        let answer = self
            .client
            .invoke()
            .function_name(&self.function_arn)
            .invocation_type(InvocationType::RequestResponse)
            .payload(Blob::new(payload))
            .send()
            .await
            .map_err(|error| {
                // The invoke may have executed the command. Anything other than
                // a construction failure is therefore indeterminate.
                GatewayError::OutcomeUnknown(format!(
                    "invoking the payment command edge failed: {}",
                    aws_sdk_lambda::error::DisplayErrorContext(&error)
                ))
            })?;
        if let Some(function_error) = answer.function_error() {
            return Err(GatewayError::OutcomeUnknown(format!(
                "the payment command edge failed: {function_error}"
            )));
        }
        let body = answer
            .payload()
            .ok_or_else(|| GatewayError::OffContract("the edge returned no payload".to_owned()))?;
        serde_json::from_slice::<PaymentResult>(body.as_ref())
            .map_err(|error| GatewayError::OffContract(error.to_string()))
    }
}

/// A gateway that refuses every command without contacting anything.
///
/// Used only where the composition deliberately has no provider path, such as a
/// startup test. A refusal is [`GatewayError::Unavailable`], which states that
/// nothing happened, and never `OutcomeUnknown`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RefusingGateway;

#[async_trait::async_trait]
impl PaymentGateway for RefusingGateway {
    async fn execute(
        &self,
        _envelope: &PaymentCommandEnvelope,
    ) -> Result<PaymentResult, GatewayError> {
        Err(GatewayError::Unavailable(
            "no payment command edge is composed in this process".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{GatewayError, RefusingGateway};

    #[test]
    fn a_refusal_states_that_nothing_happened() {
        let error = GatewayError::Unavailable("no edge".to_owned());
        assert!(!matches!(error, GatewayError::OutcomeUnknown(_)));
        assert_eq!(
            std::mem::size_of_val(&RefusingGateway),
            0,
            "the refusing gateway holds no resource"
        );
    }
}
