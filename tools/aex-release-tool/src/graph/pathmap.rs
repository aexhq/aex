//! `release/path-map.toml` — file path to owning node.
//!
//! Path classification is prefix-safe: `packages/sdk-extra` is not inside
//! `packages/sdk`. The predecessor implementation compared raw string prefixes,
//! which quietly attributed a sibling directory's changes to the wrong module.
//!
//! An unowned path is a failure of `graph verify` **and** widens selection to
//! repo-wide. Failing alone would leave a red build with an unsafe run;
//! widening alone would hide the gap forever. Both together mean the run is
//! safe and the omission is visible.

use serde::{Deserialize, Serialize};

use crate::error::{Exit, Result, ToolError, Violation};

use super::NodeId;

/// How a matched rule resolves to an owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleKind {
    /// Owner is `cargo:<first segment after the prefix>`.
    DeriveCargo,
    /// Owner is the npm package whose directory is the first segment after the
    /// prefix. Resolved through the npm manifest map, because a package name is
    /// not its directory name.
    DeriveNpm,
    /// Owner is `tf:<prefix-relative first segment>`, prefixed by the rule's
    /// own `infra/` sub-tree.
    DeriveTerraform,
    /// Owner is the rule's explicit `node`.
    Node,
    /// Any change widens the run to the whole repository.
    RepoWide,
    /// A change to the router itself forces the full graph.
    Router,
    /// Classified and owns nothing: documentation, editor and licence files.
    Ignored,
}

/// One classification rule.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Stable rule id, reported in `graph explain`.
    pub id: String,
    /// Directory prefix. Matching is segment-wise, so a prefix always ends at
    /// a path boundary whether or not it is written with a trailing slash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Exact repository-relative path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<String>,
    /// Trailing filename or extension match, applied anywhere in the tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    /// How the owner is resolved.
    pub kind: RuleKind,
    /// The explicit owner for `kind = "node"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

/// The parsed map.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathMap {
    /// Schema discriminator.
    pub schema: String,
    /// Rules in declaration order; the first match wins.
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

/// What classifying one path produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// The path belongs to this node.
    Owned {
        /// The owning node.
        node: NodeId,
        /// The rule that matched.
        rule: String,
    },
    /// The path widens the run to the whole repository.
    RepoWide {
        /// The rule that matched.
        rule: String,
    },
    /// The path is part of the router; the full graph runs.
    Router {
        /// The rule that matched.
        rule: String,
    },
    /// The path is classified and owns no node.
    Ignored {
        /// The rule that matched.
        rule: String,
    },
    /// No rule matched.
    Orphan,
}

impl PathMap {
    /// Parse from TOML text.
    ///
    /// # Errors
    /// Returns [`Exit::GraphVerification`] when the document does not parse,
    /// carries the wrong schema, or declares a rule that cannot match or
    /// cannot resolve.
    pub fn parse(text: &str) -> Result<Self> {
        let map: Self = toml::from_str(text).map_err(|err| {
            ToolError::single(
                Exit::GraphVerification,
                "path-map-unparseable",
                format!("release/path-map.toml does not parse: {err}"),
            )
        })?;
        if map.schema != "aex.path-map.v1" {
            return Err(ToolError::single(
                Exit::GraphVerification,
                "path-map-unparseable",
                format!("unknown path-map schema `{}`", map.schema),
            ));
        }
        let mut violations = Vec::new();
        for rule in &map.rules {
            let selectors = usize::from(rule.prefix.is_some())
                + usize::from(rule.exact.is_some())
                + usize::from(rule.suffix.is_some());
            if selectors != 1 {
                violations.push(Violation::new(
                    "path-map-rule-invalid",
                    format!(
                        "rule `{}` declares {selectors} selectors; exactly one of \
                         prefix/exact/suffix is required",
                        rule.id
                    ),
                ));
            }
            if (rule.kind == RuleKind::Node) != rule.node.is_some() {
                violations.push(Violation::new(
                    "path-map-rule-invalid",
                    format!(
                        "rule `{}` must declare `node` if and only if kind = \"node\"",
                        rule.id
                    ),
                ));
            }
        }
        if violations.is_empty() {
            Ok(map)
        } else {
            Err(ToolError::many(Exit::GraphVerification, violations))
        }
    }

    /// Classify one repository-relative, `/`-separated path.
    ///
    /// `npm_dirs` maps a package directory to its npm package name, because
    /// `packages/sdk` publishes as `@aexhq/sdk`.
    #[must_use]
    pub fn classify(
        &self,
        path: &str,
        npm_dirs: &std::collections::BTreeMap<String, String>,
    ) -> Classification {
        for rule in &self.rules {
            let Some(remainder) = rule_match(rule, path) else {
                continue;
            };
            return match rule.kind {
                RuleKind::RepoWide => Classification::RepoWide {
                    rule: rule.id.clone(),
                },
                RuleKind::Router => Classification::Router {
                    rule: rule.id.clone(),
                },
                RuleKind::Ignored => Classification::Ignored {
                    rule: rule.id.clone(),
                },
                RuleKind::Node => rule.node.as_ref().map_or(Classification::Orphan, |node| {
                    Classification::Owned {
                        node: NodeId(node.clone()),
                        rule: rule.id.clone(),
                    }
                }),
                RuleKind::DeriveCargo => first_segment(&remainder).map_or(
                    Classification::Ignored {
                        rule: rule.id.clone(),
                    },
                    |segment| Classification::Owned {
                        node: NodeId::cargo(segment),
                        rule: rule.id.clone(),
                    },
                ),
                RuleKind::DeriveNpm => {
                    let Some(segment) = first_segment(&remainder) else {
                        return Classification::Ignored {
                            rule: rule.id.clone(),
                        };
                    };
                    let dir = format!("{}{segment}", rule.prefix.as_deref().unwrap_or_default());
                    npm_dirs
                        .get(dir.trim_end_matches('/'))
                        .map_or(Classification::Orphan, |name| Classification::Owned {
                            node: NodeId::npm(name),
                            rule: rule.id.clone(),
                        })
                }
                RuleKind::DeriveTerraform => {
                    let Some(segment) = first_segment(&remainder) else {
                        return Classification::Ignored {
                            rule: rule.id.clone(),
                        };
                    };
                    let prefix = rule.prefix.as_deref().unwrap_or_default();
                    let relative = prefix.trim_start_matches("infra/").trim_end_matches('/');
                    Classification::Owned {
                        node: NodeId::terraform(&format!("{relative}/{segment}")),
                        rule: rule.id.clone(),
                    }
                }
            };
        }
        Classification::Orphan
    }
}

/// Segment-wise prefix, exact or suffix match; returns the remainder after a
/// prefix match.
fn rule_match(rule: &Rule, path: &str) -> Option<String> {
    if let Some(exact) = &rule.exact {
        return (exact == path).then(String::new);
    }
    if let Some(suffix) = &rule.suffix {
        return path.ends_with(suffix).then(String::new);
    }
    let prefix = rule.prefix.as_ref()?;
    let normalized = prefix.trim_end_matches('/');
    if normalized.is_empty() {
        return Some(path.to_owned());
    }
    let rest = path.strip_prefix(normalized)?;
    // The stripped remainder must start at a path boundary, so `packages/sdk`
    // never claims `packages/sdk-extra/index.ts`.
    rest.strip_prefix('/').map(std::borrow::ToOwned::to_owned)
}

fn first_segment(remainder: &str) -> Option<&str> {
    let segment = remainder.split('/').next()?;
    // A file directly inside the prefix (`crates/README.md`) has no owning
    // member: the remainder holds no further separator.
    if segment.is_empty() || !remainder.contains('/') {
        return None;
    }
    Some(segment)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{Classification, PathMap};

    const MAP: &str = r#"
schema = "aex.path-map.v1"

[[rule]]
id = "router"
prefix = "tools/aex-release-tool/"
kind = "router"

[[rule]]
id = "release-registries"
prefix = "release/"
kind = "router"

[[rule]]
id = "cargo-crates"
prefix = "crates/"
kind = "derive-cargo"

[[rule]]
id = "npm-packages"
prefix = "packages/"
kind = "derive-npm"

[[rule]]
id = "terraform-modules"
prefix = "infra/modules/"
kind = "derive-terraform"

[[rule]]
id = "workspace-lockfile"
exact = "Cargo.lock"
kind = "repo-wide"

[[rule]]
id = "contract-bundle"
prefix = "api/generated/"
kind = "node"
node = "bundle:contract"

[[rule]]
id = "documentation"
suffix = ".md"
kind = "ignored"
"#;

    fn npm_dirs() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("packages/sdk".to_owned(), "@aexhq/sdk".to_owned()),
            (
                "packages/sdk-extra".to_owned(),
                "@aexhq/sdk-extra".to_owned(),
            ),
        ])
    }

    fn classify(path: &str) -> Classification {
        PathMap::parse(MAP).unwrap().classify(path, &npm_dirs())
    }

    #[test]
    fn a_crate_source_file_resolves_to_its_crate() {
        match classify("crates/aex-wire/src/lib.rs") {
            Classification::Owned { node, rule } => {
                assert_eq!(node.as_str(), "cargo:aex-wire");
                assert_eq!(rule, "cargo-crates");
            }
            other => panic!("expected an owned classification, got {other:?}"),
        }
    }

    #[test]
    fn a_sibling_directory_is_not_inside_a_shorter_prefix() {
        // The whole point of segment-wise matching: `packages/sdk` must not
        // claim `packages/sdk-extra`.
        let extra = classify("packages/sdk-extra/src/index.ts");
        let sdk = classify("packages/sdk/src/index.ts");
        match (extra, sdk) {
            (Classification::Owned { node: a, .. }, Classification::Owned { node: b, .. }) => {
                assert_eq!(a.as_str(), "npm:@aexhq/sdk-extra");
                assert_eq!(b.as_str(), "npm:@aexhq/sdk");
            }
            other => panic!("expected two owned classifications, got {other:?}"),
        }
    }

    #[test]
    fn terraform_modules_resolve_below_infra() {
        match classify("infra/modules/kms-key/main.tf") {
            Classification::Owned { node, .. } => {
                assert_eq!(node.as_str(), "tf:modules/kms-key");
            }
            other => panic!("expected an owned classification, got {other:?}"),
        }
    }

    #[test]
    fn the_lockfile_widens_the_run_to_the_repository() {
        assert!(matches!(
            classify("Cargo.lock"),
            Classification::RepoWide { .. }
        ));
    }

    #[test]
    fn a_router_change_is_classified_as_router() {
        assert!(matches!(
            classify("tools/aex-release-tool/src/graph/select.rs"),
            Classification::Router { .. }
        ));
        assert!(matches!(
            classify("release/units.toml"),
            Classification::Router { .. }
        ));
    }

    #[test]
    fn an_explicit_node_rule_resolves_to_that_node() {
        match classify("api/generated/bundle.lock.json") {
            Classification::Owned { node, .. } => assert_eq!(node.as_str(), "bundle:contract"),
            other => panic!("expected an owned classification, got {other:?}"),
        }
    }

    #[test]
    fn an_unmatched_path_is_an_orphan() {
        assert_eq!(classify("stray/thing.rs"), Classification::Orphan);
    }

    #[test]
    fn first_match_wins_so_the_router_beats_the_generic_documentation_rule() {
        assert!(matches!(
            classify("tools/aex-release-tool/README.md"),
            Classification::Router { .. }
        ));
        assert!(matches!(
            classify("SECURITY.md"),
            Classification::Ignored { .. }
        ));
    }

    #[test]
    fn a_file_directly_inside_a_derive_prefix_owns_no_member() {
        // `crates/README.md` names no crate; it must not resolve to a node
        // called `README.md`.
        assert!(matches!(
            classify("crates/README.md"),
            Classification::Ignored { .. }
        ));
    }

    #[test]
    fn a_rule_with_two_selectors_is_rejected() {
        let text = r#"
schema = "aex.path-map.v1"
[[rule]]
id = "bad"
prefix = "crates/"
exact = "Cargo.lock"
kind = "ignored"
"#;
        let err = PathMap::parse(text).unwrap_err();
        assert_eq!(err.rules(), vec!["path-map-rule-invalid"]);
    }

    #[test]
    fn a_node_rule_without_a_node_is_rejected() {
        let text = r#"
schema = "aex.path-map.v1"
[[rule]]
id = "bad"
prefix = "crates/"
kind = "node"
"#;
        let err = PathMap::parse(text).unwrap_err();
        assert_eq!(err.rules(), vec!["path-map-rule-invalid"]);
    }

    #[test]
    fn an_unknown_schema_is_rejected() {
        let err = PathMap::parse("schema = \"aex.path-map.v2\"").unwrap_err();
        assert_eq!(err.rules(), vec!["path-map-unparseable"]);
    }
}
