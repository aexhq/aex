//! Aex-hosted customer-app Hand delivery through API Gateway Management API.

use async_trait::async_trait;
use aws_sdk_apigatewaymanagement::error::SdkError;
use aws_sdk_apigatewaymanagement::primitives::Blob;
use brain::customer::{CustomerDelivery, CustomerDeliveryRequest, CustomerEnvironmentDeliveryPort};

pub struct ApiGatewayCustomerDelivery {
    client: aws_sdk_apigatewaymanagement::Client,
}

impl ApiGatewayCustomerDelivery {
    pub async fn from_env() -> anyhow::Result<Self> {
        let callback_url = std::env::var("AEX_CUSTOMER_HAND_CALLBACK_URL")
            .map_err(|_| anyhow::anyhow!("AEX_CUSTOMER_HAND_CALLBACK_URL is not set"))?;
        validate_callback_url(&callback_url)?;
        let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".into());
        let shared = aws_config::from_env()
            .region(aws_config::Region::new(region))
            .load()
            .await;
        let config = aws_sdk_apigatewaymanagement::config::Builder::from(&shared)
            .endpoint_url(callback_url)
            .build();
        Ok(Self {
            client: aws_sdk_apigatewaymanagement::Client::from_conf(config),
        })
    }
}

#[async_trait]
impl CustomerEnvironmentDeliveryPort for ApiGatewayCustomerDelivery {
    async fn send(&self, request: CustomerDeliveryRequest) -> brain::Result<CustomerDelivery> {
        // Brain owns both the tagged command vocabulary and the 24 KiB bound. The AWS SDK's
        // standard retry policy handles retryable gateway responses; customer runners fence and
        // deduplicate offers by operation/epoch.
        let frame = request.command.to_frame()?;
        match self
            .client
            .post_to_connection()
            .connection_id(request.connection_id)
            .data(Blob::new(frame))
            .send()
            .await
        {
            Ok(_) => Ok(CustomerDelivery::Delivered),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(|service| service.is_gone_exception()) =>
            {
                Ok(CustomerDelivery::Gone)
            }
            Err(SdkError::ServiceError(error))
                if error.err().is_forbidden_exception()
                    || error.err().is_limit_exceeded_exception()
                    || error.err().is_payload_too_large_exception() =>
            {
                tracing::warn!(error = %error.err(), "customer Environment gateway rejected delivery");
                Ok(CustomerDelivery::Unavailable)
            }
            Err(SdkError::ServiceError(error)) => {
                // A future/unmodelled service response can be emitted after the gateway accepted
                // the frame. Do not claim a safe retry boundary we cannot prove.
                tracing::warn!(error = %error.err(), "customer Environment gateway delivery outcome is unknown");
                Ok(CustomerDelivery::Unknown)
            }
            Err(SdkError::ConstructionFailure(error)) => {
                tracing::warn!(error = ?error, "customer Environment delivery was not constructed");
                Ok(CustomerDelivery::Unavailable)
            }
            Err(error) => {
                // Timeout, dispatch and malformed-response failures may have reached the socket;
                // reporting Unknown prevents Brain from claiming a safe replay.
                tracing::warn!(error = %error, "customer Environment delivery outcome is unknown");
                Ok(CustomerDelivery::Unknown)
            }
        }
    }
}

fn validate_callback_url(value: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(value)
        .map_err(|error| anyhow::anyhow!("AEX_CUSTOMER_HAND_CALLBACK_URL: {error}"))?;
    if url.scheme() != "https" {
        anyhow::bail!("AEX_CUSTOMER_HAND_CALLBACK_URL must use HTTPS");
    }
    if url.host_str().is_none() {
        anyhow::bail!("AEX_CUSTOMER_HAND_CALLBACK_URL must have a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("AEX_CUSTOMER_HAND_CALLBACK_URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        anyhow::bail!("AEX_CUSTOMER_HAND_CALLBACK_URL must not contain a query or fragment");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_is_an_https_management_endpoint_without_embedded_credentials() {
        assert!(
            validate_callback_url("https://abc.execute-api.us-east-1.amazonaws.com/production")
                .is_ok()
        );
        assert!(validate_callback_url("http://localhost:3000/dev").is_err());
        assert!(validate_callback_url("https://user:secret@example.test/dev").is_err());
        assert!(validate_callback_url("https://example.test/dev?token=nope").is_err());
    }
}
