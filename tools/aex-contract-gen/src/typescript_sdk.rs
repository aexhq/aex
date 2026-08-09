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
//! Only the operation's addressing and retry vocabulary is projected. Request and
//! response bodies stay in `@aexhq/wire`, which is where the validators live.

use std::collections::BTreeSet;

use crate::ir::{ContractIr, OperationIr};

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
    out.push_str("  /** Path placeholders, in the order the contract declares them. */\n");
    out.push_str("  readonly pathParams: readonly string[];\n");
    out.push_str("  /** The closed set of query parameters the operation accepts. */\n");
    out.push_str("  readonly queryParams: readonly string[];\n");
    out.push_str("  /** Whether the operation still answers while the account is paused. */\n");
    out.push_str("  readonly pauseExempt: boolean;\n");
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
        out.push_str("  },\n");
    }
    out.push_str("});\n");
    out
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
