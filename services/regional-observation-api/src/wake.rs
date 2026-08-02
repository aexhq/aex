//! Keyed wake hints for long-lived observation readers.
//!
//! A wake never carries response data and never establishes completeness. It
//! only interrupts an adaptive fallback timer; every connection still reads its
//! own authoritative range after its last durable cursor. A missed or coalesced
//! wake therefore changes latency, not correctness.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aex_observation_domain::keys::ScopeKey;
use tokio::sync::watch;

/// Process-wide keyed wake registry.
#[derive(Clone, Debug, Default)]
pub struct WakeHub {
    senders: Arc<Mutex<BTreeMap<ScopeKey, watch::Sender<u64>>>>,
}

impl WakeHub {
    /// Subscribes one connection to its exact authority scope.
    #[must_use]
    pub fn subscribe(&self, scope: ScopeKey) -> WakeSubscription {
        let mut senders = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let receiver = if let Some(sender) = senders.get(&scope) {
            sender.subscribe()
        } else {
            let (sender, receiver) = watch::channel(0);
            senders.insert(scope, sender);
            receiver
        };
        WakeSubscription { receiver }
    }

    /// Coalesces one hint for every connection currently following `scope`.
    ///
    /// Returns the number of receivers notified. No registered connection is a
    /// successful no-op: the adaptive authority poll remains the recovery path.
    pub fn notify(&self, scope: ScopeKey) -> usize {
        let mut senders = self
            .senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(sender) = senders.get(&scope) else {
            return 0;
        };
        let receivers = sender.receiver_count();
        if receivers == 0 {
            senders.remove(&scope);
            return 0;
        }
        sender.send_modify(|generation| *generation = generation.wrapping_add(1));
        receivers
    }

    /// Number of scopes with at least one current or recently dropped watcher.
    #[must_use]
    pub fn registered_scopes(&self) -> usize {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

/// One connection's coalescing wake receiver.
#[derive(Debug)]
pub struct WakeSubscription {
    receiver: watch::Receiver<u64>,
}

impl WakeSubscription {
    /// Waits for a matching hint or the correctness-preserving fallback timer.
    ///
    /// `true` means a hint arrived. `false` means the caller must perform its
    /// periodic authoritative read because hints may be lost at any boundary.
    pub async fn wait(&mut self, fallback: Duration) -> bool {
        tokio::time::timeout(fallback, self.receiver.changed())
            .await
            .is_ok_and(|result| result.is_ok())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aex_observation_domain::keys::ScopeKey;
    use aex_wire::ids::{PrefixedId as _, SessionId, Uuid7, WorkspaceId};

    use super::WakeHub;

    fn session(seed: u8) -> ScopeKey {
        ScopeKey::Session(SessionId::from_uuid7(Uuid7::compose(1, [seed; 10])))
    }

    fn workspace(seed: u8) -> ScopeKey {
        ScopeKey::Workspace(WorkspaceId::from_uuid7(Uuid7::compose(1, [seed; 10])))
    }

    #[tokio::test]
    async fn hints_are_keyed_coalesced_and_never_required_for_progress() {
        let hub = WakeHub::default();
        let mut one = hub.subscribe(session(1));
        let mut duplicate = hub.subscribe(session(1));
        let mut other = hub.subscribe(workspace(2));

        assert_eq!(hub.notify(session(1)), 2);
        assert_eq!(hub.notify(session(1)), 2, "duplicates coalesce in watch");
        assert!(one.wait(Duration::from_secs(1)).await);
        assert!(duplicate.wait(Duration::from_secs(1)).await);
        assert!(!other.wait(Duration::from_millis(1)).await);
        assert_eq!(hub.notify(session(9)), 0, "loss is only a latency fact");
    }

    #[test]
    fn unused_scope_entries_are_reclaimed_on_the_next_hint() {
        let hub = WakeHub::default();
        let subscription = hub.subscribe(session(1));
        assert_eq!(hub.registered_scopes(), 1);
        drop(subscription);
        assert_eq!(hub.notify(session(1)), 0);
        assert_eq!(hub.registered_scopes(), 0);
    }
}
