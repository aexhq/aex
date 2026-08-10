//! The route-table emitter.
//!
//! One `RouteId` enum and one `ROUTES` slice, both in the same global
//! `operationId` order, so `route(id)` is an index rather than a match arm and
//! the two can never drift.

use crate::ir::ContractIr;
use crate::load::pascal_case;
use crate::rustsrc::{Source, quote};

/// Renders `crates/aex-wire/src/generated/routes.rs`.
#[must_use]
pub fn rust_routes(ir: &ContractIr, digest: &str) -> String {
    let operations = ir.operations();
    let mut source = Source::new("The route registry: one row per public operation.", digest);
    source.line("use crate::error::ErrorCode;");
    source.line("use crate::idempotency::IdempotencyKind;");
    source.line("use crate::routes::BodyClass;");
    source.line("use crate::routes::EtagPolicy;");
    source.line("use crate::routes::Plane;");
    source.line("use crate::routes::PrincipalKind;");
    source.line("use crate::routes::RouteDescriptor;");
    source.line("use crate::routes::TransportKind;");
    source.line("use crate::scopes::ScopeId;");
    source.line("use crate::types::HttpMethod;");
    source.blank();
    source.doc(
        0,
        "Every public operation. The discriminant is the index into `ROUTES`, so the two are \
         one table and cannot drift apart.",
    );
    source.line("#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]");
    source.line("pub enum RouteId {");
    for operation in &operations {
        source.doc(
            4,
            &format!(
                "`{} {}` — {}",
                operation.method, operation.path, operation.summary
            ),
        );
        source.line(&format!("    {},", operation.variant));
    }
    source.line("}");
    source.blank();
    source.line("impl RouteId {");
    source.doc(4, "Every route, in `operationId` order.");
    source.const_slice(
        4,
        "pub const ALL: &'static [RouteId]",
        &operations
            .iter()
            .map(|operation| format!("RouteId::{}", operation.variant))
            .collect::<Vec<_>>(),
    );
    source.blank();
    source.doc(4, "The `operationId`.");
    source.line("    #[must_use]");
    source.line("    pub const fn as_str(self) -> &'static str {");
    source.line("        match self {");
    for operation in &operations {
        source.arm(12, &operation.variant, &quote(&operation.id));
    }
    source.line("        }");
    source.line("    }");
    source.blank();
    source.doc(4, "Resolves an `operationId`.");
    source.line("    #[must_use]");
    source.line("    pub fn parse(text: &str) -> Option<Self> {");
    source.line("        Self::ALL.iter().copied().find(|it| it.as_str() == text)");
    source.line("    }");
    source.line("}");
    source.blank();
    source.doc(
        0,
        "Every route descriptor, indexed by `RouteId`. Sorted by `operationId`.",
    );
    source.line("pub static ROUTES: &[RouteDescriptor] = &[");
    for operation in &operations {
        source.line("    RouteDescriptor {");
        source.line(&format!("        id: RouteId::{},", operation.variant));
        source.line(&format!("        operation_id: {},", quote(&operation.id)));
        source.line(&format!(
            "        plane: Plane::{},",
            pascal_case(&operation.plane)
        ));
        source.line(&format!(
            "        fragment: {},",
            quote(&operation.fragment)
        ));
        source.line(&format!(
            "        serving_artifact: {},",
            quote(&operation.serving_artifact)
        ));
        source.line(&format!(
            "        deferred: {},",
            operation.deferred_reason.is_some()
        ));
        source.line(&format!(
            "        method: HttpMethod::{},",
            pascal_case(&operation.method.to_lowercase())
        ));
        source.line(&format!("        template: {},", quote(&operation.path)));
        source.slice_field(
            8,
            "path_params",
            &operation
                .path_params
                .iter()
                .map(|param| quote(&param.name))
                .collect::<Vec<_>>(),
        );
        source.slice_field(
            8,
            "query_params",
            &operation
                .query_params
                .iter()
                .map(|param| quote(&param.name))
                .collect::<Vec<_>>(),
        );
        let scope = operation.scope.as_ref().map_or_else(
            || "None".to_owned(),
            |scope| format!("Some(ScopeId::{})", pascal_case(&scope.replace(':', "_"))),
        );
        source.line(&format!("        required_scope: {scope},"));
        let principal = operation.alt_principal.as_ref().map_or_else(
            || "None".to_owned(),
            |kind| format!("Some(PrincipalKind::{})", pascal_case(kind)),
        );
        source.line(&format!("        alt_principal: {principal},"));
        source.line(&format!(
            "        idempotency: IdempotencyKind::{},",
            pascal_case(&operation.idempotency)
        ));
        source.line(&format!(
            "        body_class: BodyClass::{},",
            pascal_case(&operation.body_class)
        ));
        source.line(&format!(
            "        transport: TransportKind::{},",
            pascal_case(&operation.transport)
        ));
        source.line(&format!(
            "        success_status: {},",
            operation.success_status
        ));
        source.line(&format!(
            "        etag: EtagPolicy::{},",
            pascal_case(&operation.etag)
        ));
        source.slice_field(
            8,
            "errors",
            &operation
                .errors
                .iter()
                .map(|code| format!("ErrorCode::{}", pascal_case(code)))
                .collect::<Vec<_>>(),
        );
        let request = operation
            .request
            .as_ref()
            .map_or_else(|| "None".to_owned(), |id| format!("Some({})", quote(id)));
        source.line(&format!("        request_schema: {request},"));
        let response = operation
            .success
            .as_ref()
            .map_or_else(|| "None".to_owned(), |id| format!("Some({})", quote(id)));
        source.line(&format!("        response_schema: {response},"));
        source.line(&format!("        safe_retry: {},", operation.safe_retry));
        source.line(&format!(
            "        pause_exempt: {},",
            operation.pause_exempt
        ));
        source.line("    },");
    }
    source.line("];");
    source.finish()
}
