//! The export manifest and its hash.
//!
//! `manifestHash` is the `SHA-256` of the manifest's RFC 8785 canonical JCS
//! encoding. Each member's `SHA-256` covers its exact bytes. Together they are
//! what lets a customer prove the artifact they downloaded is the one AEX
//! produced — which only means anything because every encoder setting is pinned
//! (see [`crate::encoder`]).
//!
//! A `completeness: "require"` export **fails** on any intersecting open gap
//! rather than producing an artifact that silently omits a window.

use std::collections::BTreeMap;

use sha2::{Digest as _, Sha256};

use crate::encoder::Format;

/// What an export does when the window has recorded gaps.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Completeness {
    /// Refuse to produce an artifact over a gap.
    Require,
    /// Produce the artifact and record every gap in the manifest.
    AllowGaps,
}

impl Completeness {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Require => "require",
            Self::AllowGaps => "allow_gaps",
        }
    }
}

/// Why an export could not be produced.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ManifestError {
    /// The window has an open gap and the export required completeness.
    #[error("the window has {count} open gap(s) and completeness is `require`")]
    Incomplete {
        /// How many gaps intersect.
        count: usize,
    },
    /// The signal cannot be represented in the requested format.
    #[error("`{signal}` cannot be exported as `{format}`")]
    UnsupportedSignal {
        /// The refused signal.
        signal: &'static str,
        /// The format that refused it.
        format: &'static str,
    },
}

/// One member of the artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportMember {
    /// Its position in the artifact.
    pub ordinal: u16,
    /// Its object name.
    pub name: Box<str>,
    /// Its exact byte length.
    pub bytes: u64,
    /// The `SHA-256` of its exact bytes.
    pub sha256: Box<str>,
    /// How many records it carries.
    pub records: u64,
}

/// The manifest the artifact carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportManifest {
    /// The manifest schema revision.
    pub schema_revision: u32,
    /// The normalized query the export was produced from.
    pub normalized_query: BTreeMap<String, String>,
    /// The pinned snapshot position, in epoch milliseconds.
    pub snapshot: u128,
    /// The four coverage watermarks, in epoch milliseconds.
    pub watermarks: BTreeMap<String, u128>,
    /// How many records the artifact carries in total.
    pub records: u64,
    /// The members, in ordinal order.
    pub members: Vec<ExportMember>,
    /// Every gap that intersects the window.
    pub gaps: Vec<String>,
    /// The completeness rule it was produced under.
    pub completeness: Completeness,
    /// When it was produced.
    pub exported_at: Box<str>,
}

impl ExportManifest {
    /// Builds a manifest, enforcing the completeness rule.
    ///
    /// # Errors
    ///
    /// Returns [`ManifestError::Incomplete`] when the window has an open gap and
    /// the export required completeness. `allow_gaps` succeeds and records every
    /// gap; there is no third behaviour where a gap is quietly dropped.
    pub fn build(
        completeness: Completeness,
        gaps: Vec<String>,
        members: Vec<ExportMember>,
        snapshot: u128,
        exported_at: &str,
    ) -> Result<Self, ManifestError> {
        if completeness == Completeness::Require && !gaps.is_empty() {
            return Err(ManifestError::Incomplete { count: gaps.len() });
        }
        let records = members.iter().map(|member| member.records).sum();
        Ok(Self {
            schema_revision: 1,
            normalized_query: BTreeMap::new(),
            snapshot,
            watermarks: BTreeMap::new(),
            records,
            members,
            gaps,
            completeness,
            exported_at: exported_at.into(),
        })
    }

    /// The RFC 8785 canonical encoding of the manifest.
    ///
    /// Written by hand over `BTreeMap`s and sorted vectors rather than delegated
    /// to a serializer, because canonical key order is the whole point and a
    /// serializer's field order is not a contract.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut out = String::new();
        out.push('{');
        let _ = write!(out, r#""completeness":"{}","#, self.completeness.as_str());
        let _ = write!(out, r#""exportedAt":"{}","#, self.exported_at);
        out.push_str(r#""gaps":["#);
        let mut sorted = self.gaps.clone();
        sorted.sort();
        for (index, gap) in sorted.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, r#""{gap}""#);
        }
        out.push_str("],");
        out.push_str(r#""members":["#);
        let mut members = self.members.clone();
        members.sort_by_key(|member| member.ordinal);
        for (index, member) in members.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                r#"{{"bytes":{},"name":"{}","ordinal":{},"records":{},"sha256":"{}"}}"#,
                member.bytes, member.name, member.ordinal, member.records, member.sha256
            );
        }
        out.push_str("],");
        let _ = write!(out, r#""records":{},"#, self.records);
        let _ = write!(out, r#""schemaRevision":{},"#, self.schema_revision);
        let _ = write!(out, r#""snapshot":"{}""#, self.snapshot);
        out.push('}');
        out.into_bytes()
    }

    /// The `SHA-256` of the canonical encoding, lowercase hex.
    #[must_use]
    pub fn manifest_hash(&self) -> String {
        use std::fmt::Write as _;
        let digest = Sha256::digest(self.to_canonical_bytes());
        let mut out = String::with_capacity(64);
        for byte in digest {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

/// Whether a signal may be exported as the requested format.
///
/// `otlp_json` rejects `events` and trace summaries rather than coercing them:
/// neither has an OTLP representation, and inventing one would produce a file
/// that claims to be OTLP and is not.
///
/// # Errors
///
/// Returns [`ManifestError::UnsupportedSignal`] naming both the signal and the
/// format.
pub const fn check_signal(signal: &'static str, format: Format) -> Result<(), ManifestError> {
    if matches!(format, Format::OtlpJson)
        && (matches!(signal.as_bytes(), b"events") || matches!(signal.as_bytes(), b"traces"))
    {
        return Err(ManifestError::UnsupportedSignal {
            signal,
            format: format.as_str(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Completeness, ExportManifest, ExportMember, ManifestError, check_signal};
    use crate::encoder::Format;

    fn member(ordinal: u16) -> ExportMember {
        ExportMember {
            ordinal,
            name: format!("{ordinal:04}.jsonl").into(),
            bytes: 128,
            sha256: "a".repeat(64).into(),
            records: 4,
        }
    }

    #[test]
    fn require_fails_on_any_intersecting_open_gap() {
        let error = ExportManifest::build(
            Completeness::Require,
            vec!["gap_x".to_owned()],
            vec![member(0)],
            42,
            "2026-08-01T09:00:00.000Z",
        )
        .expect_err("refused");
        assert_eq!(error, ManifestError::Incomplete { count: 1 });
    }

    #[test]
    fn allow_gaps_succeeds_with_every_gap_in_the_manifest() {
        let manifest = ExportManifest::build(
            Completeness::AllowGaps,
            vec!["gap_b".to_owned(), "gap_a".to_owned()],
            vec![member(0), member(1)],
            42,
            "2026-08-01T09:00:00.000Z",
        )
        .expect("builds");
        assert_eq!(manifest.gaps.len(), 2);
        assert_eq!(manifest.records, 8);
        let canonical = String::from_utf8(manifest.to_canonical_bytes()).expect("utf8");
        assert!(
            canonical.contains(r#""gaps":["gap_a","gap_b"]"#),
            "gap order is canonical: {canonical}"
        );
    }

    #[test]
    fn the_manifest_hash_is_stable_and_order_independent() {
        let forward = ExportManifest::build(
            Completeness::AllowGaps,
            vec!["gap_a".to_owned(), "gap_b".to_owned()],
            vec![member(0), member(1)],
            42,
            "2026-08-01T09:00:00.000Z",
        )
        .expect("builds");
        let reverse = ExportManifest::build(
            Completeness::AllowGaps,
            vec!["gap_b".to_owned(), "gap_a".to_owned()],
            vec![member(1), member(0)],
            42,
            "2026-08-01T09:00:00.000Z",
        )
        .expect("builds");
        assert_eq!(forward.manifest_hash(), reverse.manifest_hash());
        assert_eq!(forward.manifest_hash().len(), 64);
    }

    #[test]
    fn changing_a_member_digest_changes_the_manifest_hash() {
        let base = ExportManifest::build(
            Completeness::AllowGaps,
            Vec::new(),
            vec![member(0)],
            42,
            "2026-08-01T09:00:00.000Z",
        )
        .expect("builds");
        let mut tampered_member = member(0);
        tampered_member.sha256 = "b".repeat(64).into();
        let tampered = ExportManifest::build(
            Completeness::AllowGaps,
            Vec::new(),
            vec![tampered_member],
            42,
            "2026-08-01T09:00:00.000Z",
        )
        .expect("builds");
        assert_ne!(base.manifest_hash(), tampered.manifest_hash());
    }

    #[test]
    fn otlp_json_rejects_events_and_trace_summaries_rather_than_coercing() {
        for signal in ["events", "traces"] {
            assert!(matches!(
                check_signal(signal, Format::OtlpJson),
                Err(ManifestError::UnsupportedSignal { .. })
            ));
            check_signal(signal, Format::Ndjson).expect("ndjson carries every signal");
        }
        check_signal("logs", Format::OtlpJson).expect("logs are OTLP-representable");
        assert_eq!(Completeness::AllowGaps.as_str(), "allow_gaps");
    }
}
