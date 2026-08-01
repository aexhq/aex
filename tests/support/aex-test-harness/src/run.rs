//! Run identity, lane, TTL, resource prefix and tag set.
//!
//! Every live, e2e, user, load and soak run mints exactly one [`TestRun`]. Its
//! id is the join key between the resources the run creates, the cleanup ledger
//! it writes, the tags the janitor sweeps by, and the receipt the release tool
//! records. Nothing else may name a resource.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::budget::Budget;
use crate::canary::SecretCanary;
use crate::ledger::CleanupLedger;

/// RFC 3339 serialization for the timestamps that reach the ledger and the
/// receipt.
///
/// The `time` crate offers this behind a feature the workspace does not enable,
/// and the format is fixed by the receipt schema, so it is written out here
/// rather than widening the dependency.
pub mod rfc3339 {
    use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    /// Renders a timestamp as an RFC 3339 string.
    ///
    /// # Errors
    ///
    /// Returns a serializer error when the timestamp cannot be formatted, which
    /// only happens for a value outside the representable range.
    pub fn serialize<S: Serializer>(
        value: &OffsetDateTime,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let text = value
            .format(&Rfc3339)
            .map_err(|error| serde::ser::Error::custom(error.to_string()))?;
        serializer.serialize_str(&text)
    }

    /// Parses an RFC 3339 string back into a timestamp.
    ///
    /// # Errors
    ///
    /// Returns a deserializer error when the text is not RFC 3339.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<OffsetDateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        OffsetDateTime::parse(&text, &Rfc3339).map_err(D::Error::custom)
    }

    /// The same rendering for an optional timestamp.
    pub mod option {
        use serde::{Deserialize as _, Deserializer, Serializer, de::Error as _};
        use time::OffsetDateTime;
        use time::format_description::well_known::Rfc3339;

        /// Renders an optional timestamp as an RFC 3339 string or `null`.
        ///
        /// # Errors
        ///
        /// Returns a serializer error when a present timestamp cannot be
        /// formatted.
        pub fn serialize<S: Serializer>(
            value: &Option<OffsetDateTime>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            match value {
                None => serializer.serialize_none(),
                Some(inner) => {
                    let text = inner
                        .format(&Rfc3339)
                        .map_err(|error| serde::ser::Error::custom(error.to_string()))?;
                    serializer.serialize_some(&text)
                }
            }
        }

        /// Parses an optional RFC 3339 string.
        ///
        /// # Errors
        ///
        /// Returns a deserializer error when a present value is not RFC 3339.
        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<OffsetDateTime>, D::Error> {
            let text = Option::<String>::deserialize(deserializer)?;
            match text {
                None => Ok(None),
                Some(inner) => OffsetDateTime::parse(&inner, &Rfc3339)
                    .map(Some)
                    .map_err(D::Error::custom),
            }
        }
    }
}

/// Which lane minted a run.
///
/// The lane decides the default TTL, the janitor's residue deadline and which
/// evidence class the resulting receipt belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// The default lane: no remote dependency, no credential.
    Unit,
    /// Real local engines from `release/policy/test-images.toml`.
    Integration,
    /// The shallowest lane against a deployed artifact.
    Smoke,
    /// Seam and fault scenarios against a deployed composition.
    E2e,
    /// Packed or published artifact journeys from outside the workspace.
    User,
    /// A capacity or pressure workload.
    Load,
    /// A long-running drift and leak workload.
    Soak,
}

impl Lane {
    /// Every lane, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::Unit,
        Self::Integration,
        Self::Smoke,
        Self::E2e,
        Self::User,
        Self::Load,
        Self::Soak,
    ];

    /// The lane's name as it appears in policy, tags and receipts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Integration => "integration",
            Self::Smoke => "smoke",
            Self::E2e => "e2e",
            Self::User => "user",
            Self::Load => "load",
            Self::Soak => "soak",
        }
    }

    /// The lane's default time-to-live, read from
    /// `release/policy/test-profiles.toml`.
    ///
    /// # Panics
    ///
    /// Panics when the embedded policy document does not declare this lane. The
    /// document is compiled in, so that is a build-time defect rather than a
    /// run-time condition, and failing loudly beats inventing a TTL that no
    /// janitor agrees with.
    #[must_use]
    pub fn default_ttl(self) -> Ttl {
        static TABLE: OnceLock<BTreeMap<String, i64>> = OnceLock::new();
        let table = TABLE.get_or_init(|| {
            #[derive(Deserialize)]
            struct Document {
                values: Values,
            }
            #[derive(Deserialize)]
            struct Values {
                lane_ttl_minutes: BTreeMap<String, i64>,
            }
            let document: Document = toml::from_str(crate::TEST_PROFILES_TOML)
                .expect("release/policy/test-profiles.toml is embedded and must parse");
            document.values.lane_ttl_minutes
        });
        let minutes = *table.get(self.as_str()).unwrap_or_else(|| {
            panic!(
                "release/policy/test-profiles.toml declares no [values.lane_ttl_minutes] row for lane `{}`",
                self.as_str()
            )
        });
        Ttl(Duration::minutes(minutes))
    }
}

impl std::fmt::Display for Lane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How long a run's resources may exist before the janitor treats them as
/// residue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Ttl(#[serde(with = "duration_seconds")] pub Duration);

impl Ttl {
    /// The grace period the janitor adds before calling a surviving resource
    /// residue.
    ///
    /// Read from `[janitor].residue_grace_minutes` rather than declared here,
    /// because the janitor sweeps by the same number and a second copy is a
    /// second answer to "is this run still allowed to be alive".
    #[must_use]
    pub fn residue_grace() -> Duration {
        Duration::minutes(crate::ledger::janitor_policy().residue_grace_minutes)
    }

    /// The instant after which a surviving resource is residue.
    #[must_use]
    pub fn residue_deadline(self, started_at: OffsetDateTime) -> OffsetDateTime {
        started_at + self.0 + Self::residue_grace()
    }
}

mod duration_seconds {
    use serde::{Deserialize as _, Deserializer, Serializer};
    use time::Duration;

    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(value.whole_seconds())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        Ok(Duration::seconds(i64::deserialize(deserializer)?))
    }
}

/// A test run identifier: `tr_` followed by 32 lowercase hex characters of a
/// `UUIDv7`, so ids sort by mint time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TestRunId(String);

impl TestRunId {
    /// The fixed prefix every id carries, from `[janitor]` in
    /// `release/policy/test-profiles.toml`.
    ///
    /// The shape lives in policy rather than here because the janitor validates
    /// it from the same document. A minter and a validator that disagreed about
    /// the shape would produce a sweep that reclaims nothing.
    #[must_use]
    pub fn prefix() -> &'static str {
        &crate::ledger::janitor_policy().run_id_prefix
    }

    /// The number of hex characters after the prefix.
    #[must_use]
    pub fn hex_len() -> usize {
        crate::ledger::janitor_policy().run_id_hex_len
    }

    /// Mints a new time-ordered id.
    #[must_use]
    pub fn mint() -> Self {
        Self(format!(
            "{}{}",
            Self::prefix(),
            hex::encode(Uuid::now_v7().as_bytes())
        ))
    }

    /// The id as it appears in prefixes, tags and ledger entries.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether a string has the exact shape of a run id.
    #[must_use]
    pub fn is_well_formed(text: &str) -> bool {
        text.strip_prefix(Self::prefix()).is_some_and(|hex_part| {
            hex_part.len() == Self::hex_len()
                && hex_part
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }
}

impl std::fmt::Display for TestRunId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One run's identity and the four things every live test needs from it.
#[derive(Debug)]
pub struct TestRun {
    id: TestRunId,
    lane: Lane,
    owner: String,
    started_at: OffsetDateTime,
    expires_at: OffsetDateTime,
    budget: Budget,
    canary: SecretCanary,
    ledger: CleanupLedger,
}

impl TestRun {
    /// Mints a run for `lane`, owned by `owner`, with `budget_micro_usd` of
    /// spend allowed before the run stops.
    ///
    /// `owner` is one of the closed owner values the owning package declares in
    /// its `[package.metadata.aex]`; it reaches tags and the ledger unchanged so
    /// a swept resource names a team, not a machine.
    #[must_use]
    pub fn mint(lane: Lane, owner: &str, budget_micro_usd: u64) -> Self {
        let id = TestRunId::mint();
        let started_at = OffsetDateTime::now_utc();
        let ttl = lane.default_ttl();
        Self {
            expires_at: started_at + ttl.0,
            ledger: CleanupLedger::new(id.clone()),
            budget: Budget::new(budget_micro_usd),
            canary: SecretCanary::mint(),
            id,
            lane,
            owner: owner.to_owned(),
            started_at,
        }
    }

    /// The run id.
    #[must_use]
    pub fn id(&self) -> &TestRunId {
        &self.id
    }

    /// The lane that minted the run.
    #[must_use]
    pub const fn lane(&self) -> Lane {
        self.lane
    }

    /// The owning stream.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// When the run was minted.
    #[must_use]
    pub const fn started_at(&self) -> OffsetDateTime {
        self.started_at
    }

    /// When the run's resources become sweepable.
    #[must_use]
    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }

    /// The run's spend budget.
    #[must_use]
    pub const fn budget(&self) -> &Budget {
        &self.budget
    }

    /// The run's secret canary.
    #[must_use]
    pub const fn canary(&self) -> &SecretCanary {
        &self.canary
    }

    /// The run's cleanup ledger.
    #[must_use]
    pub const fn ledger(&self) -> &CleanupLedger {
        &self.ledger
    }

    /// The prefix every creatable resource name carries.
    #[must_use]
    pub fn resource_prefix(&self) -> String {
        format!("aextest-{}-", self.id)
    }

    /// A creatable resource name for `logical_name`.
    #[must_use]
    pub fn resource_name(&self, logical_name: &str) -> String {
        format!("{}{logical_name}", self.resource_prefix())
    }

    /// The `DynamoDB` partition-key prefix for this run's items.
    #[must_use]
    pub fn dynamodb_partition_prefix(&self) -> String {
        format!("TEST#{}#", self.id)
    }

    /// The S3 key prefix for this run's objects.
    #[must_use]
    pub fn s3_prefix(&self) -> String {
        format!("test/{}/", self.id)
    }

    /// The tag set every taggable resource carries.
    ///
    /// The keys and the marker value come from `[janitor]` in
    /// `release/policy/test-profiles.toml`, which is the same document the
    /// janitor sweeps by, so the stamper and the sweeper cannot drift. The four
    /// tags that existed before the janitor are unchanged; the marker is added
    /// beside them, and it is what the janitor's refusal turns on.
    #[must_use]
    pub fn tags(&self) -> BTreeMap<&'static str, String> {
        let policy = crate::ledger::janitor_policy();
        let mut tags = BTreeMap::new();
        tags.insert(
            policy.synthetic_tag.as_str(),
            policy.synthetic_value.clone(),
        );
        tags.insert(policy.run_id_tag.as_str(), self.id.to_string());
        tags.insert(policy.owner_tag.as_str(), self.owner.clone());
        tags.insert(policy.lane_tag.as_str(), self.lane.to_string());
        tags.insert(
            policy.expires_at_tag.as_str(),
            self.expires_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| self.expires_at.unix_timestamp().to_string()),
        );
        tags
    }

    /// A provider idempotency key that cannot collide with another run's key
    /// for the same logical operation.
    #[must_use]
    pub fn idempotency_key(&self, logical_key: &str) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(self.id.as_str().as_bytes());
        hasher.update(logical_key.as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    /// The directory this run's ledger, first-failure record and attachments
    /// are written to.
    #[must_use]
    pub fn artifact_dir(&self) -> std::path::PathBuf {
        std::path::PathBuf::from("target")
            .join("aex-test")
            .join(self.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{Lane, TestRun, TestRunId, Ttl};
    use time::OffsetDateTime;

    #[test]
    fn a_minted_id_has_the_declared_shape() {
        let id = TestRunId::mint();
        assert!(TestRunId::is_well_formed(id.as_str()), "{id}");
        assert_eq!(
            id.as_str().len(),
            TestRunId::prefix().len() + TestRunId::hex_len()
        );
        assert_eq!(TestRunId::prefix(), "tr_");
        assert_eq!(TestRunId::hex_len(), 32);
    }

    #[test]
    fn a_malformed_id_is_rejected() {
        assert!(!TestRunId::is_well_formed("tr_"));
        assert!(!TestRunId::is_well_formed(&format!(
            "tr_{}",
            "A".repeat(32)
        )));
        assert!(!TestRunId::is_well_formed(&format!(
            "xx_{}",
            "a".repeat(32)
        )));
        assert!(!TestRunId::is_well_formed(&format!(
            "tr_{}",
            "a".repeat(31)
        )));
    }

    #[test]
    fn every_lane_declares_a_ttl_and_a_residue_deadline() {
        let started_at = OffsetDateTime::now_utc();
        for lane in Lane::ALL {
            let ttl = lane.default_ttl();
            assert!(ttl.0.is_positive(), "{lane} has a non-positive ttl");
            assert!(ttl.residue_deadline(started_at) > started_at + ttl.0);
        }
        assert_eq!(Lane::Soak.default_ttl().0.whole_hours(), 30);
        assert_eq!(Lane::E2e.default_ttl().0.whole_hours(), 2);
        assert_eq!(Ttl::residue_grace().whole_minutes(), 30);
    }

    /// The janitor never reclaims a run whose deadline has not passed, so the
    /// deadline must be strictly later than the TTL a live lane is relying on.
    #[test]
    fn a_runs_residue_deadline_is_strictly_after_its_own_expiry() {
        let run = TestRun::mint(Lane::E2e, "test-architecture", 1_000);
        let deadline = run.lane().default_ttl().residue_deadline(run.started_at());
        assert!(deadline > run.expires_at());
        assert_eq!(deadline - run.expires_at(), Ttl::residue_grace());
    }

    #[test]
    fn every_name_a_run_can_mint_carries_its_id() {
        let run = TestRun::mint(Lane::E2e, "test-architecture", 1_000);
        let id = run.id().to_string();
        for name in [
            run.resource_prefix(),
            run.resource_name("bucket"),
            run.dynamodb_partition_prefix(),
            run.s3_prefix(),
        ] {
            assert!(name.contains(&id), "`{name}` does not carry `{id}`");
        }
        assert_eq!(
            run.tags().get("aex:test-owner").map(String::as_str),
            Some("test-architecture")
        );
        assert_eq!(run.tags().len(), 5);
    }

    /// The marker is the janitor's floor: without it, nothing is ever
    /// reclaimed. A run that stopped stamping it would silently make its own
    /// residue unsweepable, so the stamping is asserted here as well as in the
    /// janitor's own refusal tests.
    #[test]
    fn every_run_stamps_the_synthetic_marker_the_janitor_refuses_without() {
        let policy = crate::ledger::janitor_policy();
        let run = TestRun::mint(Lane::Smoke, "delivery", 1);
        let tags = run.tags();
        assert_eq!(
            tags.get(policy.synthetic_tag.as_str()).map(String::as_str),
            Some(policy.synthetic_value.as_str())
        );
        assert_eq!(
            tags.get(policy.run_id_tag.as_str()).map(String::as_str),
            Some(run.id().as_str())
        );
        assert_eq!(
            tags.get(policy.lane_tag.as_str()).map(String::as_str),
            Some("smoke")
        );
        assert!(tags.contains_key(policy.expires_at_tag.as_str()));
    }

    #[test]
    fn an_idempotency_key_is_deterministic_per_run_and_distinct_across_runs() {
        let first = TestRun::mint(Lane::Load, "brain-core", 1);
        let second = TestRun::mint(Lane::Load, "brain-core", 1);
        assert_eq!(
            first.idempotency_key("turn-1"),
            first.idempotency_key("turn-1")
        );
        assert_ne!(
            first.idempotency_key("turn-1"),
            first.idempotency_key("turn-2")
        );
        assert_ne!(
            first.idempotency_key("turn-1"),
            second.idempotency_key("turn-1")
        );
    }
}
