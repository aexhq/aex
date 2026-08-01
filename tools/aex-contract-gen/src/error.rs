//! Every way the generator refuses to produce output.
//!
//! There is no warning level. A contract the generator cannot fully resolve is
//! not written at all, because a partially generated bundle is worse than none:
//! it compiles.

/// A generator failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenError {
    /// A file could not be read.
    #[error("cannot read `{path}`: {reason}")]
    Io {
        /// Workspace-relative path.
        path: String,
        /// The underlying failure.
        reason: String,
    },
    /// A document could not be parsed.
    #[error("cannot parse `{path}`: {reason}")]
    Parse {
        /// Workspace-relative path.
        path: String,
        /// The parser's message.
        reason: String,
    },
    /// The input tree contained a symbolic link.
    #[error("`{path}` is a symbolic link; the authored tree must be plain files")]
    Symlink {
        /// Workspace-relative path.
        path: String,
    },
    /// A required input directory produced nothing.
    #[error("`{what}` produced no input")]
    EmptyInput {
        /// Which input tree was empty.
        what: &'static str,
    },
    /// A registry row failed validation.
    #[error("registry `{registry}`: {detail}")]
    Registry {
        /// Which registry.
        registry: &'static str,
        /// What exactly failed.
        detail: String,
    },
    /// A schema failed validation.
    #[error("schema `{id}`: {detail}")]
    Schema {
        /// The offending `SchemaId`.
        id: String,
        /// What exactly failed.
        detail: String,
    },
    /// Two files declared the same `SchemaId`.
    #[error("duplicate schema `{id}` declared in `{first}` and `{second}`")]
    DuplicateSchema {
        /// The offending `SchemaId`.
        id: String,
        /// First declaring file.
        first: String,
        /// Second declaring file.
        second: String,
    },
    /// A reference did not resolve.
    #[error("`{from}` references unknown schema `{target}`")]
    UnresolvedRef {
        /// Where the reference lives.
        from: String,
        /// The unresolvable target.
        target: String,
    },
    /// Two operations shared an `operationId`.
    #[error("duplicate operationId `{id}`")]
    DuplicateOperation {
        /// The offending `operationId`.
        id: String,
    },
    /// Two operations shared a method and path within one plane.
    #[error("duplicate route `{method} {path}`")]
    DuplicateRoute {
        /// HTTP method.
        method: String,
        /// Path template.
        path: String,
    },
    /// A plane header disagreed with its location.
    #[error("plane `{plane}`: {detail}")]
    Plane {
        /// Plane id.
        plane: String,
        /// What exactly failed.
        detail: String,
    },
    /// An operation failed validation.
    #[error("operation `{id}`: {detail}")]
    Operation {
        /// The offending `operationId`.
        id: String,
        /// What exactly failed.
        detail: String,
    },
    /// A plane's operation count drifted from the pinned total.
    #[error("plane `{plane}` has {found} operations, expected {expected}")]
    OperationCount {
        /// Plane id.
        plane: String,
        /// The pinned total.
        expected: usize,
        /// What was actually assembled.
        found: usize,
    },
    /// A conformance case named something the contract does not contain.
    #[error("conformance case `{case}`: {detail}")]
    Corpus {
        /// Workspace-relative case path.
        case: String,
        /// What exactly failed.
        detail: String,
    },
    /// A required corpus floor was not met.
    #[error("conformance floor `{category}`: {detail}")]
    CorpusFloor {
        /// Which category.
        category: &'static str,
        /// What exactly is missing.
        detail: String,
    },
}
