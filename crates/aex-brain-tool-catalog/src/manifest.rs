//! Immutable tool manifest types and their structural validation.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use aex_wire::CanonicalJson;
use aex_wire::ids::{ContentHash, ResourceName};

use crate::wire_pending::{DurableOperationSupport, EffectClass, ExecutorRoute};

/// A provider-visible tool name.
///
/// The grammar is the intersection of the six launch providers' function-name
/// grammars. Construction is deliberately private so an unvalidated name
/// cannot enter a catalog.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ToolName(Box<str>);

impl ToolName {
    /// Largest provider-visible name in bytes.
    pub const MAX_BYTES: usize = 64;

    /// Parses a built-in or registered-custom name.
    ///
    /// # Errors
    ///
    /// Returns a typed grammar failure. The `mcp__` prefix is reserved for
    /// [`Self::namespaced_mcp`].
    pub fn parse(value: &str) -> Result<Self, ToolNameError> {
        Self::parse_inner(value, false)
    }

    /// Constructs `mcp__<server>__<remote>` without truncation.
    ///
    /// # Errors
    ///
    /// Returns a typed grammar or length failure for the composed name.
    pub fn namespaced_mcp(server: &str, remote: &str) -> Result<Self, ToolNameError> {
        let value = format!("mcp__{server}__{remote}");
        Self::parse_inner(&value, true)
    }

    /// The provider-visible spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse_inner(value: &str, allow_mcp_prefix: bool) -> Result<Self, ToolNameError> {
        if value.is_empty() {
            return Err(ToolNameError::Empty);
        }
        if value.len() > Self::MAX_BYTES {
            return Err(ToolNameError::TooLong { bytes: value.len() });
        }
        if !allow_mcp_prefix && value.starts_with("mcp__") {
            return Err(ToolNameError::ReservedPrefix);
        }
        if let Some((at, byte)) = value
            .bytes()
            .enumerate()
            .find(|(_, byte)| !byte.is_ascii_alphanumeric() && *byte != b'_' && *byte != b'-')
        {
            return Err(ToolNameError::IllegalByte { at, byte });
        }
        Ok(Self(value.into()))
    }
}

impl fmt::Debug for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("ToolName").field(&self.0).finish()
    }
}

impl fmt::Display for ToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ToolName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let value = Box::<str>::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

/// Why a tool name was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolNameError {
    /// The name had no bytes.
    #[error("tool name must not be empty")]
    Empty,
    /// The name exceeded the cross-provider ceiling.
    #[error("tool name is {bytes} bytes; the maximum is 64")]
    TooLong {
        /// Observed UTF-8 byte length.
        bytes: usize,
    },
    /// A byte was outside the ASCII intersection grammar.
    #[error("tool name byte {byte:#04x} at offset {at} is not allowed")]
    IllegalByte {
        /// Zero-based byte offset.
        at: usize,
        /// Rejected byte.
        byte: u8,
    },
    /// A non-MCP source tried to claim the MCP namespace.
    #[error("the `mcp__` prefix is reserved for MCP-derived tool names")]
    ReservedPrefix,
}

/// A catalog document failed the stricter integer-only canonicalization rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogCanonicalError {
    /// A floating-point JSON number was present.
    #[error("catalog documents permit integers only; found a float at `{pointer}`")]
    FloatingPoint {
        /// RFC 6901 pointer to the number.
        pointer: String,
    },
    /// The workspace JCS encoder rejected the document.
    #[error(transparent)]
    Canonical(#[from] aex_wire::canonical::CanonicalError),
    /// Serde could not represent the value as JSON.
    #[error("catalog value is not JSON: {reason}")]
    NotJson {
        /// Serde's diagnostic.
        reason: String,
    },
}

/// Applies the catalog's integer-only guard and then delegates byte ordering
/// and rendering to the workspace's sole JCS implementation.
///
/// # Errors
///
/// Returns [`CatalogCanonicalError::FloatingPoint`] before canonicalization if
/// any float is present, including inside an embedded schema.
pub fn canonical_catalog_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CatalogCanonicalError> {
    let json = serde_json::to_value(value).map_err(|error| CatalogCanonicalError::NotJson {
        reason: error.to_string(),
    })?;
    reject_floats(&json, "")?;
    Ok(aex_wire::canonical::to_jcs_bytes(&json)?)
}

fn reject_floats(value: &Value, pointer: &str) -> Result<(), CatalogCanonicalError> {
    match value {
        Value::Number(number) if number.is_f64() => Err(CatalogCanonicalError::FloatingPoint {
            pointer: pointer.to_owned(),
        }),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                reject_floats(item, &format!("{pointer}/{index}"))?;
            }
            Ok(())
        }
        Value::Object(members) => {
            for (key, member) in members {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                reject_floats(member, &format!("{pointer}/{escaped}"))?;
            }
            Ok(())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

/// Where a tool effect executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolBoundary {
    /// Fold-local control tools.
    BrainControl,
    /// Child-agent scheduling tools.
    Subagent,
    /// Brain-owned fetch and search.
    ManagedWeb,
    /// Trusted platform persistence of a selected sandbox file.
    PlatformStorage,
    /// A registered MCP server.
    Mcp,
    /// Guest filesystem operations.
    HandsFilesystem,
    /// Guest development operations.
    HandsDevelopment,
    /// Guest browser operations.
    HandsBrowser,
    /// A registered custom guest tool.
    RegisteredCustom,
}

/// The proof used to recover an interrupted tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryClass {
    /// Recompute from the committed canonical argument hash.
    RecomputeFromInputHash,
    /// Query the already-created durable operation.
    QueryDurableOperation,
    /// Rebuild a result from a committed checksummed receipt.
    ReconstructFromReceipt,
    /// Stop the run if dispatch may have happened.
    InterruptOnAmbiguity,
}

impl RecoveryClass {
    /// Whether this recovery class needs a durable operation query.
    #[must_use]
    pub const fn durable_operation_support(self) -> DurableOperationSupport {
        match self {
            Self::QueryDurableOperation => DurableOperationSupport::Query,
            Self::RecomputeFromInputHash
            | Self::ReconstructFromReceipt
            | Self::InterruptOnAmbiguity => DurableOperationSupport::None,
        }
    }
}

/// How approval policy applies to a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// The tool never creates a policy approval.
    Never,
    /// The session's exact-name policy decides.
    WhenPolicyRequires,
    /// Approval is mandatory for every call.
    Always,
}

/// Hard resource bounds attached to one tool descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBounds {
    /// Largest canonical argument document.
    pub max_input_bytes: u32,
    /// Largest stored executor result.
    pub max_result_bytes: u32,
    /// Largest result admitted into the next model context.
    pub max_context_bytes: u32,
    /// Largest transport frame.
    pub max_frame_bytes: u32,
    /// Attached wall-clock ceiling.
    pub timeout_ms: u32,
    /// Detached wall-clock ceiling; zero means not detachable.
    pub max_detached_ms: u32,
    /// Permit units charged to the tool lane.
    pub concurrency_weight: u16,
}

/// A set over the four launch usage dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageDimensionSet(u8);

impl UsageDimensionSet {
    /// No Brain-side usage fact.
    pub const EMPTY: Self = Self(0);
    /// `compute.millicpu_ms.v1`.
    pub const COMPUTE: Self = Self(1 << 0);
    /// `memory.byte_ms.v1`.
    pub const MEMORY: Self = Self(1 << 1);
    /// `storage.byte_min.v1`.
    pub const STORAGE: Self = Self(1 << 2);
    /// `data_transfer.egress_byte.v1`.
    pub const DATA_TRANSFER: Self = Self(1 << 3);

    /// The empty set.
    #[must_use]
    pub const fn empty() -> Self {
        Self::EMPTY
    }

    /// Set union.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit in `other` is present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl Serialize for UsageDimensionSet {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;
        let mut sequence = serializer.serialize_seq(None)?;
        for (dimension, bit) in [
            ("compute.millicpu_ms.v1", Self::COMPUTE),
            ("memory.byte_ms.v1", Self::MEMORY),
            ("storage.byte_min.v1", Self::STORAGE),
            ("data_transfer.egress_byte.v1", Self::DATA_TRANSFER),
        ] {
            if self.contains(bit) {
                sequence.serialize_element(dimension)?;
            }
        }
        sequence.end()
    }
}

impl<'de> Deserialize<'de> for UsageDimensionSet {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let dimensions = Vec::<Box<str>>::deserialize(deserializer)?;
        let mut set = Self::empty();
        for dimension in dimensions {
            let bit = match dimension.as_ref() {
                "compute.millicpu_ms.v1" => Self::COMPUTE,
                "memory.byte_ms.v1" => Self::MEMORY,
                "storage.byte_min.v1" => Self::STORAGE,
                "data_transfer.egress_byte.v1" => Self::DATA_TRANSFER,
                unknown => {
                    return Err(D::Error::custom(format!(
                        "unknown usage dimension `{unknown}`"
                    )));
                }
            };
            if set.contains(bit) {
                return Err(D::Error::custom(format!(
                    "duplicate usage dimension `{dimension}`"
                )));
            }
            set = set.union(bit);
        }
        Ok(set)
    }
}

/// Which network boundary, if any, a tool crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressClass {
    /// No Internet boundary.
    None,
    /// Brain-managed Internet access.
    ManagedInternet,
    /// A remote MCP endpoint.
    McpRemote,
    /// Internet access inside the guest boundary.
    GuestInternet,
}

/// Credential material required before advertisement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialClass {
    /// No credential.
    None,
    /// A resolved workspace secret with this exact resource name.
    WorkspaceSecret {
        /// Required secret name.
        name: ResourceName,
    },
    /// The fenced token for a Hands endpoint.
    HandsEndpointToken,
}

/// Determinism promised by the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Determinism {
    /// Same committed inputs produce byte-identical output.
    Deterministic,
    /// Repeated calls settle the same managed effect.
    Idempotent,
    /// External state may change the result.
    Nondeterministic,
}

/// One pinned executor capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequirement {
    /// Stable capability key.
    pub key: Box<str>,
    /// Oldest admitted revision.
    pub min_revision: u32,
}

/// An argument-selected effect variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolVariant {
    /// The `background` value selecting this row.
    pub background: bool,
    /// Effect class committed before dispatch.
    pub effect: EffectClass,
    /// Recovery proof committed before dispatch.
    pub recovery: RecoveryClass,
    /// Detached ceiling for this row.
    pub max_detached_ms: u32,
}

/// The complete immutable behavior contract for one tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    /// Provider-visible name.
    pub name: ToolName,
    /// Human display title.
    pub title: Box<str>,
    /// Model-visible description.
    pub description: Box<str>,
    /// JSON Schema 2020-12 argument contract.
    pub input_schema: CanonicalJson,
    /// JSON Schema 2020-12 result contract.
    pub result_schema: CanonicalJson,
    /// Security/ownership boundary.
    pub boundary: ToolBoundary,
    /// Concrete executor route.
    pub route: ExecutorRoute,
    /// Default effect class.
    pub effect: EffectClass,
    /// Default recovery proof.
    pub recovery: RecoveryClass,
    /// Approval behavior.
    pub approval: ApprovalPolicy,
    /// Resource ceilings.
    pub bounds: ToolBounds,
    /// Brain-side usage dimensions.
    pub usage: UsageDimensionSet,
    /// Network boundary.
    pub egress: EgressClass,
    /// Credential prerequisite.
    pub credential: CredentialClass,
    /// Determinism promise.
    pub determinism: Determinism,
    /// Argument-selected rows, empty for every tool except `run_command`.
    pub variants: Vec<ToolVariant>,
}

/// Why an entry is retained but unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    /// The qualified name exceeded the provider bound.
    NameTooLong,
    /// The name collided with another pinned entry.
    NameCollision,
    /// A schema failed the structural profile.
    InvalidSchema,
    /// An MCP header annotation was unsafe.
    InvalidMcpHeaderAnnotation,
    /// The executor did not declare a required capability.
    UnsupportedCapability,
    /// The remote server was not conformant.
    ServerNonConformant,
}

/// Registration state retained in the signed revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EntryState {
    /// Advertisable when readiness also succeeds.
    Active,
    /// Retained for audit but never routed.
    Excluded {
        /// Exact exclusion reason.
        reason: ExclusionReason,
    },
}

/// One signed manifest row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolManifestEntry {
    /// Tool behavior contract.
    pub descriptor: ToolDescriptor,
    /// Exact executor artifact identity.
    pub implementation_digest: ContentHash,
    /// Capabilities that must be ready.
    pub required_capabilities: Vec<CapabilityRequirement>,
    /// Registration state.
    pub state: EntryState,
}

/// Validates one entry against V2-V11.
///
/// # Errors
///
/// Returns the first structural violation in stable invariant order.
pub fn validate_entry(entry: &ToolManifestEntry) -> Result<(), CatalogViolation> {
    let descriptor = &entry.descriptor;
    if !route_matches(descriptor.boundary, descriptor.route) {
        return Err(CatalogViolation::RouteBoundaryMismatch {
            boundary: descriptor.boundary,
            route: descriptor.route,
        });
    }
    if !coherent_recovery(descriptor.effect, descriptor.recovery) {
        return Err(CatalogViolation::IncoherentRecovery);
    }
    if descriptor.bounds.concurrency_weight == 0
        && descriptor.effect != EffectClass::Pure
        && descriptor.route != ExecutorRoute::Park
        && !(descriptor.route == ExecutorRoute::SubagentScheduler
            && descriptor.effect == EffectClass::IdempotentManaged)
    {
        return Err(CatalogViolation::ZeroWeightExternalEffect);
    }
    if descriptor.bounds.max_context_bytes > descriptor.bounds.max_result_bytes
        || descriptor.bounds.max_context_bytes > 65_536
    {
        return Err(CatalogViolation::ContextBound {
            context: descriptor.bounds.max_context_bytes,
            result: descriptor.bounds.max_result_bytes,
        });
    }
    let has_transfer = descriptor.usage.contains(UsageDimensionSet::DATA_TRANSFER);
    if (descriptor.egress == EgressClass::None) == has_transfer {
        return Err(CatalogViolation::DataTransferMeterMismatch);
    }
    if is_hands(descriptor.boundary) && !descriptor.usage.is_empty() {
        return Err(CatalogViolation::HandsMeterMustBeEmpty);
    }
    if descriptor.approval == ApprovalPolicy::Always && descriptor.effect == EffectClass::Pure {
        return Err(CatalogViolation::PureToolCannotAlwaysRequireApproval);
    }
    validate_schema("input", &descriptor.input_schema)?;
    validate_schema("result", &descriptor.result_schema)?;
    if descriptor.determinism == Determinism::Deterministic
        && descriptor.effect != EffectClass::Pure
    {
        return Err(CatalogViolation::DeterministicExternalEffect);
    }
    if descriptor.title.len() > 128 || descriptor.description.len() > 4_096 {
        return Err(CatalogViolation::DisplayTextBound);
    }
    Ok(())
}

const fn route_matches(boundary: ToolBoundary, route: ExecutorRoute) -> bool {
    matches!(
        (boundary, route),
        (
            ToolBoundary::BrainControl,
            ExecutorRoute::Control | ExecutorRoute::Park
        ) | (
            ToolBoundary::Subagent,
            ExecutorRoute::SubagentScheduler | ExecutorRoute::Park
        ) | (ToolBoundary::ManagedWeb, ExecutorRoute::ManagedWeb)
            | (
                ToolBoundary::PlatformStorage,
                ExecutorRoute::PlatformStorage
            )
            | (ToolBoundary::Mcp, ExecutorRoute::Mcp)
            | (
                ToolBoundary::HandsFilesystem,
                ExecutorRoute::HandsFilesystem
            )
            | (
                ToolBoundary::HandsDevelopment,
                ExecutorRoute::HandsDevelopment
            )
            | (ToolBoundary::HandsBrowser, ExecutorRoute::HandsBrowser)
            | (
                ToolBoundary::RegisteredCustom,
                ExecutorRoute::RegisteredCustom
            )
    )
}

const fn coherent_recovery(effect: EffectClass, recovery: RecoveryClass) -> bool {
    matches!(
        (effect, recovery),
        (
            EffectClass::Pure,
            RecoveryClass::RecomputeFromInputHash | RecoveryClass::ReconstructFromReceipt
        ) | (
            EffectClass::IdempotentManaged | EffectClass::DurableDetached,
            RecoveryClass::QueryDurableOperation
        ) | (
            EffectClass::NonReplayable,
            RecoveryClass::InterruptOnAmbiguity
        )
    )
}

const fn is_hands(boundary: ToolBoundary) -> bool {
    matches!(
        boundary,
        ToolBoundary::HandsFilesystem
            | ToolBoundary::HandsDevelopment
            | ToolBoundary::HandsBrowser
            | ToolBoundary::RegisteredCustom
    )
}

fn validate_schema(side: &'static str, schema: &CanonicalJson) -> Result<(), CatalogViolation> {
    let value = schema.to_value();
    if let Err(error) = reject_floats(&value, "") {
        return Err(CatalogViolation::InvalidSchema {
            side,
            reason: error.to_string(),
        });
    }
    let Some(root) = value.as_object() else {
        return Err(CatalogViolation::InvalidSchema {
            side,
            reason: "schema root is not an object".to_owned(),
        });
    };
    if root.get("type").and_then(Value::as_str) != Some("object")
        || root.get("additionalProperties").and_then(Value::as_bool) != Some(false)
    {
        return Err(CatalogViolation::InvalidSchema {
            side,
            reason: "schema root must be type object with additionalProperties false".to_owned(),
        });
    }
    let mut properties = 0_usize;
    inspect_schema(&value, 0, &mut properties)
        .map_err(|reason| CatalogViolation::InvalidSchema { side, reason })?;
    jsonschema::validator_for(&value).map_err(|error| CatalogViolation::InvalidSchema {
        side,
        reason: error.to_string(),
    })?;
    Ok(())
}

fn inspect_schema(value: &Value, depth: usize, properties: &mut usize) -> Result<(), String> {
    if depth > 8 {
        return Err("schema nesting exceeds 8".to_owned());
    }
    match value {
        Value::Object(object) => {
            for forbidden in [
                "$dynamicRef",
                "$dynamicAnchor",
                "unevaluatedProperties",
                "$comment",
            ] {
                if object.contains_key(forbidden) {
                    return Err(format!("schema keyword `{forbidden}` is forbidden"));
                }
            }
            if let Some(reference) = object.get("$ref").and_then(Value::as_str)
                && !reference.starts_with("#/")
            {
                return Err("external $ref is forbidden".to_owned());
            }
            if let Some(member_properties) = object.get("properties").and_then(Value::as_object) {
                *properties += member_properties.len();
                if *properties > 64 {
                    return Err("schema has more than 64 properties".to_owned());
                }
            }
            for member in object.values() {
                inspect_schema(member, depth + 1, properties)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                inspect_schema(item, depth + 1, properties)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

/// One stable catalog structural violation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogViolation {
    /// Boundary and executor ownership disagree.
    #[error("executor route {route:?} is invalid for boundary {boundary:?}")]
    RouteBoundaryMismatch {
        /// Declared boundary.
        boundary: ToolBoundary,
        /// Declared route.
        route: ExecutorRoute,
    },
    /// The effect has no admissible recovery proof.
    #[error("effect and recovery class are incoherent")]
    IncoherentRecovery,
    /// A zero-weight effect would perform external work.
    #[error("zero concurrency weight is reserved for pure tools and durable parks")]
    ZeroWeightExternalEffect,
    /// Context bytes exceeded their two ceilings.
    #[error("context bound {context} exceeds result bound {result} or 65536")]
    ContextBound {
        /// Model-context bound.
        context: u32,
        /// Stored-result bound.
        result: u32,
    },
    /// The egress class and transfer meter disagree.
    #[error("data-transfer metering must exactly match a Brain egress boundary")]
    DataTransferMeterMismatch,
    /// Hands descriptors would double-bill runtime facts.
    #[error("Hands descriptors must declare an empty Brain usage meter set")]
    HandsMeterMustBeEmpty,
    /// A pure tool cannot force an approval effect.
    #[error("a pure tool cannot require approval on every call")]
    PureToolCannotAlwaysRequireApproval,
    /// An embedded schema failed the bounded profile.
    #[error("invalid {side} schema: {reason}")]
    InvalidSchema {
        /// Which schema failed.
        side: &'static str,
        /// Stable diagnostic.
        reason: String,
    },
    /// A deterministic declaration covered an external effect.
    #[error("only pure tools may be deterministic")]
    DeterministicExternalEffect,
    /// Display-only text exceeded its bound.
    #[error("tool title or description exceeds its display bound")]
    DisplayTextBound,
}
