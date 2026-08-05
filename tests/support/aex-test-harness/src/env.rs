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

/// Reads the first present environment variable from an ordered list.
///
/// This supports bounded compatibility aliases without allowing a live test to
/// self-skip. The first name is authoritative: a present-but-empty or
/// non-Unicode value fails instead of falling through to a lower-priority
/// alias. When every name is absent, the panic names every accepted variable.
///
/// # Panics
///
/// Panics when `names` is empty, when the first present value is empty or not
/// valid Unicode, or when every named variable is absent.
#[must_use]
pub fn require_first(names: &[&str]) -> String {
    require_first_named(names).1
}

/// Reads the first present environment variable and returns its accepted name
/// together with its value.
///
/// Use this when a live harness must retain the credential source identity
/// without retaining the credential itself in a longer-lived plan or matrix.
/// The same precedence and fail-closed rules as [`require_first`] apply.
///
/// # Panics
///
/// Panics under the same conditions as [`require_first`].
#[must_use]
pub fn require_first_named<'a>(names: &'a [&'a str]) -> (&'a str, String) {
    classify_first_named(names.iter().map(|name| (*name, std::env::var(name))))
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

/// The pure half of [`require_first`].
///
/// Results must be supplied in precedence order. Only `NotPresent` advances to
/// the next name; every other result is authoritative.
///
/// # Panics
///
/// Panics under the same conditions as [`require_first`].
#[must_use]
pub fn classify_first<'a, I>(values: I) -> String
where
    I: IntoIterator<Item = (&'a str, Result<String, VarError>)>,
{
    classify_first_named(values).1
}

/// The pure half of [`require_first_named`].
///
/// Results must be supplied in precedence order. Only `NotPresent` advances to
/// the next name; every other result is authoritative.
///
/// # Panics
///
/// Panics under the same conditions as [`require_first_named`].
#[must_use]
pub fn classify_first_named<'a, I>(values: I) -> (&'a str, String)
where
    I: IntoIterator<Item = (&'a str, Result<String, VarError>)>,
{
    let mut absent = Vec::new();
    for (name, value) in values {
        match value {
            Err(VarError::NotPresent) => absent.push(name),
            other => return (name, classify(name, other)),
        }
    }

    assert!(
        !absent.is_empty(),
        "at least one required environment variable name must be supplied"
    );
    panic!(
        "required environment variables `{}` are absent; a live prerequisite is a failure, never a skip",
        absent.join("`, `")
    );
}

#[cfg(test)]
mod tests {
    use super::{classify, classify_first, classify_first_named};
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

    #[test]
    fn the_first_present_variable_wins() {
        assert_eq!(
            classify_first([
                ("AEX_PRIMARY", Ok("primary".to_owned())),
                ("AEX_LEGACY", Ok("legacy".to_owned())),
            ]),
            "primary"
        );
    }

    #[test]
    fn an_absent_primary_falls_back_to_a_present_alias() {
        assert_eq!(
            classify_first([
                ("AEX_PRIMARY", Err(VarError::NotPresent)),
                ("AEX_LEGACY", Ok("legacy".to_owned())),
            ]),
            "legacy"
        );
    }

    #[test]
    fn the_selected_alias_name_is_preserved_without_changing_precedence() {
        assert_eq!(
            classify_first_named([
                ("AEX_PRIMARY", Err(VarError::NotPresent)),
                ("AEX_LEGACY", Ok("legacy".to_owned())),
            ]),
            ("AEX_LEGACY", "legacy".to_owned())
        );
    }

    #[test]
    #[should_panic(expected = "`AEX_PRIMARY` is set but empty")]
    fn an_empty_primary_does_not_fall_through_to_an_alias() {
        let _ = classify_first([
            ("AEX_PRIMARY", Ok(" ".to_owned())),
            ("AEX_LEGACY", Ok("legacy".to_owned())),
        ]);
    }

    #[test]
    #[should_panic(expected = "`AEX_PRIMARY`, `AEX_LEGACY` are absent")]
    fn all_absent_candidates_fail_with_every_accepted_name() {
        let _ = classify_first([
            ("AEX_PRIMARY", Err(VarError::NotPresent)),
            ("AEX_LEGACY", Err(VarError::NotPresent)),
        ]);
    }

    #[test]
    #[should_panic(expected = "at least one required environment variable name")]
    fn an_empty_candidate_list_is_a_configuration_error() {
        let _ = classify_first(std::iter::empty::<(&str, Result<String, VarError>)>());
    }
}
