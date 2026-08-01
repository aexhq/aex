//! The private-path allowlist gate.
//!
//! The classifier is public and tested; the private repository only invokes it.
//! A boundary rule that lives in an unreviewed private script is a boundary
//! rule nobody can audit, which is the same as not having one.
//!
//! Every tracked private path must match the deny list (rejected), or exactly
//! one category (accepted, then content-asserted). Zero matches is
//! `unclassified` and more than one is `ambiguous`; both fail. There is no
//! default-allow.

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError, Violation};

/// One permitted category of private file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Category id.
    pub id: String,
    /// Glob patterns.
    pub globs: Vec<String>,
    /// Content assertions applied after a match.
    #[serde(default)]
    pub assertions: Vec<String>,
}

/// One forbidden class of private file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Deny {
    /// Deny id.
    pub id: String,
    /// Glob patterns.
    #[serde(default)]
    pub globs: Vec<String>,
    /// A named content predicate, where a glob cannot express the rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

/// `aex.private-path-policy.v1`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Schema discriminator.
    pub schema: String,
    /// Policy version.
    pub version: u32,
    /// Permitted categories.
    pub categories: Vec<Category>,
    /// Forbidden classes.
    pub deny: Vec<Deny>,
}

/// What one path resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Verdict {
    /// Accepted into a category.
    Allowed {
        /// The category.
        category: String,
    },
    /// Rejected by a deny rule.
    Denied {
        /// The deny id.
        deny: String,
    },
    /// Matched no category.
    Unclassified,
    /// Matched more than one category.
    Ambiguous {
        /// Every category that matched.
        categories: Vec<String>,
    },
}

/// Match a `{a,b}`-and-`**`-capable glob against a `/`-separated path.
///
/// This is a small, total matcher rather than a general glob library: the
/// policy is a fixed vocabulary, and a matcher whose behaviour nobody can
/// enumerate is not a boundary anybody can reason about.
#[must_use]
pub fn glob_match(pattern: &str, path: &str) -> bool {
    for expanded in expand_braces(pattern) {
        if match_segments(
            &expanded.split('/').collect::<Vec<_>>(),
            &path.split('/').collect::<Vec<_>>(),
        ) {
            return true;
        }
    }
    false
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_owned()];
    };
    let Some(close) = pattern[open..].find('}').map(|offset| open + offset) else {
        return vec![pattern.to_owned()];
    };
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    pattern[open + 1..close]
        .split(',')
        .flat_map(|choice| expand_braces(&format!("{prefix}{choice}{suffix}")))
        .collect()
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.first(), path.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            // `**` matches zero or more segments.
            (0..=path.len()).any(|skip| match_segments(&pattern[1..], &path[skip..]))
        }
        (None, Some(_)) | (Some(_), None) => false,
        (Some(segment), Some(candidate)) => {
            match_one(segment, candidate) && match_segments(&pattern[1..], &path[1..])
        }
    }
}

fn match_one(pattern: &str, candidate: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let candidate: Vec<char> = candidate.chars().collect();
    let (mut p, mut c) = (0usize, 0usize);
    let (mut star, mut mark) = (usize::MAX, 0usize);
    while c < candidate.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == candidate[c]) {
            p += 1;
            c += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = p;
            mark = c;
            p += 1;
        } else if star != usize::MAX {
            p = star + 1;
            mark += 1;
            c = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// Classify one path.
#[must_use]
pub fn classify(policy: &Policy, path: &str) -> Verdict {
    for deny in &policy.deny {
        if deny.globs.iter().any(|glob| glob_match(glob, path)) {
            return Verdict::Denied {
                deny: deny.id.clone(),
            };
        }
    }
    let matched: Vec<String> = policy
        .categories
        .iter()
        .filter(|category| category.globs.iter().any(|glob| glob_match(glob, path)))
        .map(|category| category.id.clone())
        .collect();
    match matched.len() {
        0 => Verdict::Unclassified,
        1 => Verdict::Allowed {
            category: matched.into_iter().next().unwrap_or_default(),
        },
        _ => Verdict::Ambiguous {
            categories: matched,
        },
    }
}

/// Credential shapes that must never appear in a tracked file.
///
/// The value itself is never printed on a hit; only its location. Printing it
/// would move the exposure into the CI log, which is the thing the rule exists
/// to prevent.
#[must_use]
pub fn find_secret_shapes(content: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    let patterns: &[(&str, &str)] = &[
        ("private-key-block", "-----BEGIN"),
        ("openai-style-key", "sk-"),
        ("github-token", "ghp_"),
        ("github-oauth-token", "gho_"),
        ("github-user-token", "ghu_"),
        ("github-server-token", "ghs_"),
        ("github-refresh-token", "ghr_"),
        ("stripe-webhook-secret", "whsec_"),
        ("aws-access-key-id", "AKIA"),
    ];
    for (name, needle) in patterns {
        if let Some(position) = content.find(needle) {
            let tail = &content[position + needle.len()..];
            let run: String = tail
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
                .collect();
            let long_enough = match *name {
                "private-key-block" => content.contains("PRIVATE KEY-----"),
                "openai-style-key" => run.len() >= 20,
                // Every other shape is a fixed-width identifier of at least
                // sixteen characters after its prefix.
                _ => run.len() >= 16,
            };
            if long_enough {
                found.push(*name);
            }
        }
    }
    found
}

/// Run the classifier over a path set.
///
/// `read` supplies file content for the assertions, so the caller decides
/// whether that comes from disk, from a diff, or from a fixture.
///
/// # Errors
/// Returns [`Exit::PrivatePathUnclassified`] carrying every offending path.
pub fn check<'a>(
    policy: &Policy,
    paths: &'a [String],
    read: impl Fn(&'a str) -> Option<String>,
) -> Result<()> {
    let mut violations = Vec::new();
    for path in paths {
        match classify(policy, path) {
            Verdict::Denied { deny } => violations.push(Violation::new(
                "private-path-denied",
                format!("`{path}` matches deny rule `{deny}`"),
            )),
            Verdict::Unclassified => violations.push(Violation::new(
                "private-path-unclassified",
                format!("unclassified: `{path}`"),
            )),
            Verdict::Ambiguous { categories } => violations.push(Violation::new(
                "private-path-ambiguous",
                format!("ambiguous: `{path}` matches {}", categories.join(", ")),
            )),
            Verdict::Allowed { category } => {
                let assertions = policy
                    .categories
                    .iter()
                    .find(|candidate| candidate.id == category)
                    .map(|candidate| candidate.assertions.clone())
                    .unwrap_or_default();
                if assertions.iter().any(|a| a == "no-plaintext-secret")
                    && let Some(content) = read(path)
                {
                    for shape in find_secret_shapes(&content) {
                        violations.push(Violation::new(
                            "private-path-plaintext-secret",
                            format!(
                                "`{path}` holds a `{shape}` credential shape; treat the value \
                                 as exposed and rotate it"
                            ),
                        ));
                    }
                }
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::PrivatePathUnclassified, violations))
    }
}

#[cfg(test)]
mod tests {
    use super::{Verdict, classify, find_secret_shapes, glob_match};

    #[test]
    fn double_star_matches_zero_or_more_segments() {
        assert!(glob_match(
            "composition/roots/**/*.tf",
            "composition/roots/a.tf"
        ));
        assert!(glob_match(
            "composition/roots/**/*.tf",
            "composition/roots/dev/eu-west-1/main.tf"
        ));
        assert!(!glob_match("composition/roots/**/*.tf", "composition/a.tf"));
    }

    #[test]
    fn brace_alternation_expands() {
        assert!(glob_match(
            "composition/{dev,prd}/**/*.json",
            "composition/dev/x.json"
        ));
        assert!(glob_match(
            "composition/{dev,prd}/**/*.json",
            "composition/prd/a/b.json"
        ));
        assert!(!glob_match(
            "composition/{dev,prd}/**/*.json",
            "composition/local/x.json"
        ));
    }

    #[test]
    fn a_single_star_does_not_cross_a_path_boundary() {
        assert!(!glob_match("*.rs", "src/main.rs"));
        assert!(glob_match("**/*.rs", "src/main.rs"));
    }

    fn policy() -> super::Policy {
        serde_json::from_str(include_str!(
            "../../../release/policy/private-path-policy.json"
        ))
        .expect("the shipped policy must parse")
    }

    #[test]
    fn the_shipped_policy_classifies_one_path_per_category() {
        let cases = [
            ("composition/roots/dev/main.tf", "environment-root"),
            ("composition/dev/binding.json", "composition"),
            ("composition/dev/secrets.dev.json", "secret-reference"),
            ("business-data/price-book.json", "business-data"),
            ("operations/runbooks/deploy.md", "operations-record"),
            ("operations/contacts/oncall.md", "private-contact"),
        ];
        let policy = policy();
        for (path, expected) in cases {
            match classify(&policy, path) {
                Verdict::Allowed { category } => assert_eq!(category, expected, "for {path}"),
                Verdict::Ambiguous { categories } => {
                    // A path may legitimately sit under two globs only if the
                    // policy says so; today none does.
                    panic!("`{path}` is ambiguous across {categories:?}");
                }
                other => panic!("`{path}` classified as {other:?}"),
            }
        }
    }

    #[test]
    fn every_deny_class_rejects_its_representative_path() {
        let cases = [
            ("services/api/src/main.rs", "service-implementation"),
            ("db/0001_init.sql", "database-migration"),
            ("contracts/openapi/session.yaml", "private-contract"),
            ("suites/tests/session.md", "correctness-test"),
        ];
        let policy = policy();
        for (path, expected) in cases {
            match classify(&policy, path) {
                Verdict::Denied { deny } => assert_eq!(deny, expected, "for {path}"),
                other => panic!("`{path}` classified as {other:?}, expected a denial"),
            }
        }
    }

    #[test]
    fn an_unlisted_path_is_unclassified_rather_than_allowed() {
        assert_eq!(
            classify(&policy(), "scratch/notes.txt"),
            Verdict::Unclassified
        );
    }

    #[test]
    fn credential_shapes_are_detected_without_echoing_the_value() {
        assert_eq!(
            find_secret_shapes("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"),
            vec!["aws-access-key-id"]
        );
        assert!(
            find_secret_shapes("token: ghp_abcdefghijklmnopqrstuvwxyz01").contains(&"github-token")
        );
        assert!(
            find_secret_shapes("-----BEGIN OPENSSH PRIVATE KEY-----")
                .contains(&"private-key-block")
        );
        assert!(
            find_secret_shapes("AKIA").is_empty(),
            "a bare prefix is not a key"
        );
    }
}
