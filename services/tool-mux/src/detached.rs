//! Process-local runners for fast detached Tool Mux adapters.
//!
//! Handles carry deterministic call identity. Replaying `start` in the same
//! service process observes the existing runner, while the underlying MCP and
//! storage adapters retain their own idempotency rules across restarts.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};

use aex_tool_mux::{ExecutorOutput, FullOutput, ToolCallIdentity};
use aex_wire::ids::{PrefixedId as _, Uuid7};
use sha2::{Digest as _, Sha256};

#[derive(Clone, Default)]
/// Process-local registry for an adapter whose underlying operation is idempotent.
pub struct DetachedExecutions {
    states: Arc<Mutex<BTreeMap<Uuid7, State>>>,
}

#[derive(Clone)]
enum State {
    Running(tokio::task::AbortHandle),
    Completed(Result<ExecutorOutput, String>),
}

impl DetachedExecutions {
    /// Starts one deterministic execution once in this process.
    pub fn start<F>(&self, operation: Uuid7, future: F)
    where
        F: Future<Output = Result<ExecutorOutput, String>> + Send + 'static,
    {
        let mut states = self.states.lock().expect("detached execution mutex");
        if states.contains_key(&operation) {
            return;
        }
        let (release, released) = tokio::sync::oneshot::channel();
        let shared = Arc::clone(&self.states);
        let task = tokio::spawn(async move {
            let _ = released.await;
            let outcome = future.await;
            let mut states = shared.lock().expect("detached execution mutex");
            if matches!(states.get(&operation), Some(State::Running(_))) {
                states.insert(operation, State::Completed(outcome));
            }
        });
        states.insert(operation, State::Running(task.abort_handle()));
        drop(states);
        let _ = release.send(());
    }

    /// Reads one execution without starting it.
    pub fn read(&self, operation: Uuid7) -> Result<Option<ExecutorOutput>, String> {
        match self
            .states
            .lock()
            .expect("detached execution mutex")
            .get(&operation)
            .cloned()
        {
            Some(State::Running(_)) => Ok(None),
            Some(State::Completed(result)) => result.map(Some),
            None => Err("detached tool operation is unknown".to_owned()),
        }
    }

    /// Cancels one execution and retains a stable cancelled result.
    pub fn cancel(&self, operation: Uuid7) {
        let mut states = self.states.lock().expect("detached execution mutex");
        if let Some(State::Running(task)) = states.get(&operation) {
            task.abort();
        }
        states.insert(operation, State::Completed(Ok(cancelled_output())));
    }
}

/// Derives a namespace-separated, deterministic operation id from call identity.
pub fn operation_id(call: &ToolCallIdentity, namespace: &str) -> Uuid7 {
    let digest = Sha256::digest(
        serde_json::to_vec(&(namespace, call))
            .unwrap_or_else(|_| unreachable!("tool identity is encodable")),
    );
    let mut entropy = [0_u8; 10];
    entropy.copy_from_slice(&digest[..10]);
    Uuid7::compose(call.session.uuid7().unix_millis(), entropy)
}

fn cancelled_output() -> ExecutorOutput {
    let body = br#"{"error":"operation cancelled"}"#.to_vec();
    ExecutorOutput {
        preview: body.clone(),
        full: FullOutput::Inline(body),
        is_error: true,
    }
}
