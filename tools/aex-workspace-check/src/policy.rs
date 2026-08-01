//! The policy documents the registry reads.
//!
//! `release/policy/test-profiles.toml` and `release/policy/seams.toml` are
//! embedded rather than read from disk, so the checker validates the same
//! policy that shipped with it and a stream cannot loosen a profile by editing
//! a file the binary happens to find first.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// `release/policy/test-profiles.toml`, verbatim.
pub const TEST_PROFILES_TOML: &str = include_str!("../../../release/policy/test-profiles.toml");
/// `release/policy/seams.toml`, verbatim.
pub const SEAMS_TOML: &str = include_str!("../../../release/policy/seams.toml");
/// `release/policy/workload-registry.toml`, verbatim.
pub const WORKLOAD_REGISTRY_TOML: &str =
    include_str!("../../../release/policy/workload-registry.toml");
/// `release/policy/test-images.toml`, verbatim.
///
/// The checker reads it to derive the image references no source file outside
/// `aex-test-harness` may contain, so adding an image to the substrate bans its
/// literal everywhere else in the same change.
pub const TEST_IMAGES_TOML: &str = include_str!("../../../release/policy/test-images.toml");

/// The closed value sets of `[package.metadata.aex]`.
#[derive(Debug, Clone, Deserialize)]
pub struct Values {
    /// Permitted `owner` values.
    pub owner: Vec<String>,
    /// Permitted `role` values.
    pub role: Vec<String>,
    /// Permitted `artifact` values.
    pub artifact: Vec<String>,
    /// Permitted `layers` entries.
    pub layers: Vec<String>,
    /// Permitted `concerns` entries.
    pub concerns: Vec<String>,
    /// Permitted `security_tier` values.
    pub security_tier: Vec<String>,
    /// Permitted `risk` entries.
    pub risk: Vec<String>,
    /// Default TTL per lane, in minutes.
    pub lane_ttl_minutes: BTreeMap<String, i64>,
}

/// The minimum evidence a role owes.
#[derive(Debug, Clone, Deserialize)]
pub struct RoleProfile {
    /// Layers the role must declare.
    pub layers: Vec<String>,
    /// Concerns the role must declare.
    pub concerns: Vec<String>,
    /// Fields the role may never excuse with a `not_applicable` entry.
    #[serde(default)]
    pub forbid_not_applicable: Vec<String>,
    /// Why those fields are structurally true for this role.
    #[serde(default)]
    pub forbid_reason: Option<String>,
    /// The role's minimum evidence, for humans.
    pub evidence: String,
}

/// Where a concern's evidence must live.
#[derive(Debug, Clone, Deserialize)]
pub struct ConcernRule {
    /// The layer that carries the concern.
    pub requires_layer: String,
    /// The name the evidence conventionally lives under. It is quoted in the
    /// failure message so a stream is told where to put the case; the check
    /// itself is on the layer, because `Q-SELECTION` forbids a filename
    /// convention from deciding what a lane runs.
    #[serde(default)]
    pub conventional_target: Option<String>,
}

/// An Area 9 evidence class with a named owning package.
#[derive(Debug, Clone, Deserialize)]
pub struct EvidenceClass {
    /// The package that owes the class.
    pub owner_package: String,
    /// The concern the owner must declare to carry it.
    pub requires_concern: String,
    /// What the class actually asserts.
    pub detail: String,
}

/// One external seam.
#[derive(Debug, Clone, Deserialize)]
pub struct Seam {
    /// How much the local substrate establishes: `none`, `partial` or `full`.
    pub proves_locally: String,
    /// Whether a local substitute may never satisfy the receipt.
    pub requires_live: bool,
    /// What the local substrate covers.
    #[serde(default)]
    pub local: String,
    /// What only the real system can establish.
    #[serde(default)]
    pub live_only: String,
}

/// One blocking or diagnostic capacity gate and who owes it.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkloadGate {
    /// The owning stream.
    pub owner: String,
    /// The live package whose `load` target executes it.
    pub target: String,
    /// `budget` or `diagnostic`.
    pub kind: String,
    /// Whether a miss blocks.
    pub blocking: bool,
    /// The assertion.
    pub assert: String,
    /// Where the threshold comes from.
    pub source: String,
}

#[derive(Debug, Deserialize)]
struct ProfilesDocument {
    values: Values,
    role: BTreeMap<String, RoleProfile>,
    concern: BTreeMap<String, ConcernRule>,
    #[serde(default)]
    evidence_class: BTreeMap<String, EvidenceClass>,
}

#[derive(Debug, Deserialize)]
struct SeamsDocument {
    seam: BTreeMap<String, Seam>,
}

#[derive(Debug, Deserialize)]
struct WorkloadDocument {
    gate: BTreeMap<String, WorkloadGate>,
}

/// Everything the registry rules read from policy.
#[derive(Debug)]
pub struct Policy {
    /// The closed value sets.
    pub values: Values,
    /// Role to minimum evidence.
    pub roles: BTreeMap<String, RoleProfile>,
    /// Concern to the target that carries it.
    pub concerns: BTreeMap<String, ConcernRule>,
    /// Evidence classes with a named owner.
    pub evidence_classes: BTreeMap<String, EvidenceClass>,
    /// The closed seam registry.
    pub seams: BTreeMap<String, Seam>,
    /// The declared capacity gates.
    pub gates: BTreeMap<String, WorkloadGate>,
}

impl Policy {
    /// The embedded policy.
    ///
    /// # Panics
    ///
    /// Panics when an embedded document does not parse. The documents are
    /// compiled in, so that is a build-time defect and failing loudly beats
    /// checking a workspace against half a policy.
    #[must_use]
    pub fn embedded() -> &'static Self {
        static POLICY: OnceLock<Policy> = OnceLock::new();
        POLICY.get_or_init(|| {
            let profiles: ProfilesDocument = toml::from_str(TEST_PROFILES_TOML)
                .expect("release/policy/test-profiles.toml is embedded and must parse");
            let seams: SeamsDocument = toml::from_str(SEAMS_TOML)
                .expect("release/policy/seams.toml is embedded and must parse");
            let workloads: WorkloadDocument = toml::from_str(WORKLOAD_REGISTRY_TOML)
                .expect("release/policy/workload-registry.toml is embedded and must parse");
            Policy {
                values: profiles.values,
                roles: profiles.role,
                concerns: profiles.concern,
                evidence_classes: profiles.evidence_class,
                seams: seams.seam,
                gates: workloads.gate,
            }
        })
    }

    /// The profile for `role`, if the role exists.
    #[must_use]
    pub fn role(&self, role: &str) -> Option<&RoleProfile> {
        self.roles.get(role)
    }

    /// The permitted values for a named field, for a failure message.
    #[must_use]
    pub fn permitted(&self, field: &str) -> Vec<String> {
        match field {
            "owner" => self.values.owner.clone(),
            "role" => self.values.role.clone(),
            "artifact" => self.values.artifact.clone(),
            "layers" => self.values.layers.clone(),
            "concerns" => self.values.concerns.clone(),
            "security_tier" => self.values.security_tier.clone(),
            "risk" => self.values.risk.clone(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Policy;

    #[test]
    fn every_declared_role_has_a_profile_and_every_profile_has_a_role() {
        let policy = Policy::embedded();
        for role in &policy.values.role {
            assert!(
                policy.roles.contains_key(role),
                "role `{role}` is a permitted value with no profile"
            );
        }
        for role in policy.roles.keys() {
            assert!(
                policy.values.role.contains(role),
                "profile `{role}` is not a permitted role value"
            );
        }
    }

    #[test]
    fn every_declared_concern_has_a_rule_and_every_rule_has_a_concern() {
        let policy = Policy::embedded();
        for concern in &policy.values.concerns {
            assert!(
                policy.concerns.contains_key(concern),
                "concern `{concern}` has no rule"
            );
        }
        for concern in policy.concerns.keys() {
            assert!(
                policy.values.concerns.contains(concern),
                "rule `{concern}` is not a permitted concern"
            );
        }
    }

    #[test]
    fn every_profile_only_names_permitted_layers_and_concerns() {
        let policy = Policy::embedded();
        for (role, profile) in &policy.roles {
            for layer in &profile.layers {
                assert!(policy.values.layers.contains(layer), "{role}: {layer}");
            }
            for concern in &profile.concerns {
                assert!(
                    policy.values.concerns.contains(concern),
                    "{role}: {concern}"
                );
            }
            for field in &profile.forbid_not_applicable {
                assert!(
                    profile.forbid_reason.is_some(),
                    "role `{role}` forbids excusing `{field}` without saying why"
                );
            }
        }
    }

    #[test]
    fn every_concern_rule_points_at_a_permitted_layer() {
        let policy = Policy::embedded();
        for (concern, rule) in &policy.concerns {
            assert!(
                policy.values.layers.contains(&rule.requires_layer),
                "{concern}: {}",
                rule.requires_layer
            );
        }
    }

    #[test]
    fn the_seam_registry_is_the_declared_launch_set_and_every_row_needs_live_evidence() {
        let policy = Policy::embedded();
        assert_eq!(policy.seams.len(), 34, "the launch seam set is 34 rows");
        for (id, seam) in &policy.seams {
            assert!(seam.requires_live, "`{id}` claims no live evidence is owed");
            assert!(
                matches!(seam.proves_locally.as_str(), "none" | "partial" | "full"),
                "`{id}` declares proves_locally `{}`",
                seam.proves_locally
            );
            if seam.proves_locally == "none" {
                assert!(
                    seam.local.is_empty(),
                    "`{id}` proves nothing locally yet lists local coverage"
                );
            } else {
                assert!(
                    !seam.local.is_empty(),
                    "`{id}` claims partial local coverage without saying what"
                );
            }
            assert!(
                !seam.live_only.is_empty(),
                "`{id}` names nothing that only the real system proves"
            );
        }
        assert!(
            !policy.seams.keys().any(|id| id.contains("clickhouse")),
            "Area 11 removes ClickHouse; Q-DEPENDENCY's pinned-ClickHouse clause is void"
        );
    }

    #[test]
    fn the_six_direct_provider_seams_are_declared_and_no_gateway_seam_is() {
        let policy = Policy::embedded();
        for provider in [
            "openai",
            "anthropic",
            "deepseek",
            "zai",
            "moonshotai",
            "google",
        ] {
            assert!(
                policy
                    .seams
                    .contains_key(&format!("provider.{provider}.stream")),
                "provider `{provider}` has no seam"
            );
        }
        assert!(!policy.seams.contains_key("provider.gateway.stream"));
        assert!(!policy.seams.contains_key("provider.openrouter.stream"));
    }

    #[test]
    fn every_capacity_gate_names_an_owner_a_live_target_and_a_threshold_source() {
        let policy = Policy::embedded();
        assert!(!policy.gates.is_empty());
        for (id, gate) in &policy.gates {
            assert!(
                policy.values.owner.contains(&gate.owner),
                "{id}: {}",
                gate.owner
            );
            assert!(
                gate.target.starts_with("aex-live-"),
                "{id}: {}",
                gate.target
            );
            assert!(
                matches!(gate.kind.as_str(), "budget" | "diagnostic"),
                "{id} declares kind `{}`; a prelaunch gate is never an SLO",
                gate.kind
            );
            assert!(
                !gate.source.trim().is_empty(),
                "{id} has no threshold source"
            );
            assert!(!gate.assert.trim().is_empty(), "{id} asserts nothing");
        }
    }

    #[test]
    fn every_lane_declares_a_ttl() {
        let policy = Policy::embedded();
        for lane in [
            "unit",
            "integration",
            "smoke",
            "e2e",
            "user",
            "load",
            "soak",
        ] {
            assert!(
                policy.values.lane_ttl_minutes.contains_key(lane),
                "lane `{lane}` has no TTL"
            );
        }
    }

    #[test]
    fn the_permitted_set_is_reported_for_every_closed_field() {
        let policy = Policy::embedded();
        for field in [
            "owner",
            "role",
            "artifact",
            "layers",
            "concerns",
            "security_tier",
            "risk",
        ] {
            assert!(!policy.permitted(field).is_empty(), "{field}");
        }
        assert!(
            policy.permitted("tier").is_empty(),
            "an unknown field has no permitted set"
        );
    }
}
