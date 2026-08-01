//! Temporary peer surface required until the central identity stream merges.

/// TODO(cross-stream): replaced by `aex_rds_data::transaction::CommitOutcomeUnknown` at merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitOutcomeUnknown {
    /// Provider transaction identity whose commit response was lost.
    pub transaction_id: String,
}

/// TODO(cross-stream): replaced by `aex_rds_data::row::FieldValue` at merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdsField {
    /// Data API integer member.
    Long(i64),
    /// Data API string member.
    String(String),
    /// A floating member was present; its value is deliberately inaccessible.
    DoublePresent,
    /// Database null.
    Null,
}
