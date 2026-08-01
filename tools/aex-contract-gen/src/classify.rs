//! The machine diff classifier.
//!
//! It reports and never auto-fails. A prelaunch breaking cut is legal and
//! expected; what must not happen is a breaking cut nobody noticed, so the
//! classifier's job is visibility, and CI's job is to select the coordinated
//! composition lane when a `Breaking` row appears.

use std::collections::BTreeSet;

use serde_json::Value;

/// How compatible a single change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Prose only; no decoder anywhere observes it.
    DocumentationOnly,
    /// A response gained something a declared-additive relaxation permits.
    AdditiveResponse,
    /// Old clients keep working, but the server must ship first.
    MateriallyCompatible,
    /// Some existing caller or decoder stops working.
    Breaking,
}

/// What the change is about.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Subject {
    /// One operation.
    Route(String),
    /// One schema.
    Schema(String),
    /// One error code.
    ErrorCode(String),
    /// One registry as a whole.
    Registry(String),
    /// Document-level metadata.
    Info,
}

/// One classified difference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct Change {
    /// How compatible it is.
    pub classification: Classification,
    /// What it is about.
    pub subject: Subject,
    /// A precise, human-readable description.
    pub detail: String,
}

/// Classifies `head` against `base`. An empty result means the bundles agree.
#[must_use]
pub fn classify(base: &Value, head: &Value) -> Vec<Change> {
    let mut changes = Vec::new();
    classify_info(base, head, &mut changes);
    classify_routes(base, head, &mut changes);
    classify_schemas(base, head, &mut changes);
    classify_error_registry(base, head, &mut changes);
    changes.sort();
    changes.dedup();
    changes
}

/// Document-level metadata.
fn classify_info(base: &Value, head: &Value, changes: &mut Vec<Change>) {
    if base["contractVersion"] != head["contractVersion"] {
        changes.push(Change {
            classification: Classification::Breaking,
            subject: Subject::Info,
            detail: "contractVersion changed".to_owned(),
        });
    }
}

/// Every operation across both planes, keyed by `operationId`.
fn operations(bundle: &Value) -> Vec<(String, &Value)> {
    let mut all = Vec::new();
    if let Some(planes) = bundle["planes"].as_object() {
        for plane in planes.values() {
            if let Some(list) = plane["operations"].as_array() {
                for operation in list {
                    if let Some(id) = operation["operationId"].as_str() {
                        all.push((id.to_owned(), operation));
                    }
                }
            }
        }
    }
    all.sort_by(|left, right| left.0.cmp(&right.0));
    all
}

/// Route additions, removals and per-field drift.
fn classify_routes(base: &Value, head: &Value, changes: &mut Vec<Change>) {
    let base_ops = operations(base);
    let head_ops = operations(head);
    let base_ids: BTreeSet<&str> = base_ops.iter().map(|(id, _)| id.as_str()).collect();
    let head_ids: BTreeSet<&str> = head_ops.iter().map(|(id, _)| id.as_str()).collect();

    for id in head_ids.difference(&base_ids) {
        changes.push(Change {
            classification: Classification::MateriallyCompatible,
            subject: Subject::Route((*id).to_owned()),
            detail: "new operation".to_owned(),
        });
    }
    for id in base_ids.difference(&head_ids) {
        changes.push(Change {
            classification: Classification::Breaking,
            subject: Subject::Route((*id).to_owned()),
            detail: "operation removed".to_owned(),
        });
    }

    for (id, before) in &base_ops {
        let Some((_, after)) = head_ops.iter().find(|(other, _)| other == id) else {
            continue;
        };
        for field in [
            "method",
            "path",
            "requiredScope",
            "idempotency",
            "successStatus",
            "pauseExempt",
            "requestSchema",
            "responseSchema",
            "bodyClass",
            "transport",
            "etag",
        ] {
            if before[field] != after[field] {
                changes.push(Change {
                    classification: Classification::Breaking,
                    subject: Subject::Route(id.clone()),
                    detail: format!("`{field}` changed"),
                });
            }
        }
        if before["summary"] != after["summary"] {
            changes.push(Change {
                classification: Classification::DocumentationOnly,
                subject: Subject::Route(id.clone()),
                detail: "summary changed".to_owned(),
            });
        }
        match (before["safeRetry"].as_bool(), after["safeRetry"].as_bool()) {
            (Some(false), Some(true)) => changes.push(Change {
                classification: Classification::MateriallyCompatible,
                subject: Subject::Route(id.clone()),
                detail: "safeRetry widened to true".to_owned(),
            }),
            (Some(true), Some(false)) => changes.push(Change {
                classification: Classification::Breaking,
                subject: Subject::Route(id.clone()),
                detail: "safeRetry narrowed to false".to_owned(),
            }),
            _ => {}
        }
        let before_errors = string_set(&before["errors"]);
        let after_errors = string_set(&after["errors"]);
        for code in after_errors.difference(&before_errors) {
            changes.push(Change {
                classification: Classification::MateriallyCompatible,
                subject: Subject::Route(id.clone()),
                detail: format!("declares additional error `{code}`"),
            });
        }
        for code in before_errors.difference(&after_errors) {
            changes.push(Change {
                classification: Classification::Breaking,
                subject: Subject::Route(id.clone()),
                detail: format!("no longer declares error `{code}`"),
            });
        }
    }
}

/// The declared additive-response relaxations, as `schema` plus `field` pairs.
fn additive_fields(bundle: &Value) -> BTreeSet<(String, String)> {
    let mut set = BTreeSet::new();
    if let Some(list) = bundle["registries"]["evolution"]["additiveResponseFields"].as_array() {
        for entry in list {
            if let (Some(schema), Some(field)) = (entry["schema"].as_str(), entry["field"].as_str())
            {
                set.insert((schema.to_owned(), field.to_owned()));
            }
        }
    }
    set
}

/// The enums a decoder may widen.
fn open_enums(bundle: &Value) -> BTreeSet<String> {
    string_set(&bundle["registries"]["evolution"]["openEnums"])
}

/// Schema additions, removals, property drift and enum widening.
fn classify_schemas(base: &Value, head: &Value, changes: &mut Vec<Change>) {
    let empty = serde_json::Map::new();
    let base_schemas = base["schemas"].as_object().unwrap_or(&empty);
    let head_schemas = head["schemas"].as_object().unwrap_or(&empty);
    let additive = additive_fields(head);
    let open = open_enums(head);

    for id in head_schemas.keys() {
        if !base_schemas.contains_key(id) {
            changes.push(Change {
                classification: Classification::MateriallyCompatible,
                subject: Subject::Schema(id.clone()),
                detail: "new schema".to_owned(),
            });
        }
    }
    for (id, before) in base_schemas {
        let Some(after) = head_schemas.get(id) else {
            changes.push(Change {
                classification: Classification::Breaking,
                subject: Subject::Schema(id.clone()),
                detail: "schema removed".to_owned(),
            });
            continue;
        };
        classify_properties(id, before, after, &additive, changes);
        classify_enum(id, before, after, &open, changes);
    }
}

/// Property-level drift inside one object schema.
fn classify_properties(
    id: &str,
    before: &Value,
    after: &Value,
    additive: &BTreeSet<(String, String)>,
    changes: &mut Vec<Change>,
) {
    let empty = serde_json::Map::new();
    let before_props = before["properties"].as_object().unwrap_or(&empty);
    let after_props = after["properties"].as_object().unwrap_or(&empty);
    let before_required = string_set(&before["required"]);
    let after_required = string_set(&after["required"]);

    for name in after_props.keys() {
        if before_props.contains_key(name) {
            continue;
        }
        let classification = if after_required.contains(name.as_str()) {
            Classification::Breaking
        } else if additive.contains(&(id.to_owned(), name.clone())) {
            Classification::AdditiveResponse
        } else {
            Classification::Breaking
        };
        changes.push(Change {
            classification,
            subject: Subject::Schema(id.to_owned()),
            detail: format!("new property `{name}`"),
        });
    }
    for (name, before_property) in before_props {
        let Some(after_property) = after_props.get(name) else {
            changes.push(Change {
                classification: Classification::Breaking,
                subject: Subject::Schema(id.to_owned()),
                detail: format!("property `{name}` removed"),
            });
            continue;
        };
        if strip_descriptions(before_property) != strip_descriptions(after_property) {
            changes.push(Change {
                classification: Classification::Breaking,
                subject: Subject::Schema(id.to_owned()),
                detail: format!("property `{name}` changed type or bounds"),
            });
        } else if before_property["description"] != after_property["description"] {
            changes.push(Change {
                classification: Classification::DocumentationOnly,
                subject: Subject::Schema(id.to_owned()),
                detail: format!("property `{name}` description changed"),
            });
        }
        if before_required.contains(name.as_str()) != after_required.contains(name.as_str()) {
            changes.push(Change {
                classification: Classification::Breaking,
                subject: Subject::Schema(id.to_owned()),
                detail: format!("property `{name}` changed requiredness"),
            });
        }
    }
}

/// Enum widening and narrowing.
fn classify_enum(
    id: &str,
    before: &Value,
    after: &Value,
    open: &BTreeSet<String>,
    changes: &mut Vec<Change>,
) {
    let before_values = string_set(&before["enum"]);
    let after_values = string_set(&after["enum"]);
    if before_values.is_empty() && after_values.is_empty() {
        return;
    }
    let classification = if open.contains(id) {
        Classification::AdditiveResponse
    } else {
        Classification::Breaking
    };
    for value in after_values.difference(&before_values) {
        changes.push(Change {
            classification,
            subject: Subject::Schema(id.to_owned()),
            detail: format!("new enum value `{value}`"),
        });
    }
    for value in before_values.difference(&after_values) {
        changes.push(Change {
            classification: Classification::Breaking,
            subject: Subject::Schema(id.to_owned()),
            detail: format!("enum value `{value}` removed"),
        });
    }
}

/// Error-registry drift.
fn classify_error_registry(base: &Value, head: &Value, changes: &mut Vec<Change>) {
    let rows = |bundle: &Value| -> Vec<Value> {
        bundle["registries"]["errors"]["errors"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    let before = rows(base);
    let after = rows(head);
    let code_of = |row: &Value| row["code"].as_str().unwrap_or_default().to_owned();
    let before_codes: BTreeSet<String> = before.iter().map(code_of).collect();
    let after_codes: BTreeSet<String> = after.iter().map(code_of).collect();

    for code in after_codes.difference(&before_codes) {
        changes.push(Change {
            classification: Classification::AdditiveResponse,
            subject: Subject::ErrorCode(code.clone()),
            detail: "new error code".to_owned(),
        });
    }
    for code in before_codes.difference(&after_codes) {
        changes.push(Change {
            classification: Classification::Breaking,
            subject: Subject::ErrorCode(code.clone()),
            detail: "error code removed".to_owned(),
        });
    }
    for row in &before {
        let code = code_of(row);
        let Some(updated) = after.iter().find(|other| code_of(other) == code) else {
            continue;
        };
        for field in ["status", "retryable", "class", "precedenceStage"] {
            if row[field] != updated[field] {
                changes.push(Change {
                    classification: Classification::Breaking,
                    subject: Subject::ErrorCode(code.clone()),
                    detail: format!("`{field}` changed"),
                });
            }
        }
        if row["message"] != updated["message"] {
            changes.push(Change {
                classification: Classification::DocumentationOnly,
                subject: Subject::ErrorCode(code.clone()),
                detail: "message changed".to_owned(),
            });
        }
    }
    if base["registries"]["evolution"]["requestUnknownFields"] != Value::from("reject")
        || head["registries"]["evolution"]["requestUnknownFields"] != Value::from("reject")
    {
        changes.push(Change {
            classification: Classification::Breaking,
            subject: Subject::Registry("evolution".to_owned()),
            detail: "requestUnknownFields is not `reject`".to_owned(),
        });
    }
}

/// A JSON array of strings as a set.
fn string_set(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The value with every `description` member removed, recursively.
fn strip_descriptions(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "description")
                .map(|(key, member)| (key.clone(), strip_descriptions(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_descriptions).collect()),
        other => other.clone(),
    }
}
