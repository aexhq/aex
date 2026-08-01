//! Reserved-name rules, written as `const fn` so [`crate::generated`] can assert
//! them at compile time rather than at start-up.
//!
//! A reserved prefix belongs to the telemetry runtime or to an upstream semantic
//! convention. Product code that reuses one would silently collide with a name
//! whose meaning it does not control, so the collision is made a build failure.

/// Whether `name` begins with `prefix`.
///
/// This is a `const fn` because [`crate::generated`] asserts the reserved-name
/// rule inside a `const` block; `str::starts_with` is not const-callable.
#[must_use]
pub const fn starts_with(name: &str, prefix: &str) -> bool {
    let name = name.as_bytes();
    let prefix = prefix.as_bytes();
    if prefix.len() > name.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if name[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// Whether `name` begins with any of `prefixes`.
#[must_use]
pub const fn matches_any(name: &str, prefixes: &[&str]) -> bool {
    let mut index = 0;
    while index < prefixes.len() {
        if starts_with(name, prefixes[index]) {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{matches_any, starts_with};

    #[test]
    fn prefix_longer_than_name_never_matches() {
        assert!(!starts_with("otel", "otel."));
    }

    #[test]
    fn exact_and_extended_matches_hold() {
        assert!(starts_with("otel.", "otel."));
        assert!(starts_with("otel.scope.name", "otel."));
        assert!(!starts_with("aex.otel.scope", "otel."));
    }

    #[test]
    fn empty_prefix_matches_everything() {
        assert!(starts_with("aex.plane", ""));
    }

    #[test]
    fn matches_any_agrees_with_starts_with() {
        let prefixes = ["aex.reserved.", "otel.", "telemetry.sdk."];
        assert!(matches_any("telemetry.sdk.name", &prefixes));
        assert!(!matches_any("aex.plane", &prefixes));
        assert!(!matches_any("aex.plane", &[]));
    }
}
