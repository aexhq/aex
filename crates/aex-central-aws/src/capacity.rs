//! The regional capacity authority, invoked as a Lambda function per region.
//!
//! Central control does not own capacity and cannot write a limit row: the
//! producer lives behind its own Cargo feature and its own IAM keyspace. What it
//! owns is the *trigger* — one `bootstrap` per newly placed workspace — and this
//! adapter is the whole of that.
//!
//! # Why "did not answer" is never "carry on"
//!
//! [`super::regional`] classifies an ambiguous invoke as `Unknown` because a
//! lost response may have created a workspace, and reconciling is the only safe
//! answer. Here the asymmetry is different and simpler. The caller's next act is
//! to publish a placement, and a placement published without a complete
//! effective-limit set does not degrade — every request for that workspace is
//! refused as an unknown credential, because the admission snapshot requires the
//! edge-limits row and treats its absence as a contradiction.
//!
//! So this adapter has exactly two answers: the set is durable, or it may not
//! be. Ambiguity is the second, and the caller leaves the placement unwritten
//! and retries. Bootstrap is idempotent, so retrying costs a no-op.

use std::collections::BTreeMap;

use aex_internal_contracts::capacity::{CapacityBootstrap, CapacityOutcome};
use aex_wire::ids::WorkspaceId;
use aex_wire::types::Region;
use aws_sdk_lambda::Client;
use aws_sdk_lambda::primitives::Blob;
use aws_sdk_lambda::types::InvocationType;

/// Why one capacity bootstrap did not establish a complete effective set.
///
/// Every arm is "do not publish a placement". They are distinguished only so the
/// released outbox row names which one happened, because a configuration gap and
/// a refusing authority need different operators.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapacityBootstrapError {
    /// No capacity function is configured for the workspace's region.
    #[error("regional_capacity_not_configured")]
    Unconfigured,
    /// The invocation did not produce a readable answer.
    #[error("regional_capacity_unavailable")]
    Unavailable,
    /// The controller refused deterministically, and not because the set exists.
    #[error("regional_capacity_refused:{code}")]
    Refused {
        /// The controller's stable closed reason code.
        code: String,
    },
}

/// The regional capacity authority over `lambda:InvokeFunction`.
#[derive(Debug, Clone)]
pub struct LambdaRegionalCapacity {
    client: Client,
    functions: BTreeMap<Region, String>,
}

impl LambdaRegionalCapacity {
    /// Binds the port over one controller function per reachable region.
    #[must_use]
    pub fn new(client: Client, functions: BTreeMap<Region, String>) -> Self {
        Self { client, functions }
    }

    /// Which regions this port can reach.
    #[must_use]
    pub fn regions(&self) -> Vec<Region> {
        self.functions.keys().copied().collect()
    }

    /// Materialises one workspace's complete effective-limit set.
    ///
    /// Returns `Ok(())` exactly when the set is durable afterwards — including
    /// the case where somebody else made it durable first.
    ///
    /// # Errors
    ///
    /// Returns [`CapacityBootstrapError`] for every answer that does not prove
    /// the set exists.
    pub async fn bootstrap(
        &self,
        region: Region,
        workspace: WorkspaceId,
    ) -> Result<(), CapacityBootstrapError> {
        let Some(function) = self.functions.get(&region) else {
            return Err(CapacityBootstrapError::Unconfigured);
        };
        let body = serde_json::to_vec(&CapacityBootstrap::new(workspace))
            .map_err(|_| CapacityBootstrapError::Unavailable)?;
        let output = self
            .client
            .invoke()
            .function_name(function)
            .invocation_type(InvocationType::RequestResponse)
            .payload(Blob::new(body))
            .send()
            .await
            .map_err(|_| CapacityBootstrapError::Unavailable)?;
        if !(200..300).contains(&output.status_code()) || output.function_error().is_some() {
            return Err(CapacityBootstrapError::Unavailable);
        }
        let Some(blob) = output.payload else {
            return Err(CapacityBootstrapError::Unavailable);
        };
        let outcome: CapacityOutcome =
            serde_json::from_slice(blob.as_ref()).map_err(|_| CapacityBootstrapError::Unavailable)?;
        if outcome.set_is_materialised() {
            return Ok(());
        }
        match outcome {
            CapacityOutcome::Refused { code, .. } => Err(CapacityBootstrapError::Refused { code }),
            // Unreachable: `set_is_materialised` is true for every `Applied`.
            CapacityOutcome::Applied { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CapacityBootstrapError, LambdaRegionalCapacity};
    use aex_wire::ids::PrefixedId as _;
    use aex_wire::types::Region;
    use std::collections::BTreeMap;

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime")
            .block_on(future)
    }

    #[test]
    fn a_region_with_no_controller_is_refused_rather_than_defaulted_to_a_neighbour() {
        let config = aws_sdk_lambda::Config::builder()
            .region(aws_sdk_lambda::config::Region::new("eu-west-1"))
            .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
            .build();
        let port = LambdaRegionalCapacity::new(
            aws_sdk_lambda::Client::from_conf(config),
            BTreeMap::from([(Region::EuWest1, "arn:aws:lambda:eu-west-1:1:function:c".to_owned())]),
        );
        assert_eq!(port.regions(), vec![Region::EuWest1]);
        let workspace = aex_wire::ids::WorkspaceId::from_uuid7(aex_wire::ids::Uuid7::compose(
            1_754_051_696_789,
            [4; 10],
        ));
        assert_eq!(
            run(port.bootstrap(Region::UsEast1, workspace)),
            Err(CapacityBootstrapError::Unconfigured),
            "a workspace placed where no controller is composed must not borrow another region's"
        );
    }

    /// Every failure renders as a stable, greppable string on the released
    /// outbox row. A bootstrap that failed silently would be indistinguishable
    /// from one that never ran.
    #[test]
    fn every_refusal_names_itself() {
        assert_eq!(
            CapacityBootstrapError::Unconfigured.to_string(),
            "regional_capacity_not_configured"
        );
        assert_eq!(
            CapacityBootstrapError::Unavailable.to_string(),
            "regional_capacity_unavailable"
        );
        assert_eq!(
            CapacityBootstrapError::Refused {
                code: "revision_conflict".to_owned()
            }
            .to_string(),
            "regional_capacity_refused:revision_conflict"
        );
    }
}
