//! The Brain-to-executor tool invocation.
//!
//! One request type, one response type, and the request names **no tenant**.
//!
//! # Why there is no principal, organization or workspace field
//!
//! That absence is the mechanism, not an omission to be tidied later. The
//! executor spends the platform's own money on a customer's behalf, so "whose
//! call is this" is the only question it has to get right — and a tenant id in a
//! request body is a tenant id a caller can lie about. The executor therefore
//! resolves the tenant from the signed envelope and from nowhere else, exactly
//! as the run-hook payload the guest receives carries no credential slot because
//! there is nowhere to put one.
//!
//! `tests/no_tenant_in_tool_exec.rs` holds the type to it: it pins the exact
//! serialized key set and proves that every tenant-shaped spelling is refused by
//! the decoder rather than merely unread by the handler.
//!
//! # Who may send one
//!
//! `brain-mux`, and nothing else. The executor has no public listener; the
//! envelope is signed by a key only `brain-mux` holds, addressed to
//! [`crate::assertion::AssertionAudience::ToolExec`], and carries
//! `PrincipalKind::AgentSession`, which `central-authz` cannot mint. Three
//! independent gates, none of which depends on the network one holding.

use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde::{Deserializer, Serializer};

use aex_wire::ids::{ContentHash, ResourceName};
use base64::Engine as _;

use crate::SchemaVersion;
use crate::assertion::IssuedAssertion;

/// The most canonical argument bytes one call may carry.
///
/// Derived, not chosen: `ToolBounds::max_frame_bytes` is a constant 65 536 for
/// every one of the catalogue's tools, so this is the largest argument document
/// the tool contract itself admits on any transport. The per-tool
/// `max_input_bytes` is tighter — 8 448 for `web_fetch`, 4 096 for `web_search`
/// — and the executor still applies it from the manifest. This bound exists one
/// layer earlier, to stop a hostile body making a decoder allocate before
/// anything has been verified.
pub const MAX_ARGUMENTS_JCS_BYTES: usize = 65_536;

/// The longest deadline one call may declare.
///
/// Two independent derivations agree on this number, which is why it is not a
/// guess. It is the largest `timeout_ms` any `EgressClass::ManagedInternet` tool
/// declares (`web_fetch`, 30 000 ms), so no reachable tool needs more. And it is
/// `aex_identity_domain::assertion::ASSERTION_MAX_LIFETIME_MS`, so a call can
/// never outlive the envelope that authorised it — a longer deadline would mean
/// a vendor request still in flight under an authorisation that has expired.
pub const MAX_TOOL_EXEC_DEADLINE_MS: u32 = 30_000;

/// The deterministic identity of the effect a call belongs to.
///
/// The Brain's `EffectId` is `blake3(agent ‖ seq ‖ kind)[..16]`, so it already
/// names one agent's one step and a redelivered wake derives the same value.
/// Carried here as raw bytes rather than as that type because this crate must
/// not depend on the Brain's domain, and spelled in lowercase hexadecimal on the
/// wire because that is how the Brain already writes it into a sort key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectRef([u8; 16]);

impl EffectRef {
    /// Wraps the sixteen bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    #[must_use]
    pub const fn get(&self) -> &[u8; 16] {
        &self.0
    }

    /// Lowercase hexadecimal, the one spelling.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(32);
        for byte in self.0 {
            out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
            out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
        }
        out
    }
}

impl std::fmt::Debug for EffectRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "EffectRef({})", self.to_hex())
    }
}

impl Serialize for EffectRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for EffectRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.len() != 32
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(D::Error::custom(
                "an effect reference is 32 lowercase hexadecimal characters",
            ));
        }
        let mut bytes = [0_u8; 16];
        for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
            let high = char::from(pair[0])
                .to_digit(16)
                .ok_or_else(|| D::Error::custom("an effect reference is lowercase hexadecimal"))?;
            let low = char::from(pair[1])
                .to_digit(16)
                .ok_or_else(|| D::Error::custom("an effect reference is lowercase hexadecimal"))?;
            bytes[index] = u8::try_from(high * 16 + low).unwrap_or_default();
        }
        Ok(Self(bytes))
    }
}

/// The canonical, already schema-validated argument document.
///
/// Base64url on the wire rather than nested JSON so the bytes the Brain
/// validated against the manifest schema are, byte for byte, the bytes the
/// executor hands to the tool. Re-encoding through a JSON value would let a
/// serializer's key order or number formatting change the document between the
/// validation and the use.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ArgumentsJcs(Vec<u8>);

impl ArgumentsJcs {
    /// Wraps canonical bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecError::ArgumentsTooLarge`] past
    /// [`MAX_ARGUMENTS_JCS_BYTES`] and for an empty document: canonical JSON is
    /// never zero bytes, so an empty one is a caller that dropped the field.
    pub fn new(bytes: Vec<u8>) -> Result<Self, ToolExecError> {
        if bytes.is_empty() || bytes.len() > MAX_ARGUMENTS_JCS_BYTES {
            return Err(ToolExecError::ArgumentsTooLarge);
        }
        Ok(Self(bytes))
    }

    /// The canonical bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes the document is.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the document is empty, which construction already forbids.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Deliberately opaque: arguments are model-directed content and a debug line is
/// one of the places it would otherwise be copied to.
impl std::fmt::Debug for ArgumentsJcs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ArgumentsJcs(<{} bytes>)", self.0.len())
    }
}

impl Serialize for ArgumentsJcs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for ArgumentsJcs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        // Bound the encoded form before decoding it: four base64 characters are
        // three bytes, so this refuses an oversized document without allocating
        // it first.
        if text.len() > MAX_ARGUMENTS_JCS_BYTES.div_ceil(3) * 4 {
            return Err(D::Error::custom(
                "the canonical argument document exceeds the transport bound",
            ));
        }
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let bytes = engine
            .decode(&text)
            .map_err(|_| D::Error::custom("expected canonical unpadded base64url"))?;
        if engine.encode(&bytes) != text {
            return Err(D::Error::custom("expected canonical unpadded base64url"));
        }
        Self::new(bytes).map_err(D::Error::custom)
    }
}

/// Why a tool-exec value was refused at construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ToolExecError {
    /// The argument document was empty or past [`MAX_ARGUMENTS_JCS_BYTES`].
    #[error("a canonical argument document is 1..={MAX_ARGUMENTS_JCS_BYTES} bytes")]
    ArgumentsTooLarge,
    /// The declared deadline was zero or past [`MAX_TOOL_EXEC_DEADLINE_MS`].
    #[error("a tool-exec deadline is 1..={MAX_TOOL_EXEC_DEADLINE_MS} ms")]
    DeadlineOutOfRange,
}

/// A bounded wall-clock deadline for one invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeadlineMs(u32);

impl DeadlineMs {
    /// Wraps a deadline.
    ///
    /// # Errors
    ///
    /// Returns [`ToolExecError::DeadlineOutOfRange`] outside
    /// `1..=MAX_TOOL_EXEC_DEADLINE_MS`. A zero deadline is refused rather than
    /// treated as "no deadline": the one thing a bound must never mean is its
    /// own absence.
    pub const fn new(value: u32) -> Result<Self, ToolExecError> {
        if value == 0 || value > MAX_TOOL_EXEC_DEADLINE_MS {
            return Err(ToolExecError::DeadlineOutOfRange);
        }
        Ok(Self(value))
    }

    /// The deadline in milliseconds.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Serialize for DeadlineMs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(self.0)
    }
}

impl<'de> Deserialize<'de> for DeadlineMs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(u32::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// One tool invocation the Brain asks the executor to perform.
///
/// **This type names no tenant.** See the module documentation; the absence is
/// load-bearing and is pinned by a test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ToolExecRequest {
    /// Which envelope version this is.
    pub schema_version: SchemaVersion,
    /// The signed authorization, audience `tool_exec`.
    ///
    /// The only field that says anything about who this call is for, and the one
    /// field the caller cannot author.
    pub assertion: IssuedAssertion,
    /// Which tool to run.
    pub tool: ResourceName,
    /// The manifest revision the arguments were validated against.
    ///
    /// Layer three of the framework's versioning: the executor refuses a
    /// manifest it did not itself verify, so the two ends cannot disagree about
    /// what the tool's schema and bounds are.
    pub manifest: ContentHash,
    /// The canonical argument document.
    pub arguments_jcs: ArgumentsJcs,
    /// The effect this call settles.
    pub effect: EffectRef,
    /// Which attempt of that effect this is, from zero.
    pub attempt: u16,
    /// The wall-clock ceiling for this invocation.
    pub deadline_ms: DeadlineMs,
}

/// One part of a tool result.
///
/// A flat vocabulary rather than the model catalogue's recursive one: a result
/// crossing this boundary is bytes and text, and nothing here needs to nest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolResultPart {
    /// A UTF-8 fragment.
    Text {
        /// The fragment.
        text: String,
    },
}

/// Why the executor did not run a call.
///
/// A refusal is an **answer**: the exchange succeeded and the executor decided.
/// An internal fault is an invocation error instead, because a caller must not
/// report "you are over your ceiling" when the truth is "we could not tell".
///
/// The vocabulary is the outcomes a caller can act on differently, and no more.
/// A ceiling refusal deliberately carries no remaining budget, so a hostile
/// customer driving the model cannot binary-search it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecRefusal {
    /// The envelope did not verify, was addressed elsewhere, had lapsed, or
    /// named an account that is not active. One arm on purpose: distinguishing
    /// them would tell an unauthenticated caller which it was.
    NotAuthorized,
    /// An organization ceiling was reached. Maps to the existing `limit_exceeded`
    /// quota code rather than a new one.
    LimitExceeded,
    /// This executor does not run this tool, or does not hold this manifest.
    Unsupported,
}

/// What the executor answers a request with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolExecResponse {
    /// The tool ran. `isError` is the tool's own verdict on its work, which is a
    /// result for the model to read rather than a dispatch failure.
    Completed {
        /// Which envelope version this is.
        schema_version: SchemaVersion,
        /// The result content.
        content: Vec<ToolResultPart>,
        /// Whether the tool reported failure.
        is_error: bool,
        /// How long the invocation took.
        duration_ms: u32,
        /// A digest over the canonical result content.
        checksum: ContentHash,
    },
    /// The executor decided not to run it.
    Refused {
        /// Which envelope version this is.
        schema_version: SchemaVersion,
        /// Which decision it made.
        reason: ToolExecRefusal,
    },
}
