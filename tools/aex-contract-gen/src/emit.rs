//! Every emitter. Each one reads only [`ContractIr`] and returns bytes.
//!
//! Nothing here touches the filesystem, reads a clock, or iterates an unordered
//! collection, which is what makes "regenerate twice" a design property rather
//! than a lucky run.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::ir::{ContractIr, FieldIr, FieldType, LimitShape, OperationIr, SchemaBody, SchemaIr};
use crate::jcs;
use crate::rustsrc::{Source, quote};
use crate::tree::GeneratedTree;

/// The generator's own identity, recorded in every lock file.
pub const GENERATOR: &str = "aex-contract-gen";
/// The generator's version, recorded in every lock file.
pub const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Renders every output file for `ir`.
#[must_use]
pub fn emit_all(ir: &ContractIr) -> GeneratedTree {
    let bundle = bundle_document(ir);
    let digest = jcs::digest(&bundle);
    let mut tree = GeneratedTree::default();

    tree.insert("api/generated/bundle.json", jcs::to_pretty(&bundle));
    tree.insert(
        "api/generated/bundle.lock.json",
        jcs::to_pretty(&lock_document(ir, &digest)),
    );
    for (id, schema) in &ir.schemas {
        tree.insert(
            &format!("api/generated/schemas/{id}.json"),
            jcs::to_pretty(&published_schema(ir, schema)),
        );
    }
    for name in ["errors", "evolution", "ids", "limits", "routes", "scopes"] {
        tree.insert(
            &format!("api/generated/registries/{name}.json"),
            jcs::to_pretty(&registry_document(ir, name)),
        );
    }
    for plane in &ir.planes {
        tree.insert(
            &format!("api/generated/openapi/aex-{}.json", plane.id),
            jcs::to_pretty(&openapi_document(ir, &plane.id)),
        );
    }

    tree.insert("crates/aex-wire/src/generated/mod.rs", rust_mod(&digest));
    tree.insert(
        "crates/aex-wire/src/generated/ids.rs",
        rust_ids(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/errors.rs",
        rust_errors(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/scopes.rs",
        rust_scopes(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/limits.rs",
        rust_limits(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/models.rs",
        crate::emit_models::rust_models(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/routes.rs",
        crate::emit_routes::rust_routes(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/server.rs",
        crate::emit_server::rust_server(ir, &digest),
    );
    tree.insert(
        "crates/aex-wire/src/generated/client.rs",
        crate::emit_client::rust_client(ir, &digest),
    );
    tree.insert(
        "packages/wire/src/generated/models.ts",
        crate::typescript::typescript_wire(ir, &digest),
    );
    tree.insert(
        "packages/sdk/src/generated/routes.ts",
        crate::typescript_sdk::typescript_sdk_routes(ir, &digest),
    );
    tree.insert(
        "packages/sdk/src/generated/resources.ts",
        crate::typescript_sdk::typescript_sdk_resources(ir, &digest),
    );
    tree.insert(
        "packages/sdk/src/generated/errors.ts",
        crate::typescript_sdk::typescript_sdk_errors(ir, &digest),
    );
    tree.insert(
        "conformance/routes/bindings.jsonl",
        route_binding_corpus(ir),
    );
    tree.insert("conformance/routes/surface.jsonl", route_surface_corpus(ir));
    tree
}

/// The contract digest for `ir`.
#[must_use]
pub fn contract_digest(ir: &ContractIr) -> String {
    jcs::digest(&bundle_document(ir))
}

// ---------------------------------------------------------------------------
// JSON artifacts
// ---------------------------------------------------------------------------

/// The published JSON Schema 2020-12 document for one schema.
fn published_schema(ir: &ContractIr, schema: &SchemaIr) -> Value {
    let mut document = Map::new();
    document.insert(
        "$schema".to_owned(),
        json!("https://json-schema.org/draft/2020-12/schema"),
    );
    document.insert(
        "$id".to_owned(),
        json!(format!("https://schemas.aex.dev/v1/{}.json", schema.id)),
    );
    document.insert("title".to_owned(), json!(schema.id));
    document.insert("description".to_owned(), json!(schema.doc));
    for (key, value) in schema_body(ir, schema) {
        document.insert(key, value);
    }
    Value::Object(document)
}

/// The type-specific part of a published schema.
fn schema_body(ir: &ContractIr, schema: &SchemaIr) -> Map<String, Value> {
    let mut body = Map::new();
    match &schema.body {
        SchemaBody::Object { fields } => {
            body.insert("type".to_owned(), json!("object"));
            body.insert("additionalProperties".to_owned(), json!(false));
            let required: Vec<&str> = fields
                .iter()
                .filter(|field| !field.optional)
                .map(|field| field.wire.as_str())
                .collect();
            body.insert("required".to_owned(), json!(required));
            let mut properties = Map::new();
            for field in fields {
                properties.insert(field.wire.clone(), field_schema(ir, field));
            }
            body.insert("properties".to_owned(), Value::Object(properties));
        }
        SchemaBody::Enum { values } => {
            body.insert("type".to_owned(), json!("string"));
            body.insert(
                "enum".to_owned(),
                json!(values.iter().map(|v| v.value.as_str()).collect::<Vec<_>>()),
            );
        }
        SchemaBody::Union { tag, variants } => {
            body.insert("x-aex-discriminator".to_owned(), json!(tag));
            body.insert(
                "oneOf".to_owned(),
                json!(
                    variants
                        .iter()
                        .map(|variant| json!({
                            "$ref": format!("aex:schema:{}", variant.payload),
                            "x-aex-discriminator-value": variant.value,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
    body
}

/// The JSON Schema fragment for one member.
fn field_schema(ir: &ContractIr, field: &FieldIr) -> Value {
    let mut value = type_schema(ir, &field.ty);
    if let Value::Object(map) = &mut value {
        map.insert("description".to_owned(), json!(field.doc));
    }
    value
}

/// The JSON Schema fragment for one field type.
fn type_schema(ir: &ContractIr, ty: &FieldType) -> Value {
    match ty {
        FieldType::Text { min, max, pattern } => {
            let mut map = Map::new();
            map.insert("type".to_owned(), json!("string"));
            map.insert("minLength".to_owned(), json!(min));
            map.insert("maxLength".to_owned(), json!(max));
            if let Some(pattern) = pattern {
                map.insert("pattern".to_owned(), json!(pattern));
            }
            Value::Object(map)
        }
        FieldType::Id(key) => {
            let row = ir.ids.iter().find(|row| &row.key == key);
            let prefix = row.map_or("", |row| row.prefix.as_str());
            json!({
                "type": "string",
                "x-aex-id": key,
                "pattern": format!("^{prefix}_[0-9a-hjkmnp-tv-z]{{26}}$"),
            })
        }
        FieldType::Timestamp => json!({
            "type": "string",
            "x-aex-time": "rfc3339-ms",
            "pattern": r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$",
        }),
        FieldType::Decimal => json!({ "type": "string", "pattern": r"^(0|[1-9]\d*)$" }),
        FieldType::Cents => {
            json!({ "type": "string", "pattern": r"^(0|[1-9]\d*)$", "x-aex-money": "cents" })
        }
        FieldType::Integer { min, max } => {
            json!({ "type": "integer", "minimum": min, "maximum": max })
        }
        FieldType::Float => json!({ "type": "number", "x-aex-float": true }),
        FieldType::Bool => json!({ "type": "boolean" }),
        FieldType::Ref(id) => json!({ "$ref": format!("aex:schema:{id}") }),
        FieldType::Array(inner, max) => json!({
            "type": "array",
            "maxItems": max,
            "items": type_schema(ir, inner),
        }),
        FieldType::Map(inner, max) => json!({
            "type": "object",
            "maxProperties": max,
            "additionalProperties": type_schema(ir, inner),
        }),
        FieldType::Region => json!({
            "type": "string",
            "enum": ["us-east-1", "us-east-2", "us-west-2", "ap-northeast-1", "eu-west-1"],
        }),
        FieldType::ComputeSize => json!({
            "type": "string",
            "enum": ["512mb", "1gb", "2gb", "4gb", "8gb"],
        }),
        FieldType::ContentHash => json!({ "type": "string", "pattern": "^sha256:[0-9a-f]{64}$" }),
        FieldType::ResourceName => {
            json!({ "type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$" })
        }
        FieldType::FilePath => {
            json!({ "type": "string", "maxLength": 4096, "x-aex-path": "posix" })
        }
        FieldType::TraceId => json!({ "type": "string", "pattern": "^[0-9a-f]{32}$" }),
        FieldType::SpanId => json!({ "type": "string", "pattern": "^[0-9a-f]{16}$" }),
        FieldType::HttpsUrl => json!({ "type": "string", "maxLength": 2048, "format": "uri" }),
        FieldType::ETag => json!({ "type": "string", "maxLength": 256 }),
        FieldType::Cursor => json!({ "type": "string", "maxLength": 4096, "x-aex-cursor": true }),
        FieldType::JsonPointer => json!({ "type": "string", "format": "json-pointer" }),
        FieldType::CanonicalJson => json!({ "x-aex-canonical-json": true }),
        FieldType::ByteRange => json!({ "$ref": "aex:schema:ByteRange" }),
        FieldType::Scope => json!({ "type": "string", "x-aex-registry": "scopes" }),
        FieldType::LimitId => json!({ "type": "string", "x-aex-registry": "limits" }),
        FieldType::ErrorCode => json!({ "type": "string", "x-aex-registry": "errors" }),
        FieldType::MetadataValue => json!({
            "type": ["string", "number", "boolean", "null"],
            "x-aex-float": true,
        }),
        FieldType::ProviderId => json!({ "$ref": "aex:schema:ProviderId" }),
    }
}

/// One published registry document.
fn registry_document(ir: &ContractIr, name: &str) -> Value {
    match name {
        "ids" => json!({
            "ids": ir.ids.iter().map(|row| json!({
                "kind": row.key, "prefix": row.prefix, "rust": row.rust, "description": row.doc,
            })).collect::<Vec<_>>(),
        }),
        "errors" => json!({
            "errors": ir.errors.iter().map(|row| json!({
                "code": row.code, "status": row.status, "retryable": row.retryable,
                "class": row.class, "precedenceStage": row.stage, "message": row.message,
                "remedy": row.remedy,
            })).collect::<Vec<_>>(),
        }),
        "scopes" => json!({
            "scopes": ir.scopes.iter().map(|row| json!({
                "scope": row.scope, "description": row.doc,
            })).collect::<Vec<_>>(),
        }),
        "limits" => json!({
            "limits": ir.limits.iter().map(|row| json!({
                "id": row.id,
                "shape": if row.shape == LimitShape::Map { "map" } else { "scalar" },
                "description": row.doc,
            })).collect::<Vec<_>>(),
        }),
        "routes" => json!({
            "schema": "aex.route-registry.v1",
            "routes": ir.operations().into_iter().map(route_registry_document).collect::<Vec<_>>(),
        }),
        "evolution" => json!({
            "openEnums": ir.evolution.open_enums,
            "additiveResponseFields": ir.evolution.additive_response_fields.iter()
                .map(|(schema, field)| json!({ "schema": schema, "field": field }))
                .collect::<Vec<_>>(),
            "unknownFramePolicy": ir.evolution.unknown_frame_policy,
            "requestUnknownFields": ir.evolution.request_unknown_fields,
        }),
        other => json!({ "error": format!("unknown registry `{other}`") }),
    }
}

/// One operation's release-selection registry row.
///
/// Scenario ownership is intentionally added here instead of to
/// [`route_document`], which is also used by the public contract bundle.
fn route_registry_document(operation: &OperationIr) -> Value {
    let mut document = route_document(operation);
    document
        .as_object_mut()
        .expect("route documents are objects")
        .insert("scenarios".to_owned(), json!(operation.scenarios));
    document
        .as_object_mut()
        .expect("route documents are objects")
        .insert(
            "servingArtifact".to_owned(),
            json!(operation.serving_artifact),
        );
    if let Some(served_artifact) = &operation.served_artifact {
        document
            .as_object_mut()
            .expect("route documents are objects")
            .insert("servedArtifact".to_owned(), json!(served_artifact));
    }
    if let Some(reason) = &operation.deferred_reason {
        document
            .as_object_mut()
            .expect("route documents are objects")
            .insert("deferredReason".to_owned(), json!(reason));
    }
    document
}

/// One operation's registry row.
///
/// `deferred` is a boolean and never the ledger's prose. *Whether* an operation
/// is served is the most caller-visible fact there is, so it belongs to the
/// public wire identity; *why* it is not served is an internal engineering note
/// and stays in the delivery registry.
fn route_document(operation: &OperationIr) -> Value {
    let mut document = json!({
        "operationId": operation.id,
        "plane": operation.plane,
        "fragment": operation.fragment,
        "method": operation.method,
        "path": operation.path,
        "summary": operation.summary,
        "requiredScope": operation.scope,
        "altPrincipal": operation.alt_principal,
        "idempotency": operation.idempotency,
        "requestSchema": operation.request,
        "successStatus": operation.success_status,
        "responseSchema": operation.success,
        "errors": operation.errors,
        "bodyClass": operation.body_class,
        "transport": operation.transport,
        "etag": operation.etag,
        "safeRetry": operation.safe_retry,
        "pauseExempt": operation.pause_exempt,
        "pathParams": operation.path_params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        "queryParams": operation.query_params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
    });
    if operation.deferred_reason.is_some() {
        document
            .as_object_mut()
            .expect("route documents are objects")
            .insert("deferred".to_owned(), json!(true));
    }
    document
}

/// The self-contained `OpenAPI` 3.1 document for one plane.
fn openapi_document(ir: &ContractIr, plane_id: &str) -> Value {
    let plane = ir
        .planes
        .iter()
        .find(|plane| plane.id == plane_id)
        .expect("planes are loaded before emission");
    let mut paths: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    if plane
        .operations
        .iter()
        .any(|operation| !operation.errors.is_empty())
    {
        // Every declared failure response references this envelope even though
        // it is not an authored request or success schema.
        collect_reachable(ir, "ApiError", &mut reachable);
    }
    for operation in &plane.operations {
        for schema in operation.request.iter().chain(operation.success.iter()) {
            collect_reachable(ir, schema, &mut reachable);
        }
        let entry = paths.entry(operation.path.as_str()).or_default();
        entry.insert(
            operation.method.to_lowercase(),
            openapi_operation(ir, operation),
        );
    }
    let mut components = Map::new();
    for id in &reachable {
        let schema = &ir.schemas[id];
        components.insert(id.clone(), Value::Object(schema_body(ir, schema)));
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": plane.title,
            "version": ir.contract_version,
            "description": format!("The AEX {} plane.", plane.id),
        },
        "servers": [{ "url": plane.server, "description": plane.server_description }],
        "security": [{ plane.security_scheme.clone(): [] }],
        "components": {
            "schemas": Value::Object(components),
            "securitySchemes": {
                plane.security_scheme.clone(): {
                    "type": "http", "scheme": "bearer",
                },
            },
        },
        "paths": paths.into_iter()
            .map(|(path, item)| (path.to_owned(), Value::Object(item)))
            .collect::<Map<String, Value>>(),
    })
}

/// One `OpenAPI` operation object.
///
/// A deferred operation is published and marked, never omitted: omission would
/// fix the document and leave the wire mute, because a caller who reads no
/// `POST /api/sessions` and calls it anyway still needs an answer that says
/// what happened. The marker is three things a reader cannot all miss — the
/// `501` response every renderer shows, the `x-aex-deferred` extension a
/// machine reads, and a description sentence. None of the three carries the
/// ledger's reason: the reasons are engineering notes, and one that has gone
/// stale is worse than no sentence at all.
fn openapi_operation(ir: &ContractIr, operation: &OperationIr) -> Value {
    let deferred = operation.deferred_reason.is_some();
    let mut parameters = Vec::new();
    for param in &operation.path_params {
        parameters.push(json!({
            "name": param.name, "in": "path", "required": true,
            "description": param.doc, "schema": type_schema(ir, &param.ty),
        }));
    }
    for param in &operation.query_params {
        parameters.push(json!({
            "name": param.name, "in": "query", "required": !param.optional,
            "description": param.doc, "schema": type_schema(ir, &param.ty),
        }));
    }
    let mut responses = Map::new();
    let success = match &operation.success {
        Some(schema) => json!({
            "description": "Success.",
            "content": { "application/json": { "schema": { "$ref": format!("aex:schema:{schema}") } } },
        }),
        None => json!({ "description": "Success with no body." }),
    };
    responses.insert(operation.success_status.to_string(), success);
    for code in &operation.errors {
        let row = ir
            .errors
            .iter()
            .find(|row| &row.code == code)
            .expect("error codes are validated at load time");
        responses.entry(row.status.to_string()).or_insert_with(|| {
            json!({
                "description": "Error.",
                "content": { "application/json": { "schema": { "$ref": "aex:schema:ApiError" } } },
            })
        });
    }
    let mut document = json!({
        "operationId": operation.id,
        "summary": operation.summary,
        "tags": [operation.fragment],
        "parameters": parameters,
        "requestBody": operation.request.as_ref().map(|schema| json!({
            "required": true,
            "content": { "application/json": { "schema": { "$ref": format!("aex:schema:{schema}") } } },
        })),
        "responses": Value::Object(responses),
        "x-aex-idempotency": operation.idempotency,
        "x-aex-scope": operation.scope,
        "x-aex-alt-principal": operation.alt_principal,
        "x-aex-body-class": operation.body_class,
        "x-aex-transport": operation.transport,
        "x-aex-etag": operation.etag,
        "x-aex-safe-retry": operation.safe_retry,
        "x-aex-pause-exempt": operation.pause_exempt,
        "x-aex-errors": operation.errors,
    });
    if deferred {
        let object = document.as_object_mut().expect("operations are objects");
        object.insert("x-aex-deferred".to_owned(), json!(true));
        object.insert(
            "description".to_owned(),
            json!("Not yet available. This operation is published in the contract and is not served yet; calls answer 501 not_implemented."),
        );
    }
    document
}

/// Adds `id` and everything it transitively reaches to `into`.
fn collect_reachable(ir: &ContractIr, id: &str, into: &mut BTreeSet<String>) {
    if !into.insert(id.to_owned()) {
        return;
    }
    let Some(schema) = ir.schemas.get(id) else {
        return;
    };
    match &schema.body {
        SchemaBody::Object { fields } => {
            for field in fields {
                collect_type_reachable(ir, &field.ty, into);
            }
        }
        SchemaBody::Union { variants, .. } => {
            for variant in variants {
                collect_reachable(ir, &variant.payload, into);
            }
        }
        SchemaBody::Enum { .. } => {}
    }
}

fn collect_type_reachable(ir: &ContractIr, ty: &FieldType, into: &mut BTreeSet<String>) {
    match ty {
        FieldType::Ref(target) => collect_reachable(ir, target, into),
        FieldType::Array(inner, _) | FieldType::Map(inner, _) => {
            collect_type_reachable(ir, inner, into);
        }
        FieldType::ByteRange => collect_reachable(ir, "ByteRange", into),
        FieldType::ProviderId => collect_reachable(ir, "ProviderId", into),
        _ => {}
    }
}

/// The union of both planes, every map ordered by key.
fn bundle_document(ir: &ContractIr) -> Value {
    let mut planes = Map::new();
    for plane in &ir.planes {
        planes.insert(
            plane.id.clone(),
            json!({
                "title": plane.title,
                "server": plane.server,
                "securityScheme": plane.security_scheme,
                "fragments": plane.fragment_order,
                "operations": plane.operations.iter().map(route_document).collect::<Vec<_>>(),
            }),
        );
    }
    let mut schemas = Map::new();
    for (id, schema) in &ir.schemas {
        schemas.insert(id.clone(), Value::Object(schema_body(ir, schema)));
    }
    let mut registries = Map::new();
    for name in ["errors", "evolution", "ids", "limits", "scopes"] {
        registries.insert(name.to_owned(), registry_document(ir, name));
    }
    json!({
        "contractVersion": ir.contract_version,
        "generator": { "name": GENERATOR, "version": GENERATOR_VERSION },
        "planes": Value::Object(planes),
        "registries": Value::Object(registries),
        "schemas": Value::Object(schemas),
    })
}

/// The lock file: the digest, the generator identity and every input digest.
fn lock_document(ir: &ContractIr, digest: &str) -> Value {
    json!({
        "contractDigest": digest,
        "generator": {
            "name": GENERATOR,
            "version": GENERATOR_VERSION,
            "toolchain": ir.toolchain,
            "formatter": "aex-contract-gen internal renderer (rustfmt fixed point)",
        },
        "sources": ir.sources,
    })
}

/// The generated golden binding for every route, one JSON object per line.
fn route_binding_corpus(ir: &ContractIr) -> String {
    let mut out = String::new();
    for operation in ir.operations() {
        let path = sample_path(ir, operation);
        let bindings: Map<String, Value> = operation
            .path_params
            .iter()
            .map(|param| (param.name.clone(), json!(sample_value(ir, &param.ty))))
            .collect();
        let line = json!({
            "operationId": operation.id,
            "plane": operation.plane,
            "method": operation.method,
            "path": path,
            "bindings": Value::Object(bindings),
        });
        out.push_str(&String::from_utf8(jcs::to_jcs_bytes(&line)).unwrap_or_default());
        out.push('\n');
    }
    out
}

/// `conformance/routes/surface.jsonl`: the generated server and client surface.
///
/// One line per operation naming the group it mounts in, the trait it lands on,
/// the method name both sides use, and the request and response shapes. This is
/// the artifact a peer stream reads to know what it is mounting, and the floor
/// that makes "every operation is generated" an assertion rather than a claim.
fn route_surface_corpus(ir: &ContractIr) -> String {
    let groups = crate::surface::groups(ir);
    let mut out = String::new();
    for group in &groups {
        for operation in &group.operations {
            let response = crate::surface::ResponseShape::of(operation);
            let request = crate::surface::RequestShape::of(operation);
            let line = json!({
                "operationId": operation.id,
                "routeId": operation.variant,
                "plane": operation.plane,
                "group": group.key,
                "trait": group.trait_name,
                "method": operation.id,
                "requestBuilder": format!("{}_request", operation.id),
                "requestShape": request.corpus_name(),
                "requestSchema": request_schema(&request),
                "responseShape": response.corpus_name(),
                "responseSchema": response.schema(),
                "successStatus": operation.success_status,
                "transport": operation.transport,
                "idempotency": operation.idempotency,
                "etag": operation.etag,
            });
            out.push_str(&String::from_utf8(jcs::to_jcs_bytes(&line)).unwrap_or_default());
            out.push('\n');
        }
    }
    out
}

/// The `SchemaId` a request shape names, if any.
fn request_schema(shape: &crate::surface::RequestShape) -> Option<&str> {
    match shape {
        crate::surface::RequestShape::Json(schema) => Some(schema),
        crate::surface::RequestShape::None | crate::surface::RequestShape::Otlp => None,
    }
}

/// A deterministic concrete path for one operation.
fn sample_path(ir: &ContractIr, operation: &OperationIr) -> String {
    let mut path = operation.path.clone();
    for param in &operation.path_params {
        path = path.replace(&format!("{{{}}}", param.name), &sample_value(ir, &param.ty));
    }
    path
}

/// A valid `UUIDv7` Crockford suffix, used for every sampled identifier.
///
/// Fixed rather than derived: the binding corpus is a golden, so its bytes must
/// not move when an unrelated registry row changes.
const SAMPLE_ID_SUFFIX: &str = "01kyw2qa4ne00r40r40m30e209";

/// A deterministic sample value for a parameter type.
fn sample_value(ir: &ContractIr, ty: &FieldType) -> String {
    match ty {
        FieldType::Id(key) => {
            let prefix = ir
                .ids
                .iter()
                .find(|row| &row.key == key)
                .map_or("xxx", |row| row.prefix.as_str());
            format!("{prefix}_{SAMPLE_ID_SUFFIX}")
        }
        FieldType::TraceId => "4bf92f3577b34da6a3ce929d0e0e4736".to_owned(),
        FieldType::ResourceName => "sample-name".to_owned(),
        _ => "sample".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Rust emission
// ---------------------------------------------------------------------------

/// `crates/aex-wire/src/generated/mod.rs`.
fn rust_mod(digest: &str) -> String {
    let mut source = Source::new("Generated contract surface of `aex-wire`.", digest);
    source.line("pub mod client;");
    source.line("pub mod errors;");
    source.line("pub mod ids;");
    source.line("pub mod limits;");
    source.line("pub mod models;");
    source.line("pub mod routes;");
    source.line("pub mod scopes;");
    source.line("pub mod server;");
    source.finish()
}

/// `crates/aex-wire/src/generated/ids.rs`.
fn rust_ids(ir: &ContractIr, digest: &str) -> String {
    let mut source = Source::new("The identifier registry and its newtypes.", digest);
    source.line("use crate::ids::prefixed_id;");
    source.blank();
    source.doc(0, "Every resource an AEX identifier can name.");
    source.line("#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]");
    source.line("pub enum IdKind {");
    for row in &ir.ids {
        source.doc(4, &row.doc);
        source.line(&format!("    {},", row.variant));
    }
    source.line("}");
    source.blank();
    source.line("impl IdKind {");
    source.doc(4, "Every kind, in registry order.");
    source.const_slice(
        4,
        "pub const ALL: &'static [IdKind]",
        &ir.ids
            .iter()
            .map(|row| format!("IdKind::{}", row.variant))
            .collect::<Vec<_>>(),
    );
    source.blank();
    source.doc(4, "The wire prefix, without the trailing underscore.");
    source.line("    #[must_use]");
    source.line("    pub const fn prefix(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.ids {
        source.arm(12, &row.variant, &quote(&row.prefix));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(
        4,
        "The anchored pattern the registry publishes for this kind.",
    );
    source.line("    #[must_use]");
    source.line("    pub const fn pattern(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.ids {
        let pattern = quote(&format!("^{}_[0-9a-hjkmnp-tv-z]{{26}}$", row.prefix));
        source.arm(12, &row.variant, &pattern);
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "The `snake_case` registry key.");
    source.line("    #[must_use]");
    source.line("    pub const fn key(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.ids {
        source.arm(12, &row.variant, &quote(&row.key));
    }
    source.line("        }");
    source.line("    }");
    source.line("}");
    for row in &ir.ids {
        source.blank();
        source.line("prefixed_id!(");
        source.doc(4, &row.doc);
        source.line(&format!("    {},", row.rust));
        source.line(&format!("    {}", row.variant));
        source.line(");");
    }
    source.finish()
}

/// `crates/aex-wire/src/generated/errors.rs`.
fn rust_errors(ir: &ContractIr, digest: &str) -> String {
    let mut source = Source::new("The closed v1 public error vocabulary.", digest);
    source.line("use serde::{Deserialize, Serialize};");
    source.blank();
    source.line("use crate::error::{ErrorClass, PrecedenceStage};");
    source.blank();
    source.doc(
        0,
        "Every error the platform emits. Adding a variant is a compile error at every \
         exhaustive match, which is the point.",
    );
    source.line(
        "#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]",
    );
    source.line("#[serde(rename_all = \"snake_case\")]");
    source.line("pub enum ErrorCode {");
    for row in &ir.errors {
        source.doc(4, &format!("`{}` — {}", row.code, row.message));
        source.line(&format!("    {},", row.variant));
    }
    source.line("}");
    source.blank();
    source.line("impl ErrorCode {");
    source.doc(4, "Every code, in registry order.");
    source.const_slice(
        4,
        "pub const ALL: &'static [ErrorCode]",
        &ir.errors
            .iter()
            .map(|row| format!("ErrorCode::{}", row.variant))
            .collect::<Vec<_>>(),
    );
    source.blank();
    source.doc(4, "The wire spelling.");
    source.line("    #[must_use]");
    source.line("    pub const fn as_str(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.errors {
        source.arm(12, &row.variant, &quote(&row.code));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "The HTTP status this code renders at.");
    source.line("    #[must_use]");
    source.line("    pub const fn http_status(self) -> u16 {");
    source.line("        match self {");
    for row in &ir.errors {
        source.arm(12, &row.variant, &row.status.to_string());
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Whether an identical retry can succeed.");
    source.line("    #[must_use]");
    source.line("    pub const fn retryable(self) -> bool {");
    source.line("        match self {");
    for row in &ir.errors {
        source.arm(12, &row.variant, &row.retryable.to_string());
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Which failure family the code belongs to.");
    source.line("    #[must_use]");
    source.line("    pub const fn class(self) -> ErrorClass {");
    source.line("        match self {");
    for row in &ir.errors {
        let class = format!("ErrorClass::{}", crate::load::pascal_case(&row.class));
        source.arm(12, &row.variant, &class);
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Which precedence stage may emit the code.");
    source.line("    #[must_use]");
    source.line("    pub const fn precedence_stage(self) -> PrecedenceStage {");
    source.line("        match self {");
    for row in &ir.errors {
        let stage = format!("PrecedenceStage::{}", crate::load::pascal_case(&row.stage));
        source.arm(12, &row.variant, &stage);
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "The default human message.");
    source.line("    #[must_use]");
    source.line("    pub const fn default_message(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.errors {
        source.arm(12, &row.variant, &quote(&row.message));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "How a caller fixes the request, when that is knowable.");
    source.line("    #[must_use]");
    source.line("    pub const fn remedy(self) -> Option<&'static str> {");
    source.line("        match self {");
    for row in &ir.errors {
        let value = row.remedy.as_ref().map_or_else(
            || "None".to_owned(),
            |text| format!("Some({})", quote(text)),
        );
        source.arm(12, &row.variant, &value);
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Resolves a wire spelling.");
    source.line("    #[must_use]");
    source.line("    pub fn parse(text: &str) -> Option<Self> {");
    source.line("        Self::ALL.iter().copied().find(|it| it.as_str() == text)");
    source.line("    }");
    source.line("}");
    source.finish()
}

/// `crates/aex-wire/src/generated/scopes.rs`.
fn rust_scopes(ir: &ContractIr, digest: &str) -> String {
    let mut source = Source::new("The authorization scope registry.", digest);
    source.line("use serde::{Deserialize, Serialize};");
    source.blank();
    source.doc(
        0,
        "Every scope a workspace API key or account token can carry. The set is derived from \
         the route table: a scope this enum lacks is a route change, not a registry change.",
    );
    source.line(
        "#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]",
    );
    source.line("#[serde(rename_all = \"snake_case\")]");
    source.line("pub enum ScopeId {");
    for row in &ir.scopes {
        source.doc(4, &format!("`{}` — {}", row.scope, row.doc));
        source.line(&format!("    #[serde(rename = {})]", quote(&row.scope)));
        source.line(&format!("    {},", row.variant));
    }
    source.line("}");
    source.blank();
    source.line("impl ScopeId {");
    source.doc(4, "Every scope, in registry order.");
    source.const_slice(
        4,
        "pub const ALL: &'static [ScopeId]",
        &ir.scopes
            .iter()
            .map(|row| format!("ScopeId::{}", row.variant))
            .collect::<Vec<_>>(),
    );
    source.blank();
    source.doc(4, "The wire spelling.");
    source.line("    #[must_use]");
    source.line("    pub const fn as_str(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.scopes {
        source.arm(12, &row.variant, &quote(&row.scope));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Resolves a wire spelling.");
    source.line("    #[must_use]");
    source.line("    pub fn parse(text: &str) -> Option<Self> {");
    source.line("        Self::ALL.iter().copied().find(|it| it.as_str() == text)");
    source.line("    }");
    source.line("}");
    source.finish()
}

/// `crates/aex-wire/src/generated/limits.rs`.
fn rust_limits(ir: &ContractIr, digest: &str) -> String {
    let mut source = Source::new("The effective-limit registry.", digest);
    source.line("use serde::{Deserialize, Serialize};");
    source.blank();
    source.doc(
        0,
        "Whether a limit's effective value is one integer or a map of named dimensions.",
    );
    source.line(
        "#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]",
    );
    source.line("#[serde(rename_all = \"snake_case\")]");
    source.line("pub enum LimitShape {");
    source.doc(4, "A single non-negative integer.");
    source.line("    Scalar,");
    source.doc(4, "A map from a named dimension to a non-negative integer.");
    source.line("    Map,");
    source.line("}");
    source.blank();
    source.doc(
        0,
        "Every adjustable workspace safety limit. Enforcement belongs to the regional \
         applications; this crate owns only the identity and the public shape.",
    );
    source.line(
        "#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]",
    );
    source.line("#[serde(rename_all = \"snake_case\")]");
    source.line("pub enum LimitId {");
    for row in &ir.limits {
        source.doc(4, &format!("`{}` — {}", row.id, row.doc));
        source.line(&format!("    #[serde(rename = {})]", quote(&row.id)));
        source.line(&format!("    {},", row.variant));
    }
    source.line("}");
    source.blank();
    source.line("impl LimitId {");
    source.doc(4, "Every limit, in registry order.");
    source.const_slice(
        4,
        "pub const ALL: &'static [LimitId]",
        &ir.limits
            .iter()
            .map(|row| format!("LimitId::{}", row.variant))
            .collect::<Vec<_>>(),
    );
    source.blank();
    source.doc(4, "The wire spelling.");
    source.line("    #[must_use]");
    source.line("    pub const fn as_str(self) -> &'static str {");
    source.line("        match self {");
    for row in &ir.limits {
        source.arm(12, &row.variant, &quote(&row.id));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Whether the effective value is a scalar or a map.");
    source.line("    #[must_use]");
    source.line("    pub const fn shape(self) -> LimitShape {");
    source.line("        match self {");
    for row in &ir.limits {
        let shape = if row.shape == LimitShape::Map {
            "Map"
        } else {
            "Scalar"
        };
        source.arm(12, &row.variant, &format!("LimitShape::{shape}"));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(
        4,
        "The complete ordered dimension vocabulary for a map limit.",
    );
    source.line("    #[must_use]");
    source.line("    pub const fn dimensions(self) -> &'static [&'static str] {");
    source.line("        match self {");
    for row in &ir.limits {
        let dimensions = row
            .dimensions
            .iter()
            .map(|dimension| quote(dimension))
            .collect::<Vec<_>>()
            .join(", ");
        let value = format!("&[{dimensions}]");
        let line_width = 12 + "Self::".len() + row.variant.len() + " => ".len() + value.len() + 1;
        if line_width <= 100 {
            source.arm(12, &row.variant, &value);
        } else {
            source.line(&format!("            Self::{} => &[", row.variant));
            for dimension in &row.dimensions {
                source.line(&format!("                {},", quote(dimension)));
            }
            source.line("            ],");
        }
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Resolves a wire spelling.");
    source.line("    #[must_use]");
    source.line("    pub fn parse(text: &str) -> Option<Self> {");
    source.line("        Self::ALL.iter().copied().find(|it| it.as_str() == text)");
    source.line("    }");
    source.line("}");
    source.finish()
}

#[cfg(test)]
mod scenario_identity_tests {
    use super::{bundle_document, registry_document};
    use crate::{emit, jcs, load};

    #[test]
    fn delivery_selection_changes_no_wire_byte_or_contract_digest() {
        let root = load::repo_root();
        let original = load::load(&root).expect("load contract");
        let mut changed = original.clone();
        changed.planes[0].operations[0]
            .scenarios
            .push("SC-SELECTION-PROBE".to_owned());
        changed.planes[0].operations[0].serving_artifact = "selection-probe".to_owned();
        changed.planes[0].operations[0].served_artifact = Some("selection-probe".to_owned());

        let original_bundle = bundle_document(&original);
        let changed_bundle = bundle_document(&changed);
        assert_eq!(
            jcs::to_pretty(&original_bundle),
            jcs::to_pretty(&changed_bundle)
        );
        assert_eq!(
            emit::contract_digest(&original),
            emit::contract_digest(&changed)
        );
        assert_ne!(
            registry_document(&original, "routes"),
            registry_document(&changed, "routes")
        );
    }
}
