#![allow(
    missing_docs,
    reason = "exit meanings are published by the generated CLI registry"
)]

use aex_wire::error::ErrorClass;

/// Stable local CLI failure classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliErrorClass {
    Unclassified,
    Usage,
    Configuration,
    DurableTerminal,
    Deadline,
    LocalIo,
    Interrupted,
}

impl CliErrorClass {
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Unclassified => 1,
            Self::Usage => 2,
            Self::Configuration => 3,
            Self::DurableTerminal => 12,
            Self::Deadline => 13,
            Self::LocalIo => 14,
            Self::Interrupted => 130,
        }
    }
}

/// Total generated error-class to stable process-exit mapping.
#[must_use]
pub const fn exit_code_for_error_class(class: ErrorClass) -> u8 {
    match class {
        ErrorClass::Auth => 4,
        ErrorClass::NotFound => 5,
        ErrorClass::Conflict | ErrorClass::Precondition => 6,
        ErrorClass::Validation => 7,
        ErrorClass::Quota => 8,
        ErrorClass::State => 9,
        ErrorClass::Unavailable => 10,
        ErrorClass::Internal => 11,
    }
}
