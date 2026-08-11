//! The low-level client emitter.
//!
//! Two functions per operation: a request builder, which a streaming caller can
//! use on its own, and the `WireClient` method, which is the builder plus one
//! transport round trip plus the typed decode. Neither retries, sleeps, or reads
//! a clock.

use std::collections::BTreeSet;

use crate::emit_models::rust_type;
use crate::ir::{ContractIr, FieldType, OperationIr, SchemaBody};
use crate::rustsrc::{Source, quote};
use crate::surface::{RequestShape, ResponseShape, is_copy, takes_if_match};

/// Renders `crates/aex-wire/src/generated/client.rs`.
#[must_use]
pub fn rust_client(ir: &ContractIr, digest: &str) -> String {
    let operations = ir.operations();
    let mut imports: BTreeSet<String> = BTreeSet::new();
    imports.insert("crate::client::ClientError".to_owned());
    imports.insert("crate::client::PathWriter".to_owned());
    imports.insert("crate::client::QueryWriter".to_owned());
    imports.insert("crate::client::Transport".to_owned());
    imports.insert("crate::client::WireClient".to_owned());
    imports.insert("crate::client::WireRequest".to_owned());
    imports.insert("crate::client::request_headers".to_owned());
    imports.insert("crate::routes::RouteId".to_owned());
    imports.insert("crate::types::HttpMethod".to_owned());

    let mut builders = Source::bare();
    emit_param_encoders(ir, &mut builders, &mut imports);
    builders.blank();
    builders.line("// --- request builders -----------------------------------------------------");
    for operation in &operations {
        emit_builder(ir, operation, &mut builders, &mut imports);
    }

    let mut methods = Source::bare();
    methods.line("// --- client methods -------------------------------------------------------");
    methods.blank();
    methods.doc(
        0,
        "One method per public operation, over whichever transport the caller injected.",
    );
    methods.line("impl<T: Transport> WireClient<T> {");
    for (index, operation) in operations.iter().enumerate() {
        emit_method(ir, operation, index, &mut methods, &mut imports);
    }
    methods.line("}");

    let mut source = Source::new(
        "The low-level client: one request builder and one method per public operation.",
        digest,
    );
    source.imports(&imports);
    source.line(&builders.finish());
    source.line(&methods.finish());
    source.finish()
}

/// `ToParam` for every type a path segment or a query value names.
///
/// The exact mirror of the `FromParam` implementations the server module emits:
/// a value that can be decoded from a parameter can be encoded back into one,
/// and `every_parameter_type_round_trips` holds the two together.
fn emit_param_encoders(ir: &ContractIr, body: &mut Source, imports: &mut BTreeSet<String>) {
    let mut named: BTreeSet<String> = BTreeSet::new();
    for operation in ir.operations() {
        for param in operation.path_params.iter().chain(&operation.query_params) {
            if let FieldType::Ref(schema) = &param.ty {
                named.insert(schema.clone());
            }
        }
    }
    let enums: Vec<&str> = named
        .iter()
        .filter(|id| {
            matches!(
                ir.schemas
                    .get(id.as_str())
                    .expect("a parameter references a declared schema")
                    .body,
                SchemaBody::Enum { .. }
            )
        })
        .map(String::as_str)
        .collect();
    let ids: Vec<&str> = ir.ids.iter().map(|row| row.rust.as_str()).collect();
    if ids.is_empty() && enums.is_empty() {
        return;
    }
    imports.insert("crate::client::ToParam".to_owned());
    for id in &ids {
        imports.insert(format!("crate::ids::{id}"));
    }
    for name in &enums {
        imports.insert(format!("crate::models::{name}"));
    }

    body.blank();
    body.line("// --- parameter encoding ---------------------------------------------------");
    body.blank();
    body.doc(0, "Encodes a value that renders itself.");
    body.line("macro_rules! to_param_display {");
    body.line("    ($($ty:ty),* $(,)?) => {");
    body.line("        $(impl ToParam for $ty {");
    body.line("            fn to_param(&self) -> String {");
    body.line("                self.to_string()");
    body.line("            }");
    body.line("        })*");
    body.line("    };");
    body.line("}");
    body.blank();
    body.macro_list(
        0,
        "to_param_display",
        &ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>(),
    );

    if enums.is_empty() {
        return;
    }
    body.blank();
    body.doc(0, "Encodes a closed enumeration as its wire spelling.");
    body.line("macro_rules! to_param_enum {");
    body.line("    ($($ty:ty),* $(,)?) => {");
    body.line("        $(impl ToParam for $ty {");
    body.line("            fn to_param(&self) -> String {");
    body.line("                self.as_str().to_owned()");
    body.line("            }");
    body.line("        })*");
    body.line("    };");
    body.line("}");
    body.blank();
    body.macro_list(
        0,
        "to_param_enum",
        &enums
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    );
}

/// The typed arguments of one operation, in the fixed order every emitter uses.
fn arguments(
    ir: &ContractIr,
    operation: &OperationIr,
    imports: &mut BTreeSet<String>,
) -> Vec<(String, String)> {
    let mut arguments = Vec::new();
    for param in &operation.path_params {
        let rendered = rust_type(ir, &param.ty, imports);
        if is_copy(&param.ty) {
            arguments.push((param.rust.clone(), rendered));
        } else {
            arguments.push((param.rust.clone(), format!("&{rendered}")));
        }
    }
    if !operation.query_params.is_empty() {
        let name = format!("{}Query", operation.variant);
        imports.insert(format!("crate::models::{name}"));
        arguments.push(("query".to_owned(), format!("&{name}")));
    }
    match RequestShape::of(operation) {
        RequestShape::None => {}
        RequestShape::Json(schema) => {
            imports.insert(format!("crate::models::{schema}"));
            arguments.push(("body".to_owned(), format!("&{schema}")));
        }
        RequestShape::Otlp => arguments.push(("body".to_owned(), "&[u8]".to_owned())),
        RequestShape::Binary => arguments.push(("body".to_owned(), "&[u8]".to_owned())),
    }
    match operation.idempotency.as_str() {
        "idempotency_key" => {
            imports.insert("crate::idempotency::IdempotencyKey".to_owned());
            arguments.push(("idempotency_key".to_owned(), "&IdempotencyKey".to_owned()));
        }
        "operation_id" => {
            imports.insert("crate::ids::OperationId".to_owned());
            arguments.push(("operation_id".to_owned(), "OperationId".to_owned()));
        }
        _ => {}
    }
    if takes_if_match(operation) {
        imports.insert("crate::types::ETag".to_owned());
        let rendered = if operation.etag == "required_if_match" {
            "&ETag"
        } else {
            "Option<&ETag>"
        };
        arguments.push(("if_match".to_owned(), rendered.to_owned()));
    }
    arguments
}

/// The three header slots, as the builder passes them.
fn header_arguments(operation: &OperationIr) -> String {
    let idempotency = if operation.idempotency == "idempotency_key" {
        "Some(idempotency_key)"
    } else {
        "None"
    };
    let operation_id = if operation.idempotency == "operation_id" {
        "Some(operation_id)"
    } else {
        "None"
    };
    let if_match = match operation.etag.as_str() {
        "required_if_match" => "Some(if_match)",
        "optional_if_match" => "if_match",
        _ => "None",
    };
    format!("request_headers(route, {idempotency}, {operation_id}, {if_match})")
}

/// One request builder.
fn emit_builder(
    ir: &ContractIr,
    operation: &OperationIr,
    body: &mut Source,
    imports: &mut BTreeSet<String>,
) {
    let arguments = arguments(ir, operation, imports);
    body.blank();
    body.doc(0, &format!("`{} {}`", operation.method, operation.path));
    body.doc(0, &operation.summary);
    body.doc(0, "");
    body.doc(
        0,
        "Built without executing it, so a caller that needs its own transport — a frame stream, a \
         proxy, a recorded fixture — can take the request and run it.",
    );
    body.doc(0, "");
    body.doc(0, "# Errors");
    body.doc(
        0,
        "Returns [`ClientError::Encode`] when the request cannot be rendered.",
    );
    signature(
        body,
        0,
        &format!("pub fn {}_request", operation.id),
        &arguments,
        "Result<WireRequest, ClientError>",
    );
    body.line(&format!("    let route = RouteId::{};", operation.variant));
    let mutability = if operation.path_params.is_empty() {
        ""
    } else {
        "mut "
    };
    body.line(&format!(
        "    let {mutability}path = PathWriter::new(route);"
    ));
    for param in &operation.path_params {
        let borrow = if is_copy(&param.ty) { "&" } else { "" };
        body.line(&format!("    path.bind({borrow}{});", param.rust));
    }
    let query = if operation.query_params.is_empty() {
        "String::new()".to_owned()
    } else {
        body.line("    let mut writer = QueryWriter::new();");
        for param in &operation.query_params {
            if param.optional {
                body.line(&format!(
                    "    writer.put_option({}, query.{}.as_ref());",
                    quote(&param.name),
                    param.rust
                ));
            } else {
                body.line(&format!(
                    "    writer.put({}, &query.{});",
                    quote(&param.name),
                    param.rust
                ));
            }
        }
        "writer.finish()".to_owned()
    };
    let rendered_body = match RequestShape::of(operation) {
        RequestShape::None => "None".to_owned(),
        RequestShape::Json(_) => {
            imports.insert("crate::client::encode_body".to_owned());
            "Some(encode_body(route, body)?)".to_owned()
        }
        RequestShape::Otlp => "Some(body.to_vec())".to_owned(),
        RequestShape::Binary => "Some(body.to_vec())".to_owned(),
    };
    body.line("    Ok(WireRequest {");
    body.line("        route,");
    body.line(&format!(
        "        method: HttpMethod::{},",
        crate::load::pascal_case(&operation.method.to_lowercase())
    ));
    body.line("        path: path.finish()?,");
    body.line(&format!("        query: {query},"));
    body.line(&format!(
        "        headers: {},",
        header_arguments(operation)
    ));
    body.line(&format!("        body: {rendered_body},"));
    body.line("    })");
    body.line("}");
}

/// One `WireClient` method.
fn emit_method(
    ir: &ContractIr,
    operation: &OperationIr,
    index: usize,
    body: &mut Source,
    imports: &mut BTreeSet<String>,
) {
    let arguments = arguments(ir, operation, imports);
    let shape = ResponseShape::of(operation);
    let returns = shape.client_type();
    match &shape {
        ResponseShape::NoContent => {
            imports.insert("crate::client::decode_no_content".to_owned());
        }
        ResponseShape::Accepted => {
            imports.insert("crate::client::decode_response".to_owned());
            imports.insert("crate::models::Operation".to_owned());
        }
        ResponseShape::Created(schema) | ResponseShape::Plain(schema) => {
            imports.insert("crate::client::decode_response".to_owned());
            imports.insert(format!("crate::models::{schema}"));
        }
        ResponseShape::Etagged(schema) => {
            imports.insert("crate::client::decode_response_with_etag".to_owned());
            imports.insert("crate::server::WithETag".to_owned());
            imports.insert(format!("crate::models::{schema}"));
        }
        ResponseShape::Ndjson(frame) => {
            imports.insert("crate::client::NdjsonFrames".to_owned());
            imports.insert("crate::client::decode_ndjson".to_owned());
            imports.insert(format!("crate::models::{frame}"));
        }
        ResponseShape::Binary => {
            imports.insert("crate::client::decode_binary".to_owned());
        }
    }

    if index > 0 {
        body.blank();
    }
    body.doc(4, &format!("`{} {}`", operation.method, operation.path));
    body.doc(4, &operation.summary);
    body.doc(4, "");
    body.doc(4, "# Errors");
    body.doc(
        4,
        "Returns [`ClientError::Api`] for the published error envelope, \
         [`ClientError::Transport`] when the request never reached a status, and \
         [`ClientError::Decode`] when the answer does not match the contract.",
    );
    let mut with_self = vec![("&self".to_owned(), String::new())];
    with_self.extend(arguments.iter().cloned());
    signature(
        body,
        4,
        &format!("    pub async fn {}", operation.id),
        &with_self,
        &format!("Result<{returns}, ClientError>"),
    );

    body.bind_call(
        8,
        "request",
        &format!("{}_request", operation.id),
        &arguments
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
        "?;",
    );
    body.line("        let response = self.send(request).await?;");
    let decode = match &shape {
        ResponseShape::NoContent => {
            format!(
                "decode_no_content(RouteId::{}, &response)",
                operation.variant
            )
        }
        ResponseShape::Etagged(_) => format!(
            "decode_response_with_etag(RouteId::{}, &response)",
            operation.variant
        ),
        ResponseShape::Ndjson(_) => {
            format!("decode_ndjson(RouteId::{}, response)", operation.variant)
        }
        ResponseShape::Binary => {
            format!("decode_binary(RouteId::{}, response)", operation.variant)
        }
        _ => format!("decode_response(RouteId::{}, &response)", operation.variant),
    };
    let single = format!("        {decode}");
    if single.len() <= crate::rustsrc::MAX_WIDTH {
        body.line(&single);
    } else {
        body.line(&format!("        {}(", decode_name(&shape)));
        body.line(&format!("            RouteId::{},", operation.variant));
        if matches!(shape, ResponseShape::Ndjson(_) | ResponseShape::Binary) {
            body.line("            response,");
        } else {
            body.line("            &response,");
        }
        body.line("        )");
    }
    body.line("    }");
}

/// The decoder one response shape uses.
const fn decode_name(shape: &ResponseShape) -> &'static str {
    match shape {
        ResponseShape::NoContent => "decode_no_content",
        ResponseShape::Etagged(_) => "decode_response_with_etag",
        ResponseShape::Ndjson(_) => "decode_ndjson",
        ResponseShape::Binary => "decode_binary",
        ResponseShape::Accepted | ResponseShape::Created(_) | ResponseShape::Plain(_) => {
            "decode_response"
        }
    }
}

/// Renders a function signature the way `rustfmt` lays it out.
///
/// One line when it fits, otherwise one parameter per line with a trailing
/// comma — which is exactly what `rustfmt` does to a broken signature.
fn signature(
    body: &mut Source,
    indent: usize,
    prefix: &str,
    arguments: &[(String, String)],
    returns: &str,
) {
    let rendered: Vec<String> = arguments
        .iter()
        .map(|(name, ty)| {
            if ty.is_empty() {
                name.clone()
            } else {
                format!("{name}: {ty}")
            }
        })
        .collect();
    let single = format!("{prefix}({}) -> {returns} {{", rendered.join(", "));
    if single.len() <= crate::rustsrc::MAX_WIDTH {
        body.line(&single);
        return;
    }
    body.line(&format!("{prefix}("));
    let inner = indent + 4;
    for argument in &rendered {
        body.line(&format!("{:inner$}{argument},", "", inner = inner));
    }
    body.line(&format!("{:indent$}) -> {returns} {{", "", indent = indent));
}
