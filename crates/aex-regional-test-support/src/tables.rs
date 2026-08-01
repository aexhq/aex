//! Loader and canonical bundler for `migrations/regional`.
//!
//! `migrations/regional/tables/*.json` is the source of truth for every regional
//! DynamoDB table. This module reads those files, validates each one against
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

/// A DynamoDB attribute declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeDefinition {
    /// Attribute name.
    pub name: String,
    /// DynamoDB scalar attribute type.
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
    /// Always `INCLUDE`; `ALL` is rejected by the schema (D-29).
    #[serde(rename = "type")]
    pub projection_type: String,
    /// The exhaustive attribute list a query over this index may observe.
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
    /// The customer managed key alias.
    #[serde(rename = "keyAlias")]
    pub key_alias: String,
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
    /// The permitted DynamoDB actions.
    pub actions: Vec<String>,
    /// Which resource ARNs the actions apply to.
    pub resources: Vec<String>,
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
    /// `blake3:<64 hex>` over the rendered table array.
    pub digest: String,
    /// Every table, ordered by logical name.
    pub tables: Vec<TableDefinition>,
}

/// The repository root, derived from this crate's manifest directory.
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
        TableError, bundle, check_attribute_closure, definitions_directory, load_all, rebuild,
        render,
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
                "regional-authz-projection",
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
                assert_eq!(
                    index.projection.projection_type, "INCLUDE",
                    "`{}` index `{}` projects {}",
                    table.table, index.name, index.projection.projection_type
                );
            }
        }
    }

    #[test]
    fn no_projection_can_carry_a_body_a_prompt_or_a_receipt() {
        const FORBIDDEN: [&str; 10] = [
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
        ];
        for table in load_all(&definitions_directory()).expect("the definitions load") {
            for index in &table.global_secondary_indexes {
                for attribute in &index.projection.attributes {
                    assert!(
                        !FORBIDDEN.contains(&attribute.as_str()),
                        "`{}` index `{}` projects `{attribute}`",
                        table.table,
                        index.name
                    );
                }
            }
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
        let aliases: Vec<&str> = tables
            .iter()
            .map(|table| table.server_side_encryption.key_alias.as_str())
            .collect();
        let mut unique = aliases.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            aliases.len(),
            unique.len(),
            "two regional tables share a customer managed key: {aliases:?}"
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
