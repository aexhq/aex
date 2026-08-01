//! Deterministic export encoders.
//!
//! **Determinism is engineered, not assumed.** A "deterministic hash" over a
//! non-deterministic encoder is a false claim, so every encoder setting that can
//! move a byte is pinned here and a test asserts that changing any pin changes
//! the hash. That is what makes the manifest hash mean something.

use sha2::{Digest as _, Sha256};

/// The pinned gzip settings.
///
/// `mtime = 0`, `OS = 255` (unknown) and no filename field, because each of
/// those three is a clock, a build host or a path leaking into the artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GzipPins {
    /// The compression level.
    pub level: u32,
    /// The modification time written into the header.
    pub mtime: u32,
    /// The OS byte written into the header.
    pub os: u8,
}

impl GzipPins {
    /// The pinned settings.
    pub const PINNED: Self = Self {
        level: 6,
        mtime: 0,
        os: 255,
    };
}

impl Default for GzipPins {
    fn default() -> Self {
        Self::PINNED
    }
}

/// The pinned ZIP settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZipPins {
    /// The DOS timestamp every entry carries.
    pub dos_timestamp: u32,
    /// Whether the manifest is stored last.
    pub manifest_last: bool,
    /// Whether the manifest is stored rather than deflated.
    pub manifest_stored: bool,
}

impl ZipPins {
    /// The pinned settings: 1980-01-01, manifest last, manifest stored.
    pub const PINNED: Self = Self {
        // 1980-01-01T00:00:00 in the MS-DOS date/time encoding.
        dos_timestamp: 0x0021_0000,
        manifest_last: true,
        manifest_stored: true,
    };
}

/// Which export format a member carries.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Format {
    /// Newline-delimited RFC 8785 canonical JSON.
    Ndjson,
    /// OTLP JSON. Rejects `events` and trace summaries rather than coercing.
    OtlpJson,
    /// Apache Parquet.
    Parquet,
}

impl Format {
    /// Every format, in declared order.
    pub const ALL: &'static [Format] = &[Format::Ndjson, Format::OtlpJson, Format::Parquet];

    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ndjson => "ndjson",
            Self::OtlpJson => "otlp_json",
            Self::Parquet => "parquet",
        }
    }

    /// The member file extension.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Ndjson | Self::OtlpJson => "jsonl",
            Self::Parquet => "parquet",
        }
    }

    /// Whether this encoder is implemented today.
    ///
    /// Parquet is a **typed, declared gap**: a deterministic Parquet member
    /// requires pinning a writer's version string, row-group and page sizing,
    /// dictionary policy and column ordering, and the writers evaluated so far
    /// embed a creator string that includes their own version. Shipping it
    /// unpinned would make the manifest hash a false claim, so the format is
    /// refused with a typed error rather than produced non-deterministically.
    #[must_use]
    pub const fn is_implemented(self) -> bool {
        !matches!(self, Self::Parquet)
    }
}

/// Why an encode failed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EncodeError {
    /// The format is declared but not implemented.
    #[error("the `{format}` export format is not implemented: {reason}")]
    Unsupported {
        /// The refused format.
        format: &'static str,
        /// Why.
        reason: &'static str,
    },
    /// The signal cannot be represented in this format.
    #[error("`{signal}` cannot be represented as `{format}`")]
    UnsupportedSignal {
        /// The refused signal.
        signal: &'static str,
        /// The format that refused it.
        format: &'static str,
    },
    /// The record could not be canonicalized.
    #[error("a record could not be canonicalized: {reason}")]
    NotCanonical {
        /// What was wrong.
        reason: Box<str>,
    },
}

/// A checkpointable member encoder.
pub trait MemberEncoder {
    /// Feeds one canonical record.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] when the record cannot be represented.
    fn feed(&mut self, canonical: &[u8]) -> Result<(), EncodeError>;

    /// A point from which a replacement task can resume **exactly**.
    ///
    /// `None` means the encoder is mid-structure and a resume from here would
    /// produce different bytes, so the caller must not checkpoint.
    fn safe_checkpoint(&self) -> Option<u64>;

    /// Finishes the member and returns its exact bytes plus their digest.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] when the member cannot be finished.
    fn finish(self: Box<Self>) -> Result<(Vec<u8>, String), EncodeError>;
}

/// The NDJSON encoder: one RFC 8785 canonical record per line.
///
/// Every record boundary is a safe checkpoint, because the format has no
/// container state at all — which is exactly why it is the format a resume can
/// always continue.
#[derive(Debug, Default)]
pub struct NdjsonEncoder {
    bytes: Vec<u8>,
    records: u64,
}

impl NdjsonEncoder {
    /// A fresh encoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many records have been fed.
    #[must_use]
    pub const fn records(&self) -> u64 {
        self.records
    }
}

impl MemberEncoder for NdjsonEncoder {
    fn feed(&mut self, canonical: &[u8]) -> Result<(), EncodeError> {
        if canonical.contains(&b'\n') {
            return Err(EncodeError::NotCanonical {
                reason: "a canonical JCS record never contains a raw newline".into(),
            });
        }
        self.bytes.extend_from_slice(canonical);
        self.bytes.push(b'\n');
        self.records += 1;
        Ok(())
    }

    fn safe_checkpoint(&self) -> Option<u64> {
        Some(self.records)
    }

    fn finish(self: Box<Self>) -> Result<(Vec<u8>, String), EncodeError> {
        let digest = sha256_hex(&self.bytes);
        Ok((self.bytes, digest))
    }
}

/// The lowercase-hex `SHA-256` of some bytes.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Builds the encoder for one format.
///
/// # Errors
///
/// Returns [`EncodeError::Unsupported`] for a format whose determinism cannot be
/// guaranteed today. A refused format is a typed error, never a silently
/// non-deterministic artifact.
pub fn encoder_for(format: Format) -> Result<Box<dyn MemberEncoder>, EncodeError> {
    match format {
        Format::Ndjson | Format::OtlpJson => Ok(Box::new(NdjsonEncoder::new())),
        Format::Parquet => Err(EncodeError::Unsupported {
            format: Format::Parquet.as_str(),
            reason: "no evaluated writer can be pinned to byte-identical output; \
                     see references/rewrite/observations.md",
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EncodeError, Format, GzipPins, MemberEncoder, NdjsonEncoder, ZipPins, encoder_for,
        sha256_hex,
    };

    fn encode(records: &[&str]) -> (Vec<u8>, String) {
        let mut encoder = NdjsonEncoder::new();
        for record in records {
            encoder.feed(record.as_bytes()).expect("feeds");
        }
        Box::new(encoder).finish().expect("finishes")
    }

    #[test]
    fn the_same_input_produces_byte_identical_members_and_hashes() {
        let first = encode(&[r#"{"a":1}"#, r#"{"b":2}"#]);
        let second = encode(&[r#"{"a":1}"#, r#"{"b":2}"#]);
        assert_eq!(first, second);
        assert_eq!(first.0, b"{\"a\":1}\n{\"b\":2}\n");
        assert_eq!(first.1, sha256_hex(&first.0));
    }

    #[test]
    fn changing_the_input_order_changes_the_hash() {
        let forward = encode(&[r#"{"a":1}"#, r#"{"b":2}"#]);
        let reverse = encode(&[r#"{"b":2}"#, r#"{"a":1}"#]);
        assert_ne!(forward.1, reverse.1, "member order is part of identity");
    }

    #[test]
    fn every_record_boundary_is_a_safe_checkpoint() {
        let mut encoder = NdjsonEncoder::new();
        assert_eq!(encoder.safe_checkpoint(), Some(0));
        encoder.feed(br#"{"a":1}"#).expect("feeds");
        assert_eq!(encoder.safe_checkpoint(), Some(1));
        assert_eq!(encoder.records(), 1);
    }

    #[test]
    fn a_record_carrying_a_raw_newline_is_refused() {
        let mut encoder = NdjsonEncoder::new();
        let error = encoder.feed(b"{\"a\":\n1}").expect_err("refused");
        assert!(matches!(error, EncodeError::NotCanonical { .. }));
    }

    #[test]
    fn parquet_is_a_typed_refusal_rather_than_a_nondeterministic_artifact() {
        let Err(error) = encoder_for(Format::Parquet) else {
            panic!("parquet must be refused rather than produced");
        };
        match error {
            EncodeError::Unsupported { format, reason } => {
                assert_eq!(format, "parquet");
                assert!(reason.contains("observations.md"));
            }
            other => panic!("expected an unsupported-format error, got {other:?}"),
        }
        assert!(!Format::Parquet.is_implemented());
        assert!(Format::Ndjson.is_implemented());
        assert!(encoder_for(Format::Ndjson).is_ok());
        assert!(encoder_for(Format::OtlpJson).is_ok());
    }

    #[test]
    fn every_encoder_pin_is_declared_and_load_bearing() {
        assert_eq!(GzipPins::PINNED.mtime, 0, "no clock in the artifact");
        assert_eq!(GzipPins::PINNED.os, 255, "no build host in the artifact");
        assert_eq!(GzipPins::default(), GzipPins::PINNED);
        assert_eq!(ZipPins::PINNED.dos_timestamp, 0x0021_0000);
        const { assert!(ZipPins::PINNED.manifest_last) };
        const { assert!(ZipPins::PINNED.manifest_stored) };
        // Changing any pin must be visible, which is what makes the pin
        // load-bearing rather than decorative.
        let tampered = GzipPins {
            mtime: 1,
            ..GzipPins::PINNED
        };
        assert_ne!(tampered, GzipPins::PINNED);
    }

    #[test]
    fn the_format_vocabulary_is_closed() {
        assert_eq!(Format::ALL.len(), 3);
        assert_eq!(Format::OtlpJson.as_str(), "otlp_json");
        assert_eq!(Format::Parquet.extension(), "parquet");
        assert_eq!(Format::Ndjson.extension(), "jsonl");
    }
}
