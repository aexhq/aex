//! The server-trait and dispatch emitter.
//!
//! One trait per route group, one `async` method per operation, and one total
//! dispatcher per group that turns raw parts into the typed call and the typed
//! answer back into bytes. Nothing here names a framework: `axum`, `hyper` and
//! `lambda_http` all live on the far side of `RawRequest`/`RawResponse`.

use std::collections::BTreeSet;

use crate::emit_models::rust_type;
use crate::ir::{ContractIr, FieldType, OperationIr, SchemaBody};
use crate::rustsrc::{Source, quote};
use crate::surface::{GroupIr, RequestShape, ResponseShape, groups};

/// Renders `crates/aex-wire/src/generated/server.rs`.
#[must_use]
pub fn rust_server(ir: &ContractIr, digest: &str) -> String {
    let groups = groups(ir);
    let mut imports: BTreeSet<String> = BTreeSet::new();
    imports.insert("core::future::Future".to_owned());
    imports.insert("crate::dispatch::DispatchOutcome".to_owned());
    imports.insert("crate::dispatch::QueryReader".to_owned());
    imports.insert("crate::dispatch::RawRequest".to_owned());
    imports.insert("crate::dispatch::RawResponse".to_owned());
    imports.insert("crate::dispatch::RequestLimits".to_owned());
    imports.insert("crate::dispatch::wrong_group".to_owned());
    imports.insert("crate::error::WireResult".to_owned());
    imports.insert("crate::routes::Plane".to_owned());
    imports.insert("crate::routes::RouteId".to_owned());
    imports.insert("crate::server::RequestContext".to_owned());

    let mut body = Source::bare();
    emit_group_registry(&groups, &mut body);
    emit_param_decoders(ir, &mut body, &mut imports);
    for group in &groups {
        emit_trait(ir, group, &mut body, &mut imports);
        emit_dispatcher(ir, group, &mut body, &mut imports);
    }

    let mut source = Source::new(
        "The server traits and the total dispatch surface, one group per authoring fragment.",
        digest,
    );
    source.imports(&imports);
    source.line(&body.finish());
    source.finish()
}

/// The `RouteGroup` projection over the one route table.
fn emit_group_registry(groups: &[GroupIr<'_>], body: &mut Source) {
    body.blank();
    body.doc(
        0,
        "A mountable group of operations: exactly one authoring fragment on exactly one plane.",
    );
    body.doc(
        0,
        "This is a projection of `ROUTES`, not a second table. Every route belongs to exactly one \
         group and every group's slice is a subset of `RouteId::ALL`, which \
         `route_groups_partition_the_table` asserts.",
    );
    body.line("#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]");
    body.line("pub enum RouteGroup {");
    for group in groups {
        body.doc(
            4,
            &format!(
                "`{}` on the {} plane, served by `{}`.",
                group.fragment, group.plane, group.trait_name
            ),
        );
        body.line(&format!("    {},", group.pascal));
    }
    body.line("}");
    body.blank();

    body.line("impl RouteGroup {");
    body.doc(4, "Every group, in key order.");
    body.const_slice(
        4,
        "pub const ALL: &'static [RouteGroup]",
        &groups
            .iter()
            .map(|group| format!("RouteGroup::{}", group.pascal))
            .collect::<Vec<_>>(),
    );
    body.blank();
    body.doc(4, "The stable `<plane>:<fragment>` key.");
    body.line("    #[must_use]");
    body.line("    pub const fn as_str(self) -> &'static str {");
    body.line("        match self {");
    for group in groups {
        body.arm(
            12,
            &group.pascal,
            &quote(&format!("{}:{}", group.plane, group.fragment)),
        );
    }
    body.line("        }");
    body.line("    }");
    body.blank();
    body.doc(4, "Which plane serves the group.");
    body.line("    #[must_use]");
    body.line("    pub const fn plane(self) -> Plane {");
    body.line("        match self {");
    for group in groups {
        body.arm(
            12,
            &group.pascal,
            &format!("Plane::{}", crate::load::pascal_case(group.plane)),
        );
    }
    body.line("        }");
    body.line("    }");
    body.blank();
    body.doc(4, "The generated trait a composition crate mounts.");
    body.line("    #[must_use]");
    body.line("    pub const fn trait_name(self) -> &'static str {");
    body.line("        match self {");
    for group in groups {
        body.arm(12, &group.pascal, &quote(&group.trait_name));
    }
    body.line("        }");
    body.line("    }");
    body.blank();
    body.doc(
        4,
        "Every route in the group, in `RouteId` order. Mounting is a loop over this slice.",
    );
    body.line("    #[must_use]");
    body.line("    pub const fn routes(self) -> &'static [RouteId] {");
    body.line("        match self {");
    for group in groups {
        body.arm(12, &group.pascal, &group.routes_const());
    }
    body.line("        }");
    body.line("    }");
    body.line("}");
    body.blank();

    for group in groups {
        body.doc(
            0,
            &format!(
                "Every route of `{}:{}`, in `RouteId` order.",
                group.plane, group.fragment
            ),
        );
        body.const_slice(
            0,
            &format!("pub const {}: &[RouteId]", group.routes_const()),
            &group
                .operations
                .iter()
                .map(|operation| format!("RouteId::{}", operation.variant))
                .collect::<Vec<_>>(),
        );
        body.blank();
    }

    body.line("impl RouteId {");
    body.doc(4, "Which group serves this route.");
    body.line("    #[must_use]");
    body.line("    pub const fn group(self) -> RouteGroup {");
    body.line("        match self {");
    for group in groups {
        for operation in &group.operations {
            body.arm(
                12,
                &operation.variant,
                &format!("RouteGroup::{}", group.pascal),
            );
        }
    }
    body.line("        }");
    body.line("    }");
    body.line("}");
}

/// `FromParam` for every closed enumeration a path or query parameter names.
fn emit_param_decoders(ir: &ContractIr, body: &mut Source, imports: &mut BTreeSet<String>) {
    let mut named: BTreeSet<String> = BTreeSet::new();
    for operation in ir.operations() {
        for param in operation.path_params.iter().chain(&operation.query_params) {
            if let FieldType::Ref(schema) = &param.ty {
                named.insert(schema.clone());
            }
        }
    }
    let mut enums: Vec<&str> = Vec::new();
    for id in &named {
        let schema = ir
            .schemas
            .get(id)
            .expect("a parameter references a declared schema");
        if matches!(schema.body, SchemaBody::Enum { .. }) {
            enums.push(id.as_str());
        }
    }

    // Every id newtype is decodable from a parameter. Coherence cannot prove a
    // blanket implementation over `PrefixedId` does not overlap the newtype
    // implementations `dispatch` already writes, so the registry enumerates them.
    let ids: Vec<&str> = ir.ids.iter().map(|row| row.rust.as_str()).collect();
    if ids.is_empty() && enums.is_empty() {
        return;
    }
    imports.insert("crate::dispatch::FromParam".to_owned());
    imports.insert("crate::dispatch::ParamError".to_owned());
    for id in &ids {
        imports.insert(format!("crate::ids::{id}"));
    }
    for name in &enums {
        imports.insert(format!("crate::models::{name}"));
    }

    body.blank();
    body.line("// --- parameter decoding ---------------------------------------------------");
    body.blank();
    body.doc(
        0,
        "Decodes a prefixed identifier from a path segment or a query value.",
    );
    body.line("macro_rules! from_param_id {");
    body.line("    ($($ty:ty),* $(,)?) => {");
    body.line("        $(impl FromParam for $ty {");
    body.line("            fn from_param(text: &str) -> Result<Self, ParamError> {");
    body.line("                <$ty as crate::ids::PrefixedId>::parse(text)");
    body.line("                    .map_err(|reason| ParamError::new(reason.to_string()))");
    body.line("            }");
    body.line("        })*");
    body.line("    };");
    body.line("}");
    body.blank();
    body.macro_list(
        0,
        "from_param_id",
        &ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>(),
    );

    if enums.is_empty() {
        return;
    }
    body.blank();
    body.doc(0, "Decodes a closed enumeration from a query value.");
    body.line("macro_rules! from_param_enum {");
    body.line("    ($($ty:ty),* $(,)?) => {");
    body.line("        $(impl FromParam for $ty {");
    body.line("            fn from_param(text: &str) -> Result<Self, ParamError> {");
    body.line("                Self::ALL");
    body.line("                    .iter()");
    body.line("                    .copied()");
    body.line("                    .find(|candidate| candidate.as_str() == text)");
    body.line("                    .ok_or_else(|| {");
    body.line(
        "                        ParamError::new(format!(\"`{text}` is not a value of \
         `{}`\", stringify!($ty)))",
    );
    body.line("                    })");
    body.line("            }");
    body.line("        })*");
    body.line("    };");
    body.line("}");
    body.blank();
    body.macro_list(
        0,
        "from_param_enum",
        &enums
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    );
}

/// One trait per group, one method per operation.
fn emit_trait(
    ir: &ContractIr,
    group: &GroupIr<'_>,
    body: &mut Source,
    imports: &mut BTreeSet<String>,
) {
    body.blank();
    body.line(&format!(
        "// --- {}:{} ---------------------------------------------------------------",
        group.plane, group.fragment
    ));
    body.blank();
    body.doc(
        0,
        &format!(
            "The `{}` fragment of the {} plane: {} operation{}.",
            group.fragment,
            group.plane,
            group.operations.len(),
            if group.operations.len() == 1 { "" } else { "s" }
        ),
    );
    body.doc(
        0,
        "Every method returns a future that is `Send`, so the composition crate can spawn it \
         without wrapping. A method never names a status: the response type it returns is the \
         status the route declares.",
    );
    body.line(&format!(
        "pub trait {}: Send + Sync + 'static {{",
        group.trait_name
    ));
    if group.streams() {
        body.doc(
            4,
            "The frame stream this implementation produces for an NDJSON route.",
        );
        body.doc(
            4,
            "`aex-wire` deliberately does not name `Stream`: it has no async dependency, so the \
             composition crate supplies the concrete type and its own bound.",
        );
        body.line("    type FrameStream: Send + 'static;");
    }
    for (index, operation) in group.operations.iter().enumerate() {
        if index > 0 || group.streams() {
            body.blank();
        }
        body.doc(4, &format!("`{} {}`", operation.method, operation.path));
        body.doc(4, &operation.summary);
        let mut signature = format!("    fn {}(&self, cx: &RequestContext", operation.id);
        for argument in &method_arguments(ir, operation, imports) {
            signature.push_str(", ");
            signature.push_str(argument);
        }
        signature.push(')');
        let shape = ResponseShape::of(operation);
        record_response_imports(&shape, imports);
        let returns = format!(
            " -> impl Future<Output = WireResult<{}>> + Send;",
            shape.server_type()
        );
        if signature.len() + returns.len() <= crate::rustsrc::MAX_WIDTH {
            body.line(&format!("{signature}{returns}"));
            continue;
        }
        // `rustfmt` breaks a long signature one parameter per line and puts the
        // return type on the closing line.
        body.line(&format!("    fn {}(", operation.id));
        body.line("        &self,");
        body.line("        cx: &RequestContext,");
        for argument in &method_arguments(ir, operation, imports) {
            body.line(&format!("        {argument},"));
        }
        body.line(&format!(
            "    ) -> impl Future<Output = WireResult<{}>> + Send;",
            shape.server_type()
        ));
    }
    body.line("}");
}

/// The typed arguments of one operation, after `&self` and `cx`.
fn method_arguments(
    ir: &ContractIr,
    operation: &OperationIr,
    imports: &mut BTreeSet<String>,
) -> Vec<String> {
    let mut arguments = Vec::new();
    for param in &operation.path_params {
        let rendered = rust_type(ir, &param.ty, imports);
        arguments.push(format!("{}: {rendered}", param.rust));
    }
    if !operation.query_params.is_empty() {
        let name = format!("{}Query", operation.variant);
        imports.insert(format!("crate::models::{name}"));
        arguments.push(format!("query: {name}"));
    }
    match RequestShape::of(operation) {
        RequestShape::None => {}
        RequestShape::Json(schema) => {
            imports.insert(format!("crate::models::{schema}"));
            arguments.push(format!("body: {schema}"));
        }
        RequestShape::Otlp | RequestShape::Binary => {
            arguments.push("body: &[u8]".to_owned());
        }
    }
    arguments
}

/// Records the imports one response shape needs.
fn record_response_imports(shape: &ResponseShape, imports: &mut BTreeSet<String>) {
    match shape {
        ResponseShape::NoContent => {
            imports.insert("crate::server::NoContent".to_owned());
        }
        ResponseShape::Accepted => {
            imports.insert("crate::server::Accepted".to_owned());
        }
        ResponseShape::Created(schema) => {
            imports.insert("crate::server::Created".to_owned());
            imports.insert(format!("crate::models::{schema}"));
        }
        ResponseShape::Etagged(schema) => {
            imports.insert("crate::server::WithETag".to_owned());
            imports.insert(format!("crate::models::{schema}"));
        }
        ResponseShape::Plain(schema) => {
            imports.insert(format!("crate::models::{schema}"));
        }
        ResponseShape::Ndjson(_) => {
            imports.insert("crate::server::NdjsonStream".to_owned());
        }
        ResponseShape::Binary => {
            imports.insert("crate::server::BinaryBody".to_owned());
        }
    }
}

/// One total dispatcher per group.
fn emit_dispatcher(
    ir: &ContractIr,
    group: &GroupIr<'_>,
    body: &mut Source,
    imports: &mut BTreeSet<String>,
) {
    body.blank();
    body.doc(
        0,
        &format!(
            "Decodes, calls and encodes one `{}:{}` request.",
            group.plane, group.fragment
        ),
    );
    body.doc(
        0,
        "Total over `RouteId`: a route from another group is an internal error naming the \
         mismatch, never a silently wrong handler.",
    );
    body.doc(0, "# Errors");
    body.doc(
        0,
        "Returns the handler's own declared failure, or a decode failure the route declares. A \
         code the route does not declare is refused at this boundary.",
    );
    body.line(&format!(
        "pub async fn dispatch_{}<A: {} + ?Sized>(",
        group.snake, group.trait_name
    ));
    body.line("    api: &A,");
    body.line("    cx: &RequestContext,");
    body.line("    raw: RawRequest<'_>,");
    let limits = if group
        .operations
        .iter()
        .any(|operation| RequestShape::of(operation) != RequestShape::None)
    {
        "limits"
    } else {
        "_limits"
    };
    body.line(&format!("    {limits}: RequestLimits,"));
    body.line(&format!(
        ") -> WireResult<DispatchOutcome<{}>> {{",
        group.stream_type()
    ));
    let reader = if group
        .operations
        .iter()
        .any(|operation| !operation.query_params.is_empty())
    {
        "reader"
    } else {
        // The parse still runs: a route with no declared parameters must reject
        // a query string rather than ignore it.
        "_reader"
    };
    body.line(&format!(
        "    let {reader} = QueryReader::parse(raw.route, raw.query)?;"
    ));
    body.line("    match raw.route {");
    for operation in &group.operations {
        emit_arm(ir, operation, body, imports);
    }
    body.line(&format!(
        "        other => Err(wrong_group(other, {})),",
        quote(&format!("{}:{}", group.plane, group.fragment))
    ));
    body.line("    }");
    body.line("}");
}

/// One dispatch arm.
fn emit_arm(
    ir: &ContractIr,
    operation: &OperationIr,
    body: &mut Source,
    imports: &mut BTreeSet<String>,
) {
    imports.insert("crate::dispatch::declared".to_owned());
    body.line(&format!("        RouteId::{} => {{", operation.variant));

    let mut call: Vec<String> = vec!["cx".to_owned()];
    for param in &operation.path_params {
        let rendered = rust_type(ir, &param.ty, imports);
        // A segment whose type is a closed generated registry *names* a
        // resource, so a value outside the registry is `404 not_found` rather
        // than `400 invalid_request`. Every other path type describes an
        // identifier the caller minted, where a malformed value is exactly a
        // malformed request.
        let reader = if matches!(param.ty, FieldType::LimitId) {
            imports.insert("crate::dispatch::path_param_registry".to_owned());
            "path_param_registry"
        } else {
            imports.insert("crate::dispatch::path_param".to_owned());
            "path_param"
        };
        body.bind_call(
            12,
            &param.rust,
            &format!("{reader}::<{rendered}>"),
            &["&raw".to_owned(), quote(&param.name)],
            "?;",
        );
        call.push(param.rust.clone());
    }
    if !operation.query_params.is_empty() {
        let name = format!("{}Query", operation.variant);
        body.line(&format!("            let query = {name} {{"));
        for param in &operation.query_params {
            let accessor = query_accessor(param);
            body.line(&format!("                {}: {accessor}?,", param.rust));
        }
        body.line("            };");
        call.push("query".to_owned());
    }
    match RequestShape::of(operation) {
        RequestShape::None => {
            imports.insert("crate::dispatch::expect_no_body".to_owned());
            body.line("            expect_no_body(&raw)?;");
        }
        RequestShape::Json(schema) => {
            imports.insert("crate::dispatch::decode_body".to_owned());
            body.line(&format!(
                "            let body = decode_body::<{schema}>(&raw, limits)?;"
            ));
            call.push("body".to_owned());
        }
        RequestShape::Otlp => {
            imports.insert("crate::dispatch::otlp_body".to_owned());
            body.line("            let body = otlp_body(&raw, limits)?;");
            call.push("body".to_owned());
        }
        RequestShape::Binary => {
            imports.insert("crate::dispatch::binary_body".to_owned());
            body.line("            let body = binary_body(&raw, limits)?;");
            call.push("body".to_owned());
        }
    }

    let shape = ResponseShape::of(operation);
    // A `204` handler answers with the unit struct itself, so the binding is an
    // irrefutable pattern rather than a value nothing reads.
    let binding = if shape == ResponseShape::NoContent {
        "NoContent"
    } else {
        "answer"
    };
    // Two statements rather than one nested call: the handler call and the
    // declared-code check each stay inside `rustfmt`'s call width, so the
    // renderer never has to predict how a nested chain would be broken.
    body.bind_call(12, "handled", &format!("api.{}", operation.id), &call, ";");
    body.line(&format!(
        "            let {binding} = declared(raw.route, handled.await)?;"
    ));
    let rendered = match &shape {
        ResponseShape::NoContent => "DispatchOutcome::Unary(RawResponse::no_content())".to_owned(),
        ResponseShape::Accepted => {
            "DispatchOutcome::Unary(RawResponse::accepted(&answer)?)".to_owned()
        }
        ResponseShape::Created(_) => format!(
            "DispatchOutcome::Unary(RawResponse::json({}, &answer.0)?)",
            operation.success_status
        ),
        ResponseShape::Etagged(_) => {
            // The entity-tag form is the only one whose single expression is
            // wider than `rustfmt` will keep on one line, so it is bound first.
            body.line(&format!(
                "            let rendered = RawResponse::json({}, &answer.value)?;",
                operation.success_status
            ));
            body.line("            let rendered = rendered.with_etag(answer.etag);");
            "DispatchOutcome::Unary(rendered)".to_owned()
        }
        ResponseShape::Plain(_) => format!(
            "DispatchOutcome::Unary(RawResponse::json({}, &answer)?)",
            operation.success_status
        ),
        ResponseShape::Ndjson(_) => "DispatchOutcome::Ndjson(answer)".to_owned(),
        ResponseShape::Binary => format!(
            "DispatchOutcome::Unary(RawResponse::binary({}, answer.0))",
            operation.success_status
        ),
    };
    let single = format!("            Ok({rendered})");
    if single.len() <= crate::rustsrc::MAX_WIDTH {
        body.line(&single);
    } else {
        body.line("            Ok(");
        body.line(&format!("                {rendered},"));
        body.line("            )");
    }
    body.line("        }");
}

/// How one query parameter is read out of the strict reader.
fn query_accessor(param: &crate::ir::ParamIr) -> String {
    match (&param.ty, param.optional) {
        (FieldType::Integer { min, max }, true) => {
            format!(
                "reader.optional_bounded({}, {min}, {max})",
                quote(&param.name)
            )
        }
        (FieldType::Integer { min, max }, false) => {
            format!(
                "reader.required_bounded({}, {min}, {max})",
                quote(&param.name)
            )
        }
        (_, true) => format!("reader.optional({})", quote(&param.name)),
        (_, false) => format!("reader.required({})", quote(&param.name)),
    }
}
