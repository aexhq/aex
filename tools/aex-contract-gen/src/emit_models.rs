//! The Rust model emitter: every public request, response and query type.
//!
//! The schema-to-Rust mapping is total. An unmapped construct is rejected at
//! load time, so this emitter never falls back to `serde_json::Value`.

use std::collections::BTreeSet;

use crate::ir::{ContractIr, FieldType, SchemaBody, SchemaIr};
use crate::rustsrc::{Source, quote};

/// Schemas whose Rust type is hand-written in `aex-wire` rather than generated.
///
/// They still appear in the published JSON Schemas and in the OpenAPI documents;
/// only the Rust emission is suppressed, because the hand-written type carries
/// invariants a generated struct cannot (an empty range is unconstructible).
const HAND_WRITTEN: [&str; 1] = ["ByteRange"];

/// Renders `crates/aex-wire/src/generated/models.rs`.
#[must_use]
pub fn rust_models(ir: &ContractIr, digest: &str) -> String {
    let mut imports: BTreeSet<String> = BTreeSet::new();
    imports.insert("serde::Deserialize".to_owned());
    imports.insert("serde::Serialize".to_owned());

    let mut body = Source::bare();
    let mut ordered: Vec<&SchemaIr> = ir.schemas.values().collect();
    ordered.sort_by(|left, right| {
        (left.module.as_str(), left.id.as_str()).cmp(&(right.module.as_str(), right.id.as_str()))
    });

    let mut current_module = "";
    for schema in ordered {
        if HAND_WRITTEN.contains(&schema.id.as_str()) {
            continue;
        }
        if schema.module != current_module {
            current_module = &schema.module;
            body.blank();
            body.line(&format!(
                "// --- {current_module} -------------------------------------------------------"
            ));
        }
        body.blank();
        emit_schema(ir, schema, &mut body, &mut imports);
    }

    body.blank();
    body.line("// --- query parameters ------------------------------------------------------");
    for operation in ir.operations() {
        if operation.query_params.is_empty() {
            continue;
        }
        body.blank();
        body.doc(0, &format!("Query parameters of `{}`.", operation.id));
        body.line("#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]");
        body.line("#[serde(deny_unknown_fields, rename_all = \"camelCase\")]");
        body.line(&format!("pub struct {}Query {{", operation.variant));
        for param in &operation.query_params {
            let rendered = rust_type(ir, &param.ty, &mut imports);
            body.doc(4, &param.doc);
            if camel_case(&param.rust) != param.name {
                body.line(&format!("    #[serde(rename = {})]", quote(&param.name)));
            }
            if param.optional {
                body.line("    #[serde(default, skip_serializing_if = \"Option::is_none\")]");
                body.line(&format!("    pub {}: Option<{rendered}>,", param.rust));
            } else {
                body.line(&format!("    pub {}: {rendered},", param.rust));
            }
        }
        body.line("}");
    }

    let mut source = Source::new("The public request, response and query models.", digest);
    for import in &imports {
        source.line(&format!("use {import};"));
    }
    source.line(&body.finish());
    source.finish()
}

/// Emits one schema.
fn emit_schema(
    ir: &ContractIr,
    schema: &SchemaIr,
    body: &mut Source,
    imports: &mut BTreeSet<String>,
) {
    match &schema.body {
        SchemaBody::Object { fields } => {
            body.doc(0, &schema.doc);
            body.line("#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]");
            body.line("#[serde(deny_unknown_fields, rename_all = \"camelCase\")]");
            if fields.is_empty() {
                body.line(&format!("pub struct {} {{}}", schema.id));
                return;
            }
            body.line(&format!("pub struct {} {{", schema.id));
            for field in fields {
                let rendered = rust_type(ir, &field.ty, imports);
                body.doc(4, &field.doc);
                if camel_case(&field.rust) != field.wire {
                    body.line(&format!("    #[serde(rename = {})]", quote(&field.wire)));
                }
                if field.optional {
                    body.line("    #[serde(default, skip_serializing_if = \"Option::is_none\")]");
                    body.line(&format!("    pub {}: Option<{rendered}>,", field.rust));
                } else {
                    body.line(&format!("    pub {}: {rendered},", field.rust));
                }
            }
            body.line("}");
        }
        SchemaBody::Enum { values } => {
            body.doc(0, &schema.doc);
            body.line(
                "#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]",
            );
            body.line("#[serde(rename_all = \"snake_case\")]");
            body.line(&format!("pub enum {} {{", schema.id));
            for value in values {
                body.doc(4, &value.doc);
                if snake_of_pascal(&value.variant) != value.value {
                    body.line(&format!("    #[serde(rename = {})]", quote(&value.value)));
                }
                body.line(&format!("    {},", value.variant));
            }
            body.line("}");
            body.blank();
            body.line(&format!("impl {} {{", schema.id));
            body.doc(4, "Every value, in declared order.");
            body.const_slice(
                4,
                &format!("pub const ALL: &'static [{}]", schema.id),
                &values
                    .iter()
                    .map(|value| format!("{}::{}", schema.id, value.variant))
                    .collect::<Vec<_>>(),
            );
            body.blank();
            body.doc(4, "The wire spelling.");
            body.line("    #[must_use]");
            body.line("    pub const fn as_str(self) -> &'static str {");
            body.line("        match self {");
            for value in values {
                body.arm(12, &value.variant, &quote(&value.value));
            }
            body.line("        }");
            body.line("    }");
            body.line("}");
        }
        SchemaBody::Union { tag, variants } => {
            body.doc(0, &schema.doc);
            body.line("#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]");
            body.line(&format!(
                "#[serde(tag = {}, rename_all = \"snake_case\")]",
                quote(tag)
            ));
            body.line(&format!("pub enum {} {{", schema.id));
            for variant in variants {
                body.doc(4, &variant.doc);
                if snake_of_pascal(&variant.variant) != variant.value {
                    body.line(&format!("    #[serde(rename = {})]", quote(&variant.value)));
                }
                body.line(&format!("    {}({}),", variant.variant, variant.payload));
            }
            body.line("}");
        }
    }
}

/// The Rust rendering of a field type, recording every import it needs.
fn rust_type(ir: &ContractIr, ty: &FieldType, imports: &mut BTreeSet<String>) -> String {
    let mut import = |path: &str| {
        imports.insert(path.to_owned());
    };
    match ty {
        FieldType::Text { .. } => "String".to_owned(),
        FieldType::Id(key) => {
            let row = ir
                .ids
                .iter()
                .find(|row| &row.key == key)
                .expect("id kinds are validated at load time");
            import(&format!("crate::ids::{}", row.rust));
            row.rust.clone()
        }
        FieldType::Timestamp => {
            import("crate::types::Timestamp");
            "Timestamp".to_owned()
        }
        FieldType::Decimal => {
            import("crate::types::DecimalU128");
            "DecimalU128".to_owned()
        }
        FieldType::Cents => {
            import("crate::types::Cents");
            "Cents".to_owned()
        }
        FieldType::Integer { min, max } => {
            if *min >= 0 && *max <= i64::from(u32::MAX) {
                "u32".to_owned()
            } else if *min >= 0 {
                "u64".to_owned()
            } else {
                "i64".to_owned()
            }
        }
        FieldType::Float => "f64".to_owned(),
        FieldType::Bool => "bool".to_owned(),
        FieldType::Ref(id) => id.clone(),
        FieldType::Array(inner, _) => format!("Vec<{}>", rust_type(ir, inner, imports)),
        FieldType::Map(inner) => {
            let rendered = rust_type(ir, inner, imports);
            imports.insert("std::collections::BTreeMap".to_owned());
            format!("BTreeMap<String, {rendered}>")
        }
        FieldType::Region => {
            import("crate::types::Region");
            "Region".to_owned()
        }
        FieldType::ComputeSize => {
            import("crate::types::ComputeSize");
            "ComputeSize".to_owned()
        }
        FieldType::ContentHash => {
            import("crate::ids::ContentHash");
            "ContentHash".to_owned()
        }
        FieldType::ResourceName => {
            import("crate::ids::ResourceName");
            "ResourceName".to_owned()
        }
        FieldType::FilePath => {
            import("crate::ids::FilePath");
            "FilePath".to_owned()
        }
        FieldType::TraceId => {
            import("crate::ids::TraceId");
            "TraceId".to_owned()
        }
        FieldType::SpanId => {
            import("crate::ids::SpanId");
            "SpanId".to_owned()
        }
        FieldType::HttpsUrl => {
            import("crate::types::HttpsUrl");
            "HttpsUrl".to_owned()
        }
        FieldType::ETag => {
            import("crate::types::ETag");
            "ETag".to_owned()
        }
        FieldType::Cursor => {
            import("crate::cursor::Cursor");
            "Cursor".to_owned()
        }
        FieldType::JsonPointer => {
            import("crate::types::JsonPointer");
            "JsonPointer".to_owned()
        }
        FieldType::CanonicalJson => {
            import("crate::canonical::CanonicalJson");
            "CanonicalJson".to_owned()
        }
        FieldType::ByteRange => {
            import("crate::types::ByteRange");
            "ByteRange".to_owned()
        }
        FieldType::Scope => {
            import("crate::scopes::ScopeId");
            "ScopeId".to_owned()
        }
        FieldType::LimitId => {
            import("crate::limits::LimitId");
            "LimitId".to_owned()
        }
        FieldType::ErrorCode => {
            import("crate::error::ObservedErrorCode");
            "ObservedErrorCode".to_owned()
        }
        FieldType::MetadataValue => {
            import("crate::types::MetadataValue");
            "MetadataValue".to_owned()
        }
        FieldType::ProviderId => "ProviderId".to_owned(),
    }
}

/// `snake_case` to `camelCase`, exactly as `serde`'s `rename_all` does it.
#[must_use]
pub fn camel_case(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut capitalize = false;
    for character in text.chars() {
        if character == '_' {
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

/// `PascalCase` to `snake_case`, exactly as `serde`'s `rename_all` does it.
#[must_use]
pub fn snake_of_pascal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for (index, character) in text.chars().enumerate() {
        if character.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}
