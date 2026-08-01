//! Key templates for `observation-authority`.
//!
//! Every key in this module is built from validated components. `#` is the only
//! separator, and `#`, `\0` and `\u{ffff}` are excluded from every component
//! before formatting, so a key always has exactly the field count its template
//! declares. Nothing customer-supplied is ever formatted into a key without
//! passing [`component`] first.

use std::fmt::Write as _;

use aex_wire::ids::PrefixedId;
use aex_wire::ids::{SessionId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::signal::Signal;

/// The fixed width of every zero-padded sequence field.
///
/// Twenty characters, matching the regional-plane convention (decision O-26),
/// which is wide enough for `10^20 − 1`.
pub const SEQ_WIDTH: usize = 20;

/// The largest value the fixed-width sequence field can carry.
pub const SEQ_MAX: u128 = 99_999_999_999_999_999_999;

/// The maximum byte length of one key component.
pub const COMPONENT_MAX_BYTES: usize = 256;

/// The sort key of the scope deletion-state item.
pub const DELETION_SK: &str = "DELETION";

/// The sort key of an admission receipt.
pub const RECEIPT_SK: &str = "RECEIPT";

/// The sort key of a series claim.
pub const CLAIM_SK: &str = "CLAIM";

/// The sort key of the workspace ingest-quota bucket.
pub const INGEST_SK: &str = "INGEST";

/// The sort key of the regional ingress-gate item.
pub const GATE_SK: &str = "STATE";

/// Why a key component was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum KeyError {
    /// The component was empty; an empty field would collapse two templates.
    #[error("a key component may not be empty")]
    Empty,
    /// The component carried the separator or a sentinel code point.
    #[error("a key component may not contain `{found}`")]
    Separator {
        /// The rejected code point, already escaped for display.
        found: Box<str>,
    },
    /// The component was longer than [`COMPONENT_MAX_BYTES`].
    #[error("a key component may be at most {COMPONENT_MAX_BYTES} bytes, got {len}")]
    TooLong {
        /// The measured length.
        len: usize,
    },
    /// A sequence value did not fit the fixed-width field.
    #[error("sequence {value} does not fit the {SEQ_WIDTH}-character field")]
    SequenceTooWide {
        /// The offending value.
        value: u128,
    },
    /// A key did not match the template it was parsed against.
    #[error("key does not match the `{template}` template")]
    Malformed {
        /// The template that was expected.
        template: &'static str,
    },
}

/// A component that is safe to format into a key.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyComponent<'a>(&'a str);

impl<'a> KeyComponent<'a> {
    /// The validated text.
    #[must_use]
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

impl std::fmt::Display for KeyComponent<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

/// Validates one key component.
///
/// # Errors
///
/// Returns [`KeyError::Empty`] for an empty component, [`KeyError::Separator`]
/// when the text carries `#`, `\0` or `\u{ffff}`, and [`KeyError::TooLong`]
/// above [`COMPONENT_MAX_BYTES`].
pub fn component(text: &str) -> Result<KeyComponent<'_>, KeyError> {
    if text.is_empty() {
        return Err(KeyError::Empty);
    }
    if text.len() > COMPONENT_MAX_BYTES {
        return Err(KeyError::TooLong { len: text.len() });
    }
    for character in text.chars() {
        if matches!(character, '#' | '\0' | '\u{ffff}') {
            return Err(KeyError::Separator {
                found: character.escape_debug().to_string().into_boxed_str(),
            });
        }
    }
    Ok(KeyComponent(text))
}

/// Renders `value` in the fixed-width sequence field.
///
/// # Panics
///
/// Panics when `value` exceeds [`SEQ_MAX`]. Callers holding an untrusted value
/// use [`try_pad_seq`]; every internal caller holds a counter that the frontier
/// already bounded.
#[must_use]
pub fn pad_seq(value: u128) -> String {
    try_pad_seq(value).expect("a sequence above SEQ_MAX cannot be padded")
}

/// Renders `value` in the fixed-width sequence field, or `None` when it does not
/// fit.
#[must_use]
pub fn try_pad_seq(value: u128) -> Option<String> {
    if value > SEQ_MAX {
        return None;
    }
    Some(format!("{value:0SEQ_WIDTH$}"))
}

/// A UTC hour bucket, rendered `YYYY-MM-DDTHH`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BucketHour {
    /// The rendered 13 ASCII bytes.
    text: [u8; 13],
}

impl BucketHour {
    /// The rendered width of a bucket hour.
    pub const WIDTH: usize = 13;

    /// The bucket an instant falls in.
    #[must_use]
    pub fn from_timestamp(value: Timestamp) -> Self {
        let wire = value.to_wire();
        let mut text = [0u8; 13];
        text.copy_from_slice(&wire.as_bytes()[..13]);
        Self { text }
    }

    /// Parses the rendered form.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::Malformed`] for anything that is not exactly
    /// `YYYY-MM-DDTHH` with the punctuation in place and digits elsewhere.
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        const TEMPLATE: &str = "YYYY-MM-DDTHH";
        let bytes = text.as_bytes();
        if bytes.len() != Self::WIDTH {
            return Err(KeyError::Malformed { template: TEMPLATE });
        }
        let punctuation = bytes[4] == b'-' && bytes[7] == b'-' && bytes[10] == b'T';
        let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12]
            .into_iter()
            .all(|index| bytes[index].is_ascii_digit());
        if !punctuation || !digits {
            return Err(KeyError::Malformed { template: TEMPLATE });
        }
        let mut buffer = [0u8; 13];
        buffer.copy_from_slice(bytes);
        Ok(Self { text: buffer })
    }

    /// The rendered form.
    ///
    /// # Panics
    ///
    /// Never: the buffer only ever holds ASCII written by a constructor.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.text).expect("a BucketHour holds ASCII")
    }

    /// The `YYYY-MM-DD` day this bucket belongs to, used by `gsi_metric`.
    ///
    /// # Panics
    ///
    /// Never: see [`BucketHour::as_str`].
    #[must_use]
    pub fn day(&self) -> &str {
        std::str::from_utf8(&self.text[..10]).expect("a BucketHour holds ASCII")
    }

    /// The next hour bucket.
    ///
    /// # Panics
    ///
    /// Never: the bucket is derived from a representable [`Timestamp`], and the
    /// successor of any representable hour below the wire maximum is itself
    /// representable.
    #[must_use]
    pub fn next(self) -> Self {
        let start = self.start();
        let next = start
            .checked_add(time::Duration::hours(1))
            .expect("an hour past a representable bucket is representable");
        Self::from_timestamp(
            Timestamp::from_datetime_trunc_ms(next).expect("the successor hour is representable"),
        )
    }

    /// The first instant inside this bucket.
    ///
    /// # Panics
    ///
    /// Never: a `BucketHour` is only ever built from a valid rendering.
    #[must_use]
    pub fn start(self) -> time::OffsetDateTime {
        let text = format!("{}:00:00.000Z", self.as_str());
        Timestamp::parse(&text)
            .expect("a BucketHour renders a parseable instant")
            .to_datetime()
    }
}

impl std::fmt::Display for BucketHour {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which scope an observation belongs to.
///
/// A session when one exists and the workspace otherwise (decision O-05). Only a
/// workspace-key OTLP call made directly against the public route lacks a
/// session.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ScopeKey {
    /// A session-scoped observation.
    Session(SessionId),
    /// A workspace-scoped observation with no session.
    Workspace(WorkspaceId),
}

impl ScopeKey {
    /// The rendered scope component, `S#{session_id}` or `W#{workspace_id}`.
    #[must_use]
    pub fn to_key(self) -> String {
        match self {
            Self::Session(id) => format!("S#{id}"),
            Self::Workspace(id) => format!("W#{id}"),
        }
    }

    /// Parses the rendered scope component.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::Malformed`] when the discriminator is absent or the
    /// identifier does not parse as the kind the discriminator names.
    pub fn parse(text: &str) -> Result<Self, KeyError> {
        const TEMPLATE: &str = "S#{session_id} | W#{workspace_id}";
        let malformed = KeyError::Malformed { template: TEMPLATE };
        let (discriminator, id) = text.split_once('#').ok_or_else(|| malformed.clone())?;
        match discriminator {
            "S" => SessionId::parse(id)
                .map(Self::Session)
                .map_err(|_| malformed),
            "W" => WorkspaceId::parse(id)
                .map(Self::Workspace)
                .map_err(|_| malformed),
            _ => Err(malformed),
        }
    }

    /// The session, when the scope is one.
    #[must_use]
    pub const fn session(self) -> Option<SessionId> {
        match self {
            Self::Session(id) => Some(id),
            Self::Workspace(_) => None,
        }
    }

    /// A stable eight-hex-character digest of the scope, used as the
    /// `gsi_ws_accepted` tie-break so cross-session ordering needs no shared
    /// counter.
    #[must_use]
    pub fn hash8(self) -> String {
        let digest = blake3::hash(self.to_key().as_bytes());
        let bytes = digest.as_bytes();
        let mut out = String::with_capacity(8);
        for byte in &bytes[..4] {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

impl std::fmt::Display for ScopeKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_key())
    }
}

/// The classification of an `observation-authority` `DynamoDB` Stream record.
///
/// This is the whole wake filter `regional-stream` needs: every wake-relevant
/// partition key begins with the literal `OBS#`, and this function returns
/// `None` for every other item family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationWakeKey {
    /// The scope the observation belongs to.
    pub scope: ScopeKey,
    /// The signal it carries.
    pub signal: Signal,
    /// The accepted-time hour bucket.
    pub abucket: BucketHour,
    /// The partition shard inside the bucket.
    pub shard: u8,
}

/// `OBS#{scope}#{signal}#{abucket}#{shard:02}`.
#[must_use]
pub fn observation_pk(scope: &ScopeKey, signal: Signal, abucket: BucketHour, shard: u8) -> String {
    debug_assert!(
        signal.in_observation_authority(),
        "`events` are never stored in observation-authority"
    );
    format!(
        "OBS#{}#{}#{}#{shard:02}",
        scope.to_key(),
        signal.as_str(),
        abucket.as_str()
    )
}

/// `{accepted_seq:020}`.
#[must_use]
pub fn observation_sk(accepted_seq: u64) -> String {
    pad_seq(u128::from(accepted_seq))
}

/// Classifies an `observation-authority` partition key.
///
/// Returns `Some` for exactly the observation revision family and `None` for
/// every other item family, including a well-formed key naming the `events`
/// signal, which can never legitimately exist in this table.
#[must_use]
pub fn parse_observation_pk(pk: &str) -> Option<ObservationWakeKey> {
    let rest = pk.strip_prefix("OBS#")?;
    let mut fields = rest.split('#');
    let discriminator = fields.next()?;
    let scope_id = fields.next()?;
    let signal = fields.next()?;
    let abucket = fields.next()?;
    let shard = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let scope = ScopeKey::parse(&format!("{discriminator}#{scope_id}")).ok()?;
    let signal = Signal::parse(signal)?;
    if !signal.in_observation_authority() {
        return None;
    }
    let abucket = BucketHour::parse(abucket).ok()?;
    if shard.len() != 2 || !shard.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let shard = shard.parse::<u8>().ok()?;
    Some(ObservationWakeKey {
        scope,
        signal,
        abucket,
        shard,
    })
}

/// `SEG#{scope}#{signal}` — the accepted-time segment directory.
#[must_use]
pub fn segment_pk(scope: &ScopeKey, signal: Signal) -> String {
    format!("SEG#{}#{}", scope.to_key(), signal.as_str())
}

/// `SEGT#{scope}#{signal}` — the event-time segment directory.
#[must_use]
pub fn time_segment_pk(scope: &ScopeKey, signal: Signal) -> String {
    format!("SEGT#{}#{}", scope.to_key(), signal.as_str())
}

/// `FRONT#{scope}` — the scope frontier and deletion partition.
#[must_use]
pub fn frontier_pk(scope: &ScopeKey) -> String {
    format!("FRONT#{}", scope.to_key())
}

/// `SIG#{signal}` — the per-signal frontier sort key.
#[must_use]
pub fn frontier_sk(signal: Signal) -> String {
    format!("SIG#{}", signal.as_str())
}

/// `BATCH#{workspace_id}#{batch_id}`.
#[must_use]
pub fn batch_pk(workspace: WorkspaceId, batch: aex_wire::ids::TelemetryBatchId) -> String {
    format!("BATCH#{workspace}#{batch}")
}

/// `PAGE#{page:06}` — a staged page sort key.
#[must_use]
pub fn staged_page_sk(page: u32) -> String {
    format!("PAGE#{page:06}")
}

/// `BODY#{body_sha256_hex}` — a staged body pointer sort key.
#[must_use]
pub fn staged_body_sk(sha256_hex: &str) -> String {
    format!("BODY#{sha256_hex}")
}

/// `SERIES#{workspace_id}#{series_hash_hex}`.
#[must_use]
pub fn series_pk(workspace: WorkspaceId, series_hash_hex: &str) -> String {
    format!("SERIES#{workspace}#{series_hash_hex}")
}

/// `SERIESCT#{workspace_id}`.
#[must_use]
pub fn series_counter_pk(workspace: WorkspaceId) -> String {
    format!("SERIESCT#{workspace}")
}

/// `SHARD#{shard:03}` — a series counter shard sort key.
#[must_use]
pub fn series_counter_sk(shard: u16) -> String {
    format!("SHARD#{shard:03}")
}

/// `QUOTA#{workspace_id}`.
#[must_use]
pub fn quota_pk(workspace: WorkspaceId) -> String {
    format!("QUOTA#{workspace}")
}

/// `GAP#{scope}`.
#[must_use]
pub fn gap_pk(scope: &ScopeKey) -> String {
    format!("GAP#{}", scope.to_key())
}

/// `{gap_id}#{revision:020}` — a gap revision sort key.
#[must_use]
pub fn gap_sk(gap_id: aex_wire::ids::TelemetryGapId, revision: u64) -> String {
    format!("{gap_id}#{}", pad_seq(u128::from(revision)))
}

/// `SPOOL#{workspace_id}#{shard:02}`.
#[must_use]
pub fn spool_pk(workspace: WorkspaceId, shard: u8) -> String {
    format!("SPOOL#{workspace}#{shard:02}")
}

/// `EXPORT#{workspace_id}#{export_id}`.
#[must_use]
pub fn export_pk(workspace: WorkspaceId, export: aex_wire::ids::ExportId) -> String {
    format!("EXPORT#{workspace}#{export}")
}

/// `GATE#{region}`.
#[must_use]
pub fn gate_pk(region: aex_wire::types::Region) -> String {
    format!("GATE#{}", region.as_str())
}

/// The closed vocabulary of due-item control domains.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ControlDomain {
    /// Re-drive an unacknowledged spool chunk.
    SpoolRepair,
    /// Abort a `preparing` receipt past its deadline.
    BatchExpire,
    /// Launch an admitted export.
    ExportLaunch,
    /// Reap an expired or revoked export.
    ExportReap,
    /// Execute a fenced scope deletion.
    DeletionExecute,
    /// Prove a scope deletion complete.
    DeletionVerify,
    /// Verify secondary-index coherence.
    IndexVerify,
    /// Re-evaluate the regional ingress gate.
    GateEvaluate,
    /// Sweep reclaimable series claims.
    SeriesReclaim,
}

impl ControlDomain {
    /// Every domain, in declared order.
    pub const ALL: &'static [ControlDomain] = &[
        ControlDomain::SpoolRepair,
        ControlDomain::BatchExpire,
        ControlDomain::ExportLaunch,
        ControlDomain::ExportReap,
        ControlDomain::DeletionExecute,
        ControlDomain::DeletionVerify,
        ControlDomain::IndexVerify,
        ControlDomain::GateEvaluate,
        ControlDomain::SeriesReclaim,
    ];

    /// The literal key component.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SpoolRepair => "spool.repair",
            Self::BatchExpire => "batch.expire",
            Self::ExportLaunch => "export.launch",
            Self::ExportReap => "export.reap",
            Self::DeletionExecute => "deletion.execute",
            Self::DeletionVerify => "deletion.verify",
            Self::IndexVerify => "index.verify",
            Self::GateEvaluate => "gate.evaluate",
            Self::SeriesReclaim => "series.reclaim",
        }
    }

    /// Parses the literal key component.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|domain| domain.as_str() == text)
    }
}

/// `CTRL#{domain}#{shard:02}`.
#[must_use]
pub fn control_pk(domain: ControlDomain, shard: u8) -> String {
    format!("CTRL#{}#{shard:02}", domain.as_str())
}

/// `{due_at}#{item_id}` — a due-item sort key.
#[must_use]
pub fn control_sk(due_at: Timestamp, item_id: &str) -> String {
    format!("{}#{item_id}", due_at.to_wire())
}

/// The closed vocabulary of idempotency scopes on this table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyScope {
    /// An OTLP logs ingest.
    OtlpLogs,
    /// An OTLP traces ingest.
    OtlpTraces,
    /// An OTLP metrics ingest.
    OtlpMetrics,
    /// A download-grant mint for one export.
    ExportDownload(aex_wire::ids::ExportId),
    /// A revoke of one export.
    ExportRevoke(aex_wire::ids::ExportId),
}

impl IdempotencyScope {
    /// The rendered scope tag.
    #[must_use]
    pub fn tag(&self) -> String {
        match self {
            Self::OtlpLogs => "otlp.logs".to_owned(),
            Self::OtlpTraces => "otlp.traces".to_owned(),
            Self::OtlpMetrics => "otlp.metrics".to_owned(),
            Self::ExportDownload(id) => format!("export.download:{id}"),
            Self::ExportRevoke(id) => format!("export.revoke:{id}"),
        }
    }
}

/// `IDEM#{workspace_id}#{scope_tag}#{key_sha256_hex}`.
#[must_use]
pub fn idempotency_pk(
    workspace: WorkspaceId,
    scope: &IdempotencyScope,
    key_sha256_hex: &str,
) -> String {
    format!("IDEM#{workspace}#{}#{key_sha256_hex}", scope.tag())
}

#[cfg(test)]
mod tests {
    use super::{ControlDomain, IdempotencyScope, ScopeKey, control_pk, idempotency_pk};
    use aex_wire::ids::{ExportId, WorkspaceId};
    use aex_wire::ids::{PrefixedId, Uuid7};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10]))
    }

    #[test]
    fn the_control_vocabulary_is_closed_and_round_trips() {
        assert_eq!(ControlDomain::ALL.len(), 9);
        for domain in ControlDomain::ALL {
            assert_eq!(ControlDomain::parse(domain.as_str()), Some(*domain));
            assert!(!domain.as_str().contains('#'));
        }
        assert_eq!(ControlDomain::parse("materialize"), None);
        assert_eq!(
            control_pk(ControlDomain::SpoolRepair, 3),
            "CTRL#spool.repair#03"
        );
    }

    #[test]
    fn the_idempotency_vocabulary_is_closed() {
        let export = ExportId::from_uuid7(Uuid7::compose(2, [2; 10]));
        let rendered = idempotency_pk(workspace(), &IdempotencyScope::ExportDownload(export), "ff");
        assert!(rendered.starts_with("IDEM#wsp_"));
        assert!(rendered.contains(&format!("export.download:{export}")));
        assert_eq!(IdempotencyScope::OtlpLogs.tag(), "otlp.logs");
    }

    #[test]
    fn a_scope_hash_is_stable_and_eight_hex_characters() {
        let scope = ScopeKey::Workspace(workspace());
        let hash = scope.hash8();
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hash, scope.hash8());
    }
}
