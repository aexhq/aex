//! The only sanctioned way for a test to read the environment.
//!
//! A bare `std::env::var` in a test target is the seed of a self-skip: the
//! moment a value is absent the temptation is to return early and report green.
//! [`required_env!`] has no such branch - an absent prerequisite panics with the
//! variable's name, which is a failure, which is the correct verdict for a lane
//! that cannot reach what it claims to test.

use std::env::VarError;

/// Reads an environment variable, failing loudly when it is absent or empty.
///
/// ```should_panic
/// let _ = aex_test_harness::required_env!("AEX_DEFINITELY_NOT_SET_9F2C");
/// ```
#[macro_export]
macro_rules! required_env {
    ($name:expr) => {{ $crate::env::require($name) }};
}

/// The function behind [`required_env!`].
///
/// # Panics
///
/// Panics when `name` is unset, empty, or not valid Unicode. Each case names
/// the variable so the fix is obvious from the failure alone.
#[must_use]
pub fn require(name: &str) -> String {
    classify(name, std::env::var(name))
}

/// The pure half of [`require`], so every failure message is provable without
/// mutating the process environment - which edition 2024 makes `unsafe` and the
/// workspace forbids outright.
///
/// # Panics
///
/// Panics for every non-present, empty or non-Unicode value.
#[must_use]
pub fn classify(name: &str, value: Result<String, VarError>) -> String {
    match value {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => panic!(
            "required environment variable `{name}` is set but empty; a live prerequisite is a failure, never a skip"
        ),
        Err(VarError::NotPresent) => panic!(
            "required environment variable `{name}` is absent; a live prerequisite is a failure, never a skip"
        ),
        Err(VarError::NotUnicode(_)) => panic!(
            "required environment variable `{name}` is not valid Unicode; a live prerequisite is a failure, never a skip"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::classify;
    use std::env::VarError;

    #[test]
    fn a_present_value_is_returned() {
        assert_eq!(
            classify("AEX_PG_URL", Ok("postgres://x".to_owned())),
            "postgres://x"
        );
    }

    #[test]
    #[should_panic(expected = "`AEX_PG_URL` is set but empty")]
    fn an_empty_value_fails() {
        let _ = classify("AEX_PG_URL", Ok("   ".to_owned()));
    }

    #[test]
    #[should_panic(expected = "`AEX_PG_URL` is absent")]
    fn an_absent_value_fails() {
        let _ = classify("AEX_PG_URL", Err(VarError::NotPresent));
    }

    #[test]
    #[should_panic(expected = "`AEX_PG_URL` is not valid Unicode")]
    fn a_non_unicode_value_fails() {
        let _ = classify(
            "AEX_PG_URL",
            Err(VarError::NotUnicode(std::ffi::OsString::from("x"))),
        );
    }

    #[test]
    fn the_real_reader_returns_a_value_that_cargo_always_sets() {
        assert!(!super::require("CARGO_PKG_NAME").is_empty());
    }
}
