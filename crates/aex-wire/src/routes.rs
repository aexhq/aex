//! The route registry and a framework-free matcher.
//!
//! [`RouteId`] and [`ROUTES`] are generated from `api/openapi/**` and are the
//! same table indexed two ways, so they cannot drift. This crate deliberately
//! does not depend on `axum`: the HTTP composition crates bind descriptors to
//! handlers, and this one only says what the descriptors are.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
pub use crate::generated::routes::{ROUTES, RouteId};
use crate::idempotency::IdempotencyKind;
pub use crate::idempotency::PrincipalKind;
use crate::scopes::ScopeId;
use crate::types::HttpMethod;

/// Which plane serves a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plane {
    /// The single bootstrap host.
    Central,
    /// The workspace's pinned regional host.
    Regional,
}

/// How a request body is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyClass {
    /// No request body at all.
    None,
    /// A strict AEX JSON schema; unknown members are rejected.
    AexJson,
    /// The pinned standard OTLP revision, not an AEX schema.
    Otlp,
}

/// How a response is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// One response body.
    Unary,
    /// `application/x-ndjson` discriminated frames.
    Ndjson,
}

/// What a route does about entity tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EtagPolicy {
    /// Neither returns nor accepts one.
    None,
    /// Returns a strong entity tag.
    Returns,
    /// Accepts `If-Match`, and returns the new tag on success.
    OptionalIfMatch,
    /// Requires `If-Match`.
    RequiredIfMatch,
}

/// Everything the HTTP stack needs to know about one operation.
///
/// The middleware enforces the whole error-precedence order from this one table,
/// which is why a route cannot accidentally skip a stage: skipping would mean
/// omitting a field, and every field is generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteDescriptor {
    /// The route identity.
    pub id: RouteId,
    /// The globally unique `operationId`.
    pub operation_id: &'static str,
    /// Which plane serves it.
    pub plane: Plane,
    /// Which authoring fragment owns it.
    pub fragment: &'static str,
    /// HTTP method.
    pub method: HttpMethod,
    /// Path template, rooted at `/api`.
    pub template: &'static str,
    /// Path parameter names, in template order.
    pub path_params: &'static [&'static str],
    /// Query parameter names, sorted.
    pub query_params: &'static [&'static str],
    /// The scope the credential must carry.
    pub required_scope: Option<ScopeId>,
    /// An additional principal kind the route accepts.
    pub alt_principal: Option<PrincipalKind>,
    /// Which replay identity the route requires.
    pub idempotency: IdempotencyKind,
    /// How the request body is interpreted.
    pub body_class: BodyClass,
    /// How the response is delivered.
    pub transport: TransportKind,
    /// The success status.
    pub success_status: u16,
    /// What the route does about entity tags.
    pub etag: EtagPolicy,
    /// Every error code the route declares.
    pub errors: &'static [ErrorCode],
    /// The request body schema, when there is one.
    pub request_schema: Option<&'static str>,
    /// The success body schema, when there is one.
    pub response_schema: Option<&'static str>,
    /// Whether an identical retry is safe without a replay identity.
    pub safe_retry: bool,
    /// Whether the route runs while the account is paused.
    pub pause_exempt: bool,
}

impl RouteDescriptor {
    /// Whether the route declares `code`.
    #[must_use]
    pub fn declares(&self, code: ErrorCode) -> bool {
        self.errors.contains(&code)
    }
}

/// The descriptor for `id`.
///
/// `ROUTES` is generated in `RouteId` order, so this is an index rather than a
/// match arm and stays O(1) as the surface grows.
///
/// # Panics
///
/// Never: the generator emits exactly one descriptor per `RouteId`, and
/// `routes_table_is_indexed_by_route_id` asserts it.
#[must_use]
pub fn route(id: RouteId) -> &'static RouteDescriptor {
    let descriptor = &ROUTES[id as usize];
    debug_assert_eq!(descriptor.id as usize, id as usize);
    descriptor
}

/// The concrete values bound from a path template.
///
/// At most three parameters appear in any AEX template, so the binding is a
/// fixed-size array and matching never allocates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathBinding<'a> {
    /// How many slots are used.
    len: usize,
    /// `(name, value)` pairs, in template order.
    slots: [(&'a str, &'a str); 4],
}

impl<'a> PathBinding<'a> {
    /// The value bound to `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'a str> {
        self.slots[..self.len]
            .iter()
            .find(|(bound, _)| *bound == name)
            .map(|(_, value)| *value)
    }

    /// Every binding, in template order.
    #[must_use]
    pub fn as_slice(&self) -> &[(&'a str, &'a str)] {
        &self.slots[..self.len]
    }

    /// How many parameters were bound.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the template had no parameters.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Adds a binding, returning `None` when the template has too many.
    fn push(mut self, name: &'a str, value: &'a str) -> Option<Self> {
        *self.slots.get_mut(self.len)? = (name, value);
        self.len += 1;
        Some(self)
    }
}

/// Resolves `path` against the routes of `plane`.
///
/// Matching is exact and segment-wise: no prefix match, no trailing-slash
/// tolerance, no case folding. An old route therefore returns an ordinary 404
/// rather than accidentally aliasing a new one.
#[must_use]
pub fn match_route(
    plane: Plane,
    method: HttpMethod,
    path: &str,
) -> Option<(RouteId, PathBinding<'_>)> {
    for descriptor in ROUTES {
        if descriptor.plane != plane || descriptor.method != method {
            continue;
        }
        if let Some(binding) = bind_template(descriptor.template, path) {
            return Some((descriptor.id, binding));
        }
    }
    None
}

/// Binds one concrete path against one template.
fn bind_template<'a>(template: &'static str, path: &'a str) -> Option<PathBinding<'a>> {
    let mut binding = PathBinding::default();
    let mut expected = template.split('/');
    let mut actual = path.split('/');
    loop {
        match (expected.next(), actual.next()) {
            (None, None) => return Some(binding),
            (Some(pattern), Some(segment)) => {
                if let Some(name) = pattern
                    .strip_prefix('{')
                    .and_then(|rest| rest.strip_suffix('}'))
                {
                    if segment.is_empty() {
                        return None;
                    }
                    binding = binding.push(name, segment)?;
                } else if pattern != segment {
                    return None;
                }
            }
            _ => return None,
        }
    }
}

impl fmt::Display for RouteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
