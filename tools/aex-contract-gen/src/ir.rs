//! The intermediate representation every emitter reads.
//!
//! No emitter re-opens an input file. That is what makes "regenerate twice
//! produces the same bytes" a property of the design rather than a coincidence
//! of the filesystem.

use std::collections::BTreeMap;

/// Everything the generator knows about the contract.
#[derive(Debug, Clone)]
pub struct ContractIr {
    /// The single prelaunch contract line.
    pub contract_version: String,
    /// Pinned Rust toolchain channel, read from `rust-toolchain.toml`.
    pub toolchain: String,
    /// The identifier registry, in declared order.
    pub ids: Vec<IdRow>,
    /// The error registry, in declared order.
    pub errors: Vec<ErrorRow>,
    /// The scope registry, in declared order.
    pub scopes: Vec<ScopeRow>,
    /// The limit registry, in declared order.
    pub limits: Vec<LimitRow>,
    /// Every schema, keyed by its globally unique `SchemaId`.
    pub schemas: BTreeMap<String, SchemaIr>,
    /// Both planes, central first.
    pub planes: Vec<PlaneIr>,
    /// Every input file, workspace-relative with `/` separators, and its digest.
    pub sources: BTreeMap<String, String>,
    /// Declared evolution relaxations.
    pub evolution: EvolutionIr,
}

impl ContractIr {
    /// Every operation across both planes, in `RouteId` order.
    #[must_use]
    pub fn operations(&self) -> Vec<&OperationIr> {
        let mut all: Vec<&OperationIr> = self
            .planes
            .iter()
            .flat_map(|plane| plane.operations.iter())
            .collect();
        all.sort_by(|left, right| left.id.cmp(&right.id));
        all
    }
}

/// One row of the identifier registry.
#[derive(Debug, Clone)]
pub struct IdRow {
    /// `PascalCase` `IdKind` variant, for example `Session`.
    pub variant: String,
    /// `snake_case` registry key, for example `session`.
    pub key: String,
    /// Wire prefix without the underscore, for example `ses`.
    pub prefix: String,
    /// Rust newtype name, for example `SessionId`.
    pub rust: String,
    /// One-line documentation.
    pub doc: String,
}

/// One row of the error registry.
#[derive(Debug, Clone)]
pub struct ErrorRow {
    /// Wire code, `snake_case`.
    pub code: String,
    /// `PascalCase` Rust variant.
    pub variant: String,
    /// HTTP status the code renders at.
    pub status: u16,
    /// Whether an identical retry can succeed.
    pub retryable: bool,
    /// `snake_case` [`ErrorClass`](crate::ir::ErrorRow::class) key.
    pub class: String,
    /// `snake_case` precedence-stage key.
    pub stage: String,
    /// Default human message.
    pub message: String,
    /// Optional remediation hint.
    pub remedy: Option<String>,
}

/// One row of the scope registry.
#[derive(Debug, Clone)]
pub struct ScopeRow {
    /// Wire scope string, for example `sessions:write`.
    pub scope: String,
    /// `PascalCase` Rust variant.
    pub variant: String,
    /// One-line documentation.
    pub doc: String,
}

/// One row of the limit registry.
#[derive(Debug, Clone)]
pub struct LimitRow {
    /// Wire limit id, for example `query.page`.
    pub id: String,
    /// `PascalCase` Rust variant.
    pub variant: String,
    /// Whether the effective value is a scalar or a keyed map.
    pub shape: LimitShape,
    /// Complete ordered dimension vocabulary for a map limit.
    pub dimensions: Vec<String>,
    /// One-line documentation.
    pub doc: String,
}

/// The shape of a limit's effective value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitShape {
    /// A single non-negative integer.
    Scalar,
    /// A map from a named dimension to a non-negative integer.
    Map,
}

/// A named schema.
#[derive(Debug, Clone)]
pub struct SchemaIr {
    /// Globally unique `PascalCase` identifier.
    pub id: String,
    /// One-line documentation.
    pub doc: String,
    /// Which authored file declared it, workspace-relative.
    pub owner: String,
    /// Which generated Rust module it lands in.
    pub module: String,
    /// The shape.
    pub body: SchemaBody,
}

/// What a schema actually is.
#[derive(Debug, Clone)]
pub enum SchemaBody {
    /// A closed object; unknown members are rejected.
    Object {
        /// Members, in declared order.
        fields: Vec<FieldIr>,
    },
    /// A closed string enumeration.
    Enum {
        /// Values, in declared order.
        values: Vec<EnumValue>,
    },
    /// A discriminated union over object variants.
    Union {
        /// The discriminating member name, on the wire.
        tag: String,
        /// Variants, in declared order.
        variants: Vec<VariantIr>,
    },
}

/// One value of a string enumeration.
#[derive(Debug, Clone)]
pub struct EnumValue {
    /// The wire string.
    pub value: String,
    /// `PascalCase` Rust variant.
    pub variant: String,
    /// One-line documentation.
    pub doc: String,
}

/// One arm of a discriminated union.
#[derive(Debug, Clone)]
pub struct VariantIr {
    /// The discriminator value on the wire.
    pub value: String,
    /// `PascalCase` Rust variant.
    pub variant: String,
    /// The `SchemaId` carrying the arm's members.
    pub payload: String,
    /// One-line documentation.
    pub doc: String,
}

/// One member of an object schema.
#[derive(Debug, Clone)]
pub struct FieldIr {
    /// Wire member name, `camelCase`.
    pub wire: String,
    /// Rust field name, `snake_case`.
    pub rust: String,
    /// One-line documentation.
    pub doc: String,
    /// Whether the member may be absent.
    pub optional: bool,
    /// The member's type.
    pub ty: FieldType,
}

/// The total schema-to-Rust mapping. An unmapped construct is a load error, so
/// there is deliberately no `serde_json::Value` escape hatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldType {
    /// A bounded UTF-8 string.
    Text {
        /// Smallest accepted byte length.
        min: u32,
        /// Largest accepted byte length.
        max: u32,
        /// Optional anchored regular expression.
        pattern: Option<String>,
    },
    /// A prefixed resource identifier, named by its registry key.
    Id(String),
    /// An RFC 3339 UTC instant with exactly three fractional digits.
    Timestamp,
    /// A canonical non-negative decimal integer string.
    Decimal,
    /// A canonical non-negative whole-cent USD string.
    Cents,
    /// A bounded integer; both bounds are mandatory.
    Integer {
        /// Smallest accepted value.
        min: i64,
        /// Largest accepted value.
        max: i64,
    },
    /// A double, permitted only where `x-aex-float` is declared.
    Float,
    /// A boolean.
    Bool,
    /// Another schema.
    Ref(String),
    /// A homogeneous list.
    Array(Box<FieldType>, u32),
    /// A string-keyed map, with a mandatory bound on its entry count.
    ///
    /// The bound is mandatory for the same reason an array's is: an unbounded
    /// collection makes every response that echoes it unbounded, so a stored
    /// replay body's inline-or-digest threshold becomes a hot path rather than
    /// a rare one, and the 400 KB item ceiling becomes reachable.
    Map(Box<FieldType>, u32),
    /// An AWS region.
    Region,
    /// A compute shape token.
    ComputeSize,
    /// A `sha256:` content hash.
    ContentHash,
    /// A registered resource or secret name.
    ResourceName,
    /// A normalized absolute POSIX path.
    FilePath,
    /// A W3C 32-hex trace id.
    TraceId,
    /// A W3C 16-hex span id.
    SpanId,
    /// An absolute `https://` URL.
    HttpsUrl,
    /// A strong opaque entity tag.
    ETag,
    /// An opaque `cur_` continuation token.
    Cursor,
    /// An RFC 6901 pointer.
    JsonPointer,
    /// An opaque but canonical JSON document.
    CanonicalJson,
    /// An inclusive byte range.
    ByteRange,
    /// A direct BYOK provider.
    ProviderId,
    /// An authorization scope.
    Scope,
    /// A limit identifier.
    LimitId,
    /// A public error code.
    ErrorCode,
    /// A customer metadata scalar: string, number, boolean or null.
    MetadataValue,
}

impl FieldType {
    /// The `SchemaId` this type reaches directly, if any.
    #[must_use]
    pub fn referenced_schema(&self) -> Option<&str> {
        match self {
            Self::Ref(id) => Some(id),
            Self::Array(inner, _) | Self::Map(inner, _) => inner.referenced_schema(),
            _ => None,
        }
    }
}

/// One plane's assembled surface.
#[derive(Debug, Clone)]
pub struct PlaneIr {
    /// `central` or `regional`.
    pub id: String,
    /// `PascalCase` Rust variant.
    pub variant: String,
    /// Document title.
    pub title: String,
    /// Server URL template.
    pub server: String,
    /// Human description of the server template.
    pub server_description: String,
    /// The plane's default security scheme name.
    pub security_scheme: String,
    /// Fragment file stems, in the declared assembly order.
    pub fragment_order: Vec<String>,
    /// Operations, sorted by `operationId`.
    pub operations: Vec<OperationIr>,
}

/// One HTTP operation.
#[derive(Debug, Clone)]
pub struct OperationIr {
    /// Globally unique `snake_case` `operationId`.
    pub id: String,
    /// `PascalCase` `RouteId` variant.
    pub variant: String,
    /// Owning plane id.
    pub plane: String,
    /// Owning fragment stem.
    pub fragment: String,
    /// Release artifact planned to serve this operation.
    ///
    /// This is ownership and selection metadata, not evidence that the route
    /// is mounted by a runnable composition.
    pub serving_artifact: String,
    /// Release artifact that actually mounts this operation today.
    ///
    /// Absence is an intentional delivery gap. This is route-registry
    /// metadata, not part of the public wire bundle.
    pub served_artifact: Option<String>,
    /// Why the production composition intentionally does not mount this route.
    ///
    /// This is mutually exclusive with `served_artifact` and is delivery
    /// metadata rather than part of the public wire contract.
    pub deferred_reason: Option<String>,
    /// Release scenarios that exercise this operation.
    ///
    /// This is route-registry metadata, not part of the public wire bundle.
    pub scenarios: Vec<String>,
    /// HTTP method.
    pub method: String,
    /// Path template, rooted at `/api`.
    pub path: String,
    /// One-line summary.
    pub summary: String,
    /// Path parameters, in template order.
    pub path_params: Vec<ParamIr>,
    /// Query parameters, in declared order.
    pub query_params: Vec<ParamIr>,
    /// Required scope, if the route is scoped.
    pub scope: Option<String>,
    /// An additional principal kind the route accepts.
    pub alt_principal: Option<String>,
    /// Replay-identity requirement.
    pub idempotency: String,
    /// Request body schema.
    pub request: Option<String>,
    /// Success status code.
    pub success_status: u16,
    /// Success body schema; `None` means `204 No Content`.
    pub success: Option<String>,
    /// Declared error codes, sorted.
    pub errors: Vec<String>,
    /// How the request body is interpreted.
    pub body_class: String,
    /// How the response is delivered.
    pub transport: String,
    /// Entity-tag policy.
    pub etag: String,
    /// Whether an identical retry is safe without a replay identity.
    pub safe_retry: bool,
    /// Whether the route runs while the account is paused.
    pub pause_exempt: bool,
}

/// One path or query parameter.
#[derive(Debug, Clone)]
pub struct ParamIr {
    /// Wire name.
    pub name: String,
    /// Rust field name.
    pub rust: String,
    /// The parameter's type.
    pub ty: FieldType,
    /// Whether a query parameter may be omitted; path parameters are required.
    pub optional: bool,
    /// One-line documentation.
    pub doc: String,
}

/// The declared evolution relaxations.
#[derive(Debug, Clone, Default)]
pub struct EvolutionIr {
    /// Enums whose decoders accept an unrecognized value into a typed variant.
    pub open_enums: Vec<String>,
    /// Response fields that may be added without a breaking classification.
    pub additive_response_fields: Vec<(String, String)>,
    /// `surface` or `reject`; never `ignore`.
    pub unknown_frame_policy: String,
    /// Always `reject`; recorded so the classifier can assert it.
    pub request_unknown_fields: String,
}
