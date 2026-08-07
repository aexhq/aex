//! Startup configuration for a central `HTTP` composition.
//!
//! Nothing here has a default. A value that identifies a plane, a region or a
//! bound is supplied explicitly, because a defaulted one binds a process to the
//! wrong plane without saying so.

use std::time::Duration;

use aex_wire::dispatch::RequestLimits;
use aex_wire::routes::{Plane, RouteId};
use aex_wire::server::RouteGroup;
use aex_wire::types::Region;

/// Which deployment plane a process belongs to.
///
/// Distinct from [`Plane`], which says which *API* plane serves a route. A
/// central API can run on either deployment plane, and conflating the two is
/// how a dev process ends up answering production traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeploymentPlane {
    /// The development plane.
    Dev,
    /// The production plane.
    Prd,
}

impl DeploymentPlane {
    /// Every plane.
    pub const ALL: [Self; 2] = [Self::Dev, Self::Prd];

    /// The configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prd => "prd",
        }
    }

    /// Resolves a configuration spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }
}

/// Which central deployable a composition belongs to.
///
/// [`CentralServiceId::groups`] is the actual mounted group map. Planned
/// ownership remains in the generated route descriptor; an authored route may
/// therefore have a planned owner while deliberately belonging to no mounted
/// group. The generated delivery registry is the actual-mount authority and
/// the tests below prove this runtime map agrees with it exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CentralServiceId {
    /// `central-identity-api`: the two public device-flow routes.
    IdentityApi,
    /// `central-authz`: an authorizer and an internal command, no public route.
    Authz,
    /// `central-control-api`: organizations, workspaces, keys and operations.
    ControlApi,
    /// `finance-api`: billing only.
    FinanceApi,
    /// `finance-ingest`: queue-driven, no public route.
    FinanceIngest,
    /// `central-control-worker`: queue- and schedule-driven, no public route.
    ControlWorker,
}

impl CentralServiceId {
    /// Every central deployable.
    pub const ALL: [Self; 6] = [
        Self::IdentityApi,
        Self::Authz,
        Self::ControlApi,
        Self::FinanceApi,
        Self::FinanceIngest,
        Self::ControlWorker,
    ];

    /// The deployable id, matching `release/units.toml`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdentityApi => "central-identity-api",
            Self::Authz => "central-authz",
            Self::ControlApi => "central-control-api",
            Self::FinanceApi => "finance-api",
            Self::FinanceIngest => "finance-ingest",
            Self::ControlWorker => "central-control-worker",
        }
    }

    /// Resolves a deployable id.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.as_str() == text)
    }

    /// The route groups this deployable serves.
    #[must_use]
    pub const fn groups(self) -> &'static [RouteGroup] {
        match self {
            Self::IdentityApi => &[RouteGroup::Auth],
            Self::Authz | Self::FinanceIngest | Self::ControlWorker => &[],
            Self::ControlApi => &[
                RouteGroup::ApiKeys,
                RouteGroup::Bootstrap,
                RouteGroup::CentralOperations,
                RouteGroup::Organizations,
                RouteGroup::Workspaces,
            ],
            Self::FinanceApi => &[RouteGroup::Billing],
        }
    }

    /// Every route this deployable serves, in `RouteId` order.
    #[must_use]
    pub fn routes(self) -> Vec<RouteId> {
        let mut all: Vec<RouteId> = self
            .groups()
            .iter()
            .flat_map(|group| group.routes().iter().copied())
            .collect();
        all.sort_unstable();
        all
    }
}

/// Every route group the central plane serves.
#[must_use]
pub fn central_groups() -> Vec<RouteGroup> {
    RouteGroup::ALL
        .iter()
        .copied()
        .filter(|group| group.plane() == Plane::Central)
        .collect()
}

/// Why a composition refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CentralHttpConfigError {
    /// The deployment plane was not `dev` or `prd`.
    #[error("`{0}` is not a deployment plane")]
    UnknownPlane(String),
    /// The region was not a launch region.
    #[error("`{0}` is not a launch region")]
    UnknownRegion(String),
    /// The deployable id is not a central one.
    #[error("`{0}` is not a central deployable")]
    UnknownService(String),
    /// A bound was zero or above its ceiling.
    #[error("`{key}` is {found}, outside 1..={max}")]
    OutOfRange {
        /// Which bound.
        key: &'static str,
        /// What was supplied.
        found: u64,
        /// The largest admissible value.
        max: u64,
    },
}

/// The resolved configuration one central composition runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    /// Which deployment plane.
    pub plane: DeploymentPlane,
    /// Which region the process runs in.
    pub region: Region,
    /// Which deployable.
    pub service: CentralServiceId,
    /// The request body bound this composition enforces.
    pub limits: RequestLimits,
    /// How long one request may take before the edge gives up.
    pub request_deadline: Duration,
}

impl HttpConfig {
    /// The largest body a central route ever accepts.
    ///
    /// The central plane carries no OTLP payload, so one bound is enough and the
    /// OTLP bound is pinned to the same number rather than left larger: a
    /// composition that cannot receive OTLP must not advertise an OTLP ceiling.
    pub const MAX_JSON_BODY_BYTES: usize = 65_536;

    /// The longest admissible request deadline.
    pub const MAX_REQUEST_DEADLINE_MS: u64 = 30_000;

    /// Resolves a configuration from already-read strings.
    ///
    /// # Errors
    ///
    /// Returns [`CentralHttpConfigError`] naming the first value it refused. There is no
    /// partially valid configuration: a process either has every value or does
    /// not start.
    pub fn resolve(
        plane: &str,
        region: &str,
        service: &str,
        max_json_body_bytes: u64,
        request_deadline_ms: u64,
    ) -> Result<Self, CentralHttpConfigError> {
        let plane = DeploymentPlane::parse(plane)
            .ok_or_else(|| CentralHttpConfigError::UnknownPlane(plane.to_owned()))?;
        let region = Region::from_name(region)
            .ok_or_else(|| CentralHttpConfigError::UnknownRegion(region.to_owned()))?;
        let service = CentralServiceId::parse(service)
            .ok_or_else(|| CentralHttpConfigError::UnknownService(service.to_owned()))?;
        let max = u64::try_from(Self::MAX_JSON_BODY_BYTES).unwrap_or(u64::MAX);
        if max_json_body_bytes == 0 || max_json_body_bytes > max {
            return Err(CentralHttpConfigError::OutOfRange {
                key: "max_json_body_bytes",
                found: max_json_body_bytes,
                max,
            });
        }
        if request_deadline_ms == 0 || request_deadline_ms > Self::MAX_REQUEST_DEADLINE_MS {
            return Err(CentralHttpConfigError::OutOfRange {
                key: "request_deadline_ms",
                found: request_deadline_ms,
                max: Self::MAX_REQUEST_DEADLINE_MS,
            });
        }
        let bound = usize::try_from(max_json_body_bytes).unwrap_or(Self::MAX_JSON_BODY_BYTES);
        Ok(Self {
            plane,
            region,
            service,
            limits: RequestLimits {
                max_json_body_bytes: bound,
                max_otlp_body_bytes: bound,
            },
            request_deadline: Duration::from_millis(request_deadline_ms),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CentralHttpConfigError, CentralServiceId, DeploymentPlane, HttpConfig, central_groups,
    };
    use aex_wire::routes::{Plane, ROUTES, RouteId};
    use aex_wire::server::RouteGroup;
    use std::collections::BTreeSet;

    #[test]
    fn every_actually_served_central_route_has_exactly_one_runtime_owner() {
        let mut owned: Vec<RouteId> = CentralServiceId::ALL
            .iter()
            .flat_map(|service| service.routes())
            .collect();
        let count = owned.len();
        owned.sort_unstable();
        owned.dedup();
        assert_eq!(count, owned.len(), "a route is served by two deployables");

        let central: BTreeSet<RouteId> = ROUTES
            .iter()
            .filter(|route| route.plane == Plane::Central)
            .map(|route| route.id)
            .collect();
        let registry: serde_json::Value = serde_json::from_str(include_str!(
            "../../../api/generated/registries/routes.json"
        ))
        .expect("generated route registry");
        let actually_served: BTreeSet<RouteId> = registry["routes"]
            .as_array()
            .expect("route rows")
            .iter()
            .filter(|route| route["plane"] == "central" && route.get("servedArtifact").is_some())
            .map(|route| {
                RouteId::parse(route["operationId"].as_str().expect("operation id"))
                    .expect("generated operation id")
            })
            .collect();
        assert_eq!(
            owned.into_iter().collect::<BTreeSet<_>>(),
            actually_served,
            "runtime ownership must exactly equal generated actual mounts"
        );
        assert_eq!(central.len(), 27);
        assert_eq!(actually_served.len(), 26);
        assert!(central.contains(&RouteId::AccountGet));
        assert!(!actually_served.contains(&RouteId::AccountGet));
        assert_eq!(
            aex_wire::routes::route(RouteId::AccountGet).serving_artifact,
            "central-identity-api",
            "account retains its planned owner without being mounted"
        );
    }

    #[test]
    fn the_mounted_group_list_excludes_the_unserved_identity_fragment() {
        let assigned: BTreeSet<_> = CentralServiceId::ALL
            .iter()
            .flat_map(|service| service.groups().iter().copied())
            .collect();
        assert_eq!(
            assigned,
            central_groups()
                .into_iter()
                .filter(|group| *group != RouteGroup::Identity)
                .collect::<BTreeSet<_>>()
        );
        assert_eq!(assigned.len(), 7);
    }

    #[test]
    fn a_service_with_no_public_route_owns_no_group() {
        assert!(CentralServiceId::Authz.groups().is_empty());
        assert!(CentralServiceId::FinanceIngest.groups().is_empty());
        assert!(CentralServiceId::ControlWorker.groups().is_empty());
    }

    #[test]
    fn every_identity_round_trips_through_its_spelling() {
        for service in CentralServiceId::ALL {
            assert_eq!(CentralServiceId::parse(service.as_str()), Some(service));
        }
        for plane in DeploymentPlane::ALL {
            assert_eq!(DeploymentPlane::parse(plane.as_str()), Some(plane));
        }
        assert_eq!(CentralServiceId::parse("regional-session-api"), None);
        assert_eq!(DeploymentPlane::parse("staging"), None);
    }

    #[test]
    fn configuration_refuses_every_value_it_cannot_bind() {
        assert!(
            HttpConfig::resolve("dev", "eu-west-1", "central-control-api", 65_536, 5_000).is_ok()
        );
        assert_eq!(
            HttpConfig::resolve("staging", "eu-west-1", "central-control-api", 1, 1),
            Err(CentralHttpConfigError::UnknownPlane("staging".to_owned()))
        );
        assert_eq!(
            HttpConfig::resolve("dev", "mars-central-1", "central-control-api", 1, 1),
            Err(CentralHttpConfigError::UnknownRegion(
                "mars-central-1".to_owned()
            ))
        );
        assert_eq!(
            HttpConfig::resolve("dev", "eu-west-1", "brain-mux", 1, 1),
            Err(CentralHttpConfigError::UnknownService(
                "brain-mux".to_owned()
            ))
        );
        assert!(matches!(
            HttpConfig::resolve("dev", "eu-west-1", "central-authz", 0, 1),
            Err(CentralHttpConfigError::OutOfRange {
                key: "max_json_body_bytes",
                ..
            })
        ));
        assert!(matches!(
            HttpConfig::resolve("dev", "eu-west-1", "central-authz", 1, 60_000),
            Err(CentralHttpConfigError::OutOfRange {
                key: "request_deadline_ms",
                ..
            })
        ));
    }

    #[test]
    fn the_otlp_bound_never_exceeds_the_json_bound_on_a_plane_that_takes_no_telemetry() {
        let config = HttpConfig::resolve("prd", "eu-west-1", "central-control-api", 4_096, 5_000)
            .expect("a valid configuration");
        assert_eq!(config.limits.max_json_body_bytes, 4_096);
        assert_eq!(config.limits.max_otlp_body_bytes, 4_096);
    }
}
