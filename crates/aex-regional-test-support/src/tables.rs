//! Loader and canonical bundler for `migrations/regional`.
//!
//! `migrations/regional/tables/*.json` is the source of truth for every regional
//! `DynamoDB` table. This module reads those files, validates each one against
//! `migrations/regional/schema.json`, and rebuilds the canonical bundle
//! `migrations/regional/generated/regional-tables.json` that Terraform consumes
//! through `jsondecode`.
//!
//! The bundle digest is taken over the rendered bytes of the table array, not
//! over a re-canonicalised document: the renderer emits fixed-order struct
//! fields with a two-space pretty printer, so the bytes are already canonical by
//! construction. That keeps the workspace at exactly one canonicaliser
//! (RFC 8785 JCS, owned by the contract stream) instead of introducing a second.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The schema identifier every table definition file declares.
pub const TABLE_SCHEMA: &str = "aex.regional-table.v1";

/// The schema identifier the generated bundle declares.
pub const BUNDLE_SCHEMA: &str = "aex.regional-tables.v1";

/// Monotone generation of the regional table contract.
///
/// This is the first prelaunch generation. Increment it only when a new
/// generated table contract must not be admitted as the same regional schema
/// generation as its predecessor.
pub const BUNDLE_GENERATION: u32 = 1;

/// `DynamoDB`'s hard maximum number of explicitly projected non-key attributes
/// on one `INCLUDE` secondary index.
pub const INCLUDE_ATTRIBUTES_PER_INDEX_MAX: usize = 20;

/// `DynamoDB`'s hard maximum number of explicitly projected non-key attributes,
/// summed across every `INCLUDE` secondary index on one table. Repeated names
/// count once per index, exactly as the service counts them.
pub const INCLUDE_ATTRIBUTES_PER_TABLE_MAX: usize = 100;

/// Why a table definition could not be read, validated or bundled.
#[derive(Debug, thiserror::Error)]
pub enum TableError {
    /// A definition directory or file could not be read.
    #[error("cannot read `{path}`: {source}")]
    Read {
        /// The offending path, relative to the repository root.
        path: String,
        /// The underlying I/O failure.
        source: std::io::Error,
    },
    /// A definition file is not valid JSON.
    #[error("`{path}` is not valid JSON: {source}")]
    Json {
        /// The offending path.
        path: String,
        /// The underlying parse failure.
        source: serde_json::Error,
    },
    /// A definition file does not satisfy `migrations/regional/schema.json`.
    #[error("`{path}` violates migrations/regional/schema.json: {detail}")]
    Schema {
        /// The offending path.
        path: String,
        /// Every violation, newline separated.
        detail: String,
    },
    /// The file stem and the declared logical table name disagree.
    #[error("`{path}` declares table `{declared}` but the file stem is `{stem}`")]
    NameMismatch {
        /// The offending path.
        path: String,
        /// The name inside the file.
        declared: String,
        /// The name the file is stored under.
        stem: String,
    },
    /// A key or index attribute is not declared in `attributes`.
    #[error("table `{table}` uses attribute `{attribute}` in `{position}` but never declares it")]
    UndeclaredAttribute {
        /// The offending table.
        table: String,
        /// The attribute that has no declaration.
        attribute: String,
        /// Where the undeclared use appeared.
        position: String,
    },
    /// An attribute is declared but never used as a key or an index key.
    #[error("table `{table}` declares attribute `{attribute}` that no key schema or index uses")]
    UnusedAttribute {
        /// The offending table.
        table: String,
        /// The attribute nothing references.
        attribute: String,
    },
    /// One `INCLUDE` index exceeds `DynamoDB`'s hard `NonKeyAttributes` bound.
    #[error(
        "table `{table}` index `{index}` projects {observed} non-key attributes, above DynamoDB's {limit}-attribute per-index limit"
    )]
    ProjectionIndexLimit {
        /// The offending table.
        table: String,
        /// The offending index.
        index: String,
        /// The projected non-key attribute count.
        observed: usize,
        /// `DynamoDB`'s hard per-index limit.
        limit: usize,
    },
    /// The sum of all `INCLUDE` projections exceeds `DynamoDB`'s table bound.
    #[error(
        "table `{table}` projects {observed} non-key attributes across its indexes, above DynamoDB's {limit}-attribute per-table limit"
    )]
    ProjectionTableLimit {
        /// The offending table.
        table: String,
        /// The projected non-key attribute count summed across indexes.
        observed: usize,
        /// `DynamoDB`'s hard per-table limit.
        limit: usize,
    },
    /// Two definitions claim the same logical table name.
    #[error("logical table `{table}` is defined twice")]
    Duplicate {
        /// The duplicated name.
        table: String,
    },
    /// No definition files were found at all.
    #[error("`{path}` holds no table definition; an empty regional plane is never correct")]
    Empty {
        /// The directory that was searched.
        path: String,
    },
}

/// A `DynamoDB` attribute declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeDefinition {
    /// Attribute name.
    pub name: String,
    /// `DynamoDB` scalar attribute type.
    #[serde(rename = "type")]
    pub attribute_type: String,
}

/// The base table key schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySchema {
    /// Partition key attribute.
    pub partition: String,
    /// Sort key attribute.
    pub sort: String,
}

/// A secondary index projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    /// `INCLUDE` or `KEYS_ONLY`; `ALL` is rejected by the schema (D-29).
    #[serde(rename = "type")]
    pub projection_type: String,
    /// The exhaustive attribute list a query over this index may observe, empty
    /// on a `KEYS_ONLY` index.
    pub attributes: Vec<String>,
}

/// A global secondary index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalSecondaryIndex {
    /// Index name.
    pub name: String,
    /// Index partition key attribute.
    pub partition: String,
    /// Index sort key attribute.
    pub sort: String,
    /// Always true: only items carrying the index attributes appear.
    pub sparse: bool,
    /// Whether this index deliberately projects the record body.
    ///
    /// A body-shaped attribute may appear in a projection only where this is
    /// true. It is the declared exception to D-29, not a hole in it: the
    /// observation indexes carry the record a query returns, and every other
    /// index must stay unable to surface a prompt, a body or a receipt.
    #[serde(rename = "projectsRecordBody")]
    pub projects_record_body: bool,
    /// Why the index exists in this shape.
    pub rationale: String,
    /// The projection.
    pub projection: Projection,
}

/// Point-in-time recovery configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointInTimeRecovery {
    /// Always enabled.
    pub enabled: bool,
    /// Retention window in days.
    #[serde(rename = "retentionDays")]
    pub retention_days: u32,
}

/// Server-side encryption configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSideEncryption {
    /// Always `KMS`.
    #[serde(rename = "type")]
    pub encryption_type: String,
    /// The authority whose customer managed key encrypts this table.
    ///
    /// An authority id, never a physical alias: two planes share one account, so
    /// the alias is `alias/aex-{plane}-{region}-{authority}` and only an
    /// environment root can compose it.
    #[serde(rename = "keyAuthority")]
    pub key_authority: String,
    /// Why this table has the key it has.
    pub rationale: String,
}

/// Stream configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Whether a stream exists at all.
    pub enabled: bool,
    /// The view type, or `NONE` when the stream is disabled.
    #[serde(rename = "viewType")]
    pub view_type: String,
    /// The consumers permitted to read it.
    pub consumers: Vec<String>,
    /// Why the view type is the least privilege that works.
    pub rationale: String,
}

/// Time-to-live configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeToLive {
    /// Whether TTL is enabled.
    pub enabled: bool,
    /// The TTL attribute, empty when disabled.
    pub attribute: String,
    /// The item types the TTL attribute is written on.
    #[serde(rename = "appliesTo")]
    pub applies_to: Vec<String>,
    /// Why reclaiming these rows by TTL is safe.
    pub rationale: String,
}

/// One role's least-privilege action list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamGrant {
    /// The deployable role name.
    pub role: String,
    /// The permitted `DynamoDB` actions.
    pub actions: Vec<String>,
    /// Which resource ARNs the actions apply to.
    pub resources: Vec<String>,
    /// The item families a write grant owns. Empty for read-only grants and for
    /// older authorities that have not yet published item-level capability
    /// metadata.
    #[serde(rename = "itemTypes", default, skip_serializing_if = "Vec::is_empty")]
    pub item_types: Vec<String>,
    /// Optional request-shape restriction applied to this statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<IamCondition>,
}

/// One generated IAM request condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamCondition {
    /// IAM condition operator, including any required set qualifier.
    pub operator: String,
    /// IAM condition context key.
    pub key: String,
    /// Accepted request-context values.
    pub values: Vec<String>,
}

/// One regional table generation definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableDefinition {
    /// Schema identifier.
    pub schema: String,
    /// Logical table name.
    pub table: String,
    /// Owning implementation stream.
    pub owner: String,
    /// What the table is for.
    pub purpose: String,
    /// Always `PAY_PER_REQUEST`.
    #[serde(rename = "billingMode")]
    pub billing_mode: String,
    /// Always true.
    #[serde(rename = "deletionProtection")]
    pub deletion_protection: bool,
    /// Set on a table whose physical name may not be derived from the
    /// environment prefix. The name itself is an environment decision and is
    /// never carried here; the flag is what makes a derived name a refusal
    /// rather than a silent rename.
    #[serde(
        rename = "physicalNamePinned",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub physical_name_pinned: Option<bool>,
    /// Point-in-time recovery.
    #[serde(rename = "pointInTimeRecovery")]
    pub point_in_time_recovery: PointInTimeRecovery,
    /// Server-side encryption.
    #[serde(rename = "serverSideEncryption")]
    pub server_side_encryption: ServerSideEncryption,
    /// Key and index attribute declarations.
    pub attributes: Vec<AttributeDefinition>,
    /// The base table key schema.
    #[serde(rename = "keySchema")]
    pub key_schema: KeySchema,
    /// Global secondary indexes.
    #[serde(rename = "globalSecondaryIndexes")]
    pub global_secondary_indexes: Vec<GlobalSecondaryIndex>,
    /// Stream configuration.
    pub stream: StreamConfig,
    /// Time-to-live configuration.
    #[serde(rename = "timeToLive")]
    pub time_to_live: TimeToLive,
    /// The closed `itemType` discriminator vocabulary.
    #[serde(rename = "itemTypes")]
    pub item_types: Vec<String>,
    /// Least-privilege IAM action lists.
    pub iam: Vec<IamGrant>,
    /// Resource tags.
    pub tags: BTreeMap<String, String>,
}

/// The canonical bundle Terraform consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableBundle {
    /// Bundle schema identifier.
    pub schema: String,
    /// Authored monotone regional schema generation.
    pub generation: u32,
    /// `blake3:<64 hex>` over the rendered table array.
    pub digest: String,
    /// Every table, ordered by logical name.
    pub tables: Vec<TableDefinition>,
}

/// The repository root, derived from this crate's manifest directory.
///
/// # Panics
///
/// Panics when the manifest directory has fewer than two ancestors, which
/// cannot happen for a member under `crates/<name>`.
#[must_use]
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name> always has two ancestors")
        .to_path_buf()
}

/// The directory holding the per-table definition files.
#[must_use]
pub fn definitions_directory() -> PathBuf {
    repository_root()
        .join("migrations")
        .join("regional")
        .join("tables")
}

/// The checked-in generated bundle path.
#[must_use]
pub fn bundle_path() -> PathBuf {
    repository_root()
        .join("migrations")
        .join("regional")
        .join("generated")
        .join("regional-tables.json")
}

/// The JSON Schema every definition is validated against.
#[must_use]
pub fn schema_path() -> PathBuf {
    repository_root()
        .join("migrations")
        .join("regional")
        .join("schema.json")
}

fn read(path: &Path) -> Result<String, TableError> {
    std::fs::read_to_string(path).map_err(|source| TableError::Read {
        path: display(path),
        source,
    })
}

fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Validate one parsed definition document against the JSON Schema.
///
/// # Errors
///
/// Returns [`TableError::Schema`] listing every violation, or
/// [`TableError::Json`] when the schema document itself cannot be read.
pub fn validate_document(path: &str, document: &serde_json::Value) -> Result<(), TableError> {
    let schema_text = read(&schema_path())?;
    let schema: serde_json::Value =
        serde_json::from_str(&schema_text).map_err(|source| TableError::Json {
            path: "migrations/regional/schema.json".to_owned(),
            source,
        })?;
    let validator = jsonschema::validator_for(&schema).map_err(|error| TableError::Schema {
        path: "migrations/regional/schema.json".to_owned(),
        detail: error.to_string(),
    })?;
    let failures: Vec<String> = validator
        .iter_errors(document)
        .map(|error| {
            let pointer = error.instance_path().to_string();
            format!("{error} at {pointer}")
        })
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    Err(TableError::Schema {
        path: path.to_owned(),
        detail: failures.join("\n"),
    })
}

/// Structural checks the JSON Schema cannot express.
///
/// # Errors
///
/// Returns [`TableError::UndeclaredAttribute`] when a key or index key names an
/// attribute the file never declares, and [`TableError::UnusedAttribute`] when a
/// declared attribute is never used — both of which produce a table AWS refuses
/// to create, and both of which are silent until a deploy.
pub fn check_attribute_closure(definition: &TableDefinition) -> Result<(), TableError> {
    let declared: Vec<&str> = definition
        .attributes
        .iter()
        .map(|attribute| attribute.name.as_str())
        .collect();
    let mut used: Vec<&str> = vec![
        definition.key_schema.partition.as_str(),
        definition.key_schema.sort.as_str(),
    ];
    for index in &definition.global_secondary_indexes {
        used.push(index.partition.as_str());
        used.push(index.sort.as_str());
    }
    for (position, attribute) in [
        (
            "keySchema.partition",
            definition.key_schema.partition.as_str(),
        ),
        ("keySchema.sort", definition.key_schema.sort.as_str()),
    ] {
        if !declared.contains(&attribute) {
            return Err(TableError::UndeclaredAttribute {
                table: definition.table.clone(),
                attribute: attribute.to_owned(),
                position: position.to_owned(),
            });
        }
    }
    for index in &definition.global_secondary_indexes {
        for (suffix, attribute) in [
            ("partition", index.partition.as_str()),
            ("sort", index.sort.as_str()),
        ] {
            if !declared.contains(&attribute) {
                return Err(TableError::UndeclaredAttribute {
                    table: definition.table.clone(),
                    attribute: attribute.to_owned(),
                    position: format!("{}.{suffix}", index.name),
                });
            }
        }
    }
    for attribute in declared {
        if !used.contains(&attribute) {
            return Err(TableError::UnusedAttribute {
                table: definition.table.clone(),
                attribute: attribute.to_owned(),
            });
        }
    }
    Ok(())
}

/// Enforces `DynamoDB`'s non-adjustable secondary-index projection bounds before
/// a generated bundle can reach Terraform or `CreateTable`.
///
/// # Errors
///
/// Returns [`TableError::ProjectionIndexLimit`] when one `INCLUDE` index names
/// more than twenty non-key attributes, or
/// [`TableError::ProjectionTableLimit`] when the sum across a table exceeds one
/// hundred. `KEYS_ONLY` indexes contribute zero.
pub fn check_projection_limits(definition: &TableDefinition) -> Result<(), TableError> {
    let mut table_total = 0_usize;
    for index in &definition.global_secondary_indexes {
        if index.projection.projection_type != "INCLUDE" {
            continue;
        }
        let observed = index.projection.attributes.len();
        if observed > INCLUDE_ATTRIBUTES_PER_INDEX_MAX {
            return Err(TableError::ProjectionIndexLimit {
                table: definition.table.clone(),
                index: index.name.clone(),
                observed,
                limit: INCLUDE_ATTRIBUTES_PER_INDEX_MAX,
            });
        }
        table_total = table_total.saturating_add(observed);
    }
    if table_total > INCLUDE_ATTRIBUTES_PER_TABLE_MAX {
        return Err(TableError::ProjectionTableLimit {
            table: definition.table.clone(),
            observed: table_total,
            limit: INCLUDE_ATTRIBUTES_PER_TABLE_MAX,
        });
    }
    Ok(())
}

/// Read, validate and sort every table definition under `directory`.
///
/// # Errors
///
/// Returns the first [`TableError`] encountered; the directory is read in sorted
/// order so the failure is deterministic.
pub fn load_all(directory: &Path) -> Result<Vec<TableDefinition>, TableError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|source| TableError::Read {
            path: display(directory),
            source,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    entries.sort();

    let mut tables: Vec<TableDefinition> = Vec::new();
    for path in entries {
        let shown = display(&path);
        let text = read(&path)?;
        let document: serde_json::Value =
            serde_json::from_str(&text).map_err(|source| TableError::Json {
                path: shown.clone(),
                source,
            })?;
        validate_document(&shown, &document)?;
        let definition: TableDefinition =
            serde_json::from_value(document).map_err(|source| TableError::Json {
                path: shown.clone(),
                source,
            })?;
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        if stem != definition.table {
            return Err(TableError::NameMismatch {
                path: shown,
                declared: definition.table,
                stem,
            });
        }
        check_attribute_closure(&definition)?;
        check_projection_limits(&definition)?;
        if tables
            .iter()
            .any(|existing| existing.table == definition.table)
        {
            return Err(TableError::Duplicate {
                table: definition.table,
            });
        }
        tables.push(definition);
    }
    if tables.is_empty() {
        return Err(TableError::Empty {
            path: display(directory),
        });
    }
    tables.sort_by(|left, right| left.table.cmp(&right.table));
    Ok(tables)
}

/// Build the canonical bundle from an ordered table list.
///
/// # Panics
///
/// Panics only if `serde_json` cannot serialise a value it just deserialised,
/// which would be a `serde_json` defect rather than a data condition.
#[must_use]
pub fn bundle(tables: Vec<TableDefinition>) -> TableBundle {
    let body = serde_json::to_string_pretty(&tables).expect("table definitions re-serialise");
    let digest = blake3::hash(body.as_bytes());
    TableBundle {
        schema: BUNDLE_SCHEMA.to_owned(),
        generation: BUNDLE_GENERATION,
        digest: format!("blake3:{}", hex::encode(digest.as_bytes())),
        tables,
    }
}

/// Render the bundle exactly as `regional-tables.json` stores it.
///
/// # Panics
///
/// Panics only on a `serde_json` serialisation defect.
#[must_use]
pub fn render(bundle: &TableBundle) -> String {
    let mut text = serde_json::to_string_pretty(bundle).expect("the bundle re-serialises");
    text.push('\n');
    text
}

/// Load the definitions and rebuild the bundle in one step.
///
/// # Errors
///
/// Propagates every [`TableError`] from [`load_all`].
pub fn rebuild() -> Result<TableBundle, TableError> {
    Ok(bundle(load_all(&definitions_directory())?))
}

/// Read the checked-in bundle.
///
/// # Errors
///
/// Returns [`TableError::Read`] when the generated file is absent and
/// [`TableError::Json`] when it does not parse.
pub fn read_bundle() -> Result<TableBundle, TableError> {
    let path = bundle_path();
    let text = read(&path)?;
    serde_json::from_str(&text).map_err(|source| TableError::Json {
        path: display(&path),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AttributeDefinition, GlobalSecondaryIndex, KeySchema, Projection, TableDefinition,
        TableError, bundle, check_attribute_closure, check_projection_limits,
        definitions_directory, load_all, rebuild, render,
    };

    fn sample() -> TableDefinition {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        tables
            .into_iter()
            .find(|table| table.table == "session-authority")
            .expect("session-authority is defined")
    }

    #[test]
    fn every_checked_in_definition_validates_against_the_schema() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let names: Vec<&str> = tables.iter().map(|table| table.table.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "observation-authority",
                "regional-authz-projection",
                "regional-capacity-authority",
                "regional-content",
                "regional-registry",
                "regional-secret-custody",
                "regional-secret-keystore",
                "regional-work",
                "runtime-activity",
                "session-authority",
                "usage-compute-authority",
                "usage-query-projection",
                "usage-storage-authority",
                "usage-transfer-authority",
            ]
        );
    }

    #[test]
    fn no_index_projects_all() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            for index in &table.global_secondary_indexes {
                assert!(
                    ["INCLUDE", "KEYS_ONLY"].contains(&index.projection.projection_type.as_str()),
                    "`{}` index `{}` projects {}",
                    table.table,
                    index.name,
                    index.projection.projection_type
                );
                if index.projection.projection_type == "KEYS_ONLY" {
                    assert!(
                        index.projection.attributes.is_empty(),
                        "`{}` index `{}` is KEYS_ONLY and still names projected attributes",
                        table.table,
                        index.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_projection_stays_inside_dynamodb_hard_limits() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            check_projection_limits(&table).expect("the definition is creatable by DynamoDB");
        }
    }

    #[test]
    fn an_include_projection_above_twenty_attributes_is_rejected_before_bundle_generation() {
        let mut table = sample();
        table.global_secondary_indexes[0].projection.projection_type = "INCLUDE".to_owned();
        table.global_secondary_indexes[0].projection.attributes =
            (0..21).map(|value| format!("field{value}")).collect();
        assert!(matches!(
            check_projection_limits(&table),
            Err(TableError::ProjectionIndexLimit {
                observed: 21,
                limit: 20,
                ..
            })
        ));
    }

    #[test]
    fn a_table_projection_sum_above_one_hundred_is_rejected_before_bundle_generation() {
        let mut table = sample();
        let template = table.global_secondary_indexes[1].clone();
        table.global_secondary_indexes = (0..6)
            .map(|position| {
                let mut index = template.clone();
                index.name = format!("gsi_limit_{position}");
                index.projection.projection_type = "INCLUDE".to_owned();
                index.projection.attributes = (0..20)
                    .map(|attribute| format!("field{position}_{attribute}"))
                    .collect();
                index
            })
            .collect();
        assert!(matches!(
            check_projection_limits(&table),
            Err(TableError::ProjectionTableLimit {
                observed: 120,
                limit: 100,
                ..
            })
        ));
    }

    /// Attribute names that carry a record body, a prompt or a receipt.
    const BODY_SHAPED: [&str; 11] = [
        "bodyInline",
        "bodyDigest",
        "contentInline",
        "ciphertext",
        "responseInline",
        "responseDigest",
        "payload",
        "resolvedConfig",
        "resultInline",
        "enc",
        "authorityDocument",
    ];

    #[test]
    fn no_projection_can_carry_a_body_a_prompt_or_a_receipt() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            for index in &table.global_secondary_indexes {
                if index.projects_record_body {
                    continue;
                }
                for attribute in &index.projection.attributes {
                    assert!(
                        !BODY_SHAPED.contains(&attribute.as_str()),
                        "`{}` index `{}` projects `{attribute}` without declaring \
                         `projectsRecordBody`",
                        table.table,
                        index.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_body_projection_is_declared_only_where_a_body_is_actually_projected() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            for index in &table.global_secondary_indexes {
                if !index.projects_record_body {
                    continue;
                }
                assert!(
                    index
                        .projection
                        .attributes
                        .iter()
                        .any(|attribute| BODY_SHAPED.contains(&attribute.as_str())),
                    "`{}` index `{}` declares `projectsRecordBody` and projects no body; the \
                     exception must never be wider than the thing it excepts",
                    table.table,
                    index.name
                );
            }
        }
    }

    #[test]
    fn regional_stream_holds_only_the_authority_reads_and_two_wake_streams_it_uses() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let mut grants = Vec::new();
        for table in &tables {
            for grant in &table.iam {
                if grant.role == "regional-stream" {
                    grants.push((
                        table.table.as_str(),
                        grant.actions.as_slice(),
                        grant.resources.as_slice(),
                    ));
                }
            }
        }
        grants.sort_by(|left, right| (left.0, left.2.join(",")).cmp(&(right.0, right.2.join(","))));

        let strings = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        let mut expected = vec![
            (
                "observation-authority",
                strings(&[
                    "dynamodb:DescribeTable",
                    "dynamodb:GetItem",
                    "dynamodb:BatchGetItem",
                    "dynamodb:Query",
                ]),
                strings(&["table", "index/*"]),
            ),
            (
                "observation-authority",
                strings(&[
                    "dynamodb:GetRecords",
                    "dynamodb:GetShardIterator",
                    "dynamodb:DescribeStream",
                ]),
                strings(&["stream"]),
            ),
            (
                "regional-authz-projection",
                // `TransactGetItems` is the one-request admission snapshot: the
                // key authorization row, the placement and the hot limit subset
                // are read together so they cannot describe different instants.
                strings(&[
                    "dynamodb:GetItem",
                    "dynamodb:TransactGetItems",
                    "dynamodb:Query",
                ]),
                strings(&["table"]),
            ),
            (
                "session-authority",
                // `BatchGetItem` is the batched frontier bundle: one follow cycle
                // reads the session head beside every signal frontier in one
                // request, which spans both authorities.
                strings(&[
                    "dynamodb:DescribeTable",
                    "dynamodb:GetItem",
                    "dynamodb:BatchGetItem",
                    "dynamodb:Query",
                ]),
                strings(&["table", "index/*"]),
            ),
            (
                "session-authority",
                strings(&[
                    "dynamodb:GetRecords",
                    "dynamodb:GetShardIterator",
                    "dynamodb:DescribeStream",
                ]),
                strings(&["stream"]),
            ),
        ];
        expected
            .sort_by(|left, right| (left.0, left.2.join(",")).cmp(&(right.0, right.2.join(","))));
        let actual = grants
            .into_iter()
            .map(|(table, actions, resources)| (table, actions.to_vec(), resources.to_vec()))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn regional_observation_api_holds_only_its_query_and_export_control_tables() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let mut grants = Vec::new();
        for table in &tables {
            for grant in &table.iam {
                if grant.role == "regional-observation-api" {
                    grants.push((
                        table.table.as_str(),
                        grant.actions.as_slice(),
                        grant.resources.as_slice(),
                    ));
                }
            }
        }
        grants.sort_by(|left, right| (left.0, left.1.join(",")).cmp(&(right.0, right.1.join(","))));

        let strings = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        let expected = vec![
            (
                "observation-authority",
                strings(&[
                    "dynamodb:DescribeTable",
                    "dynamodb:GetItem",
                    "dynamodb:BatchGetItem",
                    "dynamodb:Query",
                ]),
                strings(&["table", "index/*"]),
            ),
            (
                "observation-authority",
                strings(&["dynamodb:PutItem", "dynamodb:UpdateItem"]),
                strings(&["table"]),
            ),
            (
                "regional-authz-projection",
                // `TransactGetItems` is the one-request admission snapshot: the
                // key authorization row, the placement and the hot limit subset
                // are read together so they cannot describe different instants.
                strings(&[
                    "dynamodb:GetItem",
                    "dynamodb:TransactGetItems",
                    "dynamodb:Query",
                ]),
                strings(&["table"]),
            ),
            (
                "session-authority",
                // `BatchGetItem` is the batched frontier bundle: a session-scoped
                // query reads the session head beside every signal frontier in
                // one request, which spans both authorities.
                strings(&[
                    "dynamodb:DescribeTable",
                    "dynamodb:GetItem",
                    "dynamodb:BatchGetItem",
                    "dynamodb:Query",
                ]),
                strings(&["table", "index/*"]),
            ),
        ];
        let mut expected = expected;
        expected
            .sort_by(|left, right| (left.0, left.1.join(",")).cmp(&(right.0, right.1.join(","))));
        let actual = grants
            .into_iter()
            .map(|(table, actions, resources)| (table, actions.to_vec(), resources.to_vec()))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn observation_api_writes_are_restricted_to_export_partition_keys() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let table = tables
            .iter()
            .find(|table| table.table == "observation-authority")
            .expect("the observation authority is declared");
        let writes = table
            .iam
            .iter()
            .filter(|grant| {
                grant.role == "regional-observation-api"
                    && grant.actions.iter().any(|action| {
                        matches!(action.as_str(), "dynamodb:PutItem" | "dynamodb:UpdateItem")
                    })
            })
            .collect::<Vec<_>>();

        assert_eq!(writes.len(), 1);
        let write = writes[0];
        assert_eq!(write.resources, ["table"]);
        let condition = write
            .condition
            .as_ref()
            .expect("export writes carry a leading-key condition");
        assert_eq!(condition.operator, "ForAllValues:StringLike");
        assert_eq!(condition.key, "dynamodb:LeadingKeys");
        assert_eq!(condition.values, ["EXPORT#*"]);
    }

    #[test]
    fn authz_projection_write_capabilities_are_disjoint_and_key_enforced() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let table = tables
            .iter()
            .find(|table| table.table == "regional-authz-projection")
            .expect("the authorization projection is declared");
        let writers = table
            .iam
            .iter()
            .filter(|grant| !grant.item_types.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(writers.len(), 2, "exactly two authorities own row families");

        let mut owned = std::collections::BTreeSet::new();
        for writer in &writers {
            assert!(
                writer.actions.iter().all(|action| matches!(
                    action.as_str(),
                    "dynamodb:GetItem" | "dynamodb:PutItem"
                )),
                "{} holds a broad action: {:?}",
                writer.role,
                writer.actions
            );
            for item_type in &writer.item_types {
                assert!(
                    owned.insert(item_type.as_str()),
                    "`{item_type}` has more than one write owner"
                );
            }
        }
        assert_eq!(
            owned,
            table
                .item_types
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
        );

        let central = writers
            .iter()
            .find(|grant| grant.role == "central-control-worker")
            .expect("central control owns its row families");
        assert!(
            !central
                .item_types
                .iter()
                .any(|kind| kind == "workspace_limit")
        );
        assert_eq!(central.actions, ["dynamodb:PutItem"]);
        assert_eq!(
            central
                .condition
                .as_ref()
                .expect("central writes are key restricted")
                .values,
            ["WS#*", "KEY#*", "FEED"]
        );

        let capacity = writers
            .iter()
            .find(|grant| grant.role == "regional-capacity-controller")
            .expect("regional capacity owns workspace limits");
        assert_eq!(
            capacity.item_types,
            [
                "workspace_limit",
                "workspace_limit_bundle_head",
                "workspace_limit_bundle",
                "workspace_edge_limits"
            ]
        );
        assert_eq!(
            capacity.actions,
            ["dynamodb:PutItem"],
            "transactional Put actions are authorized by the underlying PutItem permission"
        );
        // The hot admission subset is filed under the capacity partition rather
        // than the workspace's. `dynamodb:LeadingKeys` is the only key this
        // fence can condition on, so a `WS#` spelling would have handed the
        // capacity authority the partition that holds placement.
        assert_eq!(
            capacity
                .condition
                .as_ref()
                .expect("capacity writes are key restricted")
                .values,
            ["LIMIT#*"]
        );
    }

    #[test]
    fn capacity_authority_has_one_key_scoped_underlying_put_writer() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let table = tables
            .iter()
            .find(|table| table.table == "regional-capacity-authority")
            .expect("the capacity authority is declared");

        assert_eq!(
            table.server_side_encryption.key_authority,
            "regional-capacity"
        );
        assert_eq!(table.item_types, ["workspace_capacity", "capacity_audit"]);
        assert!(table.global_secondary_indexes.is_empty());
        assert!(!table.stream.enabled);
        assert!(!table.time_to_live.enabled);
        assert_eq!(table.iam.len(), 1);

        let writer = &table.iam[0];
        assert_eq!(writer.role, "regional-capacity-controller");
        assert_eq!(
            writer.actions,
            ["dynamodb:GetItem", "dynamodb:PutItem"],
            "transactional Put actions are authorized by the underlying PutItem permission"
        );
        assert_eq!(writer.resources, ["table"]);
        assert_eq!(writer.item_types, ["workspace_capacity", "capacity_audit"]);
        let condition = writer
            .condition
            .as_ref()
            .expect("capacity authority writes are key restricted");
        assert_eq!(condition.operator, "ForAllValues:StringLike");
        assert_eq!(condition.key, "dynamodb:LeadingKeys");
        assert_eq!(condition.values, ["WS#*"]);
    }

    #[test]
    fn the_event_indexes_project_every_field_their_decoder_requires() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let session = tables
            .iter()
            .find(|table| table.table == "session-authority")
            .expect("the session authority is declared");
        for index_name in ["gsi_workspace_events", "gsi_session_events"] {
            let index = session
                .global_secondary_indexes
                .iter()
                .find(|index| index.name == index_name)
                .unwrap_or_else(|| panic!("the {index_name} index is declared"));
            for required in [
                "itemType",
                "workspaceId",
                "sessionId",
                "eventSeq",
                "eventId",
                "type",
                "bodyDigest",
                "occurredAt",
                "outboxState",
            ] {
                assert!(
                    index
                        .projection
                        .attributes
                        .iter()
                        .any(|name| name == required),
                    "{index_name} omits decoder field `{required}`"
                );
            }
            // The regression guard for D2d: an inline body of up to 32 KiB
            // would otherwise be stored once per event index. An event that
            // projects neither body attribute is hydrated from the base row
            // by its reader instead.
            assert!(
                !index
                    .projection
                    .attributes
                    .iter()
                    .any(|name| name == "bodyInline"),
                "{index_name} must not carry a dense copy of every inline event body"
            );
        }
    }

    #[test]
    fn the_generated_bundle_matches_a_deterministic_rebuild() {
        let rebuilt = rebuild().expect("the bundle rebuilds");
        let checked_in = super::read_bundle().expect("the generated bundle is checked in");
        assert_eq!(
            render(&rebuilt),
            render(&checked_in),
            "migrations/regional/generated/regional-tables.json is stale; rerun the bundler"
        );
    }

    #[test]
    fn the_bundle_digest_is_stable_across_two_builds() {
        let first = rebuild().expect("the bundle rebuilds");
        let second = rebuild().expect("the bundle rebuilds");
        assert_eq!(first.generation, 1);
        assert_eq!(first.digest, second.digest);
        assert!(first.digest.starts_with("blake3:"));
        assert_eq!(first.digest.len(), "blake3:".len() + 64);
    }

    #[test]
    fn the_digest_changes_when_a_table_changes() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let baseline = bundle(tables.clone()).digest;
        let mut mutated = tables;
        mutated[0].purpose.push_str(" and one more sentence.");
        assert_ne!(baseline, bundle(mutated).digest);
    }

    #[test]
    fn an_undeclared_index_attribute_is_rejected() {
        let mut definition = sample();
        definition
            .global_secondary_indexes
            .push(GlobalSecondaryIndex {
                name: "gsi_ghost".to_owned(),
                partition: "ghostPk".to_owned(),
                sort: "ghostSk".to_owned(),
                sparse: true,
                projects_record_body: false,
                rationale: "a deliberate defect".to_owned(),
                projection: Projection {
                    projection_type: "INCLUDE".to_owned(),
                    attributes: vec!["sessionId".to_owned()],
                },
            });
        let error = check_attribute_closure(&definition).expect_err("the ghost index is rejected");
        assert!(
            matches!(&error, TableError::UndeclaredAttribute { attribute, .. } if attribute == "ghostPk"),
            "{error}"
        );
    }

    #[test]
    fn a_declared_but_unused_attribute_is_rejected() {
        let mut definition = sample();
        definition.attributes.push(AttributeDefinition {
            name: "orphanAttribute".to_owned(),
            attribute_type: "S".to_owned(),
        });
        let error = check_attribute_closure(&definition).expect_err("the orphan is rejected");
        assert!(
            matches!(&error, TableError::UnusedAttribute { attribute, .. } if attribute == "orphanAttribute"),
            "{error}"
        );
    }

    #[test]
    fn a_key_schema_naming_an_undeclared_attribute_is_rejected() {
        let mut definition = sample();
        definition.key_schema = KeySchema {
            partition: "hashKey".to_owned(),
            sort: "rangeKey".to_owned(),
        };
        let error = check_attribute_closure(&definition).expect_err("the key schema is rejected");
        assert!(
            matches!(&error, TableError::UndeclaredAttribute { position, .. } if position == "keySchema.partition"),
            "{error}"
        );
    }

    #[test]
    fn every_table_carries_deletion_protection_and_thirty_five_day_pitr() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            assert!(table.deletion_protection, "{}", table.table);
            assert!(table.point_in_time_recovery.enabled, "{}", table.table);
            assert_eq!(table.point_in_time_recovery.retention_days, 35);
            assert_eq!(table.billing_mode, "PAY_PER_REQUEST");
        }
    }

    #[test]
    fn the_three_secret_and_content_tables_hold_distinct_keys() {
        let tables = load_all(&definitions_directory()).expect("the definitions load");
        let authorities: Vec<&str> = tables
            .iter()
            .map(|table| table.server_side_encryption.key_authority.as_str())
            .collect();
        let mut unique = authorities.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            authorities.len(),
            unique.len(),
            "two regional tables share a customer managed key: {authorities:?}"
        );
    }

    #[test]
    fn no_key_authority_is_a_physical_alias() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            let authority = &table.server_side_encryption.key_authority;
            assert!(
                !authority.starts_with("alias/"),
                "`{}` names the physical alias `{authority}`; two planes share one account and \
                 cannot both own an alias, so the bundle carries the authority id and the \
                 environment root composes `alias/aex-{{plane}}-{{region}}-{{authority}}`",
                table.table
            );
        }
    }

    #[test]
    fn exactly_one_table_pins_its_physical_name() {
        let pinned: Vec<String> = load_all(&definitions_directory())
            .expect("the definitions load")
            .into_iter()
            .filter(|table| table.physical_name_pinned == Some(true))
            .map(|table| table.table)
            .collect();
        assert_eq!(
            pinned,
            vec!["regional-secret-keystore".to_owned()],
            "the hierarchical keyring's branch-key store is the one table whose physical name \
             is bound to its contents; any other pinned name is an environment escaping its prefix"
        );
    }

    #[test]
    fn a_disabled_stream_names_no_consumer_and_a_disabled_ttl_names_no_attribute() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            if !table.stream.enabled {
                assert_eq!(table.stream.view_type, "NONE", "{}", table.table);
                assert!(table.stream.consumers.is_empty(), "{}", table.table);
            }
            if table.time_to_live.enabled {
                assert_eq!(table.time_to_live.attribute, "expiresAtEpochSeconds");
                assert!(!table.time_to_live.applies_to.is_empty(), "{}", table.table);
            } else {
                assert!(table.time_to_live.attribute.is_empty(), "{}", table.table);
                assert!(table.time_to_live.applies_to.is_empty(), "{}", table.table);
            }
        }
    }

    #[test]
    fn a_ttl_only_ever_applies_to_a_declared_item_type() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            for item_type in &table.time_to_live.applies_to {
                assert!(
                    table.item_types.contains(item_type),
                    "`{}` TTLs `{item_type}`, which is not in its itemType vocabulary",
                    table.table
                );
            }
        }
    }

    #[test]
    fn the_secret_tables_expose_no_stream_at_all() {
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            if table.tags.get("aex:data-class").map(String::as_str) == Some("secret") {
                assert!(
                    !table.stream.enabled,
                    "`{}` exposes secret material on a change feed",
                    table.table
                );
            }
        }
    }
}
