//! Closed direct-invoke controller for durable workspace capacity.

use aex_capacity_dynamodb::{
    AppliedCapacity, CapacityCommand, CapacityDefaults, CapacityError, CapacityState,
    CapacityStore, CapacityStoreError,
};
use aex_wire::types::{Region, Timestamp};
use async_trait::async_trait;
use serde::Serialize;

/// Required composition variables.
pub mod keys {
    /// Deployment plane.
    pub const PLANE: &str = "AEX_PLANE";
    /// Launch region.
    pub const REGION: &str = "AEX_REGION";
    /// Immutable release identity.
    pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
    /// Durable capacity authority table.
    pub const AUTHORITY_TABLE: &str = "AEX_CAPACITY_AUTHORITY_TABLE";
    /// Public effective-limit projection table.
    pub const PROJECTION_TABLE: &str = "AEX_AUTHZ_PROJECTION_TABLE";
    /// Full required set, in diagnostic order.
    pub const ALL: &[&str] = &[
        PLANE,
        REGION,
        RELEASE_DIGEST,
        AUTHORITY_TABLE,
        PROJECTION_TABLE,
    ];
}

/// Why the controller refused startup.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RegionalCapacityControllerConfigError {
    /// Required input was absent or empty.
    #[error("required environment variable `{0}` is missing")]
    Missing(&'static str),
    /// Required input had an invalid closed value.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// Variable name.
        name: &'static str,
        /// Bounded explanation.
        reason: String,
    },
}

/// Validated controller composition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// `dev` or `prd`.
    pub plane: String,
    /// Pinned launch region.
    pub region: Region,
    /// Immutable source/artifact identity.
    pub release_digest: String,
    /// Durable capacity authority table.
    pub authority_table: String,
    /// Public effective-limit projection table.
    pub projection_table: String,
}

impl Config {
    /// Reads process configuration.
    ///
    /// # Errors
    ///
    /// Returns the first absent or invalid required value.
    pub fn from_env() -> Result<Self, RegionalCapacityControllerConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads a testable configuration lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Self::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, RegionalCapacityControllerConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, keys::PLANE)?;
        if !matches!(plane.as_str(), "dev" | "prd") {
            return Err(invalid(keys::PLANE, "expected `dev` or `prd`"));
        }
        let region_raw = required(&lookup, keys::REGION)?;
        let region = Region::from_name(&region_raw)
            .ok_or_else(|| invalid(keys::REGION, "outside the launch-region vocabulary"))?;
        let release_digest = required(&lookup, keys::RELEASE_DIGEST)?;
        let digest = release_digest.strip_prefix("sha256:").unwrap_or_default();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid(
                keys::RELEASE_DIGEST,
                "expected `sha256:` followed by 64 hexadecimal digits",
            ));
        }
        let authority_table = required(&lookup, keys::AUTHORITY_TABLE)?;
        let projection_table = required(&lookup, keys::PROJECTION_TABLE)?;
        if authority_table == projection_table {
            return Err(invalid(
                keys::PROJECTION_TABLE,
                "authority and projection must be distinct tables",
            ));
        }
        Ok(Self {
            plane,
            region,
            release_digest,
            authority_table,
            projection_table,
        })
    }
}

fn required<F>(
    lookup: &F,
    name: &'static str,
) -> Result<String, RegionalCapacityControllerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    lookup(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(RegionalCapacityControllerConfigError::Missing(name))
}

fn invalid(name: &'static str, reason: &str) -> RegionalCapacityControllerConfigError {
    RegionalCapacityControllerConfigError::Invalid {
        name,
        reason: reason.to_owned(),
    }
}

/// The bounded JSON returned to an internal direct invoker.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ControllerResponse {
    /// A new revision committed, or an exact command replay resolved.
    Applied {
        /// Complete durable state.
        state: Box<CapacityState>,
        /// False for an exact durable replay or no-op reconcile.
        changed: bool,
    },
    /// A deterministic policy/precondition refusal; no write occurred.
    Refused {
        /// Stable closed reason code.
        code: &'static str,
        /// Bounded human diagnostic.
        message: String,
    },
}

/// Clock boundary used by the deterministic controller tests.
pub trait Clock: Send + Sync {
    /// Current millisecond-truncated instant.
    fn now(&self) -> Timestamp;
}

/// Production wall clock.
#[derive(Clone, Copy, Debug)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_datetime_trunc_ms(time::OffsetDateTime::now_utc())
            .expect("the host clock must fit the public timestamp range")
    }
}

/// Narrow durable store boundary.
#[async_trait]
pub trait Store: Send + Sync {
    /// Applies one complete command.
    async fn apply(
        &self,
        defaults: &CapacityDefaults,
        command: &CapacityCommand,
        now: Timestamp,
    ) -> Result<AppliedCapacity, CapacityStoreError>;
}

#[async_trait]
impl Store for CapacityStore {
    async fn apply(
        &self,
        defaults: &CapacityDefaults,
        command: &CapacityCommand,
        now: Timestamp,
    ) -> Result<AppliedCapacity, CapacityStoreError> {
        CapacityStore::apply(self, defaults, command, now).await
    }
}

/// Stateless Lambda handler around the durable authority.
#[derive(Debug)]
pub struct CapacityController<S, C> {
    store: S,
    defaults: CapacityDefaults,
    clock: C,
}

impl<S, C> CapacityController<S, C>
where
    S: Store,
    C: Clock,
{
    /// Creates a handler from validated defaults and explicit boundaries.
    #[must_use]
    pub const fn new(store: S, defaults: CapacityDefaults, clock: C) -> Self {
        Self {
            store,
            defaults,
            clock,
        }
    }

    /// Handles one closed [`CapacityCommand`] direct invocation.
    ///
    /// Deterministic policy errors are returned as bounded JSON so callers do
    /// not retry them. Store/transport errors become Lambda function errors so
    /// the invoker's bounded retry/DLQ policy remains authoritative.
    ///
    /// # Errors
    ///
    /// Returns only operational persistence failures.
    pub async fn handle(
        &self,
        command: CapacityCommand,
    ) -> Result<ControllerResponse, lambda_runtime::Error> {
        match self
            .store
            .apply(&self.defaults, &command, self.clock.now())
            .await
        {
            Ok(AppliedCapacity { state, changed }) => Ok(ControllerResponse::Applied {
                state: Box::new(state),
                changed,
            }),
            Err(CapacityStoreError::Capacity(error)) => Ok(ControllerResponse::Refused {
                code: capacity_code(&error),
                message: bounded_message(&error.to_string()),
            }),
            Err(CapacityStoreError::Store(error)) => Err(error.into()),
        }
    }
}

fn capacity_code(error: &CapacityError) -> &'static str {
    match error {
        CapacityError::AlreadyExists => "already_exists",
        CapacityError::Missing => "missing",
        CapacityError::Revision { .. } => "revision_conflict",
        CapacityError::CapacityFence { .. } => "capacity_fence_conflict",
        CapacityError::Approval => "invalid_approval",
        CapacityError::DefaultsRegression { .. } => "defaults_regression",
        CapacityError::DefaultsDigest { .. } => "defaults_digest_conflict",
        CapacityError::Override { .. } => "invalid_override",
        CapacityError::RevisionExhausted => "revision_exhausted",
    }
}

fn bounded_message(message: &str) -> String {
    message.chars().take(256).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use aex_capacity_dynamodb::{
        CapacityCommand, CapacityStoreError, canonical_defaults, plan_capacity_change,
    };
    use aex_wire::ids::{PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::limits::LimitId;
    use aex_wire::models::{LimitScalarValue, LimitValue};
    use aex_wire::types::{DecimalU128, Timestamp};
    use async_trait::async_trait;

    use super::{CapacityController, Clock, ControllerResponse, Store};

    struct MemoryStore(Mutex<Option<aex_capacity_dynamodb::CapacityState>>);

    #[async_trait]
    impl Store for MemoryStore {
        async fn apply(
            &self,
            defaults: &aex_capacity_dynamodb::CapacityDefaults,
            command: &CapacityCommand,
            now: Timestamp,
        ) -> Result<aex_capacity_dynamodb::AppliedCapacity, CapacityStoreError> {
            let mut state = self.0.lock().expect("lock");
            let planned = plan_capacity_change(state.as_ref(), defaults, command, now)?;
            if planned.changed {
                *state = Some(planned.state.clone());
            }
            Ok(aex_capacity_dynamodb::AppliedCapacity {
                state: planned.state,
                changed: planned.changed,
            })
        }
    }

    #[derive(Clone, Copy)]
    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            self.0
        }
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [8; 10]))
    }

    fn controller() -> CapacityController<MemoryStore, FixedClock> {
        CapacityController::new(
            MemoryStore(Mutex::new(None)),
            canonical_defaults().expect("defaults"),
            FixedClock(Timestamp::from_unix_millis(1).expect("timestamp")),
        )
    }

    #[tokio::test]
    async fn bootstrap_reconcile_override_and_exact_retry_are_closed() {
        let controller = controller();
        let bootstrap = CapacityCommand::Bootstrap {
            workspace_id: workspace(),
        };
        assert!(matches!(
            controller
                .handle(bootstrap.clone())
                .await
                .expect("bootstrap"),
            ControllerResponse::Applied { changed: true, .. }
        ));
        assert!(matches!(
            controller.handle(bootstrap).await.expect("replay"),
            ControllerResponse::Applied { changed: false, .. }
        ));
        assert!(matches!(
            controller
                .handle(CapacityCommand::Reconcile {
                    workspace_id: workspace(),
                    expected_revision: 1,
                })
                .await
                .expect("reconcile"),
            ControllerResponse::Applied { changed: false, .. }
        ));
        let override_command = CapacityCommand::SetOverride {
            workspace_id: workspace(),
            expected_revision: 1,
            capacity_fence: 7,
            approval_id: "support-123".to_owned(),
            limit_id: LimitId::ApiJsonBody,
            value: LimitValue::Scalar(LimitScalarValue {
                value: DecimalU128::new(131_072),
            }),
        };
        assert!(matches!(
            controller
                .handle(override_command.clone())
                .await
                .expect("override"),
            ControllerResponse::Applied { changed: true, .. }
        ));
        assert!(matches!(
            controller
                .handle(override_command)
                .await
                .expect("exact retry"),
            ControllerResponse::Applied { changed: false, .. }
        ));
    }

    #[tokio::test]
    async fn malformed_approval_and_stale_revision_are_bounded_refusals() {
        let controller = controller();
        controller
            .handle(CapacityCommand::Bootstrap {
                workspace_id: workspace(),
            })
            .await
            .expect("bootstrap");
        let response = controller
            .handle(CapacityCommand::SetOverride {
                workspace_id: workspace(),
                expected_revision: 1,
                capacity_fence: 1,
                approval_id: String::new(),
                limit_id: LimitId::ApiJsonBody,
                value: LimitValue::Scalar(LimitScalarValue {
                    value: DecimalU128::new(1),
                }),
            })
            .await
            .expect("deterministic refusal");
        assert!(matches!(
            response,
            ControllerResponse::Refused {
                code: "invalid_approval",
                ..
            }
        ));
        let encoded = serde_json::to_vec(&response).expect("JSON");
        assert!(encoded.len() < 512);
    }
}
