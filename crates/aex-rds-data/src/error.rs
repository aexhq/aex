//! Failure classification for the Data `API`.
//!
//! Every failure is classified once, here, from the service exception name plus
//! the `PostgreSQL` `SQLSTATE` the Data `API` appends to its message. Nothing in
//! this module retries, and an unmatched failure is [`DataApiError::Fatal`]
//! rather than a silently retried one — a blanket retry over an unknown failure
//! is how a non-idempotent statement runs twice.

use std::fmt;

/// The five-character `PostgreSQL` `SQLSTATE` carried in a Data `API` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SqlState([u8; 5]);

impl SqlState {
    /// Parses a five-character `SQLSTATE`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 5 || !bytes.iter().all(u8::is_ascii_alphanumeric) {
            return None;
        }
        let mut code = [0_u8; 5];
        code.copy_from_slice(bytes);
        Some(Self(code))
    }

    /// The code as a string slice.
    ///
    /// # Panics
    ///
    /// Never: the constructor admits ASCII alphanumerics only.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("an SqlState holds ASCII alphanumerics only")
    }
}

impl fmt::Display for SqlState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why a field could not be turned into the requested Rust type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// A non-optional column held `isNull`.
    #[error("column {index} is null and the target type is not optional")]
    UnexpectedNull {
        /// Zero-based column index.
        index: usize,
    },
    /// A column arrived as `doubleValue`.
    ///
    /// There is no accessor that would read one. Floating point is refused at
    /// the transport boundary in every schema, not merely in the money schema.
    #[error(
        "column {index} arrived as doubleValue; this transport has no floating-point decode path"
    )]
    UnexpectedDoubleValue {
        /// Zero-based column index.
        index: usize,
    },
    /// A column arrived as a Data `API` variant the accessor cannot read.
    #[error("column {index} is not a {expected}")]
    TypeMismatch {
        /// Zero-based column index.
        index: usize,
        /// What the accessor wanted.
        expected: &'static str,
    },
    /// A `stringValue` was not valid UTF-8 after transport decoding.
    #[error("column {index} is not valid UTF-8")]
    BadUtf8 {
        /// Zero-based column index.
        index: usize,
    },
    /// A `uuid`-typed column did not parse.
    #[error("column {index} is not a UUID")]
    BadUuid {
        /// Zero-based column index.
        index: usize,
    },
    /// A `NUMERIC` column did not parse as a scaled integer.
    #[error("column {index} is not a fixed-scale numeric")]
    BadNumeric {
        /// Zero-based column index.
        index: usize,
    },
    /// An epoch-millis column was outside the representable instant range.
    #[error("column {index} is not a representable timestamp")]
    BadTimestamp {
        /// Zero-based column index.
        index: usize,
    },
    /// The record had a different column count than the row decoder expects.
    #[error("expected {expected} column(s), got {actual}")]
    ArityMismatch {
        /// How many columns the decoder projects.
        expected: usize,
        /// How many the record actually held.
        actual: usize,
    },
    /// A value did not fit the requested Rust type.
    #[error("column {index} overflows its target type")]
    Overflow {
        /// Zero-based column index.
        index: usize,
    },
    /// A `json`-typed column did not deserialize into the target type.
    #[error("column {index} is not the expected JSON shape")]
    BadJson {
        /// Zero-based column index.
        index: usize,
    },
}

/// Every way the Data `API` transport can fail.
///
/// The variants are the classification, not the recovery: nothing in this crate
/// acts on one. `Serialization` and `Deadlock` are separate arms precisely so an
/// application can retry a replay-safe transaction without also retrying a
/// `UniqueViolation`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DataApiError {
    /// The cluster is waking from auto-pause.
    #[error("the Aurora cluster is resuming")]
    Resuming,
    /// The statement exceeded the Data `API` statement timeout.
    #[error("the statement timed out")]
    Timeout,
    /// The Data `API` throttled the caller.
    #[error("the Data API throttled this caller")]
    Throttled,
    /// `SQLSTATE` `40001`: serialization failure.
    #[error("serialization failure (40001)")]
    Serialization,
    /// `SQLSTATE` `40P01`: deadlock detected.
    #[error("deadlock detected (40P01)")]
    Deadlock,
    /// `SQLSTATE` `23505`.
    #[error("unique violation on constraint `{constraint}`")]
    UniqueViolation {
        /// The exact `PostgreSQL` constraint name, so mapping is deterministic.
        constraint: String,
    },
    /// `SQLSTATE` `23503`.
    #[error("foreign key violation on constraint `{constraint}`")]
    ForeignKeyViolation {
        /// The exact `PostgreSQL` constraint name.
        constraint: String,
    },
    /// `SQLSTATE` `23514`.
    #[error("check violation on constraint `{constraint}`")]
    CheckViolation {
        /// The exact `PostgreSQL` constraint name.
        constraint: String,
    },
    /// `SQLSTATE` `23502`.
    #[error("not-null violation on column `{column}`")]
    NotNullViolation {
        /// The column that refused a null.
        column: String,
    },
    /// `SQLSTATE` `23000`, including a `RAISE` from a constraint trigger.
    #[error("integrity constraint violation: {message}")]
    IntegrityConstraintViolation {
        /// The database message, already free of parameter values.
        message: String,
    },
    /// `SQLSTATE` `42501`: the connected role lacks the privilege.
    #[error("permission denied")]
    PermissionDenied,
    /// `SQLSTATE` `42P01`/`42703`: the object or column does not exist.
    #[error("undefined object `{object}`")]
    UndefinedObject {
        /// What the statement named.
        object: String,
    },
    /// The response exceeded the configured result budget.
    #[error("result of {bytes} byte(s) exceeds the configured result budget")]
    ResultTooLarge {
        /// How many bytes arrived.
        bytes: usize,
    },
    /// One field exceeded the configured field budget.
    #[error("field {index} of {bytes} byte(s) exceeds the configured field budget")]
    FieldTooLarge {
        /// Zero-based column index.
        index: usize,
        /// How many bytes the field held.
        bytes: usize,
    },
    /// The transaction id is unknown to the Data `API`.
    #[error("the transaction is not known to the Data API")]
    TransactionNotFound,
    /// The transaction outlived the Data `API` transaction window.
    #[error("the transaction expired")]
    TransactionExpired,
    /// The client-side guard deadline elapsed first.
    #[error("the configured deadline elapsed")]
    DeadlineExceeded,
    /// A record could not be turned into the requested row type.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The endpoint could not be reached, or answered with a retryable fault.
    #[error("the Data API is unavailable: {message}")]
    Unavailable {
        /// A low-cardinality rendering of the transport failure.
        message: String,
    },
    /// Anything not matched above. Never retried.
    #[error("unclassified Data API failure{}: {message}", .code.as_ref().map(|it| format!(" ({it})")).unwrap_or_default())]
    Fatal {
        /// The service exception name or `SQLSTATE`, when one was present.
        code: Option<String>,
        /// A low-cardinality rendering of the failure.
        message: String,
    },
}

impl DataApiError {
    /// Whether an application may retry the *same* statement without changing
    /// its identity.
    ///
    /// This is a classification, not a policy: the application still has to know
    /// its own transaction is replay-safe and that no external effect happened.
    #[must_use]
    pub const fn transient(&self) -> bool {
        matches!(
            self,
            Self::Resuming
                | Self::Timeout
                | Self::Throttled
                | Self::Serialization
                | Self::Deadlock
                | Self::Unavailable { .. }
        )
    }
}

/// The Data `API` service exceptions this transport distinguishes.
///
/// Modelling the exception name as a closed enum keeps classification a pure
/// function that a test can drive exhaustively, and keeps the AWS SDK error
/// types out of every downstream signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExceptionKind {
    /// `BadRequestException` — carries the `PostgreSQL` `SQLSTATE` when the
    /// statement itself failed.
    BadRequest,
    /// `DatabaseErrorException`.
    DatabaseError,
    /// `DatabaseNotFoundException`.
    DatabaseNotFound,
    /// `DatabaseResumingException`.
    DatabaseResuming,
    /// `DatabaseUnavailableException`.
    DatabaseUnavailable,
    /// `AccessDeniedException` or `ForbiddenException`.
    AccessDenied,
    /// `HttpEndpointNotEnabledException`.
    HttpEndpointNotEnabled,
    /// `InternalServerErrorException`.
    InternalServerError,
    /// `InvalidResourceStateException`.
    InvalidResourceState,
    /// `InvalidSecretException` or `SecretsErrorException`.
    SecretUnusable,
    /// `NotFoundException`.
    NotFound,
    /// `ServiceUnavailableError`.
    ServiceUnavailable,
    /// `StatementTimeoutException`.
    StatementTimeout,
    /// `TransactionNotFoundException`.
    TransactionNotFound,
    /// `UnsupportedResultException`.
    UnsupportedResult,
    /// `ThrottlingException` or `TooManyRequestsException`.
    Throttling,
    /// The connection failed before or after the request reached the service.
    Transport,
    /// An exception this SDK revision does not name.
    Unknown,
}

impl ExceptionKind {
    /// Resolves the wire exception name.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "BadRequestException" => Self::BadRequest,
            "DatabaseErrorException" => Self::DatabaseError,
            "DatabaseNotFoundException" => Self::DatabaseNotFound,
            "DatabaseResumingException" => Self::DatabaseResuming,
            "DatabaseUnavailableException" => Self::DatabaseUnavailable,
            "AccessDeniedException" | "ForbiddenException" => Self::AccessDenied,
            "HttpEndpointNotEnabledException" => Self::HttpEndpointNotEnabled,
            "InternalServerErrorException" => Self::InternalServerError,
            "InvalidResourceStateException" => Self::InvalidResourceState,
            "InvalidSecretException" | "SecretsErrorException" => Self::SecretUnusable,
            "NotFoundException" => Self::NotFound,
            "ServiceUnavailableError" => Self::ServiceUnavailable,
            "StatementTimeoutException" => Self::StatementTimeout,
            "TransactionNotFoundException" => Self::TransactionNotFound,
            "UnsupportedResultException" => Self::UnsupportedResult,
            "ThrottlingException" | "TooManyRequestsException" => Self::Throttling,
            _ => Self::Unknown,
        }
    }
}

/// Message fragments a `BadRequestException` uses to report an auto-paused or
/// waking cluster instead of raising `DatabaseResumingException`.
const RESUMING_FRAGMENTS: [&str; 3] = ["resuming", "auto-paused", "not currently available"];

/// Classifies a Data `API` failure.
///
/// `name` is the service exception name and `message` its rendered message. The
/// function is pure, so the whole mapping table is a unit test.
#[must_use]
pub fn classify(kind: ExceptionKind, message: &str) -> DataApiError {
    match kind {
        ExceptionKind::DatabaseResuming => DataApiError::Resuming,
        ExceptionKind::StatementTimeout => DataApiError::Timeout,
        ExceptionKind::Throttling => DataApiError::Throttled,
        ExceptionKind::TransactionNotFound => DataApiError::TransactionNotFound,
        ExceptionKind::AccessDenied => DataApiError::PermissionDenied,
        ExceptionKind::DatabaseUnavailable
        | ExceptionKind::ServiceUnavailable
        | ExceptionKind::InternalServerError
        | ExceptionKind::Transport => DataApiError::Unavailable {
            message: message.to_owned(),
        },
        ExceptionKind::BadRequest | ExceptionKind::DatabaseError => classify_statement(message),
        ExceptionKind::DatabaseNotFound
        | ExceptionKind::HttpEndpointNotEnabled
        | ExceptionKind::InvalidResourceState
        | ExceptionKind::SecretUnusable
        | ExceptionKind::NotFound
        | ExceptionKind::UnsupportedResult
        | ExceptionKind::Unknown => DataApiError::Fatal {
            code: None,
            message: message.to_owned(),
        },
    }
}

/// Classifies the statement-level failures the Data `API` reports as a
/// `BadRequestException`.
fn classify_statement(message: &str) -> DataApiError {
    let lowered = message.to_ascii_lowercase();
    if RESUMING_FRAGMENTS
        .iter()
        .any(|fragment| lowered.contains(fragment))
    {
        return DataApiError::Resuming;
    }
    if lowered.contains("transaction") && lowered.contains("expired") {
        return DataApiError::TransactionExpired;
    }
    if lowered.contains("transaction") && lowered.contains("not found") {
        return DataApiError::TransactionNotFound;
    }
    let Some(state) = sql_state(message) else {
        return DataApiError::Fatal {
            code: None,
            message: message.to_owned(),
        };
    };
    match state.as_str() {
        "40001" => DataApiError::Serialization,
        "40P01" => DataApiError::Deadlock,
        "23505" => DataApiError::UniqueViolation {
            constraint: constraint(message),
        },
        "23503" => DataApiError::ForeignKeyViolation {
            constraint: constraint(message),
        },
        "23514" => DataApiError::CheckViolation {
            constraint: constraint(message),
        },
        "23502" => DataApiError::NotNullViolation {
            column: quoted_after(message, "null value in column").unwrap_or_else(unnamed),
        },
        "23000" => DataApiError::IntegrityConstraintViolation {
            message: message.to_owned(),
        },
        "42501" => DataApiError::PermissionDenied,
        "42P01" | "42703" => DataApiError::UndefinedObject {
            object: quoted_anywhere(message).unwrap_or_else(unnamed),
        },
        other => DataApiError::Fatal {
            code: Some(other.to_owned()),
            message: message.to_owned(),
        },
    }
}

/// The placeholder used when the database did not name the offending object.
fn unnamed() -> String {
    "<unnamed>".to_owned()
}

/// Extracts the `SQLState: <code>` suffix the Data `API` appends.
#[must_use]
pub fn sql_state(message: &str) -> Option<SqlState> {
    let marker = message.to_ascii_lowercase().rfind("sqlstate:")?;
    let tail = message[marker + "sqlstate:".len()..].trim_start();
    let code: String = tail
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    SqlState::parse(&code)
}

/// Extracts the constraint name from a `PostgreSQL` integrity message.
fn constraint(message: &str) -> String {
    quoted_after(message, "constraint")
        .or_else(|| quoted_anywhere(message))
        .unwrap_or_else(unnamed)
}

/// The first double-quoted token following `keyword`, case-insensitively.
fn quoted_after(message: &str, keyword: &str) -> Option<String> {
    let lowered = message.to_ascii_lowercase();
    let at = lowered.find(keyword)?;
    quoted_anywhere(&message[at + keyword.len()..])
}

/// The first double-quoted token in `text`.
fn quoted_anywhere(text: &str) -> Option<String> {
    let open = text.find('"')?;
    let rest = &text[open + 1..];
    let close = rest.find('"')?;
    Some(rest[..close].to_owned())
}

#[cfg(test)]
mod tests {
    use super::{DataApiError, ExceptionKind, SqlState, classify, sql_state};

    #[test]
    fn an_sql_state_admits_exactly_five_alphanumerics() {
        assert_eq!(
            SqlState::parse("23505").map(|it| it.as_str().to_owned()),
            Some("23505".to_owned())
        );
        assert_eq!(
            SqlState::parse("40P01").map(|it| it.as_str().to_owned()),
            Some("40P01".to_owned())
        );
        assert_eq!(SqlState::parse("2350"), None);
        assert_eq!(SqlState::parse("235055"), None);
        assert_eq!(SqlState::parse("235-5"), None);
    }

    #[test]
    fn the_sql_state_suffix_is_read_from_the_end() {
        let message = "ERROR: duplicate key value violates unique constraint \"user_email_uk\"; SQLState: 23505";
        assert_eq!(
            sql_state(message).map(|it| it.as_str().to_owned()),
            Some("23505".to_owned())
        );
    }

    #[test]
    fn a_unique_violation_names_its_constraint() {
        let message = "ERROR: duplicate key value violates unique constraint \"user_email_uk\"  Detail: Key (email)=(a@b.test) already exists.; SQLState: 23505";
        assert_eq!(
            classify(ExceptionKind::BadRequest, message),
            DataApiError::UniqueViolation {
                constraint: "user_email_uk".to_owned()
            }
        );
    }

    #[test]
    fn a_foreign_key_violation_names_its_constraint() {
        let message = "ERROR: insert or update on table \"api_key\" violates foreign key constraint \"api_key_workspace_id_fkey\"; SQLState: 23503";
        assert_eq!(
            classify(ExceptionKind::BadRequest, message),
            DataApiError::ForeignKeyViolation {
                constraint: "api_key_workspace_id_fkey".to_owned()
            }
        );
    }

    #[test]
    fn a_check_violation_names_its_constraint() {
        let message = "ERROR: new row for relation \"user\" violates check constraint \"user_status_ck\"; SQLState: 23514";
        assert_eq!(
            classify(ExceptionKind::BadRequest, message),
            DataApiError::CheckViolation {
                constraint: "user_status_ck".to_owned()
            }
        );
    }

    #[test]
    fn a_not_null_violation_names_its_column() {
        let message = "ERROR: null value in column \"email\" of relation \"user\" violates not-null constraint; SQLState: 23502";
        assert_eq!(
            classify(ExceptionKind::BadRequest, message),
            DataApiError::NotNullViolation {
                column: "email".to_owned()
            }
        );
    }

    #[test]
    fn serialization_and_deadlock_are_separate_arms() {
        assert_eq!(
            classify(
                ExceptionKind::BadRequest,
                "ERROR: could not serialize access; SQLState: 40001"
            ),
            DataApiError::Serialization
        );
        assert_eq!(
            classify(
                ExceptionKind::BadRequest,
                "ERROR: deadlock detected; SQLState: 40P01"
            ),
            DataApiError::Deadlock
        );
    }

    #[test]
    fn a_privilege_failure_is_permission_denied_from_either_source() {
        assert_eq!(
            classify(
                ExceptionKind::BadRequest,
                "ERROR: permission denied for table user; SQLState: 42501"
            ),
            DataApiError::PermissionDenied
        );
        assert_eq!(
            classify(
                ExceptionKind::AccessDenied,
                "not authorized to perform rds-data:ExecuteStatement"
            ),
            DataApiError::PermissionDenied
        );
    }

    #[test]
    fn an_undefined_object_names_what_the_statement_asked_for() {
        assert_eq!(
            classify(
                ExceptionKind::BadRequest,
                "ERROR: relation \"control.ghost\" does not exist; SQLState: 42P01"
            ),
            DataApiError::UndefinedObject {
                object: "control.ghost".to_owned()
            }
        );
    }

    #[test]
    fn a_bad_request_naming_a_paused_cluster_is_resuming() {
        for fragment in [
            "The DB cluster is resuming after being auto-paused",
            "Communications link failure: cluster auto-paused",
            "Database cluster is not currently available",
        ] {
            assert_eq!(
                classify(ExceptionKind::BadRequest, fragment),
                DataApiError::Resuming,
                "{fragment}"
            );
        }
    }

    #[test]
    fn the_service_exceptions_map_one_for_one() {
        let table = [
            (ExceptionKind::DatabaseResuming, DataApiError::Resuming),
            (ExceptionKind::StatementTimeout, DataApiError::Timeout),
            (ExceptionKind::Throttling, DataApiError::Throttled),
            (
                ExceptionKind::TransactionNotFound,
                DataApiError::TransactionNotFound,
            ),
            (ExceptionKind::AccessDenied, DataApiError::PermissionDenied),
        ];
        for (kind, expected) in table {
            assert_eq!(classify(kind, "message"), expected, "{kind:?}");
        }
    }

    #[test]
    fn an_unmatched_failure_is_fatal_and_never_transient() {
        let error = classify(ExceptionKind::Unknown, "something new");
        assert!(matches!(error, DataApiError::Fatal { code: None, .. }));
        assert!(!error.transient());
    }

    #[test]
    fn only_the_named_families_are_transient() {
        assert!(DataApiError::Serialization.transient());
        assert!(DataApiError::Deadlock.transient());
        assert!(DataApiError::Resuming.transient());
        assert!(DataApiError::Throttled.transient());
        assert!(DataApiError::Timeout.transient());
        assert!(
            !DataApiError::UniqueViolation {
                constraint: "x".to_owned()
            }
            .transient()
        );
        assert!(!DataApiError::PermissionDenied.transient());
        assert!(!DataApiError::DeadlineExceeded.transient());
    }

    #[test]
    fn an_expired_transaction_is_distinguished_from_a_missing_one() {
        assert_eq!(
            classify(
                ExceptionKind::BadRequest,
                "Transaction ABC is expired, retry the transaction"
            ),
            DataApiError::TransactionExpired
        );
        assert_eq!(
            classify(
                ExceptionKind::BadRequest,
                "Transaction ABC is not found in the service"
            ),
            DataApiError::TransactionNotFound
        );
    }
}
