//! The regional control authority, invoked as a Lambda function per region.
//!
//! # Why every ambiguity becomes [`EffectError::Unknown`]
//!
//! A workspace has two halves and only one of them is in this account's
//! database. The three answers a caller can act on are "it happened", "it
//! definitely did not happen", and "I cannot tell" — and only the third is safe
//! for a lost response. A timeout is the canonical case: the request left, the
//! region may have created the workspace, and the response never came back. The
//! adapter that turns that into a failure causes the caller to retry under a
//! fresh identity, which is exactly how one lost response becomes two
//! workspaces.
//!
//! So the classification here is deliberately pessimistic. Only a *service*
//! refusal issued before the handler could run — an unknown function, a
//! malformed request, a pre-invoke throttle — is [`EffectError::Unavailable`].
//! Everything else that is not a decoded answer is [`EffectError::Unknown`],
//! and the caller reconciles by asking the region about the workspace id the
//! central plane preassigned.

use std::collections::BTreeMap;
use std::sync::Arc;

use aex_control_app::ports::{
    DeleteWorkspaceRequest, DeleteWorkspaceResponse, EffectError, ProvisionWorkspaceRequest,
    ProvisionWorkspaceResponse, RegionalControlPort,
};
use aex_identity_app::ports::IdFactory;
use aex_internal_contracts::SchemaVersion;
use aex_internal_contracts::control::{
    RegionalControlEnvelope, RegionalControlOutcome, RegionalControlRequest, RegionalRefusal,
};
use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::Region;
use async_trait::async_trait;
use aws_sdk_lambda::Client;
use aws_sdk_lambda::error::{ProvideErrorMetadata, SdkError};
use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;

/// The refusal a region that is not configured produces.
///
/// Non-retryable: a region absent from the endpoint map is a deployment fact,
/// and retrying will not add it. It is a rejection rather than an unknown
/// because nothing was dispatched — no request left this process.
const UNCONFIGURED_REGION: &str = "regional_endpoint_not_configured";

/// The refusal a region that answered about something else produces.
const WRONG_SUBJECT: &str = "regional_answered_another_subject";

/// The regional control authority over `lambda:InvokeFunction`.
#[derive(Debug, Clone)]
pub struct LambdaRegionalControl {
    client: Client,
    functions: BTreeMap<Region, String>,
}

impl LambdaRegionalControl {
    /// Builds the port over one function per reachable region.
    ///
    /// The map is the whole placement surface: a workspace can only be created
    /// in a region this deployable was configured for, and a region that is
    /// missing is refused rather than defaulted to a neighbour.
    #[must_use]
    pub fn new(client: Client, functions: BTreeMap<Region, String>) -> Self {
        Self { client, functions }
    }

    /// Which regions this port can reach.
    #[must_use]
    pub fn regions(&self) -> Vec<Region> {
        self.functions.keys().copied().collect()
    }

    /// Invokes one region and decodes its answer.
    async fn call(
        &self,
        region: Region,
        request_id: Uuid7,
        payload: RegionalControlRequest,
    ) -> Result<RegionalControlOutcome, EffectError> {
        let Some(function) = self.functions.get(&region) else {
            return Err(EffectError::Rejected {
                code: UNCONFIGURED_REGION,
                retryable: false,
            });
        };
        let envelope = RegionalControlEnvelope {
            schema_version: SchemaVersion::V1,
            request_id,
            payload,
        };
        let body = serde_json::to_vec(&envelope).map_err(|_| EffectError::Rejected {
            code: "regional_request_not_encodable",
            retryable: false,
        })?;
        let output = self
            .client
            .invoke()
            .function_name(function)
            .invocation_type(InvocationType::RequestResponse)
            .payload(Blob::new(body))
            .send()
            .await
            .map_err(|error| invoke_error(&error))?;
        // A non-2xx invoke status means the platform did not deliver a clean
        // answer. It does not mean the handler never ran.
        if !(200..300).contains(&output.status_code()) {
            return Err(EffectError::Unknown);
        }
        // `function_error` means the handler ran and raised. Whatever it did
        // before raising is exactly what a reconciler exists to establish.
        if output.function_error().is_some() {
            return Err(EffectError::Unknown);
        }
        let Some(blob) = output.payload else {
            return Err(EffectError::Unknown);
        };
        let answer: RegionalControlEnvelope<RegionalControlOutcome> =
            serde_json::from_slice(blob.as_ref()).map_err(|_| EffectError::Unknown)?;
        if answer.schema_version != SchemaVersion::V1 {
            // A version this process cannot read is one whose effect it cannot
            // establish. Reconciling is the only honest answer.
            return Err(EffectError::Unknown);
        }
        Ok(answer.payload)
    }
}

/// Turns a wire refusal into the port's rejection.
const fn refused(reason: RegionalRefusal) -> EffectError {
    EffectError::Rejected {
        code: reason.code(),
        retryable: reason.retryable(),
    }
}

/// Maps an invoke failure onto the effect vocabulary.
///
/// The split is "did the handler have a chance to run?". Every arm that answers
/// "no" is [`EffectError::Unavailable`]; every arm that cannot answer is
/// [`EffectError::Unknown`]. A timeout is always the second, which is the whole
/// reason this function exists rather than a `?`.
fn invoke_error<E, R>(error: &SdkError<E, R>) -> EffectError
where
    E: ProvideErrorMetadata,
{
    if nothing_was_dispatched(error) {
        return EffectError::Unavailable;
    }
    // Everything left over — a client-side timeout, a response that could not
    // be read, a platform failure, or an `SdkError` variant a later SDK adds —
    // is a request that may have reached the handler. Reconciliation is the
    // only answer that is safe in both worlds, and it is deliberately the
    // *default* rather than an arm somebody has to remember to add.
    EffectError::Unknown
}

/// Whether the failure proves the handler was never entered.
///
/// Only the refusals Lambda issues before invocation qualify. A timeout does
/// not, and never will: the request had already left.
fn nothing_was_dispatched<E, R>(error: &SdkError<E, R>) -> bool
where
    E: ProvideErrorMetadata,
{
    match error {
        // The request was never built, or the connection never carried it.
        SdkError::ConstructionFailure(_) | SdkError::DispatchFailure(_) => true,
        SdkError::ServiceError(service) => matches!(
            service.err().code().unwrap_or("Unknown"),
            "ResourceNotFoundException"
                | "InvalidRequestContentException"
                | "InvalidParameterValueException"
                | "RequestTooLargeException"
                | "UnsupportedMediaTypeException"
                | "TooManyRequestsException"
                | "AccessDeniedException"
                | "ResourceConflictException"
        ),
        _ => false,
    }
}

#[async_trait]
impl RegionalControlPort for LambdaRegionalControl {
    async fn provision_workspace(
        &self,
        request: &ProvisionWorkspaceRequest,
    ) -> Result<ProvisionWorkspaceResponse, EffectError> {
        let workspace = wire_workspace(request.workspace_id)?;
        let organization = wire_organization(request.organization_id)?;
        let outcome = self
            .call(
                request.region,
                workspace.uuid7(),
                RegionalControlRequest::ProvisionWorkspace {
                    workspace,
                    organization,
                    region: request.region,
                    fence: request.fence.get(),
                    intent_hash: hex::encode(request.intent_hash.as_bytes()),
                },
            )
            .await?;
        match outcome {
            RegionalControlOutcome::WorkspaceProvisioned {
                workspace: answered,
                created,
            } if answered == workspace => Ok(ProvisionWorkspaceResponse {
                workspace_id: request.workspace_id,
                created,
            }),
            RegionalControlOutcome::Refused { reason } => Err(refused(reason)),
            // An answer about another workspace, or about a deletion, is not an
            // answer to this request. It is never treated as success.
            RegionalControlOutcome::WorkspaceProvisioned { .. }
            | RegionalControlOutcome::WorkspaceDeleted { .. } => Err(EffectError::Rejected {
                code: WRONG_SUBJECT,
                retryable: false,
            }),
        }
    }

    async fn delete_workspace(
        &self,
        request: &DeleteWorkspaceRequest,
    ) -> Result<DeleteWorkspaceResponse, EffectError> {
        let workspace = wire_workspace(request.workspace_id)?;
        let outcome = self
            .call(
                request.region,
                workspace.uuid7(),
                RegionalControlRequest::DeleteWorkspace {
                    workspace,
                    region: request.region,
                    fence: request.fence.get(),
                },
            )
            .await?;
        match outcome {
            RegionalControlOutcome::WorkspaceDeleted {
                workspace: answered,
                removed,
            } if answered == workspace => Ok(DeleteWorkspaceResponse { removed }),
            RegionalControlOutcome::Refused { reason } => Err(refused(reason)),
            RegionalControlOutcome::WorkspaceDeleted { .. }
            | RegionalControlOutcome::WorkspaceProvisioned { .. } => Err(EffectError::Rejected {
                code: WRONG_SUBJECT,
                retryable: false,
            }),
        }
    }
}

/// Every AEX row id is a `UUIDv7`; one that is not is a schema defect.
fn wire_workspace(id: uuid::Uuid) -> Result<WorkspaceId, EffectError> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(WorkspaceId::from_uuid7)
        .map_err(|_| EffectError::Rejected {
            code: "regional_subject_is_not_a_uuid_v7",
            retryable: false,
        })
}

/// The organization half of the same rule.
fn wire_organization(id: uuid::Uuid) -> Result<OrganizationId, EffectError> {
    Uuid7::from_bytes(*id.as_bytes())
        .map(OrganizationId::from_uuid7)
        .map_err(|_| EffectError::Rejected {
            code: "regional_subject_is_not_a_uuid_v7",
            retryable: false,
        })
}

/// The identifier factory a composition passes so the port mints no id itself.
///
/// Exposed so a deployable can share one factory across every port rather than
/// letting each adapter reach for its own source.
pub type SharedIds = Arc<dyn IdFactory>;

#[cfg(test)]
mod tests {
    use super::{UNCONFIGURED_REGION, refused};
    use aex_control_app::ports::EffectError;
    use aex_internal_contracts::control::RegionalRefusal;

    #[test]
    fn every_refusal_maps_to_a_stable_code_and_a_stated_retry_decision() {
        for reason in RegionalRefusal::ALL {
            let mapped = refused(reason);
            match mapped {
                EffectError::Rejected { code, retryable } => {
                    assert_eq!(code, reason.code());
                    assert_eq!(retryable, reason.retryable());
                    assert!(code.starts_with("regional_"), "{code}");
                }
                other => panic!("a refusal became {other:?}"),
            }
        }
    }

    #[test]
    fn a_superseded_fence_is_never_retryable_under_the_same_fence() {
        assert!(
            !RegionalRefusal::FenceSuperseded.retryable(),
            "retrying a superseded fence races the attempt that superseded it"
        );
        assert!(!RegionalRefusal::OrganizationMismatch.retryable());
        assert!(!RegionalRefusal::IntentConflict.retryable());
        assert!(RegionalRefusal::RegionUnavailable.retryable());
        assert!(RegionalRefusal::Internal.retryable());
    }

    #[test]
    fn every_refusal_code_is_distinct() {
        let codes: std::collections::BTreeSet<_> =
            RegionalRefusal::ALL.iter().map(|it| it.code()).collect();
        assert_eq!(codes.len(), RegionalRefusal::ALL.len());
        assert!(
            !codes.contains(UNCONFIGURED_REGION),
            "a wire code shadowed a local one"
        );
    }
}
