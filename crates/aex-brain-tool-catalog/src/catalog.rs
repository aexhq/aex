//! The compiled-in launch catalog.

use aex_wire::CanonicalJson;
use aex_wire::ids::ContentHash;
use serde_json::{Map, Value, json};

use crate::manifest::{
    ApprovalPolicy, CapabilityRequirement, CatalogCanonicalError, CatalogViolation,
    CredentialClass, Determinism, EgressClass, EntryState, RecoveryClass, ToolBoundary, ToolBounds,
    ToolDescriptor, ToolManifestEntry, ToolName, ToolNameError, ToolVariant, UsageDimensionSet,
    canonical_catalog_bytes, validate_entry,
};
use crate::wire_pending::{EffectClass, ExecutorRoute};

/// Snapshot-bound SHA-256 identity of [`builtin_catalog_bytes`].
pub const BUILTIN_CATALOG_DIGEST: &str =
    "sha256:6d54069dd0c66c7ec2dc7b5f80399b19bb1514bcb4673589bad2631e578c3dd2";

/// Builds and validates the immutable built-in rows in canonical name order.
///
/// # Errors
///
/// Returns a build error if a compiled name, schema, credential, or invariant
/// is invalid. Such an error is a build/startup defect, never a runtime skip.
pub fn builtin_entries() -> Result<Vec<ToolManifestEntry>, CatalogBuildError> {
    let mut entries = SPECS
        .iter()
        .map(build_entry)
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|left, right| left.descriptor.name.cmp(&right.descriptor.name));
    for pair in entries.windows(2) {
        if pair[0].descriptor.name == pair[1].descriptor.name {
            return Err(CatalogBuildError::DuplicateName(
                pair[0].descriptor.name.as_str().to_owned(),
            ));
        }
    }
    Ok(entries)
}

/// Canonical bytes snapshot-bound into the Brain artifact.
///
/// # Errors
///
/// Returns an error if the compiled catalog violates integer-only JCS.
pub fn builtin_catalog_bytes() -> Result<Vec<u8>, CatalogBuildError> {
    canonical_catalog_bytes(&builtin_entries()?).map_err(CatalogBuildError::Canonical)
}

/// Validates model arguments against the pinned input schema and byte bound.
///
/// # Errors
///
/// Returns [`ArgumentError::TooLarge`] before schema work when the canonical
/// document exceeds the descriptor bound, otherwise a bounded schema error.
pub fn validate_arguments(
    entry: &ToolManifestEntry,
    arguments: &Value,
) -> Result<CanonicalJson, ArgumentError> {
    let canonical = CanonicalJson::from_value(arguments)
        .map_err(|error| ArgumentError::Malformed(error.to_string()))?;
    let observed = canonical.as_bytes().len();
    let limit = entry.descriptor.bounds.max_input_bytes as usize;
    if observed > limit {
        return Err(ArgumentError::TooLarge { observed, limit });
    }
    let schema = entry.descriptor.input_schema.to_value();
    let validator = jsonschema::validator_for(&schema).map_err(|error| ArgumentError::Schema {
        reason: error.to_string(),
    })?;
    validator
        .validate(arguments)
        .map_err(|error| ArgumentError::Schema {
            reason: error.to_string(),
        })?;
    Ok(canonical)
}

/// Selects the effect/recovery row from already-validated arguments.
///
/// # Errors
///
/// Returns [`ArgumentError::Variant`] if a variant-bearing descriptor has no
/// row for the validated selector. The selection happens before dispatch.
pub fn select_effect(
    entry: &ToolManifestEntry,
    arguments: &CanonicalJson,
) -> Result<(EffectClass, RecoveryClass, u32), ArgumentError> {
    if entry.descriptor.variants.is_empty() {
        return Ok((
            entry.descriptor.effect,
            entry.descriptor.recovery,
            entry.descriptor.bounds.max_detached_ms,
        ));
    }
    let background = arguments
        .to_value()
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    entry
        .descriptor
        .variants
        .iter()
        .find(|variant| variant.background == background)
        .map(|variant| (variant.effect, variant.recovery, variant.max_detached_ms))
        .ok_or(ArgumentError::Variant)
}

fn build_entry(spec: &Spec) -> Result<ToolManifestEntry, CatalogBuildError> {
    // Platform tools are platform-paid: BYOK is an LLM-provider arrangement and
    // never a tool one, so no built-in requires a customer credential. Managed
    // web keeps `CredentialClass::None`, the same class `web_fetch` carries, and
    // the platform supplies the search key out of band. Requiring a workspace
    // secret here made `web_search` unadvertisable to every tenant, because
    // readiness drops an entry whose named secret did not resolve and nothing in
    // the workspace resolves one.
    let credential = if is_hands(spec.boundary) {
        CredentialClass::HandsEndpointToken
    } else {
        CredentialClass::None
    };
    let required_capabilities = if spec.boundary == ToolBoundary::HandsBrowser {
        vec![CapabilityRequirement {
            key: "hands.browser".into(),
            min_revision: 1,
        }]
    } else {
        Vec::new()
    };
    let variants = if spec.name == "run_command" {
        vec![
            ToolVariant {
                background: false,
                effect: EffectClass::NonReplayable,
                recovery: RecoveryClass::InterruptOnAmbiguity,
                max_detached_ms: 0,
            },
            ToolVariant {
                background: true,
                effect: EffectClass::DurableDetached,
                recovery: RecoveryClass::QueryDurableOperation,
                max_detached_ms: 43_200_000,
            },
        ]
    } else {
        Vec::new()
    };
    let name = ToolName::parse(spec.name)?;
    let entry = ToolManifestEntry {
        descriptor: ToolDescriptor {
            title: title(spec.name).into(),
            description: description(spec.name).into(),
            input_schema: schema_for(spec.name, SchemaSide::Input)?,
            result_schema: schema_for(spec.name, SchemaSide::Result)?,
            name,
            boundary: spec.boundary,
            route: spec.route,
            effect: spec.effect,
            recovery: spec.recovery,
            approval: spec.approval,
            bounds: spec.bounds,
            usage: spec.usage,
            egress: spec.egress,
            credential,
            determinism: determinism(spec.effect),
            variants,
        },
        implementation_digest: ContentHash::of(format!("aex.builtin.{}.v1", spec.name).as_bytes()),
        required_capabilities,
        state: EntryState::Active,
    };
    validate_entry(&entry)?;
    Ok(entry)
}

const fn determinism(effect: EffectClass) -> Determinism {
    match effect {
        EffectClass::Pure => Determinism::Deterministic,
        EffectClass::IdempotentManaged | EffectClass::DurableDetached => Determinism::Idempotent,
        EffectClass::NonReplayable => Determinism::Nondeterministic,
    }
}

const fn is_hands(boundary: ToolBoundary) -> bool {
    matches!(
        boundary,
        ToolBoundary::HandsFilesystem | ToolBoundary::HandsDevelopment | ToolBoundary::HandsBrowser
    )
}

fn title(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn description(name: &str) -> String {
    format!("Execute the canonical `{name}` tool under its pinned bounds and recovery policy.")
}

#[derive(Clone, Copy)]
struct Spec {
    name: &'static str,
    boundary: ToolBoundary,
    route: ExecutorRoute,
    effect: EffectClass,
    recovery: RecoveryClass,
    approval: ApprovalPolicy,
    bounds: ToolBounds,
    usage: UsageDimensionSet,
    egress: EgressClass,
}

const C: UsageDimensionSet = UsageDimensionSet::COMPUTE;
const CM: UsageDimensionSet = C.union(UsageDimensionSet::MEMORY);
const CMT: UsageDimensionSet = CM.union(UsageDimensionSet::DATA_TRANSFER);
const NONE: UsageDimensionSet = UsageDimensionSet::EMPTY;

const fn bounds(input: u32, result: u32, context: u32, timeout: u32, weight: u16) -> ToolBounds {
    ToolBounds {
        max_input_bytes: input,
        max_result_bytes: result,
        max_context_bytes: context,
        max_frame_bytes: 65_536,
        timeout_ms: timeout,
        max_detached_ms: 0,
        concurrency_weight: weight,
    }
}

macro_rules! spec {
    ($name:literal, $boundary:ident, $route:ident, $effect:ident, $recovery:ident,
     $approval:ident, $input:expr, $result:expr, $context:expr, $timeout:expr, $weight:expr,
     $usage:expr, $egress:ident) => {
        Spec {
            name: $name,
            boundary: ToolBoundary::$boundary,
            route: ExecutorRoute::$route,
            effect: EffectClass::$effect,
            recovery: RecoveryClass::$recovery,
            approval: ApprovalPolicy::$approval,
            bounds: bounds($input, $result, $context, $timeout, $weight),
            usage: $usage,
            egress: EgressClass::$egress,
        }
    };
}

const SPECS: &[Spec] = &[
    spec!(
        "todo_read",
        BrainControl,
        Control,
        Pure,
        RecomputeFromInputHash,
        Never,
        1_024,
        65_536,
        65_536,
        50,
        0,
        C,
        None
    ),
    spec!(
        "todo_write",
        BrainControl,
        Control,
        Pure,
        RecomputeFromInputHash,
        Never,
        65_536,
        8_192,
        8_192,
        50,
        0,
        C,
        None
    ),
    spec!(
        "wait",
        BrainControl,
        Park,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        256,
        512,
        512,
        0,
        0,
        NONE,
        None
    ),
    spec!(
        "request_approval",
        BrainControl,
        Park,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        16_384,
        4_096,
        4_096,
        0,
        0,
        C,
        None
    ),
    spec!(
        "submit_result",
        BrainControl,
        Control,
        Pure,
        RecomputeFromInputHash,
        Never,
        262_144,
        262_144,
        8_192,
        200,
        0,
        CM,
        None
    ),
    spec!(
        "create_subagent",
        Subagent,
        SubagentScheduler,
        IdempotentManaged,
        QueryDurableOperation,
        WhenPolicyRequires,
        262_144,
        4_096,
        4_096,
        1_000,
        0,
        C,
        None
    ),
    spec!(
        "get_subagent",
        Subagent,
        SubagentScheduler,
        Pure,
        RecomputeFromInputHash,
        Never,
        512,
        16_384,
        16_384,
        1_000,
        1,
        C,
        None
    ),
    spec!(
        "list_subagents",
        Subagent,
        SubagentScheduler,
        Pure,
        RecomputeFromInputHash,
        Never,
        1_024,
        65_536,
        65_536,
        2_000,
        1,
        C,
        None
    ),
    spec!(
        "read_subagent_history",
        Subagent,
        SubagentScheduler,
        Pure,
        RecomputeFromInputHash,
        Never,
        1_024,
        262_144,
        65_536,
        5_000,
        2,
        CM,
        None
    ),
    spec!(
        "send_subagent_message",
        Subagent,
        SubagentScheduler,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        262_144,
        1_024,
        1_024,
        1_000,
        1,
        C,
        None
    ),
    spec!(
        "dequeue_subagent",
        Subagent,
        SubagentScheduler,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        512,
        1_024,
        1_024,
        1_000,
        1,
        C,
        None
    ),
    spec!(
        "stop_subagent",
        Subagent,
        SubagentScheduler,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        512,
        1_024,
        1_024,
        1_000,
        1,
        C,
        None
    ),
    spec!(
        "wait_subagents",
        Subagent,
        Park,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        4_096,
        65_536,
        65_536,
        0,
        0,
        NONE,
        None
    ),
    spec!(
        "web_fetch",
        ManagedWeb,
        ManagedWeb,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        8_448,
        500_000,
        65_536,
        30_000,
        4,
        CMT,
        ManagedInternet
    ),
    spec!(
        "web_search",
        ManagedWeb,
        ManagedWeb,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        4_096,
        262_144,
        65_536,
        15_000,
        4,
        CMT,
        ManagedInternet
    ),
    spec!(
        "read_file",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        Never,
        2_048,
        1_000_000,
        65_536,
        60_000,
        2,
        NONE,
        None
    ),
    spec!(
        "write_file",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        1_048_576,
        4_096,
        4_096,
        60_000,
        2,
        NONE,
        None
    ),
    spec!(
        "edit_file",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        262_144,
        8_192,
        8_192,
        60_000,
        2,
        NONE,
        None
    ),
    spec!(
        "apply_patch",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        1_048_576,
        16_384,
        16_384,
        60_000,
        2,
        NONE,
        None
    ),
    spec!(
        "list_dir",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        Never,
        1_024,
        262_144,
        65_536,
        30_000,
        2,
        NONE,
        None
    ),
    spec!(
        "glob",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        Never,
        2_048,
        262_144,
        65_536,
        60_000,
        2,
        NONE,
        None
    ),
    spec!(
        "grep",
        HandsFilesystem,
        HandsFilesystem,
        NonReplayable,
        InterruptOnAmbiguity,
        Never,
        4_096,
        262_144,
        65_536,
        120_000,
        3,
        NONE,
        None
    ),
    spec!(
        "run_command",
        HandsDevelopment,
        HandsDevelopment,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        262_144,
        1_000_000,
        65_536,
        600_000,
        4,
        NONE,
        None
    ),
    spec!(
        "run_code",
        HandsDevelopment,
        HandsDevelopment,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        1_048_576,
        1_000_000,
        65_536,
        600_000,
        4,
        NONE,
        None
    ),
    spec!(
        "git",
        HandsDevelopment,
        HandsDevelopment,
        NonReplayable,
        InterruptOnAmbiguity,
        WhenPolicyRequires,
        65_536,
        1_000_000,
        65_536,
        1_800_000,
        4,
        NONE,
        None
    ),
    spec!(
        "install_packages",
        HandsDevelopment,
        HandsDevelopment,
        NonReplayable,
        InterruptOnAmbiguity,
        Always,
        65_536,
        1_000_000,
        65_536,
        1_800_000,
        4,
        NONE,
        None
    ),
    spec!(
        "process_status",
        HandsDevelopment,
        HandsDevelopment,
        Pure,
        RecomputeFromInputHash,
        Never,
        512,
        4_096,
        4_096,
        15_000,
        1,
        NONE,
        None
    ),
    spec!(
        "process_output",
        HandsDevelopment,
        HandsDevelopment,
        Pure,
        ReconstructFromReceipt,
        Never,
        1_024,
        1_000_000,
        65_536,
        30_000,
        2,
        NONE,
        None
    ),
    spec!(
        "process_stop",
        HandsDevelopment,
        HandsDevelopment,
        IdempotentManaged,
        QueryDurableOperation,
        Never,
        512,
        1_024,
        1_024,
        15_000,
        1,
        NONE,
        None
    ),
    spec!(
        "browser_launch",
        HandsBrowser,
        HandsBrowser,
        DurableDetached,
        QueryDurableOperation,
        Always,
        8_192,
        8_192,
        8_192,
        120_000,
        6,
        NONE,
        None
    ),
    spec!(
        "browser_control",
        HandsBrowser,
        HandsBrowser,
        DurableDetached,
        QueryDurableOperation,
        WhenPolicyRequires,
        65_536,
        262_144,
        65_536,
        120_000,
        4,
        NONE,
        None
    ),
    spec!(
        "browser_status",
        HandsBrowser,
        HandsBrowser,
        Pure,
        RecomputeFromInputHash,
        Never,
        512,
        8_192,
        8_192,
        15_000,
        1,
        NONE,
        None
    ),
    spec!(
        "browser_result",
        HandsBrowser,
        HandsBrowser,
        Pure,
        ReconstructFromReceipt,
        Never,
        1_024,
        4_000_000,
        65_536,
        60_000,
        2,
        NONE,
        None
    ),
];

#[derive(Clone, Copy)]
enum SchemaSide {
    Input,
    Result,
}

fn schema_for(name: &str, side: SchemaSide) -> Result<CanonicalJson, CatalogBuildError> {
    let value = match side {
        SchemaSide::Input => input_schema(name),
        SchemaSide::Result => result_schema(name),
    };
    CanonicalJson::from_value(&value).map_err(|error| CatalogBuildError::Schema(error.to_string()))
}

#[allow(
    clippy::too_many_lines,
    reason = "the launch schema inventory is one cohesive closed union and is snapshot-bound"
)]
fn input_schema(name: &str) -> Value {
    match name {
        "todo_read" => object(&[], vec![]),
        "todo_write" => object(
            &["todos"],
            vec![("todos", array(todo(), Some(0), Some(200)))],
        ),
        "wait" => object(
            &["seconds"],
            vec![
                ("seconds", integer(Some(1), Some(86_400))),
                ("reason", text(0, 256)),
            ],
        ),
        "request_approval" => object(
            &["summary"],
            vec![
                ("summary", text(1, 2_048)),
                ("detail", text(0, 12_288)),
                ("risk", enumeration(&["low", "medium", "high"])),
            ],
        ),
        "submit_result" => object(
            &["status", "summary"],
            vec![
                ("status", enumeration(&["success", "failure"])),
                ("summary", text(1, 8_192)),
                ("data", json!({"type":"object"})),
                ("files", array(text(0, 4_096), None, Some(256))),
            ],
        ),
        "create_subagent" => create_subagent_input(),
        "get_subagent" | "dequeue_subagent" => agent_id_input(false),
        "stop_subagent" => agent_id_input(true),
        "list_subagents" => object(
            &[],
            vec![
                ("state", array(enumeration(&STATES), None, Some(7))),
                ("limit", integer(Some(1), Some(200))),
                ("cursor", text(0, 2_048)),
            ],
        ),
        "read_subagent_history" => object(
            &["agentId"],
            vec![
                ("agentId", pattern(AGENT_ID)),
                ("cursor", text(0, 2_048)),
                ("limit", integer(Some(1), Some(200))),
                (
                    "include",
                    array(
                        enumeration(&["user", "assistant", "tool_result", "control"]),
                        None,
                        Some(4),
                    ),
                ),
            ],
        ),
        "send_subagent_message" => object(
            &["agentId", "text"],
            vec![("agentId", pattern(AGENT_ID)), ("text", text(1, 200_000))],
        ),
        "wait_subagents" => object(
            &["agentIds", "mode"],
            vec![
                ("agentIds", array_with_unique(pattern(AGENT_ID), 1, 512)),
                ("mode", enumeration(&["any", "all"])),
                ("timeoutSeconds", integer(Some(1), Some(86_400))),
            ],
        ),
        "web_fetch" => object(
            &["url"],
            vec![
                (
                    "url",
                    json!({"type":"string","format":"uri","minLength":8,"maxLength":8192,"pattern":"^https://"}),
                ),
                ("format", enumeration(&["markdown", "text", "raw"])),
                ("maxBytes", integer(Some(1_024), Some(500_000))),
            ],
        ),
        "web_search" => object(
            &["query"],
            vec![
                ("query", text(1, 2_000)),
                ("count", integer(Some(1), Some(20))),
                ("country", json!({"type":"string","pattern":"^[A-Z]{2}$"})),
                ("freshness", enumeration(&["day", "week", "month", "year"])),
            ],
        ),
        "read_file" => object(
            &["path"],
            vec![
                ("path", text(1, 4_096)),
                ("offsetBytes", integer(Some(0), None)),
                ("maxBytes", integer(Some(1), Some(1_000_000))),
                ("startLine", integer(Some(1), None)),
                ("endLine", integer(Some(1), None)),
            ],
        ),
        "write_file" => object(
            &["path", "content"],
            vec![
                ("path", text(1, 4_096)),
                ("content", text(0, 1_000_000)),
                ("expectedRevision", pattern(HASH)),
                ("create", json!({"type":"boolean"})),
                ("mode", pattern("^0[0-7]{3}$")),
            ],
        ),
        "edit_file" => object(
            &["path", "expectedRevision", "oldString", "newString"],
            vec![
                ("path", text(1, 4_096)),
                ("expectedRevision", pattern(HASH)),
                ("oldString", text(1, 200_000)),
                ("newString", text(0, 200_000)),
                ("replaceAll", json!({"type":"boolean"})),
            ],
        ),
        "apply_patch" => object(
            &["patch"],
            vec![
                ("patch", text(1, 1_000_000)),
                (
                    "expected",
                    array(
                        object(
                            &["path", "revision"],
                            vec![("path", text(0, 4_096)), ("revision", pattern(HASH))],
                        ),
                        None,
                        Some(64),
                    ),
                ),
                ("strip", integer(Some(0), Some(8))),
            ],
        ),
        "list_dir" => object(
            &[],
            vec![
                ("path", text(0, 4_096)),
                ("recursive", json!({"type":"boolean"})),
                ("depth", integer(Some(1), Some(10))),
                ("includeHidden", json!({"type":"boolean"})),
                ("limit", integer(Some(1), Some(2_000))),
            ],
        ),
        "glob" => object(
            &["pattern"],
            vec![
                ("pattern", text(1, 1_024)),
                ("path", text(0, 4_096)),
                ("limit", integer(Some(1), Some(2_000))),
            ],
        ),
        "grep" => object(
            &["pattern"],
            vec![
                ("pattern", text(1, 4_096)),
                ("path", text(0, 4_096)),
                ("glob", text(0, 1_024)),
                ("ignoreCase", json!({"type":"boolean"})),
                ("contextLines", integer(Some(0), Some(10))),
                ("limit", integer(Some(1), Some(2_000))),
            ],
        ),
        "run_command" => run_command_input(),
        "run_code" => object(
            &["language", "code"],
            vec![
                ("language", enumeration(&["python", "javascript", "bash"])),
                ("code", text(1, 1_000_000)),
                ("cwd", text(0, 4_096)),
                ("timeoutMs", integer(Some(1_000), Some(600_000))),
            ],
        ),
        "git" => object(
            &["args"],
            vec![
                ("args", array(text(0, 32_768), Some(1), Some(256))),
                ("cwd", text(0, 4_096)),
                ("timeoutMs", integer(Some(1_000), Some(1_800_000))),
            ],
        ),
        "install_packages" => object(
            &["ecosystem", "packages"],
            vec![
                ("ecosystem", enumeration(&["apt", "pip", "npm"])),
                (
                    "packages",
                    array(
                        object(
                            &["name"],
                            vec![("name", text(1, 256)), ("version", text(0, 128))],
                        ),
                        Some(1),
                        Some(64),
                    ),
                ),
                ("timeoutMs", integer(Some(1_000), Some(1_800_000))),
            ],
        ),
        "process_status" | "browser_status" => operation_id_input(false),
        "process_output" => object(
            &["operationId"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("fromOffset", decimal_string()),
                ("maxBytes", integer(Some(1), Some(1_000_000))),
            ],
        ),
        "process_stop" => object(
            &["operationId"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("signal", enumeration(&["term", "kill"])),
            ],
        ),
        "browser_launch" => object(
            &[],
            vec![
                (
                    "url",
                    json!({"type":"string","format":"uri","maxLength":8192,"pattern":"^https?://"}),
                ),
                (
                    "viewport",
                    object(
                        &[],
                        vec![
                            ("width", integer(Some(320), Some(3_840))),
                            ("height", integer(Some(240), Some(2_160))),
                        ],
                    ),
                ),
                ("timeoutMs", integer(Some(1_000), Some(120_000))),
            ],
        ),
        "browser_control" => object(
            &["operationId", "actions"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("actions", array(browser_action(), Some(1), Some(32))),
            ],
        ),
        "browser_result" => object(
            &["operationId"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                (
                    "include",
                    array(
                        enumeration(&["text", "screenshot", "console"]),
                        None,
                        Some(3),
                    ),
                ),
            ],
        ),
        _ => unreachable!("every built-in has an input schema"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the launch schema inventory is one cohesive closed union and is snapshot-bound"
)]
fn result_schema(name: &str) -> Value {
    match name {
        "todo_read" => object(
            &["todos"],
            vec![
                ("todos", array(todo(), None, Some(200))),
                ("counts", counts()),
            ],
        ),
        "todo_write" => object(
            &["accepted", "counts", "summary"],
            vec![
                ("accepted", integer(Some(0), Some(200))),
                ("counts", counts()),
                ("summary", text(0, 2_048)),
            ],
        ),
        "wait" => object(
            &["waitedSeconds", "wokeAt"],
            vec![
                ("waitedSeconds", integer(Some(1), None)),
                ("wokeAt", date_time()),
            ],
        ),
        "request_approval" => object(
            &["decision", "approvalId", "respondedAt"],
            vec![
                ("decision", enumeration(&["approved", "denied"])),
                ("approvalId", pattern("^apr_[0-9a-hjkmnp-tv-z]{26}$")),
                ("respondedAt", date_time()),
                ("note", text(0, 2_048)),
            ],
        ),
        "submit_result" => object(
            &["accepted", "resultRef", "bytes", "truncated"],
            vec![
                ("accepted", json!({"type":"boolean"})),
                ("resultRef", pattern(HASH)),
                ("bytes", decimal_string()),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "create_subagent" => object(
            &["agentId", "state"],
            vec![
                ("agentId", pattern(AGENT_ID)),
                ("state", enumeration(&STATES)),
                ("queuedReason", enumeration(&QUEUED_REASONS)),
                ("depth", integer(Some(1), None)),
                ("ordinal", integer(Some(0), None)),
            ],
        ),
        "get_subagent" => object(
            &["agentId", "state", "stats"],
            vec![
                ("agentId", pattern(AGENT_ID)),
                ("state", enumeration(&STATES)),
                ("queuedReason", text(0, 256)),
                ("attempt", integer(Some(0), None)),
                ("depth", integer(Some(1), None)),
                ("stats", agent_stats()),
                (
                    "result",
                    object(
                        &[],
                        vec![
                            ("status", enumeration(&["success", "failure"])),
                            ("summary", text(0, 8_192)),
                            ("resultRef", pattern(HASH)),
                        ],
                    ),
                ),
                (
                    "failure",
                    object(
                        &[],
                        vec![("code", text(0, 64)), ("message", text(0, 2_048))],
                    ),
                ),
            ],
        ),
        "list_subagents" => page_result("items"),
        "read_subagent_history" => object(
            &["events", "truncated"],
            vec![
                ("events", array(json!({"type":"object"}), None, Some(200))),
                ("nextCursor", text(0, 2_048)),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "send_subagent_message" => object(
            &["delivered", "queued"],
            vec![
                ("delivered", json!({"type":"boolean"})),
                ("queued", json!({"type":"boolean"})),
                ("mailboxSeq", integer(Some(0), None)),
            ],
        ),
        "dequeue_subagent" => object(
            &["dequeued", "state"],
            vec![
                ("dequeued", json!({"type":"boolean"})),
                ("state", text(0, 64)),
                (
                    "reason",
                    enumeration(&["already_claimed", "already_terminal", "not_your_child"]),
                ),
            ],
        ),
        "stop_subagent" => object(
            &["requested", "state"],
            vec![
                ("requested", json!({"type":"boolean"})),
                ("state", text(0, 64)),
            ],
        ),
        "wait_subagents" => object(
            &["mode", "satisfied", "terminal"],
            vec![
                ("mode", enumeration(&["any", "all"])),
                ("satisfied", json!({"type":"boolean"})),
                ("terminal", array(json!({"type":"object"}), None, Some(512))),
                ("pending", array(pattern(AGENT_ID), None, Some(512))),
                ("timedOut", json!({"type":"boolean"})),
            ],
        ),
        "web_fetch" => object(
            &[
                "url",
                "finalUrl",
                "status",
                "mediaType",
                "format",
                "bytes",
                "truncated",
                "content",
            ],
            vec![
                ("url", text(0, 8_192)),
                ("finalUrl", text(0, 8_192)),
                ("status", integer(Some(200), Some(599))),
                ("mediaType", text(0, 255)),
                ("charset", text(0, 64)),
                ("format", enumeration(&["markdown", "text", "raw"])),
                ("bytes", integer(Some(0), Some(500_000))),
                ("truncated", json!({"type":"boolean"})),
                ("redirects", array(text(0, 8_192), None, Some(5))),
                ("content", text(0, 500_000)),
                ("locator", pattern(HASH)),
            ],
        ),
        "web_search" => object(
            &["provider", "query", "results"],
            vec![
                ("provider", enumeration(&["brave", "serper"])),
                ("query", text(0, 2_000)),
                ("results", array(search_result(), None, Some(20))),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "read_file" => object(
            &["path", "revision", "bytes", "truncated", "content"],
            vec![
                ("path", text(0, 4_096)),
                ("revision", pattern(HASH)),
                ("bytes", integer(Some(0), None)),
                ("totalBytes", integer(Some(0), None)),
                ("truncated", json!({"type":"boolean"})),
                ("content", text(0, 1_000_000)),
                (
                    "encoding",
                    enumeration(&["utf8", "utf8-lossy", "binary-omitted"]),
                ),
            ],
        ),
        "write_file" => object(
            &["path", "revision", "bytes", "created"],
            vec![
                ("path", text(0, 4_096)),
                ("revision", pattern(HASH)),
                ("bytes", integer(Some(0), None)),
                ("created", json!({"type":"boolean"})),
            ],
        ),
        "edit_file" => object(
            &["path", "revision", "replacements"],
            vec![
                ("path", text(0, 4_096)),
                ("revision", pattern(HASH)),
                ("replacements", integer(Some(1), None)),
            ],
        ),
        "apply_patch" => object(
            &["files"],
            vec![("files", array(patch_file(), None, Some(64)))],
        ),
        "list_dir" => object(
            &["path", "entries", "truncated"],
            vec![
                ("path", text(0, 4_096)),
                ("entries", array(dir_entry(), None, Some(2_000))),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "glob" => object(
            &["paths", "truncated", "scanned"],
            vec![
                ("paths", array(text(0, 4_096), None, Some(2_000))),
                ("truncated", json!({"type":"boolean"})),
                ("scanned", integer(Some(0), Some(100_000))),
            ],
        ),
        "grep" => object(
            &["matches", "truncated"],
            vec![
                ("matches", array(grep_match(), None, Some(2_000))),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "run_command" | "run_code" | "git" => command_result(),
        "install_packages" => object(
            &["ecosystem", "installed"],
            vec![
                ("ecosystem", enumeration(&["apt", "pip", "npm"])),
                (
                    "installed",
                    array(
                        object(
                            &["name", "version"],
                            vec![("name", text(0, 256)), ("version", text(0, 128))],
                        ),
                        None,
                        Some(512),
                    ),
                ),
                ("exit", exit_result()),
                ("log", text(0, 1_000_000)),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "process_status" | "browser_status" => process_status_result(),
        "process_output" => object(
            &["operationId", "offset", "bytes", "content", "eof"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("offset", decimal_string()),
                ("bytes", integer(Some(0), None)),
                ("content", text(0, 1_000_000)),
                ("eof", json!({"type":"boolean"})),
                ("truncated", json!({"type":"boolean"})),
            ],
        ),
        "process_stop" => object(
            &["operationId", "state"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                (
                    "state",
                    enumeration(&["cancelling", "cancelled", "already_terminal", "unknown"]),
                ),
            ],
        ),
        "browser_launch" => object(
            &["operationId", "state"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("state", enumeration(&["accepted", "running"])),
                ("generation", pattern("^gen_[0-9a-hjkmnp-tv-z]{26}$")),
            ],
        ),
        "browser_control" => object(
            &["operationId", "applied"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("applied", integer(Some(0), None)),
                ("state", text(0, 64)),
                ("lastError", text(0, 2_048)),
            ],
        ),
        "browser_result" => object(
            &["operationId", "state"],
            vec![
                ("operationId", pattern(OPERATION_ID)),
                ("state", text(0, 64)),
                ("url", text(0, 8_192)),
                ("title", text(0, 1_024)),
                ("text", text(0, 4_000_000)),
                ("truncated", json!({"type":"boolean"})),
                ("screenshot", pattern(HASH)),
                ("console", array(text(0, 4_096), None, Some(200))),
            ],
        ),
        _ => unreachable!("every built-in has a result schema"),
    }
}

fn object(required: &[&str], properties: Vec<(&str, Value)>) -> Value {
    let mut property_map = Map::new();
    for (name, schema) in properties {
        property_map.insert(name.to_owned(), schema);
    }
    let mut root = Map::new();
    root.insert("type".to_owned(), Value::String("object".to_owned()));
    root.insert("additionalProperties".to_owned(), Value::Bool(false));
    root.insert("properties".to_owned(), Value::Object(property_map));
    if !required.is_empty() {
        root.insert(
            "required".to_owned(),
            Value::Array(
                required
                    .iter()
                    .map(|value| Value::String((*value).to_owned()))
                    .collect(),
            ),
        );
    }
    Value::Object(root)
}

fn text(min: u64, max: u64) -> Value {
    json!({"type":"string","minLength":min,"maxLength":max})
}

fn pattern(value: &str) -> Value {
    json!({"type":"string","pattern":value})
}

fn decimal_string() -> Value {
    pattern("^(0|[1-9][0-9]*)$")
}

fn date_time() -> Value {
    json!({"type":"string","format":"date-time"})
}

fn integer(minimum: Option<i64>, maximum: Option<i64>) -> Value {
    let mut value = Map::new();
    value.insert("type".to_owned(), Value::String("integer".to_owned()));
    if let Some(minimum) = minimum {
        value.insert("minimum".to_owned(), minimum.into());
    }
    if let Some(maximum) = maximum {
        value.insert("maximum".to_owned(), maximum.into());
    }
    Value::Object(value)
}

fn enumeration(values: &[&str]) -> Value {
    json!({"type":"string","enum":values})
}

fn array(items: Value, min: Option<u64>, max: Option<u64>) -> Value {
    let mut value = Map::new();
    value.insert("type".to_owned(), Value::String("array".to_owned()));
    value.insert("items".to_owned(), items);
    if let Some(minimum) = min {
        value.insert("minItems".to_owned(), minimum.into());
    }
    if let Some(maximum) = max {
        value.insert("maxItems".to_owned(), maximum.into());
    }
    Value::Object(value)
}

fn array_with_unique(items: Value, min: u64, max: u64) -> Value {
    let mut value = array(items, Some(min), Some(max));
    value
        .as_object_mut()
        .expect("array helper returns object")
        .insert("uniqueItems".to_owned(), Value::Bool(true));
    value
}

fn todo() -> Value {
    object(
        &["content", "status", "activeForm"],
        vec![
            ("content", text(1, 1_024)),
            (
                "status",
                enumeration(&["pending", "in_progress", "completed"]),
            ),
            ("activeForm", text(1, 1_024)),
        ],
    )
}

fn counts() -> Value {
    object(
        &["pending", "in_progress", "completed"],
        vec![
            ("pending", integer(Some(0), None)),
            ("in_progress", integer(Some(0), None)),
            ("completed", integer(Some(0), None)),
        ],
    )
}

fn create_subagent_input() -> Value {
    let mut root = object(
        &["prompt"],
        vec![
            ("prompt", text(1, 200_000)),
            ("system", text(0, 100_000)),
            (
                "provider",
                enumeration(&[
                    "openai",
                    "anthropic",
                    "deepseek",
                    "zai",
                    "moonshotai",
                    "google",
                ]),
            ),
            ("model", text(1, 256)),
            ("tools", array(text(0, 64), None, Some(128))),
            (
                "budget",
                object(
                    &[],
                    vec![
                        ("providerCalls", integer(Some(1), None)),
                        ("handsCalls", integer(Some(0), None)),
                        ("costMicroUsd", decimal_string()),
                        ("totalChildren", integer(Some(0), None)),
                        ("activeChildren", integer(Some(0), None)),
                        ("retainedResultBytes", decimal_string()),
                    ],
                ),
            ),
            ("label", text(0, 128)),
        ],
    );
    root.as_object_mut().expect("object helper").insert(
        "dependentRequired".to_owned(),
        json!({"provider":["model"],"model":["provider"]}),
    );
    root
}

fn agent_id_input(with_reason: bool) -> Value {
    let mut properties = vec![("agentId", pattern(AGENT_ID))];
    if with_reason {
        properties.push(("reason", text(0, 512)));
    }
    object(&["agentId"], properties)
}

fn operation_id_input(_browser: bool) -> Value {
    object(
        &["operationId"],
        vec![("operationId", pattern(OPERATION_ID))],
    )
}

fn run_command_input() -> Value {
    let mut root = object(
        &[],
        vec![
            ("command", text(1, 200_000)),
            ("argv", array(text(0, 32_768), Some(1), Some(256))),
            ("cwd", text(0, 4_096)),
            (
                "env",
                json!({"type":"object","maxProperties":64,"additionalProperties":{"type":"string","maxLength":32768}}),
            ),
            ("stdin", text(0, 200_000)),
            ("background", json!({"type":"boolean"})),
            ("timeoutMs", integer(Some(1_000), Some(600_000))),
        ],
    );
    root.as_object_mut().expect("object helper").insert("oneOf".to_owned(), json!([{"required":["command"],"not":{"required":["argv"]}},{"required":["argv"],"not":{"required":["command"]}}]));
    root
}

fn browser_action() -> Value {
    object(
        &["kind"],
        vec![
            (
                "kind",
                enumeration(&[
                    "navigate",
                    "click",
                    "type",
                    "scroll",
                    "wait",
                    "screenshot",
                    "read_text",
                ]),
            ),
            ("selector", text(0, 1_024)),
            ("url", text(0, 8_192)),
            ("text", text(0, 32_768)),
            ("ms", integer(Some(0), Some(30_000))),
        ],
    )
}

fn page_result(member: &str) -> Value {
    object(
        &[member],
        vec![
            (member, array(json!({"type":"object"}), None, Some(200))),
            ("nextCursor", text(0, 2_048)),
            ("truncated", json!({"type":"boolean"})),
        ],
    )
}

fn agent_stats() -> Value {
    object(
        &[],
        vec![
            ("turns", integer(Some(0), None)),
            ("toolCalls", integer(Some(0), None)),
            ("inputTokens", decimal_string()),
            ("outputTokens", decimal_string()),
            ("costMicroUsd", decimal_string()),
            ("budgetUsed", json!({"type":"object"})),
            ("budgetLimit", json!({"type":"object"})),
        ],
    )
}

fn search_result() -> Value {
    object(
        &["rank", "title", "url"],
        vec![
            ("rank", integer(Some(1), Some(20))),
            ("title", text(0, 512)),
            ("url", text(0, 4_096)),
            ("snippet", text(0, 2_048)),
            ("publishedAt", date_time()),
        ],
    )
}

fn patch_file() -> Value {
    object(
        &["path", "revision", "hunks"],
        vec![
            ("path", text(0, 4_096)),
            ("revision", pattern(HASH)),
            ("hunks", integer(Some(0), None)),
            (
                "action",
                enumeration(&["modified", "created", "deleted", "renamed"]),
            ),
        ],
    )
}

fn dir_entry() -> Value {
    object(
        &["name", "type", "bytes"],
        vec![
            ("name", text(0, 4_096)),
            ("type", enumeration(&["file", "dir", "symlink", "other"])),
            ("bytes", integer(Some(0), None)),
            ("modifiedAt", date_time()),
            ("mode", pattern("^0[0-7]{3}$")),
            ("target", text(0, 4_096)),
        ],
    )
}

fn grep_match() -> Value {
    object(
        &["path", "line", "text"],
        vec![
            ("path", text(0, 4_096)),
            ("line", integer(Some(1), None)),
            ("text", text(0, 4_096)),
            ("before", array(text(0, 4_096), None, Some(10))),
            ("after", array(text(0, 4_096), None, Some(10))),
        ],
    )
}

fn exit_result() -> Value {
    object(
        &["kind"],
        vec![
            (
                "kind",
                enumeration(&["ok", "non_zero", "signal", "timeout", "running"]),
            ),
            ("code", integer(None, None)),
            ("signal", text(0, 32)),
        ],
    )
}

fn command_result() -> Value {
    object(
        &["exit", "stdoutBytes", "stderrBytes", "truncated"],
        vec![
            ("operationId", pattern(OPERATION_ID)),
            ("background", json!({"type":"boolean"})),
            ("exit", exit_result()),
            ("stdout", text(0, 1_000_000)),
            ("stderr", text(0, 1_000_000)),
            ("stdoutBytes", integer(Some(0), None)),
            ("stderrBytes", integer(Some(0), None)),
            ("truncated", json!({"type":"boolean"})),
            ("locator", pattern(HASH)),
            ("workspaceRevision", decimal_string()),
        ],
    )
}

fn process_status_result() -> Value {
    object(
        &["operationId", "state"],
        vec![
            ("operationId", pattern(OPERATION_ID)),
            (
                "state",
                enumeration(&[
                    "accepted",
                    "running",
                    "succeeded",
                    "failed",
                    "cancelled",
                    "interrupted",
                ]),
            ),
            ("startedAt", date_time()),
            ("producedBytes", decimal_string()),
            ("phase", text(0, 256)),
        ],
    )
}

const HASH: &str = "^sha256:[0-9a-f]{64}$";
const AGENT_ID: &str = "^agt_[0-9a-hjkmnp-tv-z]{26}$";
const OPERATION_ID: &str = "^op_[0-9a-hjkmnp-tv-z]{26}$";
const STATES: [&str; 7] = [
    "queued",
    "starting",
    "running",
    "stopping",
    "completed",
    "failed",
    "cancelled",
];
const QUEUED_REASONS: [&str; 7] = [
    "active_budget",
    "session_active_limit",
    "provider_permits",
    "hands_permits",
    "tenant_fairness",
    "regional_capacity",
    "depth_deferred",
];

/// The compiled catalog could not be constructed.
#[derive(Debug, thiserror::Error)]
pub enum CatalogBuildError {
    /// A compiled name failed the grammar.
    #[error(transparent)]
    Name(#[from] ToolNameError),
    /// Two compiled rows had the same name.
    #[error("duplicate built-in tool name `{0}`")]
    DuplicateName(String),
    /// A schema could not be canonicalized.
    #[error("compiled schema is invalid: {0}")]
    Schema(String),
    /// A descriptor violated the structural rules.
    #[error(transparent)]
    Violation(#[from] CatalogViolation),
    /// The whole catalog failed integer-only JCS.
    #[error(transparent)]
    Canonical(#[from] CatalogCanonicalError),
}

/// A tool argument document was not admissible under its pinned descriptor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgumentError {
    /// JSON canonicalization failed.
    #[error("tool arguments are not canonical JSON: {0}")]
    Malformed(String),
    /// Canonical argument bytes exceeded the descriptor ceiling.
    #[error("tool arguments are {observed} bytes; descriptor limit is {limit}")]
    TooLarge {
        /// Canonical byte length.
        observed: usize,
        /// Descriptor ceiling.
        limit: usize,
    },
    /// JSON Schema validation failed.
    #[error("tool arguments violate the pinned schema: {reason}")]
    Schema {
        /// Validator diagnostic.
        reason: String,
    },
    /// A validated variant selector had no manifest row.
    #[error("validated tool arguments have no effect variant")]
    Variant,
}
