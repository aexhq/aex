//! Deterministic scripted tool fixture shared by Brain application tests.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use aex_brain_domain::effect::EffectClass;
use aex_brain_domain::ids::{CatalogPin, ContentHash, DetachedOperationId, Fence, ToolName};
use aex_brain_domain::journal::ExecutorRoute;

/// One deterministic route entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptedTool {
    /// Pinned catalog containing the route.
    pub pin: CatalogPin,
    /// Exact tool name.
    pub name: ToolName,
    /// Executor that should appear in receipts.
    pub executor: ExecutorRoute,
    /// Recovery class fixed before dispatch.
    pub class: EffectClass,
    /// Invocation wall bound.
    pub timeout_ms: u32,
    /// Frozen manifest digest.
    pub manifest_digest: ContentHash,
}

/// One queued invocation answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeInvocation {
    /// Model-visible completed result.
    Completed {
        /// Canonical test result document.
        result: serde_json::Value,
        /// Whether the result is a tool-level error.
        is_error: bool,
    },
    /// Accepted detached operation.
    Detached {
        /// Stable operation identity.
        operation: DetachedOperationId,
        /// First poll delay.
        poll_after_ms: u32,
    },
    /// Redacted transport failure.
    DispatchFailure {
        /// Static redacted fixture text.
        detail: String,
        /// Whether a retry could help.
        retryable: bool,
    },
}

/// One queued detached-operation status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeQueryStatus {
    /// Still running.
    Running {
        /// Next durable poll delay.
        poll_after_ms: u32,
    },
    /// Terminal model-visible result.
    Completed {
        /// Canonical test result document.
        result: serde_json::Value,
        /// Whether the result is a tool-level error.
        is_error: bool,
    },
    /// Definitive terminal failure.
    Failed {
        /// Redacted fixture text.
        detail: String,
    },
    /// No durable operation identity was committed.
    Unknown,
}

/// Thread-safe deterministic fake. It reads no clock or random source; every
/// answer must be queued explicitly by the test.
#[derive(Debug, Default)]
pub struct ToolFake {
    routes: Mutex<BTreeMap<(CatalogPin, ToolName), ScriptedTool>>,
    invocations: Mutex<VecDeque<FakeInvocation>>,
    queries: Mutex<BTreeMap<DetachedOperationId, VecDeque<FakeQueryStatus>>>,
    invoked: Mutex<Vec<ToolName>>,
    queried: Mutex<Vec<DetachedOperationId>>,
    cancelled: Mutex<Vec<(DetachedOperationId, Fence)>>,
}

impl ToolFake {
    /// Empty script.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an exact route. Replacing an existing entry is a test bug.
    ///
    /// # Panics
    ///
    /// Panics when the same pin/name pair is registered twice.
    pub fn add_route(&self, route: ScriptedTool) {
        let key = (route.pin, route.name.clone());
        let replaced = lock(&self.routes).insert(key, route);
        assert!(replaced.is_none(), "duplicate fake tool route");
    }

    /// Resolves one exact pin/name pair.
    #[must_use]
    pub fn route(&self, pin: CatalogPin, name: &ToolName) -> Option<ScriptedTool> {
        lock(&self.routes).get(&(pin, name.clone())).cloned()
    }

    /// Queues one invocation response.
    pub fn push_invocation(&self, response: FakeInvocation) {
        lock(&self.invocations).push_back(response);
    }

    /// Consumes exactly one scripted invocation response.
    ///
    /// # Panics
    ///
    /// Panics when a test invokes without arranging an answer.
    #[must_use]
    pub fn invoke(&self, name: &ToolName) -> FakeInvocation {
        lock(&self.invoked).push(name.clone());
        lock(&self.invocations)
            .pop_front()
            .expect("unscripted fake tool invocation")
    }

    /// Queues detached status responses for one operation.
    ///
    /// # Panics
    ///
    /// Panics when the operation already has a query script.
    pub fn set_query_script(
        &self,
        operation: DetachedOperationId,
        statuses: impl IntoIterator<Item = FakeQueryStatus>,
    ) {
        let statuses = statuses.into_iter().collect::<VecDeque<_>>();
        let replaced = lock(&self.queries).insert(operation, statuses);
        assert!(replaced.is_none(), "duplicate fake query script");
    }

    /// Consumes the next status for the same durable identity.
    ///
    /// # Panics
    ///
    /// Panics for an unknown operation or exhausted script.
    #[must_use]
    pub fn query(&self, operation: &DetachedOperationId) -> FakeQueryStatus {
        lock(&self.queried).push(operation.clone());
        lock(&self.queries)
            .get_mut(operation)
            .expect("unknown fake detached operation")
            .pop_front()
            .expect("exhausted fake query script")
    }

    /// Records a fenced cancellation without manufacturing a terminal state.
    pub fn cancel(&self, operation: DetachedOperationId, fence: Fence) {
        lock(&self.cancelled).push((operation, fence));
    }

    /// Invocation names in exact observed order.
    #[must_use]
    pub fn invoked(&self) -> Vec<ToolName> {
        lock(&self.invoked).clone()
    }

    /// Queried operation ids in exact observed order.
    #[must_use]
    pub fn queried(&self) -> Vec<DetachedOperationId> {
        lock(&self.queried).clone()
    }

    /// Fenced cancellations in exact observed order.
    #[must_use]
    pub fn cancelled(&self) -> Vec<(DetachedOperationId, Fence)> {
        lock(&self.cancelled).clone()
    }

    /// Asserts every invocation and query response was consumed.
    ///
    /// # Panics
    ///
    /// Panics with the outstanding script counts.
    pub fn assert_drained(&self) {
        let invocations = lock(&self.invocations).len();
        let query_counts = lock(&self.queries)
            .values()
            .map(VecDeque::len)
            .collect::<BTreeSet<_>>();
        assert_eq!(invocations, 0, "unconsumed fake tool invocations");
        assert!(
            query_counts.iter().all(|count| *count == 0),
            "unconsumed fake tool query statuses: {query_counts:?}"
        );
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{FakeInvocation, FakeQueryStatus, ToolFake};
    use aex_brain_domain::ids::{DetachedOperationId, Fence, ToolName};

    #[test]
    fn scripts_are_fifo_and_reuse_the_same_detached_identity() {
        let fake = ToolFake::new();
        let name = ToolName("web_fetch".to_owned());
        let operation = DetachedOperationId("task-7".to_owned());
        fake.push_invocation(FakeInvocation::Detached {
            operation: operation.clone(),
            poll_after_ms: 1_000,
        });
        fake.set_query_script(
            operation.clone(),
            [
                FakeQueryStatus::Running {
                    poll_after_ms: 2_000,
                },
                FakeQueryStatus::Completed {
                    result: serde_json::json!({"ok":true}),
                    is_error: false,
                },
            ],
        );

        assert!(matches!(
            fake.invoke(&name),
            FakeInvocation::Detached { .. }
        ));
        assert!(matches!(
            fake.query(&operation),
            FakeQueryStatus::Running { .. }
        ));
        assert!(matches!(
            fake.query(&operation),
            FakeQueryStatus::Completed { .. }
        ));
        fake.cancel(operation.clone(), Fence(9));
        assert_eq!(fake.invoked(), vec![name]);
        assert_eq!(fake.queried(), vec![operation.clone(), operation.clone()]);
        assert_eq!(fake.cancelled(), vec![(operation, Fence(9))]);
        fake.assert_drained();
    }
}
