//! Per-run synthetic tenant and resource prefixes.
//!
//! Every live or shared-environment run allocates its own prefix so two runs
//! cannot collide, and so a leaked resource can always be attributed to the run
//! that created it. This is the naming half of the `Q-DATA` contract; the
//! cleanup half is [`crate::teardown`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Every synthetic name starts with this, so a sweep can find test residue.
pub const NAMESPACE: &str = "aex-test";

/// Why a prefix could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrefixError {
    /// The label was empty.
    #[error("a run-prefix label must not be empty")]
    Empty,
    /// The label contained something other than lowercase letters, digits or `-`.
    #[error("run-prefix label `{label}` must be lowercase letters, digits or `-`")]
    Malformed {
        /// The rejected label.
        label: String,
    },
    /// The label was longer than [`RunPrefix::MAX_LABEL_LEN`].
    #[error("run-prefix label `{label}` is longer than {max} characters")]
    TooLong {
        /// The rejected label.
        label: String,
        /// The permitted maximum.
        max: usize,
    },
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique synthetic prefix for one test run.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunPrefix {
    value: String,
}

impl RunPrefix {
    /// Longest permitted label.
    pub const MAX_LABEL_LEN: usize = 24;

    /// Allocates a prefix that no concurrent run can produce.
    ///
    /// Uniqueness comes from the process identifier, a process-local counter and
    /// the wall clock together, so neither two threads nor two processes nor two
    /// machines share a prefix.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when `label` is empty, malformed or too long.
    pub fn new(label: &str) -> Result<Self, PrefixError> {
        Self::check(label)?;
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let token = format!("{:x}{:x}{:x}", std::process::id(), nanos, sequence);
        Ok(Self {
            value: format!("{NAMESPACE}-{label}-{token}"),
        })
    }

    /// Builds a prefix from an explicit token.
    ///
    /// Used by tests that must assert on the exact rendered name. It is not a
    /// substitute for [`RunPrefix::new`] in a shared environment.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when `label` or `token` is empty, malformed or
    /// too long.
    pub fn deterministic(label: &str, token: &str) -> Result<Self, PrefixError> {
        Self::check(label)?;
        Self::check(token)?;
        Ok(Self {
            value: format!("{NAMESPACE}-{label}-{token}"),
        })
    }

    /// The prefix itself.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// A resource name inside this run's namespace.
    ///
    /// # Errors
    ///
    /// Returns [`PrefixError`] when `name` is empty, malformed or too long.
    pub fn resource(&self, name: &str) -> Result<String, PrefixError> {
        Self::check(name)?;
        Ok(format!("{}-{name}", self.value))
    }

    /// Whether `candidate` was created by a run using this namespace.
    #[must_use]
    pub fn is_synthetic(candidate: &str) -> bool {
        candidate.starts_with(NAMESPACE)
    }

    fn check(label: &str) -> Result<(), PrefixError> {
        if label.is_empty() {
            return Err(PrefixError::Empty);
        }
        if label.len() > Self::MAX_LABEL_LEN {
            return Err(PrefixError::TooLong {
                label: label.to_owned(),
                max: Self::MAX_LABEL_LEN,
            });
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(PrefixError::Malformed {
                label: label.to_owned(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{NAMESPACE, PrefixError, RunPrefix};

    #[test]
    fn two_prefixes_from_one_process_never_collide() {
        let mut seen = Vec::with_capacity(512);
        for _ in 0..512 {
            seen.push(RunPrefix::new("regional").expect("a valid label is accepted"));
        }
        let mut unique = seen.clone();
        unique.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        unique.dedup();
        assert_eq!(unique.len(), seen.len(), "every run prefix must be unique");
    }

    #[test]
    fn every_prefix_is_recognisably_synthetic() {
        let prefix = RunPrefix::new("regional").expect("a valid label is accepted");
        assert!(prefix.as_str().starts_with(NAMESPACE));
        assert!(RunPrefix::is_synthetic(prefix.as_str()));
        assert!(!RunPrefix::is_synthetic("acme-production-workspace"));
    }

    #[test]
    fn resource_names_stay_inside_the_run_namespace() {
        let prefix =
            RunPrefix::deterministic("regional", "0001").expect("a valid label is accepted");
        assert_eq!(prefix.as_str(), "aex-test-regional-0001");
        assert_eq!(
            prefix
                .resource("primary")
                .expect("a valid resource name is accepted"),
            "aex-test-regional-0001-primary"
        );
    }

    #[test]
    fn an_empty_label_is_rejected() {
        assert_eq!(RunPrefix::new(""), Err(PrefixError::Empty));
    }

    #[test]
    fn an_uppercase_label_is_rejected() {
        let error = RunPrefix::new("Mixed").expect_err("an uppercase label is rejected");
        assert!(matches!(error, PrefixError::Malformed { .. }), "{error:?}");
    }

    #[test]
    fn an_over_long_label_is_rejected() {
        let label = "a".repeat(RunPrefix::MAX_LABEL_LEN + 1);
        let error = RunPrefix::new(&label).expect_err("an over-long label is rejected");
        assert!(matches!(error, PrefixError::TooLong { .. }), "{error:?}");
    }
}
