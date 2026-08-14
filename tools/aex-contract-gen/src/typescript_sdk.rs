//! The generated route table and error vocabulary of `@aexhq/sdk`.
//!
//! This is a third binding of the same [`ContractIr`](crate::ir::ContractIr) that
//! emits `aex-wire` and `@aexhq/wire`. It exists as its own emitter rather than as
//! an import of `@aexhq/wire` because the published SDK declares
//! `"dependencies": {}` and that stance is deliberate: a public SDK that drags a
//! validator and a peer package into every consumer's tree is a different product.
//! Emitting the table into the SDK keeps the package self-contained while
//! `aex-contract-gen -- check` remains the single proof that it matches the
//! contract.
//!
//! The SDK keeps no runtime dependency on `@aexhq/wire`: it projects the authored
//! model types without validators, then binds each generated method to its exact
//! request and success model. Runtime validation stays in `@aexhq/wire`.

use std::collections::{BTreeMap, BTreeSet};

use crate::emit_models::camel_case;
use crate::ir::{ContractIr, FieldType, OperationIr, SchemaBody};
use crate::load::pascal_case;

/// Renders dependency-free public model types for `@aexhq/sdk`.
#[must_use]
pub fn typescript_sdk_models(ir: &ContractIr, digest: &str) -> String {
    let mut out = preamble(
        digest,
        &[
            "The public wire model types used by the generated SDK resources.",
            "",
            "These declarations and identifier utilities are projected from the same",
            "authored contract as `@aexhq/wire`, but carry no runtime validator dependency.",
        ],
    );
    crate::typescript::emit_identifiers(ir, &mut out);
    out.push_str(
        "export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };\n\n",
    );

    for schema in ir.schemas.values() {
        out.push_str(&format!("/** {} */\n", comment_safe(&schema.doc)));
        match &schema.body {
            SchemaBody::Object { fields } if fields.is_empty() => {
                out.push_str(&format!(
                    "export type {} = Readonly<Record<string, never>>;\n\n",
                    schema.id
                ));
            }
            SchemaBody::Object { fields } => {
                out.push_str(&format!("export interface {} {{\n", schema.id));
                for field in fields {
                    out.push_str(&format!(
                        "  readonly {}{}: {};\n",
                        field.wire,
                        if field.optional { "?" } else { "" },
                        sdk_typescript_type(ir, &field.ty)
                    ));
                }
                out.push_str("}\n\n");
            }
            SchemaBody::Enum { values } => {
                out.push_str(&format!(
                    "export type {} = {};\n\n",
                    schema.id,
                    values
                        .iter()
                        .map(|value| quoted(&value.value))
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
            SchemaBody::Union { tag, variants } => {
                let arms = variants
                    .iter()
                    .map(|variant| {
                        format!(
                            "({} & {{ readonly {}: {} }})",
                            variant.payload,
                            tag,
                            quoted(&variant.value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                out.push_str(&format!("export type {} = {arms};\n\n", schema.id));
            }
        }
    }
    debug_assert!(out.ends_with("\n\n"));
    out.pop();
    out
}

fn sdk_typescript_type(ir: &ContractIr, ty: &FieldType) -> String {
    match ty {
        FieldType::Text { .. }
        | FieldType::Timestamp
        | FieldType::Decimal
        | FieldType::Cents
        | FieldType::ContentHash
        | FieldType::ResourceName
        | FieldType::FilePath
        | FieldType::TraceId
        | FieldType::SpanId
        | FieldType::HttpsUrl
        | FieldType::ETag
        | FieldType::Cursor
        | FieldType::JsonPointer => "string".to_owned(),
        FieldType::Id(kind) => format!("Id<{}>", quoted(kind)),
        FieldType::Integer { .. } | FieldType::Float => "number".to_owned(),
        FieldType::Bool => "boolean".to_owned(),
        FieldType::Ref(id) => id.clone(),
        FieldType::Array(inner, _) => {
            format!("ReadonlyArray<{}>", sdk_typescript_type(ir, inner))
        }
        FieldType::Map(inner, _) => format!(
            "Readonly<Record<string, {}>>",
            sdk_typescript_type(ir, inner)
        ),
        FieldType::Region => {
            "\"us-east-1\" | \"us-east-2\" | \"us-west-2\" | \"ap-northeast-1\" | \"eu-west-1\""
                .to_owned()
        }
        FieldType::ComputeSize => "\"512mb\" | \"1gb\" | \"2gb\" | \"4gb\" | \"8gb\"".to_owned(),
        FieldType::CanonicalJson => "JsonValue".to_owned(),
        FieldType::ByteRange => "ByteRange".to_owned(),
        FieldType::ProviderId => "ProviderId".to_owned(),
        FieldType::Scope => union_of(ir.scopes.iter().map(|row| row.scope.as_str())),
        FieldType::LimitId => union_of(ir.limits.iter().map(|row| row.id.as_str())),
        FieldType::ErrorCode => union_of(ir.errors.iter().map(|row| row.code.as_str())),
        FieldType::MetadataValue => "string | number | boolean | null".to_owned(),
    }
}

/// Renders `packages/sdk/src/generated/routes.ts`.
#[must_use]
pub fn typescript_sdk_routes(ir: &ContractIr, digest: &str) -> String {
    let operations = ir.operations();
    let mut out = preamble(
        digest,
        &[
            "The public route table, projected from the authored contract.",
            "",
            "The SDK is published with no runtime dependencies, so the table is emitted",
            "into the package rather than imported from `@aexhq/wire`. Nothing here is",
            "hand-maintained: `cargo run -p aex-contract-gen -- check` fails if this file",
            "is not what the contract produces.",
        ],
    );

    out.push_str("/** The contract the table below was projected from. */\n");
    out.push_str(&format!(
        "export const CONTRACT_DIGEST = {} as const;\n\n",
        quoted(digest)
    ));

    out.push_str("export type RouteId =\n");
    for (index, operation) in operations.iter().enumerate() {
        let last = index + 1 == operations.len();
        out.push_str(&format!(
            "  | {}{}\n",
            quoted(&operation.id),
            if last { ";" } else { "" }
        ));
    }
    out.push('\n');

    out.push_str("export interface RouteDescriptor {\n");
    out.push_str("  /** The `operationId` the contract declares. */\n");
    out.push_str("  readonly id: RouteId;\n");
    out.push_str("  /** HTTP method. */\n");
    out.push_str(&format!(
        "  readonly method: {};\n",
        union_of(distinct(&operations, |operation| &operation.method))
    ));
    out.push_str("  /** Path template, rooted at `/api`, with `{name}` placeholders. */\n");
    out.push_str("  readonly path: string;\n");
    out.push_str("  /** Which plane serves the operation. */\n");
    out.push_str(&format!(
        "  readonly plane: {};\n",
        union_of(distinct(&operations, |operation| &operation.plane))
    ));
    out.push_str("  /** Whether an identical retry is safe without a replay identity. */\n");
    out.push_str("  readonly safeRetry: boolean;\n");
    out.push_str("  /** Replay-identity requirement. */\n");
    out.push_str(&format!(
        "  readonly idempotency: {};\n",
        union_of(distinct(&operations, |operation| &operation.idempotency))
    ));
    out.push_str("  /** How the response is delivered. */\n");
    out.push_str(&format!(
        "  readonly transport: {};\n",
        union_of(distinct(&operations, |operation| &operation.transport))
    ));
    out.push_str("  /** How the request body is encoded. */\n");
    out.push_str(&format!(
        "  readonly bodyClass: {};\n",
        union_of(distinct(&operations, |operation| &operation.body_class))
    ));
    out.push_str("  /** Path placeholders, in the order the contract declares them. */\n");
    out.push_str("  readonly pathParams: readonly string[];\n");
    out.push_str("  /** The closed set of query parameters the operation accepts. */\n");
    out.push_str("  readonly queryParams: readonly string[];\n");
    out.push_str("  /** Whether the operation still answers while the account is paused. */\n");
    out.push_str("  readonly pauseExempt: boolean;\n");
    out.push_str(
        "  /**\n   * Whether the contract declares the operation and nothing serves it yet.\n   *\n   * Informational only. The server is the sole authority on what it serves:\n   * an installed SDK pins one contract digest forever, so refusing locally on\n   * this flag would hard-fail a call to an operation that started working.\n   */\n",
    );
    out.push_str("  readonly deferred: boolean;\n");
    out.push_str("}\n\n");

    out.push_str(
        "export const ROUTES: Readonly<Record<RouteId, RouteDescriptor>> = Object.freeze({\n",
    );
    for operation in &operations {
        out.push_str(&format!(
            "  /** `{} {}` — {} */\n",
            operation.method,
            operation.path,
            comment_safe(&operation.summary)
        ));
        out.push_str(&format!("  {}: {{\n", operation.id));
        out.push_str(&format!("    id: {},\n", quoted(&operation.id)));
        out.push_str(&format!("    method: {},\n", quoted(&operation.method)));
        out.push_str(&format!("    path: {},\n", quoted(&operation.path)));
        out.push_str(&format!("    plane: {},\n", quoted(&operation.plane)));
        out.push_str(&format!("    safeRetry: {},\n", operation.safe_retry));
        out.push_str(&format!(
            "    idempotency: {},\n",
            quoted(&operation.idempotency)
        ));
        out.push_str(&format!(
            "    transport: {},\n",
            quoted(&operation.transport)
        ));
        out.push_str(&format!(
            "    bodyClass: {},\n",
            quoted(&operation.body_class)
        ));
        out.push_str(&format!(
            "    pathParams: {},\n",
            string_array(
                operation
                    .path_params
                    .iter()
                    .map(|param| param.name.as_str())
            )
        ));
        out.push_str(&format!(
            "    queryParams: {},\n",
            string_array(
                operation
                    .query_params
                    .iter()
                    .map(|param| param.name.as_str())
            )
        ));
        out.push_str(&format!("    pauseExempt: {},\n", operation.pause_exempt));
        out.push_str(&format!(
            "    deferred: {},\n",
            operation.deferred_reason.is_some()
        ));
        out.push_str("  },\n");
    }
    out.push_str("});\n");
    out
}

/// Renders `packages/sdk/src/generated/resources.ts`.
///
/// One method per **served** operation, and nothing else. A method the
/// contract declares and no deployable serves would be a method that can only
/// fail, and publishing one puts the platform's answer in the client, where it
/// goes stale the day the route lands. NDJSON operations return typed async
/// iterables through the executor's independent streaming transport.
#[must_use]
pub fn typescript_sdk_resources(ir: &ContractIr, digest: &str) -> String {
    let operations = ir.operations();
    let mut groups: BTreeMap<&str, Vec<&OperationIr>> = BTreeMap::new();
    for operation in operations
        .iter()
        .filter(|operation| operation.deferred_reason.is_none())
    {
        groups
            .entry(operation.fragment.as_str())
            .or_default()
            .push(operation);
    }

    let mut out = preamble(
        digest,
        &[
            "The typed resource surface, one method per served operation.",
            "",
            "Nothing here is hand-maintained. A route that leaves the deferral ledger",
            "gains its method in the same regeneration that publishes it, and a route",
            "that enters the ledger loses it, so the published client surface is exactly",
            "what the platform answers.",
        ],
    );
    out.push_str("import type * as Models from \"./models.js\";\n");
    out.push_str("import type { RouteId } from \"./routes.js\";\n\n");

    out.push_str("/** The one client capability every generated resource method needs. */\n");
    out.push_str("export interface ResourceExecutor {\n");
    out.push_str("  execute<T>(\n");
    out.push_str("    routeId: RouteId,\n");
    out.push_str("    bindings?: Readonly<Record<string, string>>,\n");
    out.push_str("    options?: ExecuteOptions,\n");
    out.push_str("  ): Promise<T>;\n");
    out.push_str("  stream<T>(\n");
    out.push_str("    routeId: RouteId,\n");
    out.push_str("    bindings?: Readonly<Record<string, string>>,\n");
    out.push_str("    options?: ExecuteOptions,\n");
    out.push_str("  ): AsyncIterable<T>;\n");
    out.push_str("}\n\n");

    out.push_str("/** Everything beyond the path a single call can carry. */\n");
    out.push_str("export interface ExecuteOptions {\n");
    out.push_str("  /** Query parameters, already rendered as wire strings. */\n");
    out.push_str("  readonly query?: Readonly<Record<string, string>>;\n");
    out.push_str("  /** The request body; binary routes require `Uint8Array`, all others use canonical JSON. */\n");
    out.push_str("  readonly body?: unknown | Uint8Array;\n");
    out.push_str("  /** `Idempotency-Key`, for a route that requires one. */\n");
    out.push_str("  readonly idempotencyKey?: string;\n");
    out.push_str("  /** `Aex-Operation-Id`, for a route that admits a durable operation. */\n");
    out.push_str("  readonly operationId?: string;\n");
    out.push_str("}\n\n");

    for (fragment, members) in &groups {
        out.push_str(&format!("/** The `{fragment}` operations. */\n"));
        out.push_str(&format!("export class {} {{\n", client_name(fragment)));
        out.push_str("  readonly #executor: ResourceExecutor;\n\n");
        out.push_str("  constructor(executor: ResourceExecutor) {\n");
        out.push_str("    this.#executor = executor;\n");
        out.push_str("  }\n");
        for operation in members {
            out.push('\n');
            resource_method(&mut out, ir, operation);
        }
        out.push_str("}\n\n");
    }

    out.push_str("/**\n");
    out.push_str(" * The resource namespaces the client exposes.\n");
    out.push_str(" *\n");
    out.push_str(" * The client extends this rather than listing the namespaces itself, so a\n");
    out.push_str(" * fragment whose last served operation lands or leaves cannot be forgotten\n");
    out.push_str(" * in the hand-written half.\n");
    out.push_str(" */\n");
    out.push_str("export abstract class GeneratedResources implements ResourceExecutor {\n");
    out.push_str("  abstract execute<T>(\n");
    out.push_str("    routeId: RouteId,\n");
    out.push_str("    bindings?: Readonly<Record<string, string>>,\n");
    out.push_str("    options?: ExecuteOptions,\n");
    out.push_str("  ): Promise<T>;\n");
    out.push_str("  abstract stream<T>(\n");
    out.push_str("    routeId: RouteId,\n");
    out.push_str("    bindings?: Readonly<Record<string, string>>,\n");
    out.push_str("    options?: ExecuteOptions,\n");
    out.push_str("  ): AsyncIterable<T>;\n");
    for fragment in groups.keys() {
        out.push_str(&format!(
            "\n  /** The `{fragment}` operations. */\n  readonly {}: {} = new {}(this);\n",
            camel_case(&fragment.replace('-', "_")),
            client_name(fragment),
            client_name(fragment)
        ));
    }
    out.push_str("}\n");
    out
}

/// The class name one authoring fragment's served operations land in.
fn client_name(fragment: &str) -> String {
    format!("{}Client", pascal_case(fragment))
}

/// One generated resource method.
fn resource_method(out: &mut String, ir: &ContractIr, operation: &OperationIr) {
    let name = camel_case(&operation.id);
    let query_required = operation.query_params.iter().any(|param| !param.optional);
    let body = operation.request.is_some()
        || operation.body_class == "otlp"
        || operation.body_class == "binary";
    let return_type = if operation.transport == "binary" {
        "Uint8Array".to_owned()
    } else {
        operation
            .success
            .as_ref()
            .map_or_else(|| "void".to_owned(), |model| format!("Models.{model}"))
    };
    let idempotency_key = operation.idempotency == "idempotency_key";
    let operation_id = operation.idempotency == "operation_id";
    let required_field = !operation.path_params.is_empty()
        || query_required
        || body
        || idempotency_key
        || operation_id;
    let has_field = required_field || !operation.query_params.is_empty();
    let streaming = operation.transport == "ndjson";
    let return_wrapper = if streaming {
        format!("AsyncIterable<{return_type}>")
    } else {
        format!("Promise<{return_type}>")
    };
    let async_prefix = if streaming { "" } else { "async " };

    out.push_str(&format!(
        "  /** `{} {}` — {} */\n",
        operation.method,
        operation.path,
        comment_safe(&operation.summary)
    ));
    if has_field {
        out.push_str(&format!("  {async_prefix}{name}(params: {{\n"));
        for param in &operation.path_params {
            out.push_str(&format!(
                "    readonly {}: {};\n",
                param.name,
                sdk_method_param_type(ir, &param.ty)
            ));
        }
        if !operation.query_params.is_empty() {
            out.push_str(&format!(
                "    readonly query{}: {{\n",
                if query_required { "" } else { "?" }
            ));
            for param in &operation.query_params {
                out.push_str(&format!(
                    "      readonly {}{}: string;\n",
                    param.name,
                    if param.optional { "?" } else { "" }
                ));
            }
            out.push_str("    };\n");
        }
        if body {
            out.push_str(&format!(
                "    readonly body: {};\n",
                if operation.body_class == "binary" {
                    "Uint8Array".to_owned()
                } else {
                    operation
                        .request
                        .as_ref()
                        .map_or("unknown".to_owned(), |model| format!("Models.{model}"))
                }
            ));
        }
        if idempotency_key {
            out.push_str("    readonly idempotencyKey: string;\n");
        }
        if operation_id {
            out.push_str("    readonly operationId: Models.Id<\"operation\">;\n");
        }
        out.push_str(&format!(
            "  }}{}): {return_wrapper} {{\n",
            if required_field { "" } else { " = {}" }
        ));
    } else {
        out.push_str(&format!("  {async_prefix}{name}(): {return_wrapper} {{\n"));
    }

    let bindings = operation
        .path_params
        .iter()
        .map(|param| {
            let value = match &param.ty {
                FieldType::Integer { .. } | FieldType::Float | FieldType::Bool => {
                    format!("String(params.{})", param.name)
                }
                _ => format!("params.{}", param.name),
            };
            format!("{}: {value}", param.name)
        })
        .collect::<Vec<_>>();
    let mut options: Vec<String> = Vec::new();
    if !operation.query_params.is_empty() {
        options.push(if query_required {
            "query: params.query".to_owned()
        } else {
            // `exactOptionalPropertyTypes` refuses an explicit `undefined` where
            // the member is merely optional, so an absent value is an absent key.
            "...(params.query === undefined ? {} : { query: params.query })".to_owned()
        });
    }
    if body {
        options.push("body: params.body".to_owned());
    }
    if idempotency_key {
        options.push("idempotencyKey: params.idempotencyKey".to_owned());
    }
    if operation_id {
        options.push("operationId: params.operationId".to_owned());
    }
    let mut call = format!(
        "    return this.#executor.{}<{}>({}",
        if streaming { "stream" } else { "execute" },
        return_type,
        quoted(&operation.id)
    );
    if !bindings.is_empty() || !options.is_empty() {
        call.push_str(&if bindings.is_empty() {
            ", {}".to_owned()
        } else {
            format!(", {{ {} }}", bindings.join(", "))
        });
    }
    if !options.is_empty() {
        call.push_str(&format!(", {{ {} }}", options.join(", ")));
    }
    call.push_str(");\n");
    out.push_str(&call);
    out.push_str("  }\n");
}

fn sdk_method_param_type(ir: &ContractIr, ty: &FieldType) -> String {
    match ty {
        FieldType::Id(_) | FieldType::Ref(_) | FieldType::ByteRange | FieldType::ProviderId => {
            format!("Models.{}", sdk_typescript_type(ir, ty))
        }
        _ => sdk_typescript_type(ir, ty),
    }
}

/// Renders `packages/sdk/src/generated/errors.ts`.
#[must_use]
pub fn typescript_sdk_errors(ir: &ContractIr, digest: &str) -> String {
    let classes: BTreeSet<&str> = ir.errors.iter().map(|row| row.class.as_str()).collect();
    let mut out = preamble(
        digest,
        &[
            "The public error vocabulary, projected from the authored contract.",
            "",
            "`ERROR_METADATA` is keyed by `string` rather than by a closed union: a",
            "deployed platform can answer with a code newer than the installed SDK, and",
            "the transport decides what to construct from the lookup miss instead of",
            "failing to compile.",
        ],
    );

    out.push_str("export type ErrorClass =\n");
    for (index, class) in classes.iter().enumerate() {
        let last = index + 1 == classes.len();
        out.push_str(&format!(
            "  | {}{}\n",
            quoted(class),
            if last { ";" } else { "" }
        ));
    }
    out.push('\n');

    out.push_str(
        "export const ERROR_METADATA: Readonly<Record<string, { readonly class: ErrorClass }>> =\n",
    );
    out.push_str("  Object.freeze({\n");
    for row in &ir.errors {
        out.push_str(&format!(
            "    {}: {{ class: {} }},\n",
            row.code,
            quoted(&row.class)
        ));
    }
    out.push_str("  });\n");
    out
}

/// The `@generated` marker, the digest, and the file's own doc block.
fn preamble(digest: &str, doc: &[&str]) -> String {
    let mut out = String::new();
    out.push_str("// @generated by aex-contract-gen; DO NOT EDIT.\n");
    out.push_str(&format!("// Contract digest: {digest}\n\n"));
    out.push_str("/**\n");
    for line in doc {
        if line.is_empty() {
            out.push_str(" *\n");
        } else {
            out.push_str(&format!(" * {line}\n"));
        }
    }
    out.push_str(" */\n\n");
    out
}

/// Every distinct value one operation field takes, sorted.
fn distinct<'a>(
    operations: &'a [&'a OperationIr],
    field: fn(&'a OperationIr) -> &'a String,
) -> BTreeSet<&'a str> {
    operations
        .iter()
        .map(|operation| field(operation).as_str())
        .collect()
}

/// A TypeScript union of string literals.
fn union_of<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    values
        .into_iter()
        .map(quoted)
        .collect::<Vec<_>>()
        .join(" | ")
}

/// A TypeScript array literal of strings, rendered on one line.
fn string_array<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let values: Vec<String> = values.into_iter().map(quoted).collect();
    if values.is_empty() {
        "[]".to_owned()
    } else {
        format!("[{}]", values.join(", "))
    }
}

/// Text that cannot terminate the block comment it is rendered into.
fn comment_safe(value: &str) -> String {
    value.replace("*/", "* /")
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("a string always serializes")
}
