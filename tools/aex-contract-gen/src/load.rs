//! Reads the authored tree under `api/` into [`ContractIr`].
//!
//! Everything strict here is strict on purpose: a duplicate `SchemaId`, a
//! duplicate `operationId`, an unresolvable reference, an unbounded integer, an
//! undeclared float and an operation count that drifts from the pinned totals
//! are all hard errors. A generator that guesses is a generator whose output
//! nobody can audit.
//!
//! # Authoring format
//!
//! Schemas are authored in a compact YAML dialect rather than as raw JSON
//! Schema documents, and the *generated* `api/generated/schemas/<SchemaId>.json`
//! is the published JSON Schema 2020-12 artifact. One source produces both the
//! schema and the Rust type, so the two cannot disagree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::GenError;
use crate::ir::{
    ContractIr, EnumValue, ErrorRow, EvolutionIr, FieldIr, FieldType, IdRow, LimitRow, LimitShape,
    OperationIr, ParamIr, PlaneIr, SchemaBody, SchemaIr, ScopeRow, VariantIr,
};

/// Loads the whole authored tree.
///
/// # Errors
///
/// Returns [`GenError`] for a missing file, an unparsable document, or any of
/// the structural rules above.
pub fn load(root: &Path) -> Result<ContractIr, GenError> {
    let mut sources = BTreeMap::new();
    let toolchain = read_toolchain(root, &mut sources)?;

    let ids_file: IdsFile = read_yaml(root, "api/schemas/registries/ids.yaml", &mut sources)?;
    let errors_file: ErrorsFile =
        read_yaml(root, "api/schemas/registries/errors.yaml", &mut sources)?;
    let scopes_file: ScopesFile =
        read_yaml(root, "api/schemas/registries/scopes.yaml", &mut sources)?;
    let limits_file: LimitsFile =
        read_yaml(root, "api/schemas/registries/limits.yaml", &mut sources)?;
    let evolution_file: EvolutionFile =
        read_yaml(root, "api/schemas/registries/evolution.yaml", &mut sources)?;
    let routes_meta: RoutesMetaFile = read_yaml(
        root,
        "api/schemas/registries/routes-meta.yaml",
        &mut sources,
    )?;

    let ids = build_ids(&ids_file)?;
    let id_keys: BTreeSet<&str> = ids.iter().map(|row| row.key.as_str()).collect();
    let errors = build_errors(&errors_file)?;
    let error_codes: Vec<&str> = errors.iter().map(|row| row.code.as_str()).collect();
    let scopes = build_scopes(&scopes_file);
    let scope_names: BTreeSet<&str> = scopes.iter().map(|row| row.scope.as_str()).collect();
    let limits = build_limits(&limits_file);

    let schemas = load_schemas(root, &id_keys, &mut sources)?;
    resolve_references(&schemas)?;

    let planes = load_planes(
        root,
        &routes_meta,
        &schemas,
        error_codes.as_slice(),
        &scope_names,
        &id_keys,
        &mut sources,
    )?;

    Ok(ContractIr {
        contract_version: routes_meta.contract_version.clone(),
        toolchain,
        ids,
        errors,
        scopes,
        limits,
        schemas,
        planes,
        sources,
        evolution: EvolutionIr {
            open_enums: evolution_file.open_enums,
            additive_response_fields: evolution_file
                .additive_response_fields
                .into_iter()
                .flat_map(|entry| {
                    entry
                        .fields
                        .into_iter()
                        .map(move |field| (entry.schema.clone(), field))
                })
                .collect(),
            unknown_frame_policy: evolution_file.unknown_frame_policy,
            request_unknown_fields: evolution_file.request_unknown_fields,
        },
    })
}

// ---------------------------------------------------------------------------
// File access
// ---------------------------------------------------------------------------

/// Reads a workspace-relative YAML document and records its digest.
fn read_yaml<T: serde::de::DeserializeOwned>(
    root: &Path,
    relative: &str,
    sources: &mut BTreeMap<String, String>,
) -> Result<T, GenError> {
    let text = read_text(root, relative, sources)?;
    serde_norway::from_str(&text).map_err(|error| GenError::Parse {
        path: relative.to_owned(),
        reason: error.to_string(),
    })
}

/// Reads a workspace-relative text file and records its digest.
fn read_text(
    root: &Path,
    relative: &str,
    sources: &mut BTreeMap<String, String>,
) -> Result<String, GenError> {
    let path = root.join(relative);
    let bytes = std::fs::read(&path).map_err(|error| GenError::Io {
        path: relative.to_owned(),
        reason: error.to_string(),
    })?;
    sources.insert(relative.to_owned(), crate::jcs::digest_bytes(&bytes));
    String::from_utf8(bytes).map_err(|error| GenError::Io {
        path: relative.to_owned(),
        reason: error.to_string(),
    })
}

/// A sorted, symlink-free, extension-filtered recursive walk.
fn walk(root: &Path, relative: &str, extension: &str) -> Result<Vec<String>, GenError> {
    let mut found = Vec::new();
    let mut stack = vec![relative.to_owned()];
    while let Some(current) = stack.pop() {
        let directory = root.join(&current);
        let mut entries: Vec<_> = std::fs::read_dir(&directory)
            .map_err(|error| GenError::Io {
                path: current.clone(),
                reason: error.to_string(),
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| GenError::Io {
                path: current.clone(),
                reason: error.to_string(),
            })?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = format!("{current}/{name}");
            let metadata = entry.metadata().map_err(|error| GenError::Io {
                path: child.clone(),
                reason: error.to_string(),
            })?;
            if metadata.file_type().is_symlink() {
                return Err(GenError::Symlink { path: child });
            }
            if metadata.is_dir() {
                stack.push(child);
            } else if name.ends_with(extension) {
                found.push(child);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Reads the pinned toolchain channel from `rust-toolchain.toml`.
fn read_toolchain(root: &Path, sources: &mut BTreeMap<String, String>) -> Result<String, GenError> {
    let text = read_text(root, "rust-toolchain.toml", sources)?;
    text.lines()
        .find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("channel")?.trim_start();
            let rest = rest.strip_prefix('=')?.trim();
            Some(rest.trim_matches('"').to_owned())
        })
        .ok_or_else(|| GenError::Parse {
            path: "rust-toolchain.toml".to_owned(),
            reason: "no `channel = \"…\"` line".to_owned(),
        })
}

// ---------------------------------------------------------------------------
// Registries
// ---------------------------------------------------------------------------

/// `api/schemas/registries/ids.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdsFile {
    /// Rows, in declared order.
    ids: Vec<IdEntry>,
}

/// One authored identifier row.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdEntry {
    /// `snake_case` registry key.
    key: String,
    /// Wire prefix without the underscore.
    prefix: String,
    /// Rust newtype name.
    rust: String,
    /// One-line documentation.
    doc: String,
}

/// Validates and converts the identifier registry.
fn build_ids(file: &IdsFile) -> Result<Vec<IdRow>, GenError> {
    let mut seen_prefix = BTreeSet::new();
    let mut rows = Vec::with_capacity(file.ids.len());
    for entry in &file.ids {
        if entry.prefix.len() > 5 || entry.prefix.is_empty() {
            return Err(GenError::Registry {
                registry: "ids",
                detail: format!("prefix `{}` must be 1..=5 bytes", entry.prefix),
            });
        }
        if !seen_prefix.insert(entry.prefix.clone()) {
            return Err(GenError::Registry {
                registry: "ids",
                detail: format!("duplicate prefix `{}`", entry.prefix),
            });
        }
        rows.push(IdRow {
            variant: pascal_case(&entry.key),
            key: entry.key.clone(),
            prefix: entry.prefix.clone(),
            rust: entry.rust.clone(),
            doc: entry.doc.clone(),
        });
    }
    Ok(rows)
}

/// `api/schemas/registries/errors.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorsFile {
    /// Rows, in declared order.
    errors: Vec<ErrorEntry>,
}

/// One authored error row.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorEntry {
    /// Wire code.
    code: String,
    /// HTTP status.
    status: u16,
    /// Whether an identical retry can succeed.
    retryable: bool,
    /// Error class key.
    class: String,
    /// Precedence-stage key.
    stage: String,
    /// Default human message.
    message: String,
    /// Optional remediation hint.
    #[serde(default)]
    remedy: Option<String>,
}

/// The closed set of error classes.
const ERROR_CLASSES: [&str; 9] = [
    "auth",
    "not_found",
    "conflict",
    "precondition",
    "validation",
    "quota",
    "state",
    "unavailable",
    "internal",
];

/// The refusal every deferred operation declares, injected rather than authored.
pub const NOT_IMPLEMENTED: &str = "not_implemented";

/// The closed set of precedence stages, in evaluation order.
pub const PRECEDENCE_STAGES: [&str; 13] = [
    "transport_envelope",
    "authentication",
    "authorization",
    "placement",
    "scope",
    "account_state",
    "body_limit_and_parse",
    "operation_identity",
    "idempotency_identity",
    "tombstone_and_parent",
    "precondition",
    "domain_state",
    "commit",
];

/// Validates and converts the error registry.
fn build_errors(file: &ErrorsFile) -> Result<Vec<ErrorRow>, GenError> {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::with_capacity(file.errors.len());
    for entry in &file.errors {
        if !seen.insert(entry.code.clone()) {
            return Err(GenError::Registry {
                registry: "errors",
                detail: format!("duplicate code `{}`", entry.code),
            });
        }
        if !ERROR_CLASSES.contains(&entry.class.as_str()) {
            return Err(GenError::Registry {
                registry: "errors",
                detail: format!("`{}` has unknown class `{}`", entry.code, entry.class),
            });
        }
        if !PRECEDENCE_STAGES.contains(&entry.stage.as_str()) {
            return Err(GenError::Registry {
                registry: "errors",
                detail: format!("`{}` has unknown stage `{}`", entry.code, entry.stage),
            });
        }
        if !(100..=599).contains(&entry.status) {
            return Err(GenError::Registry {
                registry: "errors",
                detail: format!("`{}` has impossible status {}", entry.code, entry.status),
            });
        }
        rows.push(ErrorRow {
            variant: pascal_case(&entry.code),
            code: entry.code.clone(),
            status: entry.status,
            retryable: entry.retryable,
            class: entry.class.clone(),
            stage: entry.stage.clone(),
            message: entry.message.clone(),
            remedy: entry.remedy.clone(),
        });
    }
    Ok(rows)
}

/// `api/schemas/registries/scopes.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopesFile {
    /// Rows, in declared order.
    scopes: Vec<ScopeEntry>,
}

/// One authored scope row.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeEntry {
    /// Wire scope string.
    scope: String,
    /// One-line documentation.
    doc: String,
}

/// Converts the scope registry.
fn build_scopes(file: &ScopesFile) -> Vec<ScopeRow> {
    file.scopes
        .iter()
        .map(|entry| ScopeRow {
            variant: pascal_case(&entry.scope.replace(':', "_")),
            scope: entry.scope.clone(),
            doc: entry.doc.clone(),
        })
        .collect()
}

/// `api/schemas/registries/limits.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsFile {
    /// Rows, in declared order.
    limits: Vec<LimitEntry>,
}

/// One authored limit row.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitEntry {
    /// Wire limit id.
    id: String,
    /// `scalar` or `map`.
    shape: String,
    /// Complete ordered dimension vocabulary for a map limit.
    #[serde(default)]
    dimensions: Vec<String>,
    /// One-line documentation.
    doc: String,
}

/// Converts the limit registry.
fn build_limits(file: &LimitsFile) -> Vec<LimitRow> {
    file.limits
        .iter()
        .map(|entry| LimitRow {
            variant: pascal_case(&entry.id.replace('.', "_")),
            id: entry.id.clone(),
            shape: if entry.shape == "map" {
                LimitShape::Map
            } else {
                LimitShape::Scalar
            },
            dimensions: entry.dimensions.clone(),
            doc: entry.doc.clone(),
        })
        .collect()
}

/// `api/schemas/registries/evolution.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct EvolutionFile {
    /// Enums a decoder may widen.
    open_enums: Vec<String>,
    /// Response fields declared additive.
    additive_response_fields: Vec<AdditiveEntry>,
    /// `surface` or `reject`.
    unknown_frame_policy: String,
    /// Always `reject`.
    request_unknown_fields: String,
}

/// One declared additive-response relaxation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdditiveEntry {
    /// The owning schema.
    schema: String,
    /// The member paths permitted to appear.
    fields: Vec<String>,
}

/// `api/schemas/registries/routes-meta.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RoutesMetaFile {
    /// The single prelaunch contract line.
    contract_version: String,
    /// Pinned per-plane operation counts.
    expected: BTreeMap<String, usize>,
    /// The one place a path parameter's type is declared.
    path_params: BTreeMap<String, FieldEntry>,
    /// Closed release-artifact vocabulary and the API plane each artifact may
    /// own. Both planned and actual ownership are checked against this map.
    serving_artifacts: BTreeMap<String, String>,
    /// Release scenario owners, keyed `<plane>.<fragment>`.
    ///
    /// This is delivery metadata emitted only into the route registry. It is
    /// deliberately absent from the public wire bundle, so changing test
    /// selection does not mint a new API contract identity.
    scenario_owners: BTreeMap<String, RouteOwnerEntry>,
    /// Per-operation exceptions for fragments whose routes are split across
    /// deployables or transports.
    ///
    /// The resolved planned owner is emitted into both the delivery registry
    /// and generated runtime descriptor, so selection and responsibility do
    /// not mirror routing rules. Actual mount evidence remains separate.
    #[serde(default)]
    operation_owners: BTreeMap<String, RouteOwnerEntry>,
    /// Operations that are mounted by a runnable composition today, keyed by
    /// release artifact. An operation absent here is deliberately unserved.
    #[serde(default)]
    served_operations: BTreeMap<String, Vec<String>>,
    /// Operations whose production composition is intentionally incomplete,
    /// keyed by exact `operationId` with a non-empty architectural reason.
    /// A deferred operation may not also appear in `servedOperations`.
    #[serde(default)]
    deferred_operations: BTreeMap<String, String>,
}

/// Release-selection metadata for one mounted API fragment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RouteOwnerEntry {
    /// The immutable release unit that serves the fragment.
    serving_artifact: String,
    /// Scenarios that exercise the fragment through that unit.
    scenarios: Vec<String>,
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

/// One authored schema file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemasFile {
    /// Which generated Rust module the file's schemas land in.
    module: String,
    /// The schemas, keyed by `SchemaId`.
    schemas: BTreeMap<String, SchemaEntry>,
}

/// One authored schema.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SchemaEntry {
    /// One-line documentation.
    doc: String,
    /// `object`, `enum` or `union`.
    #[serde(rename = "type")]
    kind: String,
    /// Object members, keyed by wire name.
    #[serde(default)]
    fields: BTreeMap<String, FieldEntry>,
    /// Enumeration values, in declared order.
    #[serde(default)]
    values: Vec<EnumEntry>,
    /// Union discriminator member name.
    #[serde(default)]
    tag: Option<String>,
    /// Union arms, in declared order.
    #[serde(default)]
    variants: Vec<VariantEntry>,
}

/// One authored enumeration value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnumEntry {
    /// The wire string.
    value: String,
    /// One-line documentation.
    doc: String,
}

/// One authored union arm.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantEntry {
    /// The discriminator value.
    value: String,
    /// The `SchemaId` carrying the arm's members.
    payload: String,
    /// One-line documentation.
    doc: String,
}

/// One authored field.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldEntry {
    /// The field-type keyword.
    #[serde(rename = "type")]
    kind: String,
    /// One-line documentation.
    #[serde(default)]
    doc: Option<String>,
    /// Whether the member may be absent.
    #[serde(default)]
    optional: bool,
    /// Identifier registry key, for `type: id`.
    #[serde(default)]
    of: Option<String>,
    /// Target `SchemaId`, for `type: ref`.
    #[serde(default)]
    to: Option<String>,
    /// Lower bound.
    #[serde(default)]
    min: Option<i64>,
    /// Upper bound.
    #[serde(default)]
    max: Option<i64>,
    /// Anchored regular expression.
    #[serde(default)]
    pattern: Option<String>,
    /// Element type, for `type: array` and `type: map`.
    #[serde(default)]
    items: Option<Box<FieldEntry>>,
}

/// Loads every schema file under `api/schemas`, excluding the registries.
fn load_schemas(
    root: &Path,
    id_keys: &BTreeSet<&str>,
    sources: &mut BTreeMap<String, String>,
) -> Result<BTreeMap<String, SchemaIr>, GenError> {
    let mut schemas: BTreeMap<String, SchemaIr> = BTreeMap::new();
    for relative in walk(root, "api/schemas", ".yaml")? {
        if relative.starts_with("api/schemas/registries/") {
            continue;
        }
        let file: SchemasFile = read_yaml(root, &relative, sources)?;
        for (id, entry) in &file.schemas {
            let body = build_schema_body(&relative, id, entry, id_keys)?;
            let schema = SchemaIr {
                id: id.clone(),
                doc: entry.doc.clone(),
                owner: relative.clone(),
                module: file.module.clone(),
                body,
            };
            if let Some(existing) = schemas.insert(id.clone(), schema) {
                return Err(GenError::DuplicateSchema {
                    id: id.clone(),
                    first: existing.owner,
                    second: relative.clone(),
                });
            }
        }
    }
    if schemas.is_empty() {
        return Err(GenError::EmptyInput {
            what: "api/schemas",
        });
    }
    Ok(schemas)
}

/// Converts one authored schema body.
fn build_schema_body(
    path: &str,
    id: &str,
    entry: &SchemaEntry,
    id_keys: &BTreeSet<&str>,
) -> Result<SchemaBody, GenError> {
    match entry.kind.as_str() {
        "object" => {
            let mut fields = Vec::with_capacity(entry.fields.len());
            for (wire, field) in &entry.fields {
                fields.push(FieldIr {
                    rust: snake_case(wire),
                    wire: wire.clone(),
                    doc: field.doc.clone().unwrap_or_else(|| format!("`{wire}`.")),
                    optional: field.optional,
                    ty: build_field_type(path, id, wire, field, id_keys)?,
                });
            }
            Ok(SchemaBody::Object { fields })
        }
        "enum" => {
            if entry.values.is_empty() {
                return Err(GenError::Schema {
                    id: id.to_owned(),
                    detail: "an enum needs at least one value".to_owned(),
                });
            }
            Ok(SchemaBody::Enum {
                values: entry
                    .values
                    .iter()
                    .map(|value| EnumValue {
                        variant: pascal_case(&value.value),
                        value: value.value.clone(),
                        doc: value.doc.clone(),
                    })
                    .collect(),
            })
        }
        "union" => {
            let tag = entry.tag.clone().ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: "a union must declare `tag`; an untagged union is rejected".to_owned(),
            })?;
            if entry.variants.is_empty() {
                return Err(GenError::Schema {
                    id: id.to_owned(),
                    detail: "a union needs at least one variant".to_owned(),
                });
            }
            Ok(SchemaBody::Union {
                tag,
                variants: entry
                    .variants
                    .iter()
                    .map(|variant| VariantIr {
                        variant: pascal_case(&variant.value),
                        value: variant.value.clone(),
                        payload: variant.payload.clone(),
                        doc: variant.doc.clone(),
                    })
                    .collect(),
            })
        }
        other => Err(GenError::Schema {
            id: id.to_owned(),
            detail: format!("unknown schema kind `{other}`"),
        }),
    }
}

/// Converts one authored field type. Every construct is mapped explicitly.
fn build_field_type(
    path: &str,
    id: &str,
    wire: &str,
    field: &FieldEntry,
    id_keys: &BTreeSet<&str>,
) -> Result<FieldType, GenError> {
    let where_ = || format!("{path}: {id}.{wire}");
    let ty = match field.kind.as_str() {
        "string" => {
            let max = field.max.ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: `string` needs an explicit `max`", where_()),
            })?;
            FieldType::Text {
                min: u32::try_from(field.min.unwrap_or(0)).unwrap_or(0),
                max: u32::try_from(max).map_err(|_| GenError::Schema {
                    id: id.to_owned(),
                    detail: format!("{}: `max` is not a positive length", where_()),
                })?,
                pattern: field.pattern.clone(),
            }
        }
        "integer" => {
            let (Some(min), Some(max)) = (field.min, field.max) else {
                return Err(GenError::Schema {
                    id: id.to_owned(),
                    detail: format!("{}: `integer` needs both `min` and `max`", where_()),
                });
            };
            FieldType::Integer { min, max }
        }
        "id" => {
            let key = field.of.clone().ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: `id` needs `of`", where_()),
            })?;
            if !id_keys.contains(key.as_str()) {
                return Err(GenError::Schema {
                    id: id.to_owned(),
                    detail: format!("{}: unknown id kind `{key}`", where_()),
                });
            }
            FieldType::Id(key)
        }
        "ref" => FieldType::Ref(field.to.clone().ok_or_else(|| GenError::Schema {
            id: id.to_owned(),
            detail: format!("{}: `ref` needs `to`", where_()),
        })?),
        "array" => {
            let items = field.items.as_ref().ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: `array` needs `items`", where_()),
            })?;
            let max = field.max.ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: `array` needs an explicit `max`", where_()),
            })?;
            FieldType::Array(
                Box::new(build_field_type(path, id, wire, items, id_keys)?),
                u32::try_from(max).unwrap_or(u32::MAX),
            )
        }
        "map" => {
            let items = field.items.as_ref().ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: `map` needs `items`", where_()),
            })?;
            let max = field.max.ok_or_else(|| GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: `map` needs an explicit `max`", where_()),
            })?;
            FieldType::Map(
                Box::new(build_field_type(path, id, wire, items, id_keys)?),
                u32::try_from(max).map_err(|_| GenError::Schema {
                    id: id.to_owned(),
                    detail: format!("{}: `max` is not a positive entry count", where_()),
                })?,
            )
        }
        "timestamp" => FieldType::Timestamp,
        "decimal" => FieldType::Decimal,
        "cents" => FieldType::Cents,
        "float" => FieldType::Float,
        "boolean" => FieldType::Bool,
        "region" => FieldType::Region,
        "compute_size" => FieldType::ComputeSize,
        "content_hash" => FieldType::ContentHash,
        "resource_name" => FieldType::ResourceName,
        "file_path" => FieldType::FilePath,
        "trace_id" => FieldType::TraceId,
        "span_id" => FieldType::SpanId,
        "https_url" => FieldType::HttpsUrl,
        "etag" => FieldType::ETag,
        "cursor" => FieldType::Cursor,
        "json_pointer" => FieldType::JsonPointer,
        "canonical_json" => FieldType::CanonicalJson,
        "byte_range" => FieldType::ByteRange,
        "provider_id" => FieldType::ProviderId,
        "scope" => FieldType::Scope,
        "limit_id" => FieldType::LimitId,
        "error_code" => FieldType::ErrorCode,
        "metadata_value" => FieldType::MetadataValue,
        other => {
            return Err(GenError::Schema {
                id: id.to_owned(),
                detail: format!("{}: unmapped field type `{other}`", where_()),
            });
        }
    };
    Ok(ty)
}

/// Proves every `ref` and every union payload resolves.
fn resolve_references(schemas: &BTreeMap<String, SchemaIr>) -> Result<(), GenError> {
    for schema in schemas.values() {
        match &schema.body {
            SchemaBody::Object { fields } => {
                for field in fields {
                    if let Some(target) = field.ty.referenced_schema()
                        && !schemas.contains_key(target)
                    {
                        return Err(GenError::UnresolvedRef {
                            from: format!("{}.{}", schema.id, field.wire),
                            target: target.to_owned(),
                        });
                    }
                }
            }
            SchemaBody::Union { variants, .. } => {
                for variant in variants {
                    let payload =
                        schemas
                            .get(&variant.payload)
                            .ok_or_else(|| GenError::UnresolvedRef {
                                from: format!("{}::{}", schema.id, variant.value),
                                target: variant.payload.clone(),
                            })?;
                    if !matches!(payload.body, SchemaBody::Object { .. }) {
                        return Err(GenError::Schema {
                            id: schema.id.clone(),
                            detail: format!(
                                "union arm `{}` must carry an object schema, `{}` is not one",
                                variant.value, variant.payload
                            ),
                        });
                    }
                }
            }
            SchemaBody::Enum { .. } => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Planes and operations
// ---------------------------------------------------------------------------

/// One authored plane header.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PlaneFile {
    /// `central` or `regional`.
    id: String,
    /// Document title.
    title: String,
    /// Server URL template.
    server: String,
    /// Human description of the server template.
    server_description: String,
    /// Default security scheme name.
    security_scheme: String,
    /// Fragment stems, in explicit assembly order.
    fragments: Vec<String>,
}

/// One authored route fragment.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FragmentFile {
    /// Owning plane id.
    plane: String,
    /// Fragment stem; must equal the file stem.
    fragment: String,
    /// Operations, in declared order.
    operations: Vec<OperationEntry>,
}

/// One authored operation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct OperationEntry {
    /// Globally unique `snake_case` `operationId`.
    id: String,
    /// HTTP method.
    method: String,
    /// Path template.
    path: String,
    /// One-line summary.
    summary: String,
    /// Required scope.
    #[serde(default)]
    scope: Option<String>,
    /// An additional accepted principal kind.
    #[serde(default)]
    alt_principal: Option<String>,
    /// `none`, `idempotency_key` or `operation_id`.
    #[serde(default)]
    idempotency: Option<String>,
    /// Request body schema.
    #[serde(default)]
    request: Option<String>,
    /// Success status; defaults to 200 with a body and 204 without.
    #[serde(default)]
    success_status: Option<u16>,
    /// Success body schema.
    #[serde(default)]
    success: Option<String>,
    /// Declared error codes.
    errors: Vec<String>,
    /// `none`, `aex_json`, `otlp` or `binary`.
    #[serde(default)]
    body_class: Option<String>,
    /// `unary`, `ndjson` or `binary`.
    #[serde(default)]
    transport: Option<String>,
    /// `none`, `returns`, `optional_if_match` or `required_if_match`.
    #[serde(default)]
    etag: Option<String>,
    /// Whether an identical retry is safe without a replay identity.
    #[serde(default)]
    safe_retry: Option<bool>,
    /// Whether the route runs while the account is paused.
    #[serde(default)]
    pause_exempt: bool,
    /// Query parameters, keyed by wire name.
    #[serde(default)]
    query: BTreeMap<String, FieldEntry>,
}

/// The closed idempotency vocabulary.
const IDEMPOTENCY_KINDS: [&str; 3] = ["none", "idempotency_key", "operation_id"];
/// The closed body-class vocabulary.
const BODY_CLASSES: [&str; 4] = ["none", "aex_json", "otlp", "binary"];
/// The closed transport vocabulary.
const TRANSPORTS: [&str; 3] = ["unary", "ndjson", "binary"];
/// The closed entity-tag vocabulary.
const ETAG_POLICIES: [&str; 4] = ["none", "returns", "optional_if_match", "required_if_match"];
/// The closed principal vocabulary.
const PRINCIPAL_KINDS: [&str; 4] = ["account", "workspace_key", "user_session", "anonymous"];

/// Loads both planes and every fragment they declare.
fn load_planes(
    root: &Path,
    meta: &RoutesMetaFile,
    schemas: &BTreeMap<String, SchemaIr>,
    error_codes: &[&str],
    scope_names: &BTreeSet<&str>,
    id_keys: &BTreeSet<&str>,
    sources: &mut BTreeMap<String, String>,
) -> Result<Vec<PlaneIr>, GenError> {
    let actual_owners = actual_route_owners(meta)?;
    let deferred_operations = deferred_operations(meta, &actual_owners)?;
    let mut planes = Vec::new();
    let mut seen_operations: BTreeSet<String> = BTreeSet::new();
    let mut used_scenario_owners = BTreeSet::new();
    for plane_id in ["central", "regional"] {
        let relative = format!("api/openapi/plane.{plane_id}.yaml");
        let header: PlaneFile = read_yaml(root, &relative, sources)?;
        if header.id != plane_id {
            return Err(GenError::Plane {
                plane: plane_id.to_owned(),
                detail: format!("declares id `{}`", header.id),
            });
        }
        let mut operations = Vec::new();
        let mut seen_routes: BTreeSet<(String, String)> = BTreeSet::new();
        for fragment in &header.fragments {
            used_scenario_owners.insert(format!("{plane_id}.{fragment}"));
            let path = format!("api/openapi/{plane_id}/{fragment}.yaml");
            let file: FragmentFile = read_yaml(root, &path, sources)?;
            if file.plane != plane_id || &file.fragment != fragment {
                return Err(GenError::Plane {
                    plane: plane_id.to_owned(),
                    detail: format!("`{path}` declares `{}`/`{}`", file.plane, file.fragment),
                });
            }
            for entry in &file.operations {
                if !seen_operations.insert(entry.id.clone()) {
                    return Err(GenError::DuplicateOperation {
                        id: entry.id.clone(),
                    });
                }
                if !seen_routes.insert((entry.method.clone(), entry.path.clone())) {
                    return Err(GenError::DuplicateRoute {
                        method: entry.method.clone(),
                        path: entry.path.clone(),
                    });
                }
                operations.push(build_operation(
                    plane_id,
                    fragment,
                    entry,
                    meta,
                    schemas,
                    error_codes,
                    scope_names,
                    id_keys,
                    &actual_owners,
                    &deferred_operations,
                )?);
            }
        }
        operations.sort_by(|left, right| left.id.cmp(&right.id));
        let expected = meta.expected.get(plane_id).copied().unwrap_or_default();
        if operations.len() != expected {
            return Err(GenError::OperationCount {
                plane: plane_id.to_owned(),
                expected,
                found: operations.len(),
            });
        }
        planes.push(PlaneIr {
            variant: pascal_case(plane_id),
            id: header.id,
            title: header.title,
            server: header.server,
            server_description: header.server_description,
            security_scheme: header.security_scheme,
            fragment_order: header.fragments,
            operations,
        });
    }
    let declared_scenario_owners: BTreeSet<String> = meta.scenario_owners.keys().cloned().collect();
    if used_scenario_owners != declared_scenario_owners {
        let missing = used_scenario_owners
            .difference(&declared_scenario_owners)
            .cloned()
            .collect::<Vec<_>>();
        let stale = declared_scenario_owners
            .difference(&used_scenario_owners)
            .cloned()
            .collect::<Vec<_>>();
        return Err(GenError::Registry {
            registry: "routes-meta",
            detail: format!(
                "scenarioOwners must exactly match the mounted fragments; missing={missing:?}, stale={stale:?}"
            ),
        });
    }
    let stale_operation_owners = meta
        .operation_owners
        .keys()
        .filter(|operation| !seen_operations.contains(*operation))
        .cloned()
        .collect::<Vec<_>>();
    if !stale_operation_owners.is_empty() {
        return Err(GenError::Registry {
            registry: "routes-meta",
            detail: format!("operationOwners names unknown operations: {stale_operation_owners:?}"),
        });
    }
    let stale_served_operations = actual_owners
        .keys()
        .filter(|operation| !seen_operations.contains(*operation))
        .cloned()
        .collect::<Vec<_>>();
    if !stale_served_operations.is_empty() {
        return Err(GenError::Registry {
            registry: "routes-meta",
            detail: format!(
                "servedOperations names unknown operations: {stale_served_operations:?}"
            ),
        });
    }
    let stale_deferred_operations = deferred_operations
        .keys()
        .filter(|operation| !seen_operations.contains(*operation))
        .cloned()
        .collect::<Vec<_>>();
    if !stale_deferred_operations.is_empty() {
        return Err(GenError::Registry {
            registry: "routes-meta",
            detail: format!(
                "deferredOperations names unknown operations: {stale_deferred_operations:?}"
            ),
        });
    }
    let used_artifacts: BTreeSet<&str> = planes
        .iter()
        .flat_map(|plane| plane.operations.iter())
        .map(|operation| operation.serving_artifact.as_str())
        .collect();
    let declared_artifacts: BTreeSet<&str> =
        meta.serving_artifacts.keys().map(String::as_str).collect();
    if used_artifacts != declared_artifacts {
        let missing = used_artifacts
            .difference(&declared_artifacts)
            .copied()
            .collect::<Vec<_>>();
        let stale = declared_artifacts
            .difference(&used_artifacts)
            .copied()
            .collect::<Vec<_>>();
        return Err(GenError::Registry {
            registry: "routes-meta",
            detail: format!(
                "servingArtifacts must exactly match planned owners; missing={missing:?}, stale={stale:?}"
            ),
        });
    }
    Ok(planes)
}

/// Resolves the authored actual-mount table into one owner per operation.
fn actual_route_owners(meta: &RoutesMetaFile) -> Result<BTreeMap<String, String>, GenError> {
    for (artifact, plane) in &meta.serving_artifacts {
        validate_artifact_id(artifact)?;
        if !["central", "regional"].contains(&plane.as_str()) {
            return Err(GenError::Registry {
                registry: "routes-meta",
                detail: format!("serving artifact `{artifact}` declares unknown plane `{plane}`"),
            });
        }
    }
    let mut owners = BTreeMap::new();
    for (artifact, operations) in &meta.served_operations {
        validate_artifact_id(artifact)?;
        if !meta.serving_artifacts.contains_key(artifact) {
            return Err(GenError::Registry {
                registry: "routes-meta",
                detail: format!("servedOperations names undeclared serving artifact `{artifact}`"),
            });
        }
        let mut local = BTreeSet::new();
        for operation in operations {
            if !local.insert(operation) {
                return Err(GenError::Registry {
                    registry: "routes-meta",
                    detail: format!("servedOperations.{artifact} repeats operation `{operation}`"),
                });
            }
            if let Some(first) = owners.insert(operation.clone(), artifact.clone()) {
                return Err(GenError::Registry {
                    registry: "routes-meta",
                    detail: format!(
                        "operation `{operation}` is actually served by both `{first}` and `{artifact}`"
                    ),
                });
            }
        }
    }
    Ok(owners)
}

/// Validates the explicit incomplete-composition ledger.
fn deferred_operations(
    meta: &RoutesMetaFile,
    actual_owners: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, GenError> {
    let mut deferred = BTreeMap::new();
    for (operation, reason) in &meta.deferred_operations {
        if reason.trim().is_empty() {
            return Err(GenError::Registry {
                registry: "routes-meta",
                detail: format!(
                    "deferredOperations.{operation} must carry a non-empty architectural reason"
                ),
            });
        }
        if actual_owners.contains_key(operation) {
            return Err(GenError::Registry {
                registry: "routes-meta",
                detail: format!(
                    "operation `{operation}` cannot be both served and explicitly deferred"
                ),
            });
        }
        deferred.insert(operation.clone(), reason.trim().to_owned());
    }
    Ok(deferred)
}

fn validate_artifact_id(artifact: &str) -> Result<(), GenError> {
    let valid = !artifact.is_empty()
        && artifact
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && artifact
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_lowercase)
        && artifact
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !artifact.contains("--");
    if valid {
        Ok(())
    } else {
        Err(GenError::Registry {
            registry: "routes-meta",
            detail: format!("malformed serving artifact `{artifact}`"),
        })
    }
}

/// Validates and converts one operation.
#[allow(
    clippy::too_many_arguments,
    reason = "one validated conversion, not a pipeline"
)]
fn build_operation(
    plane: &str,
    fragment: &str,
    entry: &OperationEntry,
    meta: &RoutesMetaFile,
    schemas: &BTreeMap<String, SchemaIr>,
    error_codes: &[&str],
    scope_names: &BTreeSet<&str>,
    id_keys: &BTreeSet<&str>,
    actual_owners: &BTreeMap<String, String>,
    deferred_operations: &BTreeMap<String, String>,
) -> Result<OperationIr, GenError> {
    let bad = |detail: String| GenError::Operation {
        id: entry.id.clone(),
        detail,
    };
    let scenario_key = format!("{plane}.{fragment}");
    let fragment_owner = meta.scenario_owners.get(&scenario_key).ok_or_else(|| {
        bad(format!(
            "fragment `{scenario_key}` has no scenario owner in routes-meta.yaml"
        ))
    })?;
    let owner = meta
        .operation_owners
        .get(&entry.id)
        .unwrap_or(fragment_owner);
    if owner.scenarios.is_empty() {
        return Err(bad(format!(
            "fragment `{scenario_key}` has an empty scenario owner list"
        )));
    }
    validate_artifact_id(&owner.serving_artifact).map_err(|_| {
        bad(format!(
            "fragment `{scenario_key}` has invalid serving artifact `{}`",
            owner.serving_artifact
        ))
    })?;
    let Some(owner_plane) = meta.serving_artifacts.get(&owner.serving_artifact) else {
        return Err(bad(format!(
            "fragment `{scenario_key}` names undeclared serving artifact `{}`",
            owner.serving_artifact
        )));
    };
    if owner_plane != plane {
        return Err(bad(format!(
            "fragment `{scenario_key}` is on `{plane}` but serving artifact `{}` is declared on `{owner_plane}`",
            owner.serving_artifact
        )));
    }
    let served_artifact = actual_owners.get(&entry.id).cloned();
    let deferred_reason = deferred_operations.get(&entry.id).cloned();
    if let Some(actual) = &served_artifact {
        let actual_plane = meta
            .serving_artifacts
            .get(actual)
            .expect("actual_route_owners rejects undeclared artifacts");
        if actual_plane != plane {
            return Err(bad(format!(
                "operation is on `{plane}` but actual serving artifact `{actual}` is declared on `{actual_plane}`"
            )));
        }
        if actual != &owner.serving_artifact {
            return Err(bad(format!(
                "operation is planned for `{}` but claims it is actually served by `{actual}`",
                owner.serving_artifact
            )));
        }
    }
    let mut unique_scenarios = BTreeSet::new();
    for scenario in &owner.scenarios {
        if !scenario.starts_with("SC-")
            || !scenario.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '-'
            })
        {
            return Err(bad(format!(
                "fragment `{scenario_key}` has malformed scenario id `{scenario}`"
            )));
        }
        if !unique_scenarios.insert(scenario) {
            return Err(bad(format!(
                "fragment `{scenario_key}` repeats scenario `{scenario}`"
            )));
        }
    }
    if !["GET", "PUT", "POST", "DELETE"].contains(&entry.method.as_str()) {
        return Err(bad(format!("unsupported method `{}`", entry.method)));
    }
    if !entry.path.starts_with("/api/") {
        return Err(bad("every path is rooted at `/api`".to_owned()));
    }
    if let Some(scope) = &entry.scope
        && !scope_names.contains(scope.as_str())
    {
        return Err(bad(format!("unknown scope `{scope}`")));
    }
    if let Some(principal) = &entry.alt_principal
        && !PRINCIPAL_KINDS.contains(&principal.as_str())
    {
        return Err(bad(format!("unknown principal kind `{principal}`")));
    }
    let idempotency = entry
        .idempotency
        .clone()
        .unwrap_or_else(|| "none".to_owned());
    if !IDEMPOTENCY_KINDS.contains(&idempotency.as_str()) {
        return Err(bad(format!("unknown idempotency `{idempotency}`")));
    }
    let body_class = entry.body_class.clone().unwrap_or_else(|| {
        if entry.request.is_some() {
            "aex_json".to_owned()
        } else {
            "none".to_owned()
        }
    });
    if !BODY_CLASSES.contains(&body_class.as_str()) {
        return Err(bad(format!("unknown body class `{body_class}`")));
    }
    if (body_class == "aex_json") != entry.request.is_some() {
        return Err(bad(
            "exactly an `aex_json` body class declares a request schema".to_owned(),
        ));
    }
    let transport = entry
        .transport
        .clone()
        .unwrap_or_else(|| "unary".to_owned());
    if !TRANSPORTS.contains(&transport.as_str()) {
        return Err(bad(format!("unknown transport `{transport}`")));
    }
    let etag = entry.etag.clone().unwrap_or_else(|| "none".to_owned());
    if !ETAG_POLICIES.contains(&etag.as_str()) {
        return Err(bad(format!("unknown etag policy `{etag}`")));
    }
    for schema in entry.request.iter().chain(entry.success.iter()) {
        if !schemas.contains_key(schema) {
            return Err(bad(format!("unknown schema `{schema}`")));
        }
    }
    if entry.errors.is_empty() {
        return Err(bad(
            "every operation declares at least one error code".to_owned()
        ));
    }
    let mut errors: Vec<String> = entry.errors.clone();
    errors.dedup();
    for code in &errors {
        if !error_codes.contains(&code.as_str()) {
            return Err(bad(format!("unknown error code `{code}`")));
        }
    }
    // Derived, never authored: a deferred operation answers the refusal the
    // ledger implies, and `dispatch::declared` requires every code an endpoint
    // may emit to be in the route table. Authoring it into the fragments
    // instead would mean one hand edit per deferral and one more per landing,
    // each an opportunity to forget.
    if errors.iter().any(|code| code == NOT_IMPLEMENTED) {
        return Err(bad(format!(
            "`{NOT_IMPLEMENTED}` is derived from the deferral ledger and must not be authored"
        )));
    }
    if deferred_reason.is_some() {
        errors.push(NOT_IMPLEMENTED.to_owned());
    }
    // Registry order, not alphabetical: the generated `ErrorCode` discriminant
    // follows the registry, and a route's slice has to be sorted the same way or
    // a consumer cannot binary-search it.
    let position = |code: &String| error_codes.iter().position(|known| known == code);
    errors.sort_by_key(|code| position(code));
    errors.dedup();
    let success_status =
        entry
            .success_status
            .unwrap_or(if entry.success.is_some() { 200 } else { 204 });
    if transport != "binary" && entry.success.is_none() && success_status != 204 {
        return Err(bad("a bodyless success must be 204".to_owned()));
    }
    if transport == "binary" && (entry.success.is_some() || success_status != 200) {
        return Err(bad(
            "a binary transport is a bodyless-schema `200 application/octet-stream` success"
                .to_owned(),
        ));
    }
    if success_status == 204 && entry.success.is_some() {
        return Err(bad("a 204 declares no success schema".to_owned()));
    }
    // Every admission renders the same durable operation record, and the
    // generated `Accepted` response type has exactly one payload. A 202 carrying
    // anything else would make that type a lie.
    if success_status == 202 && entry.success.as_deref() != Some("Operation") {
        return Err(bad("a 202 admission answers with `Operation`".to_owned()));
    }

    let mut path_params = Vec::new();
    for segment in entry.path.split('/') {
        let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
            continue;
        };
        let declared = meta.path_params.get(name).ok_or_else(|| {
            bad(format!(
                "path parameter `{name}` is not in `routes-meta.yaml`"
            ))
        })?;
        path_params.push(ParamIr {
            rust: snake_case(name),
            ty: build_field_type("routes-meta.yaml", &entry.id, name, declared, id_keys)?,
            optional: false,
            doc: declared
                .doc
                .clone()
                .unwrap_or_else(|| format!("`{name}` path parameter.")),
            name: name.to_owned(),
        });
    }
    let mut query_params = Vec::new();
    for (name, declared) in &entry.query {
        query_params.push(ParamIr {
            rust: snake_case(name),
            ty: build_field_type("query", &entry.id, name, declared, id_keys)?,
            optional: declared.optional,
            doc: declared
                .doc
                .clone()
                .unwrap_or_else(|| format!("`{name}` query parameter.")),
            name: name.clone(),
        });
    }

    Ok(OperationIr {
        variant: pascal_case(&entry.id),
        id: entry.id.clone(),
        plane: plane.to_owned(),
        fragment: fragment.to_owned(),
        serving_artifact: owner.serving_artifact.clone(),
        served_artifact,
        deferred_reason,
        scenarios: owner.scenarios.clone(),
        method: entry.method.clone(),
        path: entry.path.clone(),
        summary: entry.summary.clone(),
        path_params,
        query_params,
        scope: entry.scope.clone(),
        alt_principal: entry.alt_principal.clone(),
        idempotency,
        request: entry.request.clone(),
        success_status,
        success: entry.success.clone(),
        errors,
        body_class,
        transport,
        etag,
        safe_retry: entry.safe_retry.unwrap_or(entry.method == "GET"),
        pause_exempt: entry.pause_exempt,
    })
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// `snake_case` or `kebab-case` to `PascalCase`.
///
/// A value that starts with a digit gets a `V` prefix, because `0644` is a valid
/// wire value and not a valid Rust identifier. The emitter then writes an
/// explicit `serde(rename)` for it, so the wire spelling is unaffected.
#[must_use]
pub fn pascal_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 1);
    if text.starts_with(|character: char| character.is_ascii_digit()) {
        out.push('V');
    }
    let mut capitalize = true;
    for character in text.chars() {
        if character == '_' || character == '-' || character == '.' || character == ':' {
            capitalize = true;
        } else if capitalize {
            out.extend(character.to_uppercase());
            capitalize = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// `camelCase` to `snake_case`, escaping Rust keywords.
#[must_use]
pub fn snake_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for (index, character) in text.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else if character == '-' || character == '.' {
            out.push('_');
        } else {
            out.push(character);
        }
    }
    if matches!(
        out.as_str(),
        "type" | "ref" | "match" | "move" | "self" | "in" | "as"
    ) {
        out.push('_');
    }
    out
}

/// The absolute repository root, resolved from this crate's manifest directory.
///
/// # Panics
///
/// Panics when the manifest directory has fewer than two ancestors, which
/// cannot happen for a workspace member at `tools/aex-contract-gen`.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/aex-contract-gen always has a workspace root two levels up")
        .to_path_buf()
}
