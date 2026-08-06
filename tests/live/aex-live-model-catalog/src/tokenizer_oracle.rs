//! Pinned offline token-count oracle for the `DeepSeek` qualification corpus.
//!
//! Upstream tokenizer bytes are intentionally not vendored: the published ZIP
//! contains no license. A protected caller supplies `tokenizer.json`; this
//! module accepts only the exact reviewed SHA-256 identity before parsing it.
//! The reduced single-user chat-template projection is pinned separately to the
//! exact template string published beside that tokenizer.

use core::fmt;

use aex_model_catalog::canonical::{CanonicalBlock, CanonicalMessage, Role, TEXT_MAX};
use aex_model_catalog::primitives::{BoundError, BoundedString};
use aex_wire::canonical::CanonicalError;
use aex_wire::{ContentHash, to_jcs_bytes};
use tokenizers::Tokenizer;

/// Official archive from the `DeepSeek` token-usage documentation.
pub const OFFICIAL_TOKENIZER_ARCHIVE_URL: &str =
    "https://cdn.deepseek.com/api-docs/deepseek_v3_tokenizer.zip";
/// SHA-256 of the exact reviewed official ZIP. The archive is not vendored.
pub const PINNED_TOKENIZER_ARCHIVE_SHA256: &str =
    "c954ca6f6e54281d72d3c27e2430cea7663f81292b39982e2f97890c66c302de";
/// SHA-256 of `deepseek_v3_tokenizer/tokenizer.json` inside the reviewed ZIP.
pub const PINNED_TOKENIZER_JSON_SHA256: &str =
    "ecb6f9fc369894346f0511f4074ca75cee5cd5f3b06d02f1ba35fcd39f8e121d";
/// SHA-256 of the exact `chat_template` string in the reviewed companion config.
pub const PINNED_CHAT_TEMPLATE_SHA256: &str =
    "3a3036eeb96ca0e48565fc1f19b4d7d3fa17ada32bb1b3d5ed539ae74cf22923";

/// Typed identity of the only tokenizer JSON this oracle admits.
pub const PINNED_TOKENIZER_DIGEST: ContentHash = ContentHash::from_bytes([
    0xec, 0xb6, 0xf9, 0xfc, 0x36, 0x98, 0x94, 0x34, 0x6f, 0x05, 0x11, 0xf4, 0x07, 0x4c, 0xa7, 0x5c,
    0xee, 0x5c, 0xd5, 0xf3, 0xb0, 0x6d, 0x02, 0xf1, 0xba, 0x35, 0xfc, 0xd3, 0x9f, 0x8e, 0x12, 0x1d,
]);
/// Typed identity of the reviewed full chat template.
pub const PINNED_CHAT_TEMPLATE_DIGEST: ContentHash = ContentHash::from_bytes([
    0x3a, 0x30, 0x36, 0xee, 0xb9, 0x6c, 0xa0, 0xe4, 0x85, 0x65, 0xfc, 0x1f, 0x19, 0xb4, 0xd7, 0xd3,
    0xfa, 0x17, 0xad, 0xa3, 0x2b, 0xb1, 0xb3, 0xd5, 0xed, 0x53, 0x9a, 0xe7, 0x4c, 0xf2, 0x29, 0x23,
]);
/// Deterministic canonical-message digest of the one-million-window 80% corpus.
pub const EIGHTY_PERCENT_CORPUS_DIGEST: ContentHash = ContentHash::from_bytes([
    0x58, 0x67, 0x3c, 0xd9, 0xa9, 0xd3, 0xba, 0x9f, 0x8c, 0x98, 0x65, 0xc5, 0xc5, 0x67, 0xb2, 0xaf,
    0x42, 0xe5, 0x6c, 0xde, 0x62, 0x83, 0x22, 0xb5, 0x45, 0xc0, 0x96, 0x83, 0x99, 0x89, 0xe2, 0xbe,
]);
/// Deterministic canonical-message digest of the one-million-window plus-one corpus.
pub const WINDOW_PLUS_ONE_CORPUS_DIGEST: ContentHash = ContentHash::from_bytes([
    0xd8, 0x3a, 0xc3, 0xa8, 0x18, 0x58, 0xd0, 0xc0, 0x0f, 0x0b, 0x22, 0x9c, 0x26, 0x8c, 0x6a, 0x9b,
    0x6d, 0x60, 0x69, 0xb9, 0xfb, 0x9d, 0x89, 0x49, 0x56, 0x63, 0x10, 0x9a, 0x71, 0x92, 0x9d, 0x1c,
]);

const BEGIN_OF_SENTENCE: &str = "<｜begin▁of▁sentence｜>";
const USER: &str = "<｜User｜>";
const ASSISTANT: &str = "<｜Assistant｜>";
const BEGIN_OF_SENTENCE_ID: u32 = 0;
const USER_ID: u32 = 128_803;
const ASSISTANT_ID: u32 = 128_804;
const SINGLE_USER_TEMPLATE_OVERHEAD_TOKENS: usize = 3;
const CANDIDATE_SCREEN_BYTES: usize = 4_096;
/// Printable two-byte pattern verified to remain one token per byte under the pin.
const PRINTABLE_CORPUS_PATTERN: &str = " 0";

/// Parsed, pinned tokenizer which performs no network or filesystem access.
pub struct DeepSeekTokenizerOracle {
    tokenizer: Tokenizer,
    tokenizer_digest: ContentHash,
}

impl fmt::Debug for DeepSeekTokenizerOracle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeepSeekTokenizerOracle")
            .field("tokenizer_digest", &self.tokenizer_digest)
            .finish_non_exhaustive()
    }
}

/// Exact long-context corpus and the identities needed to reproduce its count.
#[derive(Clone, PartialEq, Eq)]
pub struct CorpusCase {
    /// Exactly one user turn carrying one bounded text block.
    pub message: CanonicalMessage,
    /// SHA-256 of the canonical JCS rendering of [`Self::message`].
    pub corpus_digest: ContentHash,
    /// Required count after applying the pinned single-user chat-template projection.
    pub target_chat_tokens: u32,
    /// Count returned by the pinned tokenizer for the final rendered chat.
    pub exact_chat_tokens: u32,
    /// UTF-8 byte length of the text block, always within [`TEXT_MAX`].
    pub text_bytes: u32,
}

impl fmt::Debug for CorpusCase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CorpusCase")
            .field("corpus_digest", &self.corpus_digest)
            .field("target_chat_tokens", &self.target_chat_tokens)
            .field("exact_chat_tokens", &self.exact_chat_tokens)
            .field("text_bytes", &self.text_bytes)
            .finish_non_exhaustive()
    }
}

/// The exact P-16 and P-17 corpus pair for one declared context window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekCorpusPair {
    /// Reviewed tokenizer identity used for both counts.
    pub tokenizer_digest: ContentHash,
    /// Reviewed full chat-template identity whose single-user projection is used.
    pub chat_template_digest: ContentHash,
    /// Declared model context window used to derive both targets.
    pub context_window_tokens: u32,
    /// Exact floor of 80 percent of the declared context window.
    pub eighty_percent: CorpusCase,
    /// Exactly one token beyond the declared context window.
    pub window_plus_one: CorpusCase,
}

/// Why the offline oracle refused to claim an exact corpus count.
#[derive(Debug, thiserror::Error)]
pub enum TokenizerOracleError {
    /// Caller bytes are not the reviewed official tokenizer JSON.
    #[error("tokenizer digest mismatch: expected {expected:?}, got {actual:?}")]
    DigestMismatch {
        /// Required reviewed digest.
        expected: ContentHash,
        /// Digest of the supplied bytes.
        actual: ContentHash,
    },
    /// Reviewed bytes could not be decoded by the pinned tokenizer engine.
    #[error("the pinned tokenizer JSON could not be decoded")]
    InvalidTokenizer,
    /// A pinned token is missing or no longer has its reviewed id.
    #[error("the pinned tokenizer token `{token}` did not have id {expected_id}")]
    TokenIdentity {
        /// Required chat-template token.
        token: &'static str,
        /// Reviewed token id.
        expected_id: u32,
    },
    /// Empty single-user chat rendering did not produce the reviewed three ids.
    #[error("the pinned tokenizer does not implement the reviewed single-user template projection")]
    TemplateProjection,
    /// The tokenizer engine failed while encoding an in-memory corpus.
    #[error("the pinned tokenizer failed to encode the offline corpus")]
    Encode,
    /// Context arithmetic overflowed or produced no content-bearing target.
    #[error("the declared context window cannot produce both qualification targets")]
    InvalidContextWindow,
    /// Exact target would require a text block outside the canonical byte bound.
    #[error("target {target_tokens} tokens cannot fit within the {max_text_bytes}-byte text bound")]
    TargetExceedsTextBound {
        /// Exact chat-template target.
        target_tokens: u32,
        /// Canonical text-block bound.
        max_text_bytes: usize,
    },
    /// No deterministic bounded corpus candidate encoded to both exact targets.
    #[error(
        "the pinned tokenizer could not construct exact {eighty_percent}/{window_plus_one}-token corpora within the canonical text bound"
    )]
    ExactCorpusUnavailable {
        /// 80-percent target.
        eighty_percent: u32,
        /// Context-window-plus-one target.
        window_plus_one: u32,
    },
    /// A verified candidate unexpectedly failed the canonical text newtype.
    #[error(transparent)]
    TextBound(#[from] BoundError),
    /// A token or byte count could not be represented by the output schema.
    #[error("an exact tokenizer count is outside the output schema range")]
    CountOutOfRange,
    /// Canonical corpus hashing failed.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

impl DeepSeekTokenizerOracle {
    /// Loads exact caller-supplied `tokenizer.json` bytes after verifying the pin.
    ///
    /// No URL, path, cache or environment variable is consulted.
    ///
    /// # Errors
    ///
    /// Returns [`TokenizerOracleError`] for a digest mismatch, malformed
    /// tokenizer, or drift in the three tokens used by the pinned chat-template
    /// projection.
    pub fn from_pinned_json(tokenizer_json: &[u8]) -> Result<Self, TokenizerOracleError> {
        let actual = ContentHash::of(tokenizer_json);
        if actual != PINNED_TOKENIZER_DIGEST {
            return Err(TokenizerOracleError::DigestMismatch {
                expected: PINNED_TOKENIZER_DIGEST,
                actual,
            });
        }
        let tokenizer = Tokenizer::from_bytes(tokenizer_json)
            .map_err(|_| TokenizerOracleError::InvalidTokenizer)?;
        validate_tokenizer(&tokenizer)?;
        Ok(Self {
            tokenizer,
            tokenizer_digest: actual,
        })
    }

    /// Constructs and verifies the exact P-16 and P-17 single-user corpora.
    ///
    /// This is intentionally constructive rather than estimator-based: every
    /// returned message is rendered, encoded, compared to its exact target and
    /// only then admitted into [`CanonicalBlock::Text`].
    ///
    /// # Errors
    ///
    /// Returns [`TokenizerOracleError`] if arithmetic or text bounds fail, no
    /// deterministic corpus reaches both exact targets, encoding fails, or the
    /// final canonical message cannot be hashed.
    pub fn build_corpora(
        &self,
        context_window_tokens: u32,
    ) -> Result<DeepSeekCorpusPair, TokenizerOracleError> {
        build_with_counter(
            &TokenizerCounter(&self.tokenizer),
            self.tokenizer_digest,
            context_window_tokens,
        )
    }

    /// The exact tokenizer identity verified at construction.
    #[must_use]
    pub const fn tokenizer_digest(&self) -> ContentHash {
        self.tokenizer_digest
    }
}

trait ChatTokenCounter {
    fn count(&self, rendered_chat: &str) -> Result<usize, TokenizerOracleError>;
}

struct TokenizerCounter<'a>(&'a Tokenizer);

impl ChatTokenCounter for TokenizerCounter<'_> {
    fn count(&self, rendered_chat: &str) -> Result<usize, TokenizerOracleError> {
        self.0
            .encode(rendered_chat, false)
            .map(|encoding| encoding.get_ids().len())
            .map_err(|_| TokenizerOracleError::Encode)
    }
}

fn validate_tokenizer(tokenizer: &Tokenizer) -> Result<(), TokenizerOracleError> {
    for (token, expected_id) in [
        (BEGIN_OF_SENTENCE, BEGIN_OF_SENTENCE_ID),
        (USER, USER_ID),
        (ASSISTANT, ASSISTANT_ID),
    ] {
        if tokenizer.token_to_id(token) != Some(expected_id) {
            return Err(TokenizerOracleError::TokenIdentity { token, expected_id });
        }
    }
    let empty = tokenizer
        .encode(render_single_user_chat(""), false)
        .map_err(|_| TokenizerOracleError::Encode)?;
    if empty.get_ids() != [BEGIN_OF_SENTENCE_ID, USER_ID, ASSISTANT_ID] {
        return Err(TokenizerOracleError::TemplateProjection);
    }
    Ok(())
}

fn build_with_counter(
    counter: &impl ChatTokenCounter,
    tokenizer_digest: ContentHash,
    context_window_tokens: u32,
) -> Result<DeepSeekCorpusPair, TokenizerOracleError> {
    let eighty_percent_u64 = u64::from(context_window_tokens)
        .checked_mul(4)
        .ok_or(TokenizerOracleError::InvalidContextWindow)?
        / 5;
    let eighty_percent = u32::try_from(eighty_percent_u64)
        .map_err(|_| TokenizerOracleError::InvalidContextWindow)?;
    let window_plus_one = context_window_tokens
        .checked_add(1)
        .ok_or(TokenizerOracleError::InvalidContextWindow)?;
    let base = counter.count(&render_single_user_chat(""))?;
    if base != SINGLE_USER_TEMPLATE_OVERHEAD_TOKENS
        || usize::try_from(eighty_percent).map_or(true, |target| target <= base)
    {
        return Err(TokenizerOracleError::InvalidContextWindow);
    }
    ensure_target_fits(eighty_percent, base)?;
    ensure_target_fits(window_plus_one, base)?;

    // Control-byte corpora can have a favourable token/byte ratio but expand
    // under JSON escaping and have no reviewed provider-acceptance evidence.
    // This fixed printable pattern is still verified at screen and final sizes;
    // it is never assumed to retain the observed ratio.
    let screen = repeat_pattern(PRINTABLE_CORPUS_PATTERN, CANDIDATE_SCREEN_BYTES);
    if counter.count(&render_single_user_chat(&screen))?
        == base.saturating_add(CANDIDATE_SCREEN_BYTES)
    {
        let eighty_case = exact_case(counter, eighty_percent, base, PRINTABLE_CORPUS_PATTERN)?;
        let plus_one_case = exact_case(counter, window_plus_one, base, PRINTABLE_CORPUS_PATTERN)?;
        if let (Some(eighty_case), Some(plus_one_case)) = (eighty_case, plus_one_case) {
            return Ok(DeepSeekCorpusPair {
                tokenizer_digest,
                chat_template_digest: PINNED_CHAT_TEMPLATE_DIGEST,
                context_window_tokens,
                eighty_percent: eighty_case,
                window_plus_one: plus_one_case,
            });
        }
    }
    Err(TokenizerOracleError::ExactCorpusUnavailable {
        eighty_percent,
        window_plus_one,
    })
}

fn ensure_target_fits(target_tokens: u32, base: usize) -> Result<(), TokenizerOracleError> {
    let target =
        usize::try_from(target_tokens).map_err(|_| TokenizerOracleError::CountOutOfRange)?;
    if target.saturating_sub(base) > TEXT_MAX {
        return Err(TokenizerOracleError::TargetExceedsTextBound {
            target_tokens,
            max_text_bytes: TEXT_MAX,
        });
    }
    Ok(())
}

fn exact_case(
    counter: &impl ChatTokenCounter,
    target_tokens: u32,
    base: usize,
    pattern: &str,
) -> Result<Option<CorpusCase>, TokenizerOracleError> {
    let target =
        usize::try_from(target_tokens).map_err(|_| TokenizerOracleError::CountOutOfRange)?;
    let content_bytes = target
        .checked_sub(base)
        .ok_or(TokenizerOracleError::InvalidContextWindow)?;
    let content = repeat_pattern(pattern, content_bytes);
    let exact = counter.count(&render_single_user_chat(&content))?;
    if exact != target {
        return Ok(None);
    }
    let text = BoundedString::<TEXT_MAX>::new(content)?;
    let message = CanonicalMessage {
        role: Role::User,
        blocks: vec![CanonicalBlock::Text {
            text,
            annotations: Vec::new(),
        }],
    };
    let canonical = to_jcs_bytes(&message)?;
    Ok(Some(CorpusCase {
        message,
        corpus_digest: ContentHash::of(&canonical),
        target_chat_tokens: target_tokens,
        exact_chat_tokens: u32::try_from(exact)
            .map_err(|_| TokenizerOracleError::CountOutOfRange)?,
        text_bytes: u32::try_from(content_bytes)
            .map_err(|_| TokenizerOracleError::CountOutOfRange)?,
    }))
}

fn repeat_pattern(pattern: &str, byte_len: usize) -> String {
    debug_assert!(!pattern.is_empty() && pattern.is_ascii());
    let whole = byte_len / pattern.len();
    let remainder = byte_len % pattern.len();
    let mut content = String::with_capacity(byte_len);
    for _ in 0..whole {
        content.push_str(pattern);
    }
    content.push_str(&pattern[..remainder]);
    content
}

fn render_single_user_chat(content: &str) -> String {
    let mut rendered = String::with_capacity(
        BEGIN_OF_SENTENCE.len() + USER.len() + content.len() + ASSISTANT.len(),
    );
    rendered.push_str(BEGIN_OF_SENTENCE);
    rendered.push_str(USER);
    rendered.push_str(content);
    rendered.push_str(ASSISTANT);
    rendered
}

#[cfg(test)]
mod tests {
    use aex_model_catalog::canonical::{CanonicalBlock, TEXT_MAX};
    use aex_wire::ContentHash;

    use super::{
        ASSISTANT, BEGIN_OF_SENTENCE, ChatTokenCounter, DeepSeekTokenizerOracle,
        EIGHTY_PERCENT_CORPUS_DIGEST, PINNED_CHAT_TEMPLATE_DIGEST, PINNED_CHAT_TEMPLATE_SHA256,
        PINNED_TOKENIZER_DIGEST, PINNED_TOKENIZER_JSON_SHA256, TokenizerOracleError, USER,
        WINDOW_PLUS_ONE_CORPUS_DIGEST, build_with_counter,
    };

    struct ExactAsciiCounter;

    impl ChatTokenCounter for ExactAsciiCounter {
        fn count(&self, rendered_chat: &str) -> Result<usize, TokenizerOracleError> {
            let content = rendered_chat
                .strip_prefix(&format!("{BEGIN_OF_SENTENCE}{USER}"))
                .and_then(|rest| rest.strip_suffix(ASSISTANT))
                .ok_or(TokenizerOracleError::TemplateProjection)?;
            Ok(3 + content.len())
        }
    }

    struct DriftingCounter;

    impl ChatTokenCounter for DriftingCounter {
        fn count(&self, rendered_chat: &str) -> Result<usize, TokenizerOracleError> {
            let content = rendered_chat
                .strip_prefix(&format!("{BEGIN_OF_SENTENCE}{USER}"))
                .and_then(|rest| rest.strip_suffix(ASSISTANT))
                .ok_or(TokenizerOracleError::TemplateProjection)?;
            if content.is_empty() || content.len() == super::CANDIDATE_SCREEN_BYTES {
                return Ok(3 + content.len());
            }
            Ok(2 + content.len())
        }
    }

    #[test]
    fn published_hex_identities_match_the_typed_pins() {
        assert_eq!(
            PINNED_TOKENIZER_DIGEST.to_wire(),
            format!("sha256:{PINNED_TOKENIZER_JSON_SHA256}")
        );
        assert_eq!(
            PINNED_CHAT_TEMPLATE_DIGEST.to_wire(),
            format!("sha256:{PINNED_CHAT_TEMPLATE_SHA256}")
        );
    }

    #[test]
    fn a_mock_exact_tokenizer_builds_bounded_exact_cases() {
        let digest = ContentHash::of(b"mock tokenizer");
        let pair = build_with_counter(&ExactAsciiCounter, digest, 1_000_000)
            .expect("exact mock corpus is constructible");
        assert_eq!(pair.tokenizer_digest, digest);
        assert_eq!(pair.eighty_percent.target_chat_tokens, 800_000);
        assert_eq!(pair.eighty_percent.exact_chat_tokens, 800_000);
        assert_eq!(pair.eighty_percent.text_bytes, 799_997);
        assert_eq!(
            pair.eighty_percent.corpus_digest,
            EIGHTY_PERCENT_CORPUS_DIGEST
        );
        assert_eq!(pair.window_plus_one.target_chat_tokens, 1_000_001);
        assert_eq!(pair.window_plus_one.exact_chat_tokens, 1_000_001);
        assert_eq!(pair.window_plus_one.text_bytes, 999_998);
        assert_eq!(
            pair.window_plus_one.corpus_digest,
            WINDOW_PLUS_ONE_CORPUS_DIGEST
        );
        for case in [&pair.eighty_percent, &pair.window_plus_one] {
            let [CanonicalBlock::Text { text, annotations }] = case.message.blocks.as_slice()
            else {
                panic!("corpus is exactly one text block");
            };
            assert!(text.len() <= TEXT_MAX);
            assert_eq!(u32::try_from(text.len()).expect("bounded"), case.text_bytes);
            assert!(annotations.is_empty());
            assert_eq!(
                case.corpus_digest,
                ContentHash::of(&aex_wire::to_jcs_bytes(&case.message).expect("JCS"))
            );
        }
    }

    #[test]
    fn a_counter_that_misses_the_final_exact_count_fails_closed() {
        assert!(matches!(
            build_with_counter(&DriftingCounter, ContentHash::of(b"mock"), 10_000),
            Err(TokenizerOracleError::ExactCorpusUnavailable {
                eighty_percent: 8_000,
                window_plus_one: 10_001,
            })
        ));
    }

    #[test]
    fn a_target_outside_the_canonical_text_bound_fails_before_search() {
        let context = u32::try_from(TEXT_MAX).expect("TEXT_MAX is u32") + 100;
        assert!(matches!(
            build_with_counter(&ExactAsciiCounter, ContentHash::of(b"mock"), context),
            Err(TokenizerOracleError::TargetExceedsTextBound { .. })
        ));
    }

    #[test]
    fn unpinned_bytes_are_rejected_before_tokenizer_parsing() {
        let bytes = br#"{"version":"1.0"}"#;
        assert!(matches!(
            DeepSeekTokenizerOracle::from_pinned_json(bytes),
            Err(TokenizerOracleError::DigestMismatch { expected, actual })
                if expected == PINNED_TOKENIZER_DIGEST && actual == ContentHash::of(bytes)
        ));
    }
}
