//! Pending `aex-wire` vocabulary.
//!
//! The contracts stream owns `aex-wire` and is implementing it concurrently, so
//! the generated public vocabulary this crate consumes does not exist on `main`
//! yet. Every item below is the shape plan 08 §9 records as *consumed*, defined
//! here so the catalog and the gateway can be written and tested against it.
//!
//! `TODO(cross-stream): replaced by aex_wire::{ProviderId, ModelSlug,
//! ModelSelection, ProviderCredentialId, WorkspaceId, ToolCallId, ToolName,
//! ContentHash, ResourceName, ErrorCode, CatalogRevision, to_jcs_bytes} at
//! merge.`
//!
//! Nothing here re-specifies a wire rule. The canonicalizer is RFC 8785 JCS with
//! the workspace amendment recorded in `00-orchestrator-conventions.md` §8a
//! ("RFC 8785 JCS over UTF-8 byte ordering"): object members are ordered by the
//! UTF-8 bytes of their names rather than by UTF-16 code units. The two orders
//! agree for every name in this codebase, which is all ASCII.

use core::fmt;
use core::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// bounded string
// ---------------------------------------------------------------------------

/// A `String` that cannot exceed `N` UTF-8 bytes.
///
/// Every text field that crosses a boundary in this stream is bounded, so a
/// hostile or merely broken provider cannot make a buffer grow without limit.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedString<const N: usize>(String);

/// The single failure a [`BoundedString`] construction can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("string of {seen} bytes exceeds the {limit}-byte bound")]
pub struct BoundError {
    /// The declared maximum, in UTF-8 bytes.
    pub limit: usize,
    /// The length of the rejected value, in UTF-8 bytes.
    pub seen: usize,
}

impl<const N: usize> BoundedString<N> {
    /// Builds a bounded string, rejecting anything longer than `N` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BoundError`] when the value exceeds `N` UTF-8 bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, BoundError> {
        let value = value.into();
        if value.len() > N {
            return Err(BoundError { limit: N, seen: value.len() });
        }
        Ok(Self(value))
    }

    /// Builds a bounded string by truncating on a UTF-8 character boundary.
    ///
    /// This is used only by the redactor, where the alternative to a bounded
    /// prefix is dropping a diagnostic entirely.
    #[must_use]
    pub fn truncating(value: &str) -> Self {
        if value.len() <= N {
            return Self(value.to_owned());
        }
        let mut end = N;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        Self(value[..end].to_owned())
    }

    /// The borrowed contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in UTF-8 bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the value is the empty string.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The declared bound.
    #[must_use]
    pub const fn bound() -> usize {
        N
    }
}

impl<const N: usize> fmt::Debug for BoundedString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<const N: usize> fmt::Display for BoundedString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<const N: usize> Serialize for BoundedString<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const N: usize> Deserialize<'de> for BoundedString<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// provider identity
// ---------------------------------------------------------------------------

/// The six launch providers. Closed: there is no `Other` arm and no alias table.
///
/// `anthropic` is canonical. `anthrophic` is a misspelling that has appeared in
/// older AEX source and is deliberately **not** accepted here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    /// OpenAI, Responses dialect.
    #[serde(rename = "openai")]
    OpenAi,
    /// Anthropic, Messages dialect.
    #[serde(rename = "anthropic")]
    Anthropic,
    /// DeepSeek, chat-completions dialect.
    #[serde(rename = "deepseek")]
    DeepSeek,
    /// Z.AI general API, chat-completions dialect.
    #[serde(rename = "zai")]
    Zai,
    /// Moonshot AI (Kimi) international, chat-completions dialect.
    #[serde(rename = "moonshotai")]
    MoonshotAi,
    /// Google Gemini Developer API, `generateContent` dialect.
    #[serde(rename = "google")]
    Google,
}

impl ProviderId {
    /// Every provider, in wire order. Used by exhaustiveness tests and the
    /// router totality property.
    pub const ALL: [Self; 6] = [
        Self::OpenAi,
        Self::Anthropic,
        Self::DeepSeek,
        Self::Zai,
        Self::MoonshotAi,
        Self::Google,
    ];

    /// The exact wire token.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::DeepSeek => "deepseek",
            Self::Zai => "zai",
            Self::MoonshotAi => "moonshotai",
            Self::Google => "google",
        }
    }
}

/// An unrecognised provider token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{token}` is not one of the six launch providers")]
pub struct UnknownProviderToken {
    /// The rejected token, bounded so an attacker cannot grow an error string.
    pub token: BoundedString<64>,
}

impl FromStr for ProviderId {
    type Err = UnknownProviderToken;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "openai" => Ok(Self::OpenAi),
            "anthropic" => Ok(Self::Anthropic),
            "deepseek" => Ok(Self::DeepSeek),
            "zai" => Ok(Self::Zai),
            "moonshotai" => Ok(Self::MoonshotAi),
            "google" => Ok(Self::Google),
            other => Err(UnknownProviderToken { token: BoundedString::truncating(other) }),
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_wire())
    }
}

/// A provider-native model identifier, verbatim. AEX never infers or rewrites
/// one: a model id is catalog data, and an unknown id is a typed rejection.
pub type ModelSlug = BoundedString<256>;

/// The explicit `(provider, model)` pair plus an optional stable credential id,
/// exactly as A11 item 4 puts it on the public wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSelection {
    /// The provider half of the pair. Never inferred from the model name.
    pub provider: ProviderId,
    /// The provider-native model id.
    pub model: ModelSlug,
    /// An explicit provider-credential binding, or `None` for the workspace
    /// default for that provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<ProviderCredentialId>,
}

/// A stable `pcr_`-prefixed provider-credential binding id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderCredentialId(BoundedString<72>);

impl ProviderCredentialId {
    /// Builds a binding id, requiring the `pcr_` prefix.
    ///
    /// # Errors
    ///
    /// Returns [`IdFormatError`] for a missing prefix, an empty body or an
    /// over-long value.
    pub fn new(value: impl Into<String>) -> Result<Self, IdFormatError> {
        let value = value.into();
        if !value.starts_with("pcr_") || value.len() <= 4 {
            return Err(IdFormatError { expected_prefix: "pcr_" });
        }
        BoundedString::new(value)
            .map(Self)
            .map_err(|_| IdFormatError { expected_prefix: "pcr_" })
    }

    /// The borrowed contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for ProviderCredentialId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// A prefixed-identifier format violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("identifier must carry the `{expected_prefix}` prefix and a non-empty body")]
pub struct IdFormatError {
    /// The prefix the identifier kind requires.
    pub expected_prefix: &'static str,
}

/// A workspace identifier. Opaque here; the contracts stream owns its grammar.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub BoundedString<64>);

/// A provider-assigned tool-call identifier, carried verbatim.
pub type ToolCallId = BoundedString<128>;

/// A tool name as declared to the provider.
pub type ToolName = BoundedString<128>;

/// A schema or resource name, used for structured-output schema naming.
pub type ResourceName = BoundedString<128>;

/// A provider-assigned request identifier, where the provider publishes one.
pub type ProviderRequestId = BoundedString<80>;

// ---------------------------------------------------------------------------
// hashes and revisions
// ---------------------------------------------------------------------------

/// A blake3-256 content hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// Hashes the given bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Lowercase hex rendering.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// Parses a lowercase 64-character hex rendering.
    ///
    /// # Errors
    ///
    /// Returns [`HexError`] for a wrong length or a non-hex character.
    pub fn from_hex(value: &str) -> Result<Self, HexError> {
        let mut out = [0u8; 32];
        hex::decode_to_slice(value, &mut out).map_err(|_| HexError)?;
        Ok(Self(out))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.to_hex())
    }
}

/// A malformed hex rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("value is not a lowercase hex rendering of the expected length")]
pub struct HexError;

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_hex(&raw).map_err(de::Error::custom)
    }
}

/// The opaque public rendering of a catalog release: `mc1_` plus the lowercase
/// hex of the document's blake3-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CatalogRevision(pub ContentHash);

impl CatalogRevision {
    /// The `mc1_<hex>` rendering.
    #[must_use]
    pub fn to_wire(self) -> String {
        format!("mc1_{}", self.0.to_hex())
    }

    /// Parses the `mc1_<hex>` rendering.
    ///
    /// # Errors
    ///
    /// Returns [`HexError`] for a missing prefix or a malformed digest.
    pub fn from_wire(value: &str) -> Result<Self, HexError> {
        let rest = value.strip_prefix("mc1_").ok_or(HexError)?;
        ContentHash::from_hex(rest).map(Self)
    }
}

impl fmt::Debug for CatalogRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CatalogRevision({})", self.to_wire())
    }
}

impl Serialize for CatalogRevision {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for CatalogRevision {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_wire(&raw).map_err(de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// time
// ---------------------------------------------------------------------------

/// Epoch milliseconds. The catalog is a pure crate: it never reads a clock, it
/// only compares the `now` its caller supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Epoch milliseconds as an integer.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.0
    }

    /// Adds a whole number of milliseconds, saturating rather than wrapping.
    #[must_use]
    pub const fn saturating_add_millis(self, millis: i64) -> Self {
        Self(self.0.saturating_add(millis))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// public error vocabulary
// ---------------------------------------------------------------------------

/// The public wire error codes this stream can produce. Closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// The `provider` token is not one of the six.
    #[serde(rename = "unknown_provider")]
    UnknownProvider,
    /// The provider is known but the model is absent from the pinned catalog.
    #[serde(rename = "unknown_model")]
    UnknownModel,
    /// The pair exists but is not admissible: staged, deprecated, emergency
    /// disabled, receipt expired or catalog expired.
    #[serde(rename = "unqualified_provider_model")]
    UnqualifiedProviderModel,
    /// The pinned catalog revision is past `retired_at`.
    #[serde(rename = "catalog_retired")]
    CatalogRetired,
    /// No provider-credential binding resolves for the requested pair.
    #[serde(rename = "provider_credential_not_found")]
    ProviderCredentialNotFound,
    /// The pinned provider-credential binding has been revoked.
    #[serde(rename = "provider_credential_revoked")]
    ProviderCredentialRevoked,
    /// More than one default binding exists for the provider: a data invariant
    /// violation the caller must not paper over by picking one.
    #[serde(rename = "provider_credential_ambiguous")]
    ProviderCredentialAmbiguous,
}

// ---------------------------------------------------------------------------
// canonical JSON (RFC 8785 JCS, UTF-8 byte ordering)
// ---------------------------------------------------------------------------

/// A JSON value that is only ever handled in canonical form.
///
/// Construction validates that the value is canonicalizable; serialization goes
/// through [`to_jcs_bytes`], so two structurally equal values always produce
/// byte-identical output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalJson(serde_json::Value);

impl CanonicalJson {
    /// Wraps a value after proving it canonicalizable.
    ///
    /// # Errors
    ///
    /// Returns [`JcsError`] when the value contains a number JCS cannot render
    /// deterministically.
    pub fn new(value: serde_json::Value) -> Result<Self, JcsError> {
        let mut probe = Vec::new();
        write_canonical(&value, &mut probe)?;
        Ok(Self(value))
    }

    /// Parses canonical JSON from bytes and proves the bytes were already
    /// canonical.
    ///
    /// # Errors
    ///
    /// Returns [`JcsError::NotCanonical`] when re-serializing the parsed value
    /// does not reproduce the input bytes exactly.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, JcsError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| JcsError::NotJson)?;
        let mut round = Vec::new();
        write_canonical(&value, &mut round)?;
        if round != bytes {
            return Err(JcsError::NotCanonical);
        }
        Ok(Self(value))
    }

    /// The wrapped value.
    #[must_use]
    pub fn value(&self) -> &serde_json::Value {
        &self.0
    }

    /// Consumes the wrapper and returns the value.
    #[must_use]
    pub fn into_value(self) -> serde_json::Value {
        self.0
    }

    /// The canonical bytes.
    ///
    /// # Panics
    ///
    /// Never: construction already proved the value canonicalizable.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_canonical(&self.0, &mut out)
            .expect("CanonicalJson proved canonicalizable at construction");
        out
    }

    /// The blake3-256 hash of the canonical bytes.
    #[must_use]
    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of(&self.to_bytes())
    }

    /// An empty JSON object, the canonical "no arguments" value.
    #[must_use]
    pub fn empty_object() -> Self {
        Self(serde_json::Value::Object(serde_json::Map::new()))
    }
}

impl Serialize for CanonicalJson {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CanonicalJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
}

/// Why a value cannot be rendered as canonical JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum JcsError {
    /// The bytes are not JSON at all.
    #[error("value is not JSON")]
    NotJson,
    /// The bytes parse but are not the canonical rendering of what they mean.
    #[error("value is JSON but not its canonical rendering")]
    NotCanonical,
    /// A non-finite float, which JSON cannot represent.
    #[error("a non-finite number cannot be canonicalized")]
    NonFiniteNumber,
    /// A magnitude where ECMAScript switches to exponential notation. Rejected
    /// rather than guessed, so no second canonicalizer can appear later.
    #[error("number magnitude falls outside the canonical plain-decimal range")]
    NumberOutOfRange,
}

/// Canonicalizes any serializable value to RFC 8785 bytes.
///
/// # Errors
///
/// Returns [`JcsError`] when the value cannot be serialized to JSON or contains
/// a number outside the canonical plain-decimal range.
pub fn to_jcs_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, JcsError> {
    let json = serde_json::to_value(value).map_err(|_| JcsError::NotJson)?;
    let mut out = Vec::new();
    write_canonical(&json, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &serde_json::Value, out: &mut Vec<u8>) -> Result<(), JcsError> {
    match value {
        serde_json::Value::Null => out.extend_from_slice(b"null"),
        serde_json::Value::Bool(true) => out.extend_from_slice(b"true"),
        serde_json::Value::Bool(false) => out.extend_from_slice(b"false"),
        serde_json::Value::Number(n) => write_number(n, out)?,
        serde_json::Value::String(s) => write_string(s, out),
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            out.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_string(key, out);
                out.push(b':');
                let member = map.get(key).ok_or(JcsError::NotJson)?;
                write_canonical(member, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_number(n: &serde_json::Number, out: &mut Vec<u8>) -> Result<(), JcsError> {
    if let Some(u) = n.as_u64() {
        out.extend_from_slice(u.to_string().as_bytes());
        return Ok(());
    }
    if let Some(i) = n.as_i64() {
        out.extend_from_slice(i.to_string().as_bytes());
        return Ok(());
    }
    let f = n.as_f64().ok_or(JcsError::NonFiniteNumber)?;
    if !f.is_finite() {
        return Err(JcsError::NonFiniteNumber);
    }
    if f == 0.0 {
        out.extend_from_slice(b"0");
        return Ok(());
    }
    let magnitude = f.abs();
    // ECMAScript `Number::toString` uses plain decimal exactly on this range;
    // outside it the rendering is exponential and is rejected rather than
    // approximated.
    if !(1e-6..1e21).contains(&magnitude) {
        return Err(JcsError::NumberOutOfRange);
    }
    out.extend_from_slice(format!("{f}").as_bytes());
    Ok(())
}

fn write_string(value: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for ch in value.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

// ---------------------------------------------------------------------------
// opaque byte payloads
// ---------------------------------------------------------------------------

/// Base64 (standard alphabet, padded) serde for opaque provider round-trip
/// material. A byte array rendered as a JSON number sequence would triple the
/// journal size and lose its opacity.
pub mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes bytes as a padded standard-alphabet base64 string.
    ///
    /// # Errors
    ///
    /// Propagates the serializer's own failure only.
    pub fn serialize<S: Serializer>(value: &bytes::Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(value.as_ref()))
    }

    /// Deserializes a padded standard-alphabet base64 string.
    ///
    /// # Errors
    ///
    /// Returns a deserializer error for a malformed encoding.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<bytes::Bytes, D::Error> {
        let raw = String::deserialize(d)?;
        let decoded = STANDARD.decode(raw.as_bytes()).map_err(serde::de::Error::custom)?;
        Ok(bytes::Bytes::from(decoded))
    }
}

/// A JSON pointer into a catalog document, used to name the exact offending
/// location in a load error.
pub type JsonPointer = BoundedString<256>;
