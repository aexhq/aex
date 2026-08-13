//! `usage-transfer-worker` composition root (Rust Lambda ZIP).
//!
//! Exclusive responsibility: `data_transfer.egress_byte.v1` fact projection, the transfer
//! frontier and the central transfer-category outbox. One binary serves three
//! event modes under one role (`U-09`) — `stream` from this authority's own
//! `DynamoDB` stream, `sweep` from a one-minute schedule, and `receipt` from the
//! transfer settlement queue. All three need identical credentials and sit inside
//! the identical category boundary, so a fourth deployable would buy nothing and
//! would add a deployment boundary without a distinct role or credential set.
//!
//! The behaviour lives in `aex_usage_app::worker`, which is
//! category-generic over ports. This binary is the only place that names a
//! table, and it names exactly one: `aex-usage-transfer-dynamodb`. That is what makes
//! "this worker cannot write a sibling authority" a link-graph fact rather than
//! a review promise — see `src/main.rs` tests and the adapter's own
//! `tests/isolation.rs`.

use std::process::ExitCode;
use std::sync::Arc;

use aex_usage_app::worker::{BillingMode, UsageWorker, WorkerLimits};
use aex_usage_domain::wire_pending::RegionId;
use aex_usage_transfer_dynamodb::clock::SystemClock;
use aex_usage_transfer_dynamodb::projection::QueryProjection;
use aex_usage_transfer_dynamodb::queue::SettlementQueue;
use aex_usage_transfer_dynamodb::store::TransferStore;
use aex_usage_transfer_dynamodb::stream::{receipt_records, stream_records};
use lambda_runtime::{Error as LambdaError, LambdaEvent, service_fn};

const DEPLOYABLE: &str = "usage-transfer-worker";

/// Validated start-up configuration for `usage-transfer-worker`.
///
/// Nothing here has a default. A defaulted resource identifier silently binds
/// the process to the wrong plane, region or table, and a defaulted money-
/// relevant limit is a policy decision nobody made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Deployment plane this process belongs to (`dev` or `prd`).
    pub plane: String,
    /// `AWS` region this process is bound to.
    ///
    /// Parsed rather than carried as text: an absent frontier row is the empty
    /// frontier of *this* region, so a region that is not an identifier would
    /// mint a sequence nothing can read back.
    pub region: RegionId,
    /// The `usage-transfer-authority` `DynamoDB` table.
    pub authority_table: String,
    /// The shared `usage-query-projection` table.
    pub projection_table: String,
    /// The central settlement `FIFO` queue.
    pub rating_queue: String,
    /// This category's settlement receipt queue.
    pub receipt_queue: String,
    /// Whether this deployment may produce a chargeable rating request.
    pub billing_mode: BillingMode,
    /// The named limits the worker runs under.
    pub limits: WorkerLimits,
    /// Maximum stream records processed per invocation.
    pub budget: u32,
}

/// Why `usage-transfer-worker` refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UsageTransferWorkerConfigError {
    /// A required variable was absent or empty.
    #[error("required environment variable `{name}` is missing")]
    Missing {
        /// The variable that must be supplied.
        name: &'static str,
    },
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// The variable that was rejected.
        name: &'static str,
        /// Why the supplied value was rejected.
        reason: String,
    },
    /// The charging gate was configured in a state it must never occupy.
    ///
    /// `A11-METER` makes shadow versus active a gate a deployment cannot half
    /// satisfy. A shadow deployment pointed at the live rating queue would post
    /// real money while every dashboard said it was shadowing, so the two halves
    /// are checked against each other rather than trusted separately.
    #[error("billing mode `{mode}` disagrees with rating queue `{queue}`: {reason}")]
    ChargingGate {
        /// The declared mode.
        mode: &'static str,
        /// The queue it was pointed at.
        queue: String,
        /// What the disagreement is.
        reason: &'static str,
    },
}

/// Why `usage-transfer-worker` stopped.
#[derive(Debug, thiserror::Error)]
pub enum UsageTransferWorkerRunError {
    /// Start-up configuration was rejected.
    #[error(transparent)]
    Config(#[from] UsageTransferWorkerConfigError),
    /// An incoming event could not be classified.
    #[error(transparent)]
    Dispatch(#[from] DispatchError),
    /// The Lambda runtime stopped.
    #[error("the lambda runtime stopped: {0}")]
    Runtime(String),
}

/// Which trigger delivered an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventMode {
    /// The authority table's own change feed.
    Stream,
    /// The one-minute outbox sweep.
    Sweep,
    /// This category's settlement receipt queue.
    Receipt,
}

impl EventMode {
    /// The stable identifier a log line and a metric carry.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Stream => "stream",
            Self::Sweep => "sweep",
            Self::Receipt => "receipt",
        }
    }

    /// Classifies one incoming event envelope.
    ///
    /// Strict by design. An event this worker does not recognise is a wiring
    /// defect, and guessing a mode would run the wrong handler over money
    /// evidence.
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError`] for an envelope that names no known source or
    /// mixes two.
    pub fn classify(event: &serde_json::Value) -> Result<Self, DispatchError> {
        if let Some(records) = event.get("Records").and_then(serde_json::Value::as_array) {
            let mut sources: Vec<&str> = records
                .iter()
                .filter_map(|record| {
                    record
                        .get("eventSource")
                        .and_then(serde_json::Value::as_str)
                })
                .collect();
            sources.sort_unstable();
            sources.dedup();
            return match sources.as_slice() {
                ["aws:dynamodb"] => Ok(Self::Stream),
                ["aws:sqs"] => Ok(Self::Receipt),
                [] => Err(DispatchError::Unrecognised),
                _ => Err(DispatchError::MixedSources {
                    sources: sources.join(", "),
                }),
            };
        }
        if event.get("detail-type").and_then(serde_json::Value::as_str) == Some("Scheduled Event") {
            return Ok(Self::Sweep);
        }
        Err(DispatchError::Unrecognised)
    }
}

/// Why an event could not be classified.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DispatchError {
    /// The envelope named no source this worker serves.
    #[error("event envelope names no trigger this worker serves")]
    Unrecognised,
    /// One batch mixed two triggers.
    #[error("event batch mixes sources `{sources}`; one invocation serves one mode")]
    MixedSources {
        /// The sources that appeared together.
        sources: String,
    },
}

/// Environment variable naming the deployment plane.
pub const PLANE_VAR: &str = "AEX_PLANE";
/// Environment variable naming the bound `AWS` region.
pub const REGION_VAR: &str = "AEX_REGION";
/// Environment variable naming the authority table.
pub const AUTHORITY_TABLE_VAR: &str = aex_usage_transfer_dynamodb::TABLE_ENV;
/// Environment variable naming the shared query projection table.
pub const PROJECTION_TABLE_VAR: &str = "AEX_USAGE_QUERY_TABLE";
/// Environment variable naming the central settlement queue.
pub const RATING_QUEUE_VAR: &str = aex_usage_transfer_dynamodb::RATING_QUEUE_ENV;
/// Environment variable naming this category's receipt queue.
pub const RECEIPT_QUEUE_VAR: &str = aex_usage_transfer_dynamodb::RECEIPT_QUEUE_ENV;
/// Environment variable naming the charging gate.
pub const BILLING_MODE_VAR: &str = "AEX_USAGE_BILLING_MODE";
/// Environment variable naming the outbox republish threshold.
pub const REPUBLISH_AFTER_VAR: &str = "AEX_USAGE_OUTBOX_REPUBLISH_AFTER_MS";
/// Environment variable naming the sweep page budget.
pub const SWEEP_PAGE_VAR: &str = "AEX_USAGE_SWEEP_PAGE";
/// Environment variable naming the outbox attempt alarm threshold.
pub const ATTEMPT_ALARM_VAR: &str = "AEX_USAGE_OUTBOX_ATTEMPT_ALARM";
/// Environment variable naming the outbox age alarm threshold.
pub const AGE_ALARM_VAR: &str = "AEX_USAGE_OUTBOX_AGE_ALARM_MS";
/// Environment variable naming the outbox backlog ceiling.
pub const BACKLOG_CEILING_VAR: &str = "AEX_USAGE_OUTBOX_BACKLOG_CEILING";
/// Environment variable naming maximum stream records processed per invocation.
pub const BUDGET_VAR: &str = "AEX_MAX_BATCH_SIZE";

/// The suffix a shadow rating queue must carry.
pub const SHADOW_QUEUE_SUFFIX: &str = "-shadow.fifo";

/// Planes this deployable may be bound to.
const PLANES: [&str; 2] = ["dev", "prd"];

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// [`UsageTransferWorkerConfigError::Missing`] for an absent or blank variable,
    /// [`UsageTransferWorkerConfigError::Invalid`] for one that does not parse, and
    /// [`UsageTransferWorkerConfigError::ChargingGate`] when the billing mode and the rating queue
    /// disagree.
    pub fn from_env() -> Result<Self, UsageTransferWorkerConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// Tests use this directly: `std::env::set_var` is `unsafe` in edition 2024
    /// and this workspace forbids `unsafe` code.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    pub fn from_lookup<F>(lookup: F) -> Result<Self, UsageTransferWorkerConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let plane = required(&lookup, PLANE_VAR)?;
        if !PLANES.contains(&plane.as_str()) {
            return Err(UsageTransferWorkerConfigError::Invalid {
                name: PLANE_VAR,
                reason: format!("expected one of {PLANES:?}, got `{plane}`"),
            });
        }
        let region = RegionId::parse(&required(&lookup, REGION_VAR)?).map_err(|error| {
            UsageTransferWorkerConfigError::Invalid {
                name: REGION_VAR,
                reason: error.to_string(),
            }
        })?;
        let authority_table = required(&lookup, AUTHORITY_TABLE_VAR)?;
        let projection_table = required(&lookup, PROJECTION_TABLE_VAR)?;
        let rating_queue = required(&lookup, RATING_QUEUE_VAR)?;
        let receipt_queue = required(&lookup, RECEIPT_QUEUE_VAR)?;
        let billing_mode = billing_mode(&required(&lookup, BILLING_MODE_VAR)?)?;
        check_charging_gate(billing_mode, &rating_queue)?;

        let limits = WorkerLimits {
            outbox_republish_after_ms: positive(&lookup, REPUBLISH_AFTER_VAR)?,
            sweep_page: usize::try_from(positive(&lookup, SWEEP_PAGE_VAR)?).map_err(|error| {
                UsageTransferWorkerConfigError::Invalid {
                    name: SWEEP_PAGE_VAR,
                    reason: error.to_string(),
                }
            })?,
            outbox_attempt_alarm: u32::try_from(positive(&lookup, ATTEMPT_ALARM_VAR)?).map_err(
                |error| UsageTransferWorkerConfigError::Invalid {
                    name: ATTEMPT_ALARM_VAR,
                    reason: error.to_string(),
                },
            )?,
            outbox_age_alarm_ms: positive(&lookup, AGE_ALARM_VAR)?,
            outbox_backlog_ceiling: positive(&lookup, BACKLOG_CEILING_VAR)?,
        };
        let budget = u32::try_from(positive(&lookup, BUDGET_VAR)?).map_err(|error| {
            UsageTransferWorkerConfigError::Invalid {
                name: BUDGET_VAR,
                reason: error.to_string(),
            }
        })?;

        Ok(Self {
            plane,
            region,
            authority_table,
            projection_table,
            rating_queue,
            receipt_queue,
            billing_mode,
            limits,
            budget,
        })
    }
}

/// Parses the charging gate.
fn billing_mode(value: &str) -> Result<BillingMode, UsageTransferWorkerConfigError> {
    match value {
        "shadow" => Ok(BillingMode::Shadow),
        "active" => Ok(BillingMode::Active),
        other => Err(UsageTransferWorkerConfigError::Invalid {
            name: BILLING_MODE_VAR,
            reason: format!("expected `shadow` or `active`, got `{other}`"),
        }),
    }
}

/// Refuses a deployment whose declared mode and rating queue disagree.
fn check_charging_gate(
    mode: BillingMode,
    queue: &str,
) -> Result<(), UsageTransferWorkerConfigError> {
    let shadow_queue = queue.ends_with(SHADOW_QUEUE_SUFFIX);
    match (mode, shadow_queue) {
        (BillingMode::Shadow, true) | (BillingMode::Active, false) => Ok(()),
        (BillingMode::Shadow, false) => Err(UsageTransferWorkerConfigError::ChargingGate {
            mode: mode.id(),
            queue: queue.to_owned(),
            reason: "a shadow deployment must publish to the shadow rating queue",
        }),
        (BillingMode::Active, true) => Err(UsageTransferWorkerConfigError::ChargingGate {
            mode: mode.id(),
            queue: queue.to_owned(),
            reason: "an active deployment must not publish to the shadow rating queue",
        }),
    }
}

fn required<F>(lookup: &F, name: &'static str) -> Result<String, UsageTransferWorkerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(name) {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(UsageTransferWorkerConfigError::Missing { name }),
    }
}

fn positive<F>(lookup: &F, name: &'static str) -> Result<u64, UsageTransferWorkerConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    let raw = required(lookup, name)?;
    let value = raw
        .parse::<u64>()
        .map_err(|error| UsageTransferWorkerConfigError::Invalid {
            name,
            reason: format!("expected a positive integer, got `{raw}`: {error}"),
        })?;
    if value == 0 {
        return Err(UsageTransferWorkerConfigError::Invalid {
            name,
            reason: "expected a positive integer, got `0`".to_owned(),
        });
    }
    Ok(value)
}

/// Runs `usage-transfer-worker` until it stops.
///
/// The three event modes share one role because they need identical credentials
/// and sit inside the identical category boundary, so the adapters are built
/// once and every trigger is served from the same composition.
///
/// # Errors
///
/// Returns [`UsageTransferWorkerRunError::Runtime`] when the Lambda runtime stops.
pub async fn run(config: &Config) -> Result<(), UsageTransferWorkerRunError> {
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = DEPLOYABLE,
        plane = %config.plane,
        region = config.region.as_str(),
        "process started"
    );

    let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb = aws_sdk_dynamodb::Client::new(&aws);
    let clock = SystemClock::new();
    // Exactly one authority table is named here, and it is this binary's own.
    let authority = TransferStore::new(
        dynamodb.clone(),
        config.authority_table.clone(),
        config.region.clone(),
        Arc::new(clock),
    );
    let projection = QueryProjection::new(dynamodb, config.projection_table.clone());
    let queue = SettlementQueue::new(aws_sdk_sqs::Client::new(&aws), config.rating_queue.clone());
    let handler = Handler {
        worker: Arc::new(UsageWorker::new(
            authority,
            projection,
            queue,
            clock,
            config.limits,
            config.billing_mode,
        )),
        budget: config.budget,
    };

    lambda_runtime::run(service_fn(move |event: LambdaEvent<serde_json::Value>| {
        let handler = handler.clone();
        async move { handler.handle(event.payload).await }
    }))
    .await
    .map_err(|error: LambdaError| UsageTransferWorkerRunError::Runtime(error.to_string()))
}

/// The composition every trigger is served from.
#[derive(Clone)]
pub struct Handler {
    worker: Arc<UsageWorker<TransferStore, QueryProjection, SettlementQueue, SystemClock>>,
    budget: u32,
}

impl std::fmt::Debug for Handler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Handler")
            .field("mode", &self.worker.mode().id())
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl Handler {
    /// Serves one invocation.
    ///
    /// # Errors
    ///
    /// Returns a [`LambdaError`] when the event cannot be classified, when a
    /// batch cannot be read at all, or when a failure cannot be attributed to
    /// one record. Every one of those redelivers the whole batch, which is the
    /// only honest answer when the worker cannot say which record is at fault.
    pub async fn handle(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, LambdaError> {
        match EventMode::classify(&payload)? {
            EventMode::Stream => self.stream(&payload).await,
            EventMode::Sweep => self.sweep().await,
            EventMode::Receipt => self.receipts(&payload).await,
        }
    }

    /// Folds one stream batch.
    async fn stream(&self, payload: &serde_json::Value) -> Result<serde_json::Value, LambdaError> {
        let records = stream_records(payload)?;
        if records.len() > self.budget as usize {
            // The mapping's batch size and the configured budget are two
            // statements of one limit. A batch past the budget means they
            // disagree, and quietly folding it would hide the disagreement.
            return Err(LambdaError::from(format!(
                "stream batch of {} records exceeds the configured budget of {}; the \
                 event-source mapping and `{BUDGET_VAR}` disagree",
                records.len(),
                self.budget
            )));
        }
        let report = self.worker.handle_stream(&records).await?;
        Ok(serde_json::json!({
            "mode": EventMode::Stream.id(),
            "folded": report.folded,
            "reconciled": report.reconciled,
            "alreadyProjected": report.already_projected,
            "published": report.published,
            "deferred": report.deferred,
            "quarantined": report.quarantined,
            // The lowest failure only: everything at or after it is redelivered,
            // so a gap in a contiguous sequence is never checkpointed past.
            "batchItemFailures": failures(&report.failures),
        }))
    }

    /// Republishes overdue outbox rows.
    async fn sweep(&self) -> Result<serde_json::Value, LambdaError> {
        let report = self.worker.handle_sweep().await?;
        Ok(serde_json::json!({
            "mode": EventMode::Sweep.id(),
            "inspected": report.inspected,
            "republished": report.republished,
            "deferred": report.deferred,
            "alarming": report.alarming,
            "oldestAgeMs": report.oldest_age_ms,
            "backlogAlarms": self.worker.backlog_alarms(&report),
        }))
    }

    /// Applies one settlement receipt batch.
    async fn receipts(
        &self,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, LambdaError> {
        let records = receipt_records(payload)?;
        let report = self.worker.handle_receipts(&records).await?;
        Ok(serde_json::json!({
            "mode": EventMode::Receipt.id(),
            "advanced": report.advanced,
            "parked": report.parked,
            "alreadySettled": report.already_settled,
            "quarantined": report.quarantined,
            // Receipts have no ordering guarantee, so one failing message fails
            // only itself; central truth never waits on this acknowledgement.
            "batchItemFailures": failures(&report.failures),
        }))
    }
}

/// The partial-batch response Lambda reads.
fn failures(identifiers: &[String]) -> Vec<serde_json::Value> {
    identifiers
        .iter()
        .map(|identifier| serde_json::json!({ "itemIdentifier": identifier }))
        .collect()
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(error) = aex_platform_diagnostics::install_json() {
        eprintln!("{DEPLOYABLE}: diagnostics installation failed: {error}");
        return ExitCode::FAILURE;
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            // A deployable that cannot serve says why and stops. It never serves
            // a placeholder, because a placeholder over money evidence is a
            // silent under-bill nobody would notice.
            tracing::error!(
                target: "aex::diagnostics",
                event_name = "process.configuration_rejected",
                deployable = DEPLOYABLE,
                error = %error,
                "process configuration rejected"
            );
            eprintln!("{DEPLOYABLE}: refusing to start: {error}");
            return ExitCode::FAILURE;
        }
    };
    let outcome = run(&config).await;
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{DEPLOYABLE}: stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AGE_ALARM_VAR, ATTEMPT_ALARM_VAR, AUTHORITY_TABLE_VAR, BACKLOG_CEILING_VAR,
        BILLING_MODE_VAR, BUDGET_VAR, BillingMode, Config, DispatchError, EventMode, PLANE_VAR,
        PROJECTION_TABLE_VAR, RATING_QUEUE_VAR, RECEIPT_QUEUE_VAR, REGION_VAR, REPUBLISH_AFTER_VAR,
        SWEEP_PAGE_VAR, UsageTransferWorkerConfigError,
    };
    use std::collections::BTreeMap;

    /// The category this binary is bound to, and the only one it may name.
    const CATEGORY: aex_usage_domain::meter::Category = aex_usage_transfer_dynamodb::CATEGORY;

    fn complete() -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            (PLANE_VAR, "dev".to_owned()),
            (REGION_VAR, "eu-west-1".to_owned()),
            (
                AUTHORITY_TABLE_VAR,
                "dev-eu-west-1-usage-transfer".to_owned(),
            ),
            (
                PROJECTION_TABLE_VAR,
                "dev-eu-west-1-usage-query-projection".to_owned(),
            ),
            (
                RATING_QUEUE_VAR,
                "aex-dev-usage-rating-shadow.fifo".to_owned(),
            ),
            (
                RECEIPT_QUEUE_VAR,
                "aex-dev-usage-receipt-transfer".to_owned(),
            ),
            (BILLING_MODE_VAR, "shadow".to_owned()),
            (REPUBLISH_AFTER_VAR, "60000".to_owned()),
            (SWEEP_PAGE_VAR, "200".to_owned()),
            (ATTEMPT_ALARM_VAR, "10".to_owned()),
            (AGE_ALARM_VAR, "900000".to_owned()),
            (BACKLOG_CEILING_VAR, "100000".to_owned()),
            (BUDGET_VAR, "100".to_owned()),
        ])
    }

    fn read(
        vars: &BTreeMap<&'static str, String>,
    ) -> Result<Config, UsageTransferWorkerConfigError> {
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn accepts_a_complete_environment() {
        let config = read(&complete()).expect("complete environment is accepted");
        assert_eq!(config.plane, "dev");
        assert_eq!(config.region.as_str(), "eu-west-1");
        assert_eq!(config.billing_mode, BillingMode::Shadow);
        assert_eq!(config.limits.outbox_republish_after_ms, 60_000);
        assert_eq!(config.limits.sweep_page, 200);
        assert_eq!(config.limits.outbox_attempt_alarm, 10);
        assert_eq!(config.budget, 100);
    }

    #[test]
    fn names_each_missing_variable() {
        for name in complete().keys() {
            let mut vars = complete();
            vars.remove(name);
            assert_eq!(
                read(&vars),
                Err(UsageTransferWorkerConfigError::Missing { name }),
                "removing {name}"
            );
        }
    }

    #[test]
    fn rejects_a_blank_variable_as_missing() {
        let mut vars = complete();
        vars.insert(AUTHORITY_TABLE_VAR, "   ".to_owned());
        assert_eq!(
            read(&vars),
            Err(UsageTransferWorkerConfigError::Missing {
                name: AUTHORITY_TABLE_VAR
            })
        );
    }

    #[test]
    fn rejects_an_unknown_plane() {
        let mut vars = complete();
        vars.insert(PLANE_VAR, "staging".to_owned());
        assert!(matches!(
            read(&vars),
            Err(UsageTransferWorkerConfigError::Invalid {
                name: PLANE_VAR,
                ..
            })
        ));
    }

    #[test]
    fn no_money_relevant_limit_may_be_zero_or_non_numeric() {
        for name in [
            REPUBLISH_AFTER_VAR,
            SWEEP_PAGE_VAR,
            ATTEMPT_ALARM_VAR,
            AGE_ALARM_VAR,
            BACKLOG_CEILING_VAR,
            BUDGET_VAR,
        ] {
            for value in ["0", "lots", "-1"] {
                let mut vars = complete();
                vars.insert(name, value.to_owned());
                assert!(
                    matches!(
                        read(&vars),
                        Err(UsageTransferWorkerConfigError::Invalid { .. })
                    ),
                    "`{name}` accepted `{value}`"
                );
            }
        }
    }

    #[test]
    fn the_charging_gate_cannot_be_half_satisfied() {
        // Shadow pointed at the live queue would post real money while every
        // dashboard said it was shadowing.
        let mut live_queue_shadow_mode = complete();
        live_queue_shadow_mode.insert(RATING_QUEUE_VAR, "aex-dev-usage-rating.fifo".to_owned());
        assert!(matches!(
            read(&live_queue_shadow_mode),
            Err(UsageTransferWorkerConfigError::ChargingGate { .. })
        ));

        // And active pointed at the shadow queue would silently bill nothing.
        let mut shadow_queue_active_mode = complete();
        shadow_queue_active_mode.insert(BILLING_MODE_VAR, "active".to_owned());
        assert!(matches!(
            read(&shadow_queue_active_mode),
            Err(UsageTransferWorkerConfigError::ChargingGate { .. })
        ));

        // The two consistent combinations are the only ones that start.
        let mut active = complete();
        active.insert(BILLING_MODE_VAR, "active".to_owned());
        active.insert(RATING_QUEUE_VAR, "aex-dev-usage-rating.fifo".to_owned());
        assert_eq!(
            read(&active).expect("starts").billing_mode,
            BillingMode::Active
        );
        assert_eq!(
            read(&complete()).expect("starts").billing_mode,
            BillingMode::Shadow
        );
    }

    #[test]
    fn an_unknown_billing_mode_is_refused_rather_than_defaulted() {
        let mut vars = complete();
        vars.insert(BILLING_MODE_VAR, "maybe".to_owned());
        assert!(matches!(
            read(&vars),
            Err(UsageTransferWorkerConfigError::Invalid {
                name: BILLING_MODE_VAR,
                ..
            })
        ));
    }

    #[test]
    fn a_region_that_is_not_an_identifier_is_refused_rather_than_carried_as_text() {
        // An absent frontier row is the empty frontier of *this* region, and a
        // region carrying the key separator would forge a partition, so the
        // identifier grammar is applied at start-up rather than at first write.
        let mut vars = complete();
        vars.insert(REGION_VAR, "eu#west#1".to_owned());
        assert!(matches!(
            read(&vars),
            Err(UsageTransferWorkerConfigError::Invalid {
                name: REGION_VAR,
                ..
            })
        ));
    }

    #[test]
    fn the_partial_batch_response_uses_the_identifiers_lambda_reads() {
        // Lambda checkpoints past anything this list omits. A different key name
        // would be read as an empty list, and the batch would be checkpointed
        // past a record that never folded.
        assert_eq!(super::failures(&[]), Vec::<serde_json::Value>::new());
        assert_eq!(
            super::failures(&["seq-1".to_owned(), "seq-2".to_owned()]),
            vec![
                serde_json::json!({ "itemIdentifier": "seq-1" }),
                serde_json::json!({ "itemIdentifier": "seq-2" }),
            ]
        );
    }

    #[test]
    fn each_trigger_classifies_to_exactly_one_mode() {
        let stream = serde_json::json!({
            "Records": [{ "eventSource": "aws:dynamodb", "eventName": "INSERT" }]
        });
        let receipt = serde_json::json!({
            "Records": [{ "eventSource": "aws:sqs", "body": "{}" }]
        });
        let sweep = serde_json::json!({ "detail-type": "Scheduled Event", "detail": {} });

        assert_eq!(
            EventMode::classify(&stream).expect("classifies"),
            EventMode::Stream
        );
        assert_eq!(
            EventMode::classify(&receipt).expect("classifies"),
            EventMode::Receipt
        );
        assert_eq!(
            EventMode::classify(&sweep).expect("classifies"),
            EventMode::Sweep
        );
        assert_eq!(EventMode::Stream.id(), "stream");
        assert_eq!(EventMode::Sweep.id(), "sweep");
        assert_eq!(EventMode::Receipt.id(), "receipt");
    }

    #[test]
    fn an_unrecognised_or_mixed_envelope_is_refused_never_guessed() {
        // Guessing a mode would run the wrong handler over money evidence.
        for unrecognised in [
            serde_json::json!({}),
            serde_json::json!({ "Records": [] }),
            serde_json::json!({ "Records": [{ "eventSource": "aws:s3" }] }),
            serde_json::json!({ "detail-type": "Something Else" }),
        ] {
            assert!(
                EventMode::classify(&unrecognised).is_err(),
                "{unrecognised} was classified"
            );
        }

        let mixed = serde_json::json!({
            "Records": [
                { "eventSource": "aws:dynamodb" },
                { "eventSource": "aws:sqs" }
            ]
        });
        assert!(matches!(
            EventMode::classify(&mixed),
            Err(DispatchError::MixedSources { .. })
        ));
    }

    #[test]
    fn this_binary_is_bound_to_exactly_one_authority() {
        assert_eq!(CATEGORY.id(), "transfer");
        // The three environment bindings all come from this binary's one
        // adapter, so a second table cannot be introduced by editing a string.
        assert_eq!(AUTHORITY_TABLE_VAR, "AEX_USAGE_TRANSFER_TABLE");
        assert_eq!(RECEIPT_QUEUE_VAR, "AEX_USAGE_RECEIPT_TRANSFER_QUEUE");
    }

    #[test]
    fn this_binary_links_no_sibling_authority_adapter() {
        // The link graph is the control that survives a mistaken IAM grant: a
        // worker that cannot reach a sibling's expression builder cannot write
        // its table even with the wrong credentials.
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("the crate's own manifest is readable");
        for sibling in ["aex-usage-storage-dynamodb", "aex-usage-compute-dynamodb"] {
            assert!(
                !manifest.contains(sibling),
                "`{sibling}` must not be reachable from this worker"
            );
        }
        assert!(manifest.contains("aex-usage-transfer-dynamodb"));
    }
}
