//! Fast prepaid admission with bounded, conservative unbilled exposure.
//!
//! The durable ledger is settled by the background sweeper. The hot request path reserves a
//! fixed maximum action exposure in memory; it performs a synchronous reconciliation only when
//! the account cache is missing/stale or approaches its low-balance guard. Reservations expire
//! conservatively after the settlement window and are never treated as usage debits themselves.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy)]
pub struct AdmissionConfig {
    pub action_exposure_microusd: i64,
    pub account_exposure_microusd: i64,
    pub low_balance_microusd: i64,
    pub stale_after_ms: i64,
    pub reservation_ttl_ms: i64,
    /// Hard process bound for tenant admission state. This is a cache, never durable authority.
    pub max_cached_accounts: usize,
}

impl Default for AdmissionConfig {
    fn default() -> Self {
        Self {
            action_exposure_microusd: 100_000,
            account_exposure_microusd: 1_000_000,
            low_balance_microusd: 1_000_000,
            stale_after_ms: 60_000,
            reservation_ttl_ms: 60_000,
            max_cached_accounts: 10_000,
        }
    }
}

impl AdmissionConfig {
    pub fn validate(self) -> anyhow::Result<Self> {
        if self.action_exposure_microusd <= 0 {
            anyhow::bail!("AEX_ADMISSION_ACTION_EXPOSURE_MICROUSD must be positive");
        }
        if self.account_exposure_microusd < self.action_exposure_microusd {
            anyhow::bail!(
                "AEX_ADMISSION_ACCOUNT_EXPOSURE_MICROUSD must be at least the action exposure"
            );
        }
        if self.low_balance_microusd < 0 {
            anyhow::bail!("AEX_ADMISSION_LOW_BALANCE_MICROUSD cannot be negative");
        }
        if self.stale_after_ms <= 0 || self.reservation_ttl_ms <= 0 {
            anyhow::bail!("Aex admission freshness and reservation windows must be positive");
        }
        if self.reservation_ttl_ms < self.stale_after_ms {
            anyhow::bail!(
                "AEX_ADMISSION_RESERVATION_SECONDS must be at least AEX_ADMISSION_STALE_SECONDS"
            );
        }
        if !(1..=1_000_000).contains(&self.max_cached_accounts) {
            anyhow::bail!("AEX_ADMISSION_MAX_CACHED_ACCOUNTS must be between 1 and 1000000");
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecision {
    Reserved,
    Reconcile,
    Insufficient,
    ExposureExhausted,
    CacheFull,
}

#[derive(Debug, Default)]
struct AccountState {
    ledger_generation: u64,
    balance_microusd: Option<i64>,
    reconciled_ms: Option<i64>,
    reservations: VecDeque<(i64, i64)>,
    reserved_microusd: i64,
    live_root_sessions: Option<(i64, i64)>,
    live_generation: u64,
    pending_session_creates: HashMap<String, PendingSessionCreate>,
    recently_used: bool,
    active_refreshes: usize,
}

#[derive(Debug, Default)]
struct PendingSessionCreate {
    holders: usize,
    /// Brain may have committed this create even though Aex did not receive a definitive
    /// response. Its durable SQLite intent remains charged against the account limit across
    /// restart until the same idempotency identity succeeds or strong discovery covers the
    /// possibly committed root. Time alone never releases that exposure.
    uncertain: bool,
}

impl AccountState {
    fn protected(&self) -> bool {
        self.reserved_microusd > 0
            || !self.pending_session_creates.is_empty()
            || self.active_refreshes > 0
    }
}

/// A bounded second-chance cache. Each account appears exactly once in `clock`, so churn cannot
/// grow bookkeeping independently of the configured tenant bound. Eviction is correctness-safe:
/// losing a balance or root-count entry only forces an authoritative reconciliation.
#[derive(Debug, Default)]
struct AccountCache {
    entries: HashMap<String, AccountState>,
    clock: VecDeque<String>,
}

impl AccountCache {
    fn get_mut(&mut self, account_id: &str) -> Option<&mut AccountState> {
        let state = self.entries.get_mut(account_id)?;
        state.recently_used = true;
        Some(state)
    }

    fn get_or_insert(
        &mut self,
        account_id: &str,
        max_accounts: usize,
        now_ms: i64,
    ) -> Option<&mut AccountState> {
        if self.entries.contains_key(account_id) {
            return self.get_mut(account_id);
        }
        if self.entries.len() >= max_accounts && !self.evict_one(now_ms) {
            return None;
        }
        self.clock.push_back(account_id.to_owned());
        self.entries.insert(
            account_id.to_owned(),
            AccountState {
                recently_used: true,
                ..AccountState::default()
            },
        );
        self.entries.get_mut(account_id)
    }

    fn evict_one(&mut self, now_ms: i64) -> bool {
        // A constant probe budget keeps hostile new-account churn from turning the admission lock
        // into an O(tenant-count) critical section. Rotating protected entries makes later retries
        // progress through the whole clock without ever exceeding the hard bound.
        const MAX_PROBES: usize = 256;
        let probes = self.clock.len().min(MAX_PROBES);
        let mut second_chance = Vec::with_capacity(probes);
        let mut victim = None;
        for _ in 0..probes {
            let Some(account_id) = self.clock.pop_front() else {
                break;
            };
            let Some(state) = self.entries.get_mut(&account_id) else {
                continue;
            };
            prune(state, now_ms);
            if state.protected() {
                self.clock.push_back(account_id);
            } else if state.recently_used {
                state.recently_used = false;
                second_chance.push(account_id);
            } else {
                victim = Some(account_id);
                break;
            }
        }
        // If every safe candidate was recently referenced, evict the oldest one in this bounded
        // sample. This is a performance cache: eviction cannot authorize work by itself.
        if victim.is_none() && !second_chance.is_empty() {
            victim = Some(second_chance.remove(0));
        }
        for account_id in second_chance {
            self.clock.push_back(account_id);
        }
        let Some(victim) = victim else {
            return false;
        };
        self.entries.remove(&victim);
        true
    }
}

#[derive(Debug, Clone)]
pub struct Admission {
    config: AdmissionConfig,
    accounts: Arc<Mutex<AccountCache>>,
    session_refresh_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

#[derive(Debug)]
pub enum SessionCreateDecision {
    Reserved(SessionCreateReservation),
    Reconcile,
    ConcurrentLimit,
    CreateRateLimit,
    CacheFull,
}

/// One process-local claim on a unique create identity. Same-key concurrent retries share one
/// pending slot; the first durable success converts it to one live root session. A definitive
/// pre-commit rejection releases a holder, while an ambiguous transport/server outcome retains
/// the unique slot. The owning SQLite intent survives process restart until an identical retry
/// proves success or strong discovery one-to-one covers its counting exposure; there is no
/// time-only expiry. Child sessions use Brain's sealed tree limits and never enter this counter.
#[derive(Debug)]
#[must_use = "dropping the reservation releases the pending create"]
pub struct SessionCreateReservation {
    admission: Admission,
    account_id: String,
    request_id: String,
    active: bool,
}

impl SessionCreateReservation {
    pub fn commit(mut self) {
        self.admission.finish_session_create(
            &self.account_id,
            &self.request_id,
            CreateOutcome::Committed,
        );
        self.active = false;
    }

    pub fn uncertain(mut self) {
        self.admission.finish_session_create(
            &self.account_id,
            &self.request_id,
            CreateOutcome::Uncertain,
        );
        self.active = false;
    }
}

impl Drop for SessionCreateReservation {
    fn drop(&mut self) {
        if self.active {
            self.admission.finish_session_create(
                &self.account_id,
                &self.request_id,
                CreateOutcome::Rejected,
            );
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CreateOutcome {
    Committed,
    Uncertain,
    Rejected,
}

/// Keeps one balance reconciliation fenced against cache eviction until its remote/durable work
/// finishes. Dropping after any success or error releases the protection automatically.
#[derive(Debug)]
#[must_use = "the reconciliation fence must live across the durable balance read"]
pub struct ReconciliationFence {
    admission: Admission,
    account_id: String,
    generation: u64,
}

impl ReconciliationFence {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Drop for ReconciliationFence {
    fn drop(&mut self) {
        self.admission.finish_refresh(&self.account_id);
    }
}

/// Process-local serializer for the rare tenant-GSI root-count refresh. The map entry is removed
/// when the last caller drops its lease, using Arc identity/count fencing so a concurrent waiter
/// can never split onto a different mutex.
#[derive(Debug)]
#[must_use = "the refresh lease must live while using its mutex"]
pub struct SessionRefreshLease {
    admission: Admission,
    account_id: String,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl SessionRefreshLease {
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.lock.lock().await
    }
}

impl Drop for SessionRefreshLease {
    fn drop(&mut self) {
        self.admission.finish_refresh(&self.account_id);
        let mut locks = self
            .admission
            .session_refresh_locks
            .lock()
            .expect("session refresh mutex poisoned");
        let remove = locks.get(&self.account_id).is_some_and(|current| {
            Arc::ptr_eq(current, &self.lock) && Arc::strong_count(current) == 2
        });
        if remove {
            locks.remove(&self.account_id);
        }
    }
}

impl Admission {
    pub fn new(config: AdmissionConfig) -> anyhow::Result<Self> {
        Ok(Self {
            config: config.validate()?,
            accounts: Arc::new(Mutex::new(AccountCache::default())),
            session_refresh_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn config(&self) -> AdmissionConfig {
        self.config
    }

    /// Try the latency-free path. A reconciliation result never reserves by itself.
    pub fn reserve_cached(&self, account_id: &str, now_ms: i64) -> AdmissionDecision {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) =
            accounts.get_or_insert(account_id, self.config.max_cached_accounts, now_ms)
        else {
            return AdmissionDecision::CacheFull;
        };
        prune(state, now_ms);
        if state.balance_microusd.is_none()
            || state
                .reconciled_ms
                .is_none_or(|at| now_ms.saturating_sub(at) >= self.config.stale_after_ms)
        {
            return AdmissionDecision::Reconcile;
        }
        let balance = state.balance_microusd.expect("checked above");
        let available = balance.saturating_sub(state.reserved_microusd);
        if available
            < self
                .config
                .low_balance_microusd
                .saturating_add(self.config.action_exposure_microusd)
        {
            return AdmissionDecision::Reconcile;
        }
        reserve(state, self.config, available, now_ms)
    }

    /// Fresh settled balance minus still-conservative action reservations. Storage writes use
    /// this read-only fast path: they do not reserve a compute-sized action, but they also cannot
    /// bypass exposure already admitted for work. `None` requires an authoritative reconciliation.
    pub fn cached_available_balance(&self, account_id: &str, now_ms: i64) -> Option<i64> {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let state = accounts.get_or_insert(account_id, self.config.max_cached_accounts, now_ms)?;
        prune(state, now_ms);
        let reconciled_ms = state.reconciled_ms?;
        if now_ms.saturating_sub(reconciled_ms) >= self.config.stale_after_ms {
            return None;
        }
        Some(
            state
                .balance_microusd?
                .saturating_sub(state.reserved_microusd),
        )
    }

    /// Record a complete account reconciliation, then reserve against its fresh durable balance.
    pub fn reserve_reconciled(
        &self,
        account_id: &str,
        balance: i64,
        now_ms: i64,
        observed_generation: u64,
    ) -> AdmissionDecision {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) =
            accounts.get_or_insert(account_id, self.config.max_cached_accounts, now_ms)
        else {
            return AdmissionDecision::CacheFull;
        };
        prune(state, now_ms);
        if state.ledger_generation != observed_generation {
            return AdmissionDecision::Reconcile;
        }
        state.balance_microusd = Some(balance);
        state.reconciled_ms = Some(now_ms);
        let available = balance.saturating_sub(state.reserved_microusd);
        reserve(state, self.config, available, now_ms)
    }

    /// Called only after every non-final session for the account swept successfully.
    pub fn mark_reconciled(
        &self,
        account_id: &str,
        balance: i64,
        now_ms: i64,
        observed_generation: u64,
    ) -> bool {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) = accounts.get_mut(account_id) else {
            return false;
        };
        prune(state, now_ms);
        if state.ledger_generation != observed_generation {
            return false;
        }
        state.balance_microusd = Some(balance);
        state.reconciled_ms = Some(now_ms);
        true
    }

    /// Fence a durable reconciliation. A concurrent sweep or ledger mutation advances the same
    /// generation, so an older balance read cannot become authoritative later. A healthy cached
    /// balance remains usable while a background sweep runs; reservations are shared under the
    /// same lock and are subtracted when the fresh balance is installed.
    pub fn begin_reconciliation(&self, account_id: &str) -> Option<ReconciliationFence> {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let state =
            accounts.get_or_insert(account_id, self.config.max_cached_accounts, crate::now_ms())?;
        Some(self.start_reconciliation(state, account_id))
    }

    /// A background sweep must not fill the bounded hot cache with every dormant account. It may
    /// refresh an existing entry, while an absent tenant remains cold and reconciles on demand.
    pub fn begin_cached_reconciliation(&self, account_id: &str) -> Option<ReconciliationFence> {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let state = accounts.get_mut(account_id)?;
        Some(self.start_reconciliation(state, account_id))
    }

    fn start_reconciliation(
        &self,
        state: &mut AccountState,
        account_id: &str,
    ) -> ReconciliationFence {
        state.ledger_generation = state.ledger_generation.wrapping_add(1);
        state.active_refreshes = state.active_refreshes.saturating_add(1);
        ReconciliationFence {
            admission: self.clone(),
            account_id: account_id.to_owned(),
            generation: state.ledger_generation,
        }
    }

    /// A top-up, grant, refund reservation/rollback, or another out-of-band ledger mutation makes
    /// the cached settled balance ambiguous. Reservations remain conservative; the next work
    /// request performs one authoritative reconciliation.
    pub fn invalidate_balance(&self, account_id: &str) {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) = accounts.get_mut(account_id) else {
            return;
        };
        state.ledger_generation = state.ledger_generation.wrapping_add(1);
        state.balance_microusd = None;
        state.reconciled_ms = None;
    }

    /// Return the authoritative tenant-index root count while its short cache window is fresh.
    pub fn cached_live_root_sessions(&self, account_id: &str, now_ms: i64) -> Option<i64> {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let (count, reconciled_ms) = accounts.get_mut(account_id)?.live_root_sessions?;
        (now_ms.saturating_sub(reconciled_ms) < self.config.stale_after_ms).then_some(count)
    }

    pub fn session_generation(&self, account_id: &str) -> u64 {
        self.accounts
            .lock()
            .expect("admission mutex poisoned")
            .get_mut(account_id)
            .map_or(0, |state| state.live_generation)
    }

    /// Serialize only the rare stale/missing tenant-GSI refresh. Normal create admission stays a
    /// single in-memory critical section; waiters recheck it after the first refresh completes.
    pub fn session_refresh_lock(&self, account_id: &str) -> Option<SessionRefreshLease> {
        {
            let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
            let state = accounts.get_or_insert(
                account_id,
                self.config.max_cached_accounts,
                crate::now_ms(),
            )?;
            state.active_refreshes = state.active_refreshes.saturating_add(1);
        }
        let lock = self
            .session_refresh_locks
            .lock()
            .expect("session refresh mutex poisoned")
            .entry(account_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        Some(SessionRefreshLease {
            admission: self.clone(),
            account_id: account_id.to_owned(),
            lock,
        })
    }

    /// Re-enter a create identity whose durable SQLite intent proves it already crossed account
    /// admission. This is recovery, not a new capacity decision: after restart it must remain
    /// callable even when the possibly committed root itself already fills the live limit.
    pub fn resume_session_create(
        &self,
        account_id: &str,
        request_id: &str,
        now_ms: i64,
    ) -> SessionCreateDecision {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) =
            accounts.get_or_insert(account_id, self.config.max_cached_accounts, now_ms)
        else {
            return SessionCreateDecision::CacheFull;
        };
        if let Some(pending) = state.pending_session_creates.get_mut(request_id) {
            pending.holders = pending.holders.saturating_add(1);
        } else {
            state.pending_session_creates.insert(
                request_id.to_owned(),
                PendingSessionCreate {
                    holders: 1,
                    uncertain: true,
                },
            );
        }
        SessionCreateDecision::Reserved(SessionCreateReservation {
            admission: self.clone(),
            account_id: account_id.to_owned(),
            request_id: request_id.to_owned(),
            active: true,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reserve_session_create_cached(
        &self,
        account_id: &str,
        request_id: &str,
        observed_generation: u64,
        durable_pending_creates: i64,
        live_root_limit: i64,
        creates_last_hour: i64,
        create_limit: i64,
        now_ms: i64,
    ) -> SessionCreateDecision {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) =
            accounts.get_or_insert(account_id, self.config.max_cached_accounts, now_ms)
        else {
            return SessionCreateDecision::CacheFull;
        };
        reserve_session_create(
            self,
            state,
            account_id,
            request_id,
            observed_generation,
            durable_pending_creates,
            live_root_limit,
            creates_last_hour,
            create_limit,
            now_ms,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn reserve_session_create_reconciled(
        &self,
        account_id: &str,
        request_id: &str,
        observed_generation: u64,
        authoritative_live_roots: i64,
        durable_pending_creates: i64,
        live_root_limit: i64,
        creates_last_hour: i64,
        create_limit: i64,
        now_ms: i64,
    ) -> SessionCreateDecision {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) = accounts.get_mut(account_id) else {
            return SessionCreateDecision::CacheFull;
        };
        reserve_session_create(
            self,
            state,
            account_id,
            request_id,
            observed_generation,
            durable_pending_creates,
            live_root_limit,
            creates_last_hour,
            create_limit,
            now_ms,
            Some(authoritative_live_roots.max(0)),
        )
    }

    pub fn set_live_root_sessions(&self, account_id: &str, count: i64, now_ms: i64) -> bool {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) =
            accounts.get_or_insert(account_id, self.config.max_cached_accounts, now_ms)
        else {
            return false;
        };
        state.live_root_sessions = Some((count.max(0), now_ms));
        true
    }

    /// A root deletion releases one account root slot. Force the next create admission to recount
    /// the authoritative tenant index instead of guessing across an asynchronous cascade.
    pub fn invalidate_live_root_sessions(&self, account_id: &str) {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) = accounts.get_mut(account_id) else {
            return;
        };
        state.live_root_sessions = None;
        state.live_generation = state.live_generation.wrapping_add(1);
    }

    fn finish_session_create(&self, account_id: &str, request_id: &str, outcome: CreateOutcome) {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) = accounts.get_mut(account_id) else {
            return;
        };
        let Some(pending) = state.pending_session_creates.get_mut(request_id) else {
            return;
        };
        match outcome {
            CreateOutcome::Committed => {
                let was_uncertain = pending.uncertain;
                state.pending_session_creates.remove(request_id);
                if was_uncertain {
                    // A tenant-index refresh may already include the commit whose first response
                    // was lost. We cannot distinguish that from a later same-key commit locally,
                    // so invalidate instead of risking a double increment. The rare recovery path
                    // performs one authoritative recount before another root is admitted.
                    state.live_root_sessions = None;
                } else if let Some((count, _)) = state.live_root_sessions.as_mut() {
                    *count = count.saturating_add(1);
                }
                state.live_generation = state.live_generation.wrapping_add(1);
            }
            CreateOutcome::Uncertain | CreateOutcome::Rejected => {
                if matches!(outcome, CreateOutcome::Uncertain) {
                    pending.uncertain = true;
                }
                pending.holders = pending.holders.saturating_sub(1);
                if pending.holders == 0 && !pending.uncertain {
                    state.pending_session_creates.remove(request_id);
                }
            }
        }
    }

    fn finish_refresh(&self, account_id: &str) {
        let mut accounts = self.accounts.lock().expect("admission mutex poisoned");
        let Some(state) = accounts.get_mut(account_id) else {
            return;
        };
        state.active_refreshes = state.active_refreshes.saturating_sub(1);
    }

    #[cfg(test)]
    fn cached_account_count(&self) -> usize {
        self.accounts
            .lock()
            .expect("admission mutex poisoned")
            .entries
            .len()
    }

    #[cfg(test)]
    fn session_refresh_lock_count(&self) -> usize {
        self.session_refresh_locks
            .lock()
            .expect("session refresh mutex poisoned")
            .len()
    }
}

#[allow(clippy::too_many_arguments)]
fn reserve_session_create(
    admission: &Admission,
    state: &mut AccountState,
    account_id: &str,
    request_id: &str,
    observed_generation: u64,
    durable_pending_creates: i64,
    live_root_limit: i64,
    creates_last_hour: i64,
    create_limit: i64,
    now_ms: i64,
    refreshed_live_roots: Option<i64>,
) -> SessionCreateDecision {
    if let Some(pending) = state.pending_session_creates.get_mut(request_id) {
        pending.holders = pending.holders.saturating_add(1);
        return SessionCreateDecision::Reserved(SessionCreateReservation {
            admission: admission.clone(),
            account_id: account_id.to_owned(),
            request_id: request_id.to_owned(),
            active: true,
        });
    }
    if state.live_generation != observed_generation {
        return SessionCreateDecision::Reconcile;
    }
    if let Some(live_roots) = refreshed_live_roots {
        state.live_root_sessions = Some((live_roots, now_ms));
    }
    let Some((live_roots, reconciled_ms)) = state.live_root_sessions else {
        return SessionCreateDecision::Reconcile;
    };
    if now_ms.saturating_sub(reconciled_ms) >= admission.config.stale_after_ms {
        return SessionCreateDecision::Reconcile;
    }
    // The durable count includes this request's intent, while this branch proves it is not yet in
    // the process-local map. Discount exactly that identity, then take the maximum with local
    // reservations. After restart the durable side wins; during a new in-process race the local
    // side wins until SQLite insertion is visible.
    let durable_other = durable_pending_creates.saturating_sub(1).max(0);
    let local_pending = i64::try_from(state.pending_session_creates.len()).unwrap_or(i64::MAX);
    let pending = local_pending.max(durable_other);
    if live_roots.saturating_add(pending) >= live_root_limit {
        return SessionCreateDecision::ConcurrentLimit;
    }
    if creates_last_hour.saturating_add(pending) >= create_limit {
        return SessionCreateDecision::CreateRateLimit;
    }
    state.pending_session_creates.insert(
        request_id.to_owned(),
        PendingSessionCreate {
            holders: 1,
            uncertain: false,
        },
    );
    SessionCreateDecision::Reserved(SessionCreateReservation {
        admission: admission.clone(),
        account_id: account_id.to_owned(),
        request_id: request_id.to_owned(),
        active: true,
    })
}

fn reserve(
    state: &mut AccountState,
    config: AdmissionConfig,
    available: i64,
    now_ms: i64,
) -> AdmissionDecision {
    if available < config.action_exposure_microusd {
        return AdmissionDecision::Insufficient;
    }
    if state
        .reserved_microusd
        .saturating_add(config.action_exposure_microusd)
        > config.account_exposure_microusd
    {
        return AdmissionDecision::ExposureExhausted;
    }
    state.reserved_microusd = state
        .reserved_microusd
        .saturating_add(config.action_exposure_microusd);
    state.reservations.push_back((
        now_ms.saturating_add(config.reservation_ttl_ms),
        config.action_exposure_microusd,
    ));
    AdmissionDecision::Reserved
}

fn prune(state: &mut AccountState, now_ms: i64) {
    while state
        .reservations
        .front()
        .is_some_and(|(expires_ms, _)| *expires_ms <= now_ms)
    {
        if let Some((_, amount)) = state.reservations.pop_front() {
            state.reserved_microusd = state.reserved_microusd.saturating_sub(amount);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission() -> Admission {
        Admission::new(AdmissionConfig {
            action_exposure_microusd: 10,
            account_exposure_microusd: 20,
            low_balance_microusd: 25,
            stale_after_ms: 100,
            reservation_ttl_ms: 100,
            max_cached_accounts: 10_000,
        })
        .unwrap()
    }

    #[test]
    fn cache_miss_and_low_balance_reconcile_before_reserving() {
        let admission = admission();
        assert_eq!(
            admission.reserve_cached("acc", 1),
            AdmissionDecision::Reconcile
        );
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 1, 0),
            AdmissionDecision::Reserved
        );
        assert!(admission.mark_reconciled("acc", 40, 2, 0));
        assert_eq!(
            admission.reserve_cached("acc", 2),
            AdmissionDecision::Reconcile
        );
        assert_eq!(
            admission.reserve_reconciled("acc", 15, 2, 0),
            AdmissionDecision::Insufficient
        );
    }

    #[test]
    fn storage_balance_check_is_cached_but_respects_reservations_and_freshness() {
        let admission = admission();
        assert_eq!(admission.cached_available_balance("acc", 1), None);
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 1, 0),
            AdmissionDecision::Reserved
        );
        assert_eq!(admission.cached_available_balance("acc", 2), Some(90));
        assert_eq!(admission.cached_available_balance("acc", 101), None);
    }

    #[test]
    fn account_exposure_is_bounded_and_expiring() {
        let admission = admission();
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 1, 0),
            AdmissionDecision::Reserved
        );
        assert_eq!(
            admission.reserve_cached("acc", 2),
            AdmissionDecision::Reserved
        );
        assert_eq!(
            admission.reserve_cached("acc", 3),
            AdmissionDecision::ExposureExhausted
        );
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 101, 0),
            AdmissionDecision::Reserved
        );
    }

    #[test]
    fn stale_cache_requires_reconciliation() {
        let admission = admission();
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 1, 0),
            AdmissionDecision::Reserved
        );
        assert_eq!(
            admission.reserve_cached("acc", 101),
            AdmissionDecision::Reconcile
        );
    }

    #[test]
    fn reservation_cannot_expire_before_cached_balance_becomes_stale() {
        let error = Admission::new(AdmissionConfig {
            stale_after_ms: 101,
            reservation_ttl_ms: 100,
            ..admission().config()
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("must be at least AEX_ADMISSION_STALE_SECONDS")
        );
        let error = Admission::new(AdmissionConfig {
            max_cached_accounts: 0,
            ..admission().config()
        })
        .unwrap_err();
        assert!(error.to_string().contains("MAX_CACHED_ACCOUNTS"));
    }

    #[test]
    fn ledger_mutation_invalidates_only_the_cached_balance() {
        let admission = admission();
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 1, 0),
            AdmissionDecision::Reserved
        );
        admission.invalidate_balance("acc");
        assert_eq!(
            admission.reserve_cached("acc", 2),
            AdmissionDecision::Reconcile
        );
        // The first reservation was retained, so only one further action fits the account bound.
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 2, 1),
            AdmissionDecision::Reserved
        );
        assert_eq!(
            admission.reserve_cached("acc", 3),
            AdmissionDecision::ExposureExhausted
        );
    }

    #[test]
    fn invalidation_fences_a_stale_reconciliation_result() {
        let admission = admission();
        let reconciliation = admission.begin_reconciliation("acc").unwrap();
        let generation = reconciliation.generation();
        admission.invalidate_balance("acc");
        assert!(!admission.mark_reconciled("acc", 100, 2, generation));
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 2, generation),
            AdmissionDecision::Reconcile
        );
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 2, generation + 1),
            AdmissionDecision::Reserved
        );
    }

    #[test]
    fn a_background_reconciliation_does_not_flush_a_healthy_fast_path() {
        let admission = admission();
        assert_eq!(
            admission.reserve_reconciled("acc", 100, 1, 0),
            AdmissionDecision::Reserved
        );
        let reconciliation = admission.begin_reconciliation("acc").unwrap();
        let generation = reconciliation.generation();
        assert_eq!(
            admission.reserve_cached("acc", 2),
            AdmissionDecision::Reserved
        );
        assert!(admission.mark_reconciled("acc", 100, 3, generation));
        assert_eq!(
            admission.reserve_cached("acc", 4),
            AdmissionDecision::ExposureExhausted
        );
    }

    #[test]
    fn tenant_live_root_count_is_cached_and_invalidated_without_guessing() {
        let admission = admission();
        assert_eq!(admission.cached_live_root_sessions("acc", 1), None);
        admission.set_live_root_sessions("acc", 2, 1);
        assert_eq!(admission.cached_live_root_sessions("acc", 2), Some(2));
        admission.invalidate_live_root_sessions("acc");
        assert_eq!(admission.cached_live_root_sessions("acc", 4), None);
        admission.set_live_root_sessions("acc", 1, 4);
        assert_eq!(admission.cached_live_root_sessions("acc", 103), Some(1));
        assert_eq!(admission.cached_live_root_sessions("acc", 104), None);
    }

    #[test]
    fn one_hundred_racing_creates_cannot_cross_the_atomic_account_cap() {
        let admission = admission();
        admission.set_live_root_sessions("acc", 0, 1);
        let barrier = Arc::new(std::sync::Barrier::new(100));
        let mut threads = Vec::new();
        for index in 0..100 {
            let admission = admission.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                admission.reserve_session_create_cached(
                    "acc",
                    &format!("request-{index}"),
                    admission.session_generation("acc"),
                    1,
                    10,
                    0,
                    100,
                    2,
                )
            }));
        }
        let mut reservations = Vec::new();
        let mut limited = 0;
        for thread in threads {
            match thread.join().unwrap() {
                SessionCreateDecision::Reserved(reservation) => reservations.push(reservation),
                SessionCreateDecision::ConcurrentLimit => limited += 1,
                decision => panic!("unexpected create decision: {decision:?}"),
            }
        }
        assert_eq!(reservations.len(), 10);
        assert_eq!(limited, 90);

        // A definitive local or upstream pre-commit rejection drops every request guard. All
        // pending exposure is released, so a later full batch can reserve the same ten slots.
        drop(reservations);
        let mut retried = Vec::new();
        for index in 0..10 {
            match admission.reserve_session_create_cached(
                "acc",
                &format!("retry-{index}"),
                admission.session_generation("acc"),
                1,
                10,
                0,
                100,
                3,
            ) {
                SessionCreateDecision::Reserved(reservation) => retried.push(reservation),
                decision => panic!("released slot was not reusable: {decision:?}"),
            }
        }
        assert_eq!(retried.len(), 10);
    }

    #[test]
    fn same_idempotency_identity_shares_one_pending_and_one_live_slot() {
        let admission = admission();
        admission.set_live_root_sessions("acc", 0, 1);
        let mut retries = Vec::new();
        for _ in 0..100 {
            match admission.reserve_session_create_cached(
                "acc",
                "same-request",
                admission.session_generation("acc"),
                1,
                10,
                0,
                1,
                2,
            ) {
                SessionCreateDecision::Reserved(reservation) => retries.push(reservation),
                decision => panic!("same-key retry consumed another limit: {decision:?}"),
            }
        }
        retries.pop().unwrap().commit();
        drop(retries);
        assert_eq!(admission.cached_live_root_sessions("acc", 2), Some(1));

        // The create-rate ceiling also counts unique pending identities, not retry holders.
        assert!(matches!(
            admission.reserve_session_create_cached(
                "acc",
                "new-request",
                admission.session_generation("acc"),
                1,
                10,
                1,
                1,
                2,
            ),
            SessionCreateDecision::CreateRateLimit
        ));
    }

    #[test]
    fn ambiguous_create_retains_one_slot_until_the_same_identity_proves_success() {
        let config = admission().config();
        let admission = Admission::new(config).unwrap();
        assert!(admission.set_live_root_sessions("acc", 0, 1));

        let first = match admission.reserve_session_create_cached(
            "acc",
            "uncertain-request",
            admission.session_generation("acc"),
            1,
            1,
            0,
            10,
            2,
        ) {
            SessionCreateDecision::Reserved(reservation) => reservation,
            decision => panic!("initial create was not reserved: {decision:?}"),
        };
        first.uncertain();

        assert!(matches!(
            admission.reserve_session_create_cached(
                "acc",
                "different-request",
                admission.session_generation("acc"),
                2,
                1,
                0,
                10,
                2,
            ),
            SessionCreateDecision::ConcurrentLimit
        ));
        let retry = match admission.reserve_session_create_cached(
            "acc",
            "uncertain-request",
            admission.session_generation("acc"),
            1,
            1,
            0,
            10,
            2,
        ) {
            SessionCreateDecision::Reserved(reservation) => reservation,
            decision => panic!("same-key recovery was not reserved: {decision:?}"),
        };
        retry.commit();
        assert_eq!(
            admission.cached_live_root_sessions("acc", 2),
            None,
            "an ambiguous success forces one authoritative recount instead of double-counting a GSI refresh"
        );
        assert!(matches!(
            admission.reserve_session_create_reconciled(
                "acc",
                "different-request",
                admission.session_generation("acc"),
                1,
                1,
                1,
                0,
                10,
                2,
            ),
            SessionCreateDecision::ConcurrentLimit
        ));

        // The process cache starts cold after restart, while the caller supplies the durable
        // SQLite uncertain-intent count. Neither an eventual GSI miss nor cache loss frees it.
        let restarted = Admission::new(config).unwrap();
        assert_eq!(restarted.cached_live_root_sessions("acc", 3), None);
        assert!(matches!(
            restarted.reserve_session_create_cached(
                "acc",
                "after-restart",
                restarted.session_generation("acc"),
                1,
                1,
                0,
                10,
                3,
            ),
            SessionCreateDecision::Reconcile
        ));
        assert!(matches!(
            restarted.reserve_session_create_reconciled(
                "acc",
                "after-restart",
                restarted.session_generation("acc"),
                1,
                1,
                1,
                0,
                10,
                3,
            ),
            SessionCreateDecision::ConcurrentLimit
        ));
        let resumed = match restarted.resume_session_create("acc", "uncertain-request", 4) {
            SessionCreateDecision::Reserved(reservation) => reservation,
            decision => {
                panic!("durable same-key recovery was blocked by its own slot: {decision:?}")
            }
        };
        resumed.uncertain();
    }

    #[test]
    fn cache_churn_is_bounded_and_never_evicts_live_exposure_or_refreshes() {
        let config = AdmissionConfig {
            max_cached_accounts: 4,
            ..admission().config()
        };
        let admission = Admission::new(config).unwrap();
        let now = 1_000;

        assert_eq!(
            admission.reserve_reconciled("exposure", 100, now, 0),
            AdmissionDecision::Reserved
        );
        assert!(admission.set_live_root_sessions("pending", 0, now));
        let pending = match admission.reserve_session_create_cached(
            "pending",
            "request",
            admission.session_generation("pending"),
            1,
            10,
            0,
            10,
            now,
        ) {
            SessionCreateDecision::Reserved(reservation) => reservation,
            decision => panic!("pending create was not reserved: {decision:?}"),
        };
        let reconciliation = admission.begin_reconciliation("reconciliation").unwrap();
        let first_refresh = admission.session_refresh_lock("session-refresh").unwrap();
        let second_refresh = admission.session_refresh_lock("session-refresh").unwrap();
        assert_eq!(admission.cached_account_count(), 4);
        assert_eq!(admission.session_refresh_lock_count(), 1);

        assert_eq!(
            admission.reserve_cached("overflow", now),
            AdmissionDecision::CacheFull,
            "the hard bound must fail closed while every candidate is protected"
        );
        assert_eq!(admission.cached_account_count(), 4);
        drop(first_refresh);
        assert_eq!(admission.session_refresh_lock_count(), 1);
        drop(second_refresh);
        assert_eq!(
            admission.session_refresh_lock_count(),
            0,
            "the last Arc-fenced caller removes the per-account mutex"
        );
        drop(reconciliation);
        drop(pending);

        // Once the conservative action reservation expires, every subsequent cold tenant can
        // replace a safe cache entry without growing either process map.
        for index in 0..100 {
            assert_eq!(
                admission.reserve_cached(
                    &format!("churn-{index}"),
                    now + config.reservation_ttl_ms + 1,
                ),
                AdmissionDecision::Reconcile
            );
            assert!(admission.cached_account_count() <= config.max_cached_accounts);
            let refresh = admission
                .session_refresh_lock(&format!("churn-{index}"))
                .unwrap();
            drop(refresh);
            assert_eq!(admission.session_refresh_lock_count(), 0);
        }

        // Restart has no unsafe warm assumption: the cache is empty and an account must
        // reconcile before it can reserve work.
        let restarted = Admission::new(config).unwrap();
        assert_eq!(restarted.cached_account_count(), 0);
        assert!(
            restarted
                .begin_cached_reconciliation("background-only")
                .is_none()
        );
        assert_eq!(restarted.cached_account_count(), 0);
        assert_eq!(
            restarted.reserve_cached("exposure", now),
            AdmissionDecision::Reconcile
        );
    }
}
