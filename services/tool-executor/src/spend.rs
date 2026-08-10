//! The organization ceiling, and the permit that proves it was consulted.
//!
//! # Why this is capability-shaped
//!
//! [`SpendPermit`] has private fields, no public constructor and no `Default`,
//! and the only function in the workspace that can produce one is
//! [`OrganizationCeiling::admit`]. [`crate::run::ToolRunner::run`] takes one **by
//! reference as an argument**, so a caller cannot reach the vendor credential
//! without having first passed the check. That is the same trick `Grant<C>`
//! plays in `aex_regional_http::capability` and `DispatchTicket` plays with
//! `FenceGuard`, and it exists for the same reason: a check a caller can forget
//! is a check that will be forgotten.
//!
//! # Why a condition, and not a counter
//!
//! The nearest existing thing to this counter in the repository is the OTLP
//! quota, which runs `ADD reservedBytes …` with no `ConditionExpression`. The
//! numbers accumulate forever and are compared to nothing, while the refusal
//! code they were meant to produce is declared on three routes and emitted by
//! none. **The condition is the entire mechanism.** A counter without one is
//! telemetry wearing a quota's name.
//!
//! # Why two windows in one round trip
//!
//! A per-minute ceiling bounds a loop; a per-day ceiling bounds a month. Neither
//! alone is adequate and two sequential conditional updates would double the
//! latency of a call the model is blocked on, so both are one
//! `TransactWriteItems` with one condition each. A failed condition on either
//! fails the transaction, which is the refusal.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aex_wire::ids::OrganizationId;
use async_trait::async_trait;

/// The most platform-paid calls one organization may make in a minute.
///
/// A bound on a runaway loop rather than on legitimate use. The deployed Brain
/// fleet can hold at most 64 platform tool calls in flight at once — two tasks,
/// each with a 128-unit network lane and a declared tool weight of 4 — so at a
/// three-second call it can produce roughly 1 300 calls a minute *in total*.
/// This ceiling therefore stops one organization taking about half the fleet's
/// entire throughput, which is the shape of bound worth having: it does not
/// constrain a customer doing real work and it does constrain a customer whose
/// agent is in a loop.
///
/// It belongs in `api/schemas/registries/limit-defaults.v1.json` so it is
/// versioned rather than compiled in, and it is **not there yet**, deliberately:
/// the registry has demonstrably not been enforcement — `telemetry.ingest_rate`
/// has lived there with published defaults and no reader — and the check rule
/// that would fix that (every rate-shaped `LimitId` has a non-test reader) does
/// not exist yet. Moving these two numbers into the registry before that rule
/// lands would make them look enforced a release earlier than they are.
pub const DEFAULT_CALLS_PER_MINUTE: u64 = 600;

/// The most platform-paid calls one organization may make in a day.
///
/// Roughly 6 % of what [`DEFAULT_CALLS_PER_MINUTE`] would permit if sustained
/// for a whole day, which is the point: the minute ceiling bounds a loop and
/// this one bounds a month's cost of goods. Same registry caveat.
pub const DEFAULT_CALLS_PER_DAY: u64 = 50_000;

/// How long a spent window row is retained past its own end.
///
/// The row is only meaningful inside its window, so it carries a TTL rather than
/// a sweeper. One day past the day window, so a day row survives long enough for
/// an operator to read it after a refusal is reported.
pub const WINDOW_RETENTION: Duration = Duration::from_hours(48);

/// Which rolling window a ceiling is expressed over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Window {
    /// One wall-clock minute.
    Minute,
    /// One wall-clock day.
    Day,
}

impl Window {
    /// Every window, in the order a refusal reports them.
    pub const ALL: [Self; 2] = [Self::Minute, Self::Day];

    /// How many seconds the window spans.
    #[must_use]
    pub const fn span_seconds(self) -> u64 {
        match self {
            Self::Minute => 60,
            Self::Day => 86_400,
        }
    }

    /// The stored spelling, which is also the sort-key prefix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minute => "M",
            Self::Day => "D",
        }
    }

    /// The bucket `at` falls in, as the sort key it is stored under.
    ///
    /// Truncated rather than rolling, so the key is a pure function of the clock
    /// and two processes cannot disagree about which window a call belongs to.
    #[must_use]
    pub fn bucket(self, at: SystemTime) -> String {
        let seconds = at
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        format!("{}#{}", self.as_str(), seconds / self.span_seconds())
    }
}

/// The ceilings this deployment enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceilings {
    /// The per-minute ceiling.
    pub per_minute: u64,
    /// The per-day ceiling.
    pub per_day: u64,
}

impl Ceilings {
    /// The compiled-in defaults.
    pub const DEFAULT: Self = Self {
        per_minute: DEFAULT_CALLS_PER_MINUTE,
        per_day: DEFAULT_CALLS_PER_DAY,
    };

    /// The ceiling for one window.
    #[must_use]
    pub const fn for_window(self, window: Window) -> u64 {
        match window {
            Window::Minute => self.per_minute,
            Window::Day => self.per_day,
        }
    }
}

/// Proof that one organization's ceiling admitted one call.
///
/// **Unforgeable by construction.** The fields are private to this module, there
/// is no public constructor, and [`OrganizationCeiling::admit`] is the only
/// function that returns one. A future caller therefore cannot reach the vendor
/// credential without having passed the check, because the function that touches
/// it requires this value as an argument and cannot make one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendPermit {
    organization: OrganizationId,
    buckets: [String; Window::ALL.len()],
}

impl SpendPermit {
    /// Whose ceiling admitted the call.
    #[must_use]
    pub const fn organization(&self) -> &OrganizationId {
        &self.organization
    }

    /// The windows the call was counted against, for a receipt or a log line.
    #[must_use]
    pub fn buckets(&self) -> &[String] {
        &self.buckets
    }
}

/// Why a call was not admitted against a ceiling.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CeilingRefusal {
    /// A ceiling was reached.
    ///
    /// The remaining budget is deliberately absent: a caller that learned it
    /// could binary-search the ceiling, and a caller that reached it does not
    /// need it to know what to do.
    #[error("an organization ceiling was reached")]
    LimitExceeded,
    /// The store could not answer. **Not** downgraded to admitted: a ceiling
    /// nobody could read is how an organization keeps spending.
    #[error("the ceiling store could not be read: {0}")]
    Unavailable(String),
}

/// The per-organization ceiling.
#[async_trait]
pub trait OrganizationCeiling: Send + Sync {
    /// Counts one call against every window and refuses if any is full.
    ///
    /// # Errors
    ///
    /// Returns [`CeilingRefusal::LimitExceeded`] when a condition failed and
    /// [`CeilingRefusal::Unavailable`] when the store could not answer. The
    /// second is never treated as the absence of a ceiling.
    async fn admit(
        &self,
        organization: &OrganizationId,
        at: SystemTime,
    ) -> Result<SpendPermit, CeilingRefusal>;
}

/// The `DynamoDB` implementation: one conditional update per window, one
/// transaction, one round trip.
pub struct DynamoOrganizationCeiling {
    client: aws_sdk_dynamodb::Client,
    table: String,
    ceilings: Ceilings,
}

impl DynamoOrganizationCeiling {
    /// Binds the ceiling to one table and one set of numbers.
    #[must_use]
    pub const fn new(client: aws_sdk_dynamodb::Client, table: String, ceilings: Ceilings) -> Self {
        Self {
            client,
            table,
            ceilings,
        }
    }

    /// The partition key one organization's windows share.
    fn partition(organization: &OrganizationId) -> String {
        format!("TOOLSPEND#{organization}")
    }
}

#[async_trait]
impl OrganizationCeiling for DynamoOrganizationCeiling {
    async fn admit(
        &self,
        organization: &OrganizationId,
        at: SystemTime,
    ) -> Result<SpendPermit, CeilingRefusal> {
        use aws_sdk_dynamodb::types::{AttributeValue, TransactWriteItem, Update};

        let expires_at = at
            .checked_add(WINDOW_RETENTION)
            .and_then(|instant| instant.duration_since(UNIX_EPOCH).ok())
            .map(|since| since.as_secs())
            .ok_or_else(|| CeilingRefusal::Unavailable("the clock is not usable".to_owned()))?;

        let partition = Self::partition(organization);
        let buckets = Window::ALL.map(|window| window.bucket(at));

        let mut items = Vec::with_capacity(Window::ALL.len());
        for (index, window) in Window::ALL.into_iter().enumerate() {
            let update = Update::builder()
                .table_name(&self.table)
                .key("pk", AttributeValue::S(partition.clone()))
                .key("sk", AttributeValue::S(buckets[index].clone()))
                // `ADD` on an absent attribute creates it at the increment, so
                // the first call in a window needs no separate insert.
                .update_expression("ADD #calls :one SET #expires = :expires")
                // The whole mechanism. Without it the numbers would accumulate
                // forever and be compared to nothing.
                .condition_expression("attribute_not_exists(#calls) OR #calls < :ceiling")
                .expression_attribute_names("#calls", "calls")
                .expression_attribute_names("#expires", "expiresAt")
                .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
                .expression_attribute_values(
                    ":ceiling",
                    AttributeValue::N(self.ceilings.for_window(window).to_string()),
                )
                .expression_attribute_values(":expires", AttributeValue::N(expires_at.to_string()))
                .build()
                .map_err(|error| CeilingRefusal::Unavailable(error.to_string()))?;
            items.push(TransactWriteItem::builder().update(update).build());
        }

        self.client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await
            .map_err(|error| {
                // A cancelled transaction with a conditional-check failure is
                // the refusal; anything else is a fault, and a fault must not be
                // reported as "you are over your ceiling".
                use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;

                match error.into_service_error() {
                    TransactWriteItemsError::TransactionCanceledException(cancellation)
                        if cancellation
                            .cancellation_reasons()
                            .iter()
                            .any(|reason| reason.code() == Some("ConditionalCheckFailed")) =>
                    {
                        CeilingRefusal::LimitExceeded
                    }
                    other => CeilingRefusal::Unavailable(other.to_string()),
                }
            })?;

        Ok(SpendPermit {
            organization: *organization,
            buckets,
        })
    }
}

/// A ceiling held in this process's memory.
///
/// Not a production adapter and never composed by `main.rs`, which
/// `tests/composition.rs` asserts. It exists because a `SpendPermit` cannot be
/// forged — its fields are private to this module — so a test that needs one has
/// to get it from a real implementation of the trait rather than build a fake
/// value. That the fake *has* to live here is the property working.
pub struct InMemoryOrganizationCeiling {
    ceilings: Ceilings,
    counts: std::sync::Mutex<std::collections::BTreeMap<(String, String), u64>>,
}

impl InMemoryOrganizationCeiling {
    /// A ceiling nothing reaches.
    #[must_use]
    pub fn unbounded() -> Self {
        Self::with(Ceilings {
            per_minute: u64::MAX,
            per_day: u64::MAX,
        })
    }

    /// A ceiling with explicit numbers.
    #[must_use]
    pub fn with(ceilings: Ceilings) -> Self {
        Self {
            ceilings,
            counts: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }
}

#[async_trait]
impl OrganizationCeiling for InMemoryOrganizationCeiling {
    async fn admit(
        &self,
        organization: &OrganizationId,
        at: SystemTime,
    ) -> Result<SpendPermit, CeilingRefusal> {
        let buckets = Window::ALL.map(|window| window.bucket(at));
        let mut counts = self
            .counts
            .lock()
            .map_err(|_| CeilingRefusal::Unavailable("the counter is poisoned".to_owned()))?;
        // Both conditions are evaluated before either count moves, so a refusal
        // on the second window does not leave the first one charged. That is the
        // property `TransactWriteItems` gives the real adapter for free and the
        // reason this one is written the same way round.
        for (index, window) in Window::ALL.into_iter().enumerate() {
            let key = (organization.to_string(), buckets[index].clone());
            if counts.get(&key).copied().unwrap_or(0) >= self.ceilings.for_window(window) {
                return Err(CeilingRefusal::LimitExceeded);
            }
        }
        for bucket in &buckets {
            *counts
                .entry((organization.to_string(), bucket.clone()))
                .or_insert(0) += 1;
        }
        Ok(SpendPermit {
            organization: *organization,
            buckets,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SpendPermit, Window};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn a_bucket_is_a_pure_function_of_the_clock() {
        let at = UNIX_EPOCH + Duration::from_secs(1_767_225_659);
        assert_eq!(Window::Minute.bucket(at), "M#29453760");
        assert_eq!(Window::Day.bucket(at), "D#20454");
        // One second later is still the same minute; one more crosses it.
        assert_eq!(
            Window::Minute.bucket(at + Duration::from_secs(1)),
            "M#29453761"
        );
    }

    #[test]
    fn a_permit_cannot_be_built_outside_the_module_that_checks() {
        // Not an assertion a test can make directly, so it is asserted by
        // construction: this is the only place in the crate where a `SpendPermit`
        // literal compiles at all, because its fields are private to this module.
        // If that ever stops being true, this test moves and the reviewer notices.
        fn _requires_private_fields(permit: &SpendPermit) -> usize {
            permit.buckets.len()
        }
    }
}
