//! The real Data `API` transport.
//!
//! This is the only module that names an AWS type. Everything above it works
//! against [`Transport`], which is why the budget, deadline and ambiguity rules
//! are unit-testable and why a repository can count statements without a
//! network.

use async_trait::async_trait;
use aws_sdk_rdsdata::Client;
use aws_sdk_rdsdata::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_rdsdata::types::SqlParameter;

use crate::client::{ExecuteResponse, Transport, TransportError};
use crate::config::DataApiConfig;
use crate::error::ExceptionKind;
use crate::transaction::TransactionId;

/// The Data `API` transport over the AWS SDK.
#[derive(Debug, Clone)]
pub struct AwsTransport {
    client: Client,
    resource_arn: String,
    secret_arn: String,
    database: String,
}

impl AwsTransport {
    /// Builds a transport for one cluster.
    #[must_use]
    pub fn new(client: Client, config: &DataApiConfig) -> Self {
        Self {
            client,
            resource_arn: config.resource_arn.as_str().to_owned(),
            secret_arn: config.secret_arn.as_str().to_owned(),
            database: config.database.as_str().to_owned(),
        }
    }
}

/// Maps an SDK failure onto the known/unknown split.
///
/// A service error carries a named exception, so the outcome is known. A
/// dispatch, timeout, response or construction failure means the request may or
/// may not have been applied, which is [`TransportError::Indeterminate`].
fn transport_error<E, R>(error: &SdkError<E, R>) -> TransportError
where
    E: ProvideErrorMetadata + std::fmt::Debug,
{
    match error {
        SdkError::ServiceError(service) => TransportError::Service {
            kind: ExceptionKind::from_name(service.err().code().unwrap_or("Unknown")),
            message: service.err().message().unwrap_or_default().to_owned(),
        },
        SdkError::TimeoutError(_) => TransportError::Indeterminate {
            message: "the request timed out".to_owned(),
        },
        SdkError::DispatchFailure(_) => TransportError::Indeterminate {
            message: "the request could not be dispatched".to_owned(),
        },
        SdkError::ResponseError(_) => TransportError::Indeterminate {
            message: "the response could not be read".to_owned(),
        },
        SdkError::ConstructionFailure(_) => TransportError::Indeterminate {
            message: "the request could not be constructed".to_owned(),
        },
        _ => TransportError::Indeterminate {
            message: "the request outcome is unknown".to_owned(),
        },
    }
}

#[async_trait]
impl Transport for AwsTransport {
    async fn execute(
        &self,
        sql: &str,
        parameters: Vec<SqlParameter>,
        transaction: Option<&TransactionId>,
    ) -> Result<ExecuteResponse, TransportError> {
        let mut request = self
            .client
            .execute_statement()
            .resource_arn(&self.resource_arn)
            .secret_arn(&self.secret_arn)
            .database(&self.database)
            .sql(sql)
            .set_parameters(Some(parameters))
            .include_result_metadata(false);
        if let Some(id) = transaction {
            request = request.transaction_id(id.as_str());
        }
        let output = request
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        let rows_affected = u64::try_from(output.number_of_records_updated()).unwrap_or(0);
        Ok(ExecuteResponse {
            records: output.records.unwrap_or_default(),
            rows_affected,
        })
    }

    async fn begin(&self) -> Result<TransactionId, TransportError> {
        let output = self
            .client
            .begin_transaction()
            .resource_arn(&self.resource_arn)
            .secret_arn(&self.secret_arn)
            .database(&self.database)
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        output
            .transaction_id
            .map(TransactionId::new)
            .ok_or_else(|| TransportError::Indeterminate {
                message: "the service opened a transaction without naming it".to_owned(),
            })
    }

    async fn commit(&self, transaction: &TransactionId) -> Result<String, TransportError> {
        let output = self
            .client
            .commit_transaction()
            .resource_arn(&self.resource_arn)
            .secret_arn(&self.secret_arn)
            .transaction_id(transaction.as_str())
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        Ok(output.transaction_status.unwrap_or_default())
    }

    async fn rollback(&self, transaction: &TransactionId) -> Result<(), TransportError> {
        self.client
            .rollback_transaction()
            .resource_arn(&self.resource_arn)
            .secret_arn(&self.secret_arn)
            .transaction_id(transaction.as_str())
            .send()
            .await
            .map_err(|error| transport_error(&error))?;
        Ok(())
    }
}
