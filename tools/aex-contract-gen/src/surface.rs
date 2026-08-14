//! The route-group and response-shape model both surface emitters read.
//!
//! The server traits, the dispatchers and the client all have to agree on which
//! operations belong together, what a method is called, and which Rust shape a
//! success takes. Deriving that once here is what keeps the three emitters from
//! disagreeing; each of them would otherwise re-decide it from the same fields
//! and eventually decide differently.

use std::collections::{BTreeMap, BTreeSet};

use crate::ir::{ContractIr, FieldType, OperationIr};
use crate::load::pascal_case;

/// One mountable group of operations: exactly one authoring fragment on exactly
/// one plane.
#[derive(Debug, Clone)]
pub struct GroupIr<'a> {
    /// Stable key, `<fragment>` or `<plane>-<fragment>` when the stem collides.
    pub key: String,
    /// `snake_case` form, used for the dispatcher and the route constant.
    pub snake: String,
    /// `PascalCase` form, used for the [`RouteGroup`] variant.
    pub pascal: String,
    /// The generated trait name.
    pub trait_name: String,
    /// Which plane serves the group.
    pub plane: &'a str,
    /// Which authoring fragment owns it.
    pub fragment: &'a str,
    /// Its operations, in `operationId` order.
    pub operations: Vec<&'a OperationIr>,
}

impl GroupIr<'_> {
    /// Whether any operation in the group answers with a frame stream.
    #[must_use]
    pub fn streams(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| operation.transport == "ndjson")
    }

    /// The `SCREAMING_SNAKE_CASE` name of the group's route constant.
    #[must_use]
    pub fn routes_const(&self) -> String {
        format!("{}_ROUTES", self.snake.to_uppercase())
    }

    /// The stream type the group's dispatcher yields.
    #[must_use]
    pub fn stream_type(&self) -> String {
        if self.streams() {
            "A::FrameStream".to_owned()
        } else {
            "crate::dispatch::NoStream".to_owned()
        }
    }
}

/// Every group, in key order.
///
/// A fragment stem that appears on both planes is disambiguated by plane; every
/// other stem stands alone. `operations` collides today and nothing else does,
/// which is exactly why the rule is derived rather than hard-coded.
#[must_use]
pub fn groups(ir: &ContractIr) -> Vec<GroupIr<'_>> {
    let mut planes_by_stem: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for operation in ir.operations() {
        planes_by_stem
            .entry(operation.fragment.as_str())
            .or_default()
            .insert(operation.plane.as_str());
    }

    let mut grouped: BTreeMap<String, GroupIr<'_>> = BTreeMap::new();
    for operation in ir.operations() {
        let stem = operation.fragment.as_str();
        let ambiguous = planes_by_stem
            .get(stem)
            .is_some_and(|planes| planes.len() > 1);
        let key = if ambiguous {
            format!("{}-{stem}", operation.plane)
        } else {
            stem.to_owned()
        };
        let snake = key.replace('-', "_");
        let pascal = pascal_case(&key);
        grouped
            .entry(key.clone())
            .or_insert_with(|| GroupIr {
                trait_name: format!("{pascal}Api"),
                key,
                snake,
                pascal,
                plane: operation.plane.as_str(),
                fragment: stem,
                operations: Vec::new(),
            })
            .operations
            .push(operation);
    }
    grouped.into_values().collect()
}

/// The Rust shape one operation answers with.
///
/// Derived wholly from the route table, which is what makes it impossible for a
/// handler to choose a status: it never names one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseShape {
    /// `204`, no body.
    NoContent,
    /// `201`, the named schema.
    Created(String),
    /// `200` with a strong entity tag.
    Etagged(String),
    /// `200`, the named schema.
    Plain(String),
    /// `200 application/x-ndjson`, a stream of the named frame schema.
    Ndjson(String),
    /// One bounded `application/octet-stream` body.
    Binary,
}

impl ResponseShape {
    /// The shape `operation` answers with.
    ///
    /// # Panics
    ///
    /// Never: `load` rejects a success status other than `204` without a schema.
    #[must_use]
    pub fn of(operation: &OperationIr) -> Self {
        let schema = || {
            operation
                .success
                .clone()
                .expect("a non-204 success declares a schema")
        };
        if operation.transport == "ndjson" {
            return Self::Ndjson(schema());
        }
        if operation.transport == "binary" {
            return Self::Binary;
        }
        match operation.success_status {
            204 => Self::NoContent,
            201 => Self::Created(schema()),
            _ if operation.etag != "none" => Self::Etagged(schema()),
            _ => Self::Plain(schema()),
        }
    }

    /// How a server-trait method writes this shape.
    #[must_use]
    pub fn server_type(&self) -> String {
        match self {
            Self::NoContent => "NoContent".to_owned(),
            Self::Created(schema) => format!("Created<{schema}>"),
            Self::Etagged(schema) => format!("WithETag<{schema}>"),
            Self::Plain(schema) => schema.clone(),
            Self::Ndjson(_) => "NdjsonStream<Self::FrameStream>".to_owned(),
            Self::Binary => "BinaryBody".to_owned(),
        }
    }

    /// How a client method writes this shape.
    #[must_use]
    pub fn client_type(&self) -> String {
        match self {
            Self::NoContent => "()".to_owned(),
            Self::Created(schema) | Self::Etagged(schema) | Self::Plain(schema) => {
                if matches!(self, Self::Etagged(_)) {
                    format!("WithETag<{schema}>")
                } else {
                    schema.clone()
                }
            }
            Self::Ndjson(frame) => format!("NdjsonFrames<{frame}>"),
            Self::Binary => "Vec<u8>".to_owned(),
        }
    }

    /// The `SchemaId` this shape names, if any.
    #[must_use]
    pub fn schema(&self) -> Option<&str> {
        match self {
            Self::NoContent | Self::Binary => None,
            Self::Created(schema)
            | Self::Etagged(schema)
            | Self::Plain(schema)
            | Self::Ndjson(schema) => Some(schema),
        }
    }

    /// The stable name recorded in the surface corpus.
    #[must_use]
    pub fn corpus_name(&self) -> &'static str {
        match self {
            Self::NoContent => "no_content",
            Self::Created(_) => "created",
            Self::Etagged(_) => "etagged",
            Self::Plain(_) => "plain",
            Self::Ndjson(_) => "ndjson",
            Self::Binary => "binary",
        }
    }
}

/// What one operation accepts as a request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestShape {
    /// The route declares no body.
    None,
    /// A strict AEX JSON schema.
    Json(String),
    /// The pinned standard OTLP revision, bounded but not interpreted.
    Otlp,
    /// Bounded opaque bytes.
    Binary,
}

impl RequestShape {
    /// The shape `operation` accepts.
    ///
    /// # Panics
    ///
    /// Never: `load` rejects an `aex_json` body class without a request schema.
    #[must_use]
    pub fn of(operation: &OperationIr) -> Self {
        match operation.body_class.as_str() {
            "aex_json" => Self::Json(
                operation
                    .request
                    .clone()
                    .expect("an `aex_json` body declares a schema"),
            ),
            "otlp" => Self::Otlp,
            "binary" => Self::Binary,
            _ => Self::None,
        }
    }

    /// The stable name recorded in the surface corpus.
    #[must_use]
    pub const fn corpus_name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Json(_) => "aex_json",
            Self::Otlp => "otlp",
            Self::Binary => "binary",
        }
    }
}

/// Whether a route accepts or requires `If-Match`.
#[must_use]
pub fn takes_if_match(operation: &OperationIr) -> bool {
    matches!(
        operation.etag.as_str(),
        "optional_if_match" | "required_if_match"
    )
}

/// Whether a parameter type is `Copy`, and therefore passed to a client builder
/// by value rather than by reference.
///
/// Driven by the type table rather than by a name list, so a new parameter type
/// has to say which side it is on.
#[must_use]
pub const fn is_copy(ty: &FieldType) -> bool {
    matches!(
        ty,
        FieldType::Id(_)
            | FieldType::Timestamp
            | FieldType::Decimal
            | FieldType::Cents
            | FieldType::Integer { .. }
            | FieldType::Float
            | FieldType::Bool
            | FieldType::Region
            | FieldType::ComputeSize
            | FieldType::ContentHash
            | FieldType::TraceId
            | FieldType::SpanId
            | FieldType::ProviderId
            | FieldType::Scope
            | FieldType::LimitId
            | FieldType::ErrorCode
    )
}
