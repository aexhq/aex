//! Fail-closed executor readiness and provider advertisement.

use std::collections::{BTreeMap, BTreeSet};

use aex_wire::ids::{ContentHash, ResourceName};

use crate::manifest::{
    CapabilityRequirement, CredentialClass, EntryState, ToolManifestEntry, ToolName,
    canonical_catalog_bytes,
};
use crate::wire_pending::ExecutorRoute;

/// Which compiled built-ins a session requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinSelection {
    /// Every built-in whose optional authority is ready.
    Default,
    /// No built-ins.
    None,
    /// An exact, fail-closed list.
    Exact(Vec<ToolName>),
}

/// Number of ready executors claiming each canonical route.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecutorRegistry(BTreeMap<ExecutorRoute, u16>);

impl ExecutorRegistry {
    /// Declares one ready executor for every supplied route.
    #[must_use]
    pub fn new(routes: impl IntoIterator<Item = ExecutorRoute>) -> Self {
        let mut registry = Self::default();
        for route in routes {
            registry.declare_ready(route);
        }
        registry
    }

    /// Adds one ready executor claim.
    pub fn declare_ready(&mut self, route: ExecutorRoute) {
        let count = self.0.entry(route).or_default();
        *count = count.saturating_add(1);
    }

    /// Removes every ready claim for a route.
    pub fn remove(&mut self, route: ExecutorRoute) {
        self.0.remove(&route);
    }

    fn count(&self, route: ExecutorRoute) -> u16 {
        self.0.get(&route).copied().unwrap_or(0)
    }
}

/// Qualified executor capabilities by stable key and revision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySet(BTreeMap<Box<str>, u32>);

impl CapabilitySet {
    /// Builds a capability set.
    #[must_use]
    pub fn new<'a>(capabilities: impl IntoIterator<Item = (&'a str, u32)>) -> Self {
        Self(
            capabilities
                .into_iter()
                .map(|(key, revision)| (Box::<str>::from(key), revision))
                .collect(),
        )
    }

    fn satisfies(&self, requirement: &CapabilityRequirement) -> bool {
        self.0
            .get(requirement.key.as_ref())
            .is_some_and(|revision| *revision >= requirement.min_revision)
    }
}

/// Workspace secret names that resolved during session setup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedSecretNames(BTreeSet<ResourceName>);

impl ResolvedSecretNames {
    /// Parses the exact resolved-name set.
    ///
    /// # Errors
    ///
    /// Returns a resource-name grammar error; invalid config never becomes an
    /// absent credential.
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self, ReadinessFailure> {
        let mut parsed = BTreeSet::new();
        for name in names {
            parsed.insert(
                ResourceName::parse(name)
                    .map_err(|error| ReadinessFailure::InvalidSecretName(error.to_string()))?,
            );
        }
        Ok(Self(parsed))
    }

    fn contains(&self, name: &ResourceName) -> bool {
        self.0.contains(name)
    }
}

/// Inputs fixed before the first model dispatch.
#[derive(Clone, Copy)]
pub struct ReadinessInput<'a> {
    /// Pinned compiled and registered entries.
    pub entries: &'a [ToolManifestEntry],
    /// Executors linked into this composition.
    pub executors: &'a ExecutorRegistry,
    /// Qualified guest/runtime capabilities.
    pub capabilities: &'a CapabilitySet,
    /// Successfully resolved workspace secret names.
    pub secrets: &'a ResolvedSecretNames,
    /// Built-in selection from the resolved config.
    pub selection: &'a BuiltinSelection,
    /// Exact names protected by session policy.
    pub approval_required: &'a [&'a str],
}

/// The byte-reproducible provider-facing catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedCatalog {
    /// Sorted active entries with exactly one ready route.
    pub entries: Vec<ToolManifestEntry>,
    /// SHA-256 over the canonical advertised entries.
    pub digest: ContentHash,
}

impl AdvertisedCatalog {
    /// Whether an exact name was advertised.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries
            .binary_search_by(|entry| entry.descriptor.name.as_str().cmp(name))
            .is_ok()
    }
}

/// Produces an advertised catalog only when every included route is unique and
/// ready.
///
/// Optional authorities are honest capability discovery: under
/// [`BuiltinSelection::Default`], an unqualified browser capability or an
/// unresolved [`CredentialClass::WorkspaceSecret`] excludes those entries. An
/// exact selection turns the same absence into a typed error.
///
/// No built-in carries a workspace secret. Platform tools are platform-paid, so
/// the credential arm applies only to entries registered with one; a built-in
/// may not be withheld from a tenant for want of a customer key.
///
/// # Errors
///
/// Fails closed on duplicates, unknown names, missing or ambiguous executors,
/// explicitly selected unavailable tools, and approval policy names that would
/// protect nothing.
pub fn advertise(input: ReadinessInput<'_>) -> Result<AdvertisedCatalog, ReadinessFailure> {
    let mut names = BTreeSet::new();
    for entry in input.entries {
        if !names.insert(entry.descriptor.name.as_str()) {
            return Err(ReadinessFailure::DuplicateName {
                name: entry.descriptor.name.as_str().to_owned(),
            });
        }
    }

    let selected = match input.selection {
        BuiltinSelection::Default => None,
        BuiltinSelection::None => Some(BTreeSet::new()),
        BuiltinSelection::Exact(requested) => {
            let mut exact = BTreeSet::new();
            for name in requested {
                if !names.contains(name.as_str()) {
                    return Err(ReadinessFailure::UnknownSelection {
                        name: name.as_str().to_owned(),
                    });
                }
                exact.insert(name.as_str());
            }
            Some(exact)
        }
    };

    let mut advertised = Vec::new();
    for entry in input.entries {
        if !matches!(entry.state, EntryState::Active)
            || selected
                .as_ref()
                .is_some_and(|selection| !selection.contains(entry.descriptor.name.as_str()))
        {
            continue;
        }
        if let Some(requirement) = entry
            .required_capabilities
            .iter()
            .find(|requirement| !input.capabilities.satisfies(requirement))
        {
            if matches!(input.selection, BuiltinSelection::Exact(_)) {
                return Err(ReadinessFailure::MissingCapability {
                    name: entry.descriptor.name.as_str().to_owned(),
                    key: requirement.key.to_string(),
                    min_revision: requirement.min_revision,
                });
            }
            continue;
        }
        if let CredentialClass::WorkspaceSecret { name } = &entry.descriptor.credential
            && !input.secrets.contains(name)
        {
            if matches!(input.selection, BuiltinSelection::Exact(_)) {
                return Err(ReadinessFailure::MissingCredential {
                    name: entry.descriptor.name.as_str().to_owned(),
                    secret: name.as_str().to_owned(),
                });
            }
            continue;
        }
        match input.executors.count(entry.descriptor.route) {
            0 => {
                return Err(ReadinessFailure::NoReadyExecutor {
                    name: entry.descriptor.name.as_str().to_owned(),
                    route: entry.descriptor.route,
                });
            }
            1 => advertised.push(entry.clone()),
            candidates => {
                return Err(ReadinessFailure::AmbiguousRoute {
                    name: entry.descriptor.name.as_str().to_owned(),
                    route: entry.descriptor.route,
                    candidates,
                });
            }
        }
    }
    advertised.sort_by(|left, right| left.descriptor.name.cmp(&right.descriptor.name));

    let unknown_approval = input
        .approval_required
        .iter()
        .filter(|name| {
            advertised
                .binary_search_by(|entry| entry.descriptor.name.as_str().cmp(name))
                .is_err()
        })
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    if !unknown_approval.is_empty() {
        return Err(ReadinessFailure::ApprovalPolicyNamesUnknown {
            names: unknown_approval,
        });
    }

    let bytes = canonical_catalog_bytes(&advertised)
        .map_err(|error| ReadinessFailure::Canonical(error.to_string()))?;
    Ok(AdvertisedCatalog {
        entries: advertised,
        digest: ContentHash::of(&bytes),
    })
}

/// A session tool surface could not be composed honestly.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadinessFailure {
    /// Two pinned entries claimed one provider-visible name.
    #[error("duplicate tool name `{name}`")]
    DuplicateName {
        /// Colliding name.
        name: String,
    },
    /// No executor was ready for a selected route.
    #[error("tool `{name}` has no ready executor for {route:?}")]
    NoReadyExecutor {
        /// Tool name.
        name: String,
        /// Canonical route.
        route: ExecutorRoute,
    },
    /// More than one executor claimed a selected route.
    #[error("tool `{name}` has {candidates} ready executors for {route:?}")]
    AmbiguousRoute {
        /// Tool name.
        name: String,
        /// Canonical route.
        route: ExecutorRoute,
        /// Ready claim count.
        candidates: u16,
    },
    /// An exact selection named no pinned entry.
    #[error("unknown selected tool `{name}`")]
    UnknownSelection {
        /// Unknown name.
        name: String,
    },
    /// An explicitly selected capability was unqualified.
    #[error("tool `{name}` requires capability `{key}` revision {min_revision}")]
    MissingCapability {
        /// Tool name.
        name: String,
        /// Capability key.
        key: String,
        /// Minimum revision.
        min_revision: u32,
    },
    /// An explicitly selected workspace credential did not resolve.
    #[error("tool `{name}` requires workspace secret `{secret}`")]
    MissingCredential {
        /// Tool name.
        name: String,
        /// Secret resource name.
        secret: String,
    },
    /// Approval policy contained names outside the advertised surface.
    #[error("approval policy names unadvertised tools: {names:?}")]
    ApprovalPolicyNamesUnknown {
        /// Unknown names.
        names: Vec<String>,
    },
    /// Config contained an invalid resource name.
    #[error("invalid resolved secret name: {0}")]
    InvalidSecretName(String),
    /// Canonical advertisement hashing failed.
    #[error("advertised catalog is not canonical: {0}")]
    Canonical(String),
}
