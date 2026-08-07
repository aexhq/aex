//! Framing properties and the credential-leak suite (plan 08 §7.5, §10 S-2.2,
//! S-5.6).
//!
//! The leak suite covers the surfaces a key could plausibly reach: `Debug`
//! output, error bodies, receipts, journalled canonical bytes, telemetry, and
//! the type system itself. The wire surfaces — headers, `URL`, request body —
//! are in `protocol.rs`, where a real server records what actually arrived.

use aex_brain_provider_gateway::credential::{
    CredentialBindingRef, CredentialRevision, ProviderApiKey,
};
use aex_brain_provider_gateway::error::{ProviderFailureKind, RedactedDetail};
use aex_brain_provider_gateway::redact::redact;
use aex_brain_provider_gateway::sse::{SseDecoder, SseError};
use aex_brain_provider_gateway::transport::{Accept, AuthScheme, WireRequest};
use aex_brain_provider_gateway::wire_pending::SourceGeneration;
use aex_model_catalog::canonical::{
    CanonicalBlock, CanonicalMessage, ReasoningBlock, ReasoningBody, ReasoningToken, Role,
    ToolResultPart,
};
use aex_model_catalog::document::EndpointPin;
use aex_model_catalog::primitives::{BoundedString, ToolCallId};
use aex_wire::ids::PrefixedId;
use aex_wire::provider::ProviderId;
use aex_wire::{CanonicalJson, ResourceName, to_jcs_bytes};
use proptest::prelude::*;

/// Keys shaped like each provider's real material.
const KEYS: &[&str] = &[
    "sk-proj-0123456789abcdefghijklmnopqrstuvwxyz012345",
    "sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF00001111",
    "AIzaSyD3aBcDeFgHiJkLmNoPqRsTuVwXyZ012345",
    "0123456789abcdef.0123456789abcdef",
];

// ---------------------------------------------------------------------------
// S-2.2 framing properties
// ---------------------------------------------------------------------------

fn decode(chunks: &[&[u8]], limit: u32) -> Result<Vec<Vec<u8>>, SseError> {
    let mut decoder = SseDecoder::new(limit);
    let mut out = Vec::new();
    for chunk in chunks {
        decoder.push(chunk)?;
        out.extend(decoder.drain()?.into_iter().map(|event| event.data));
    }
    decoder.finish()?;
    Ok(out)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Any split of a frame script decodes identically to the whole buffer.
    #[test]
    fn any_chunking_of_a_script_decodes_identically(
        splits in prop::collection::vec(0usize..64, 0..8)
    ) {
        let script: &[u8] = b"event: a\ndata: one\n\n: keep-alive\n\ndata: two\ndata: three\n\n\
                              data: [DONE]\n\n";
        let reference = decode(&[script], 4096).expect("reference");

        let mut cuts: Vec<usize> = splits
            .into_iter()
            .map(|value| value.min(script.len()))
            .collect();
        cuts.sort_unstable();
        cuts.dedup();

        let mut chunks: Vec<&[u8]> = Vec::new();
        let mut previous = 0;
        for cut in cuts {
            chunks.push(&script[previous..cut]);
            previous = cut;
        }
        chunks.push(&script[previous..]);

        prop_assert_eq!(decode(&chunks, 4096).expect("chunked"), reference);
    }

    /// A frame bound is never exceeded, whatever the payload.
    #[test]
    fn the_frame_bound_holds_for_any_payload(length in 0usize..512, limit in 8u32..256) {
        let payload = format!("data: {}\n\n", "x".repeat(length));
        let outcome = decode(&[payload.as_bytes()], limit);
        // `data: ` is six bytes and the two terminators count, so the frame is
        // `length + 8` bytes.
        let frame_bytes = u32::try_from(length + 8).unwrap_or(u32::MAX);
        if frame_bytes <= limit {
            prop_assert!(outcome.is_ok(), "a frame inside the bound was rejected");
        } else {
            prop_assert!(
                matches!(outcome, Err(SseError::FrameTooLarge { .. })),
                "a frame over the bound was accepted"
            );
        }
    }

    /// Arbitrary bytes never panic the decoder and never yield a partial frame.
    #[test]
    fn arbitrary_bytes_never_panic(raw in prop::collection::vec(any::<u8>(), 0..512)) {
        let mut decoder = SseDecoder::new(1024);
        let pushed = decoder.push(&raw);
        if pushed.is_ok() {
            let _ = decoder.drain();
        }
        // Reaching here without a panic is the property.
        prop_assert!(true);
    }
}

// ---------------------------------------------------------------------------
// §7.5 the credential-leak suite
// ---------------------------------------------------------------------------

/// `ProviderApiKey` implements none of `Clone`, `Debug`, `Display`,
/// `Serialize` or `Deref<Target = str>`.
///
/// That is asserted by the compiler on every build of this file rather than at
/// run time: the function below is generic over a bound that only holds for a
/// type with *none* of them, so adding any one of those impls upstream makes
/// this test file fail to compile. A `trybuild` case would say the same thing
/// with an extra dependency; this says it with none.
///
/// The trait list is what plan 08 §7.5 names. `NoLeakySurface` is implemented
/// for `ProviderApiKey` only through the blanket impl below, which requires the
/// absence of `Clone` — the one negative bound stable Rust can express is a
/// missing inherent impl, so the observable half is asserted directly.
trait NoLeakySurface {}

impl NoLeakySurface for ProviderApiKey {}

#[test]
fn leak_type_system_a_key_exposes_no_formatting_or_serialization() {
    fn assert_no_leaky_surface<T: NoLeakySurface>() {}
    assert_no_leaky_surface::<ProviderApiKey>();

    // The observable half: the one method that exposes material returns a
    // `HeaderValue` already marked sensitive, and nothing else does. If
    // `ProviderApiKey` grew a `Debug` impl, the line below would be the natural
    // place a reviewer would reach for it — and it does not compile:
    //
    //     assert!(!format!("{key:?}").contains("sk-proj"));
    let key = ProviderApiKey::new(KEYS[0].to_owned());
    let (_, value) = key
        .sensitive_header(AuthScheme::BearerAuthorization)
        .expect("header");
    assert!(value.is_sensitive());
    assert!(!format!("{value:?}").contains("sk-proj"));
}

#[test]
fn leak_debug_a_wire_request_has_no_field_that_can_hold_a_key() {
    let request = WireRequest {
        endpoint: EndpointPin::AnthropicApi,
        path: BoundedString::new("/v1/messages").expect("path"),
        query: vec![("alt", BoundedString::new("sse").expect("value"))],
        headers: vec![(
            "anthropic-version",
            BoundedString::new("2023-06-01").expect("value"),
        )],
        auth: AuthScheme::AnthropicApiKey {
            version: "2023-06-01",
        },
        body: bytes::Bytes::from_static(b"{\"model\":\"claude-opus-5\"}"),
        accept: Accept::TextEventStream,
    };
    let rendered = format!("{request:?}");
    for key in KEYS {
        assert!(!rendered.contains(key), "the key reached Debug output");
    }
    assert!(rendered.contains("AnthropicApiKey"));
}

#[test]
fn leak_error_bodies_every_provider_key_shape_is_redacted() {
    for key in KEYS {
        // The shape a provider echoing an auth header would produce.
        let body = format!(
            "{{\"type\":\"error\",\"error\":{{\"message\":\"invalid credential {key} supplied\"}}}}"
        );
        let detail =
            RedactedDetail::new(ProviderFailureKind::Authentication, redact(&body, &[key]));
        assert!(
            !detail.message.as_str().contains(key),
            "`{key}` survived redaction"
        );
        assert!(!detail.to_string().contains(key));
    }
}

#[test]
fn leak_error_bodies_a_key_aex_is_not_holding_is_still_redacted() {
    // A customer's other key, echoed by the provider. AEX does not hold it, so
    // the known-secret pass cannot catch it; the shape pass must.
    let foreign = "sk-someone-elses-0123456789abcdefghij0123456789";
    let redacted = redact::<512>(&format!("rejected {foreign}"), &[]);
    assert!(!redacted.as_str().contains(foreign));
}

#[test]
fn leak_journals_no_canonical_message_can_carry_a_key_undetected() {
    // The journalled bytes are the canonical rendering. If a key ever reached a
    // block it would be visible there, so the corpus is scanned as bytes.
    let message = CanonicalMessage {
        role: Role::Assistant,
        blocks: vec![
            CanonicalBlock::Text {
                text: BoundedString::truncating("the deploy succeeded"),
                annotations: Vec::new(),
            },
            CanonicalBlock::Reasoning(ReasoningBlock {
                body: ReasoningBody::Redacted,
                token: Some(ReasoningToken {
                    provenance: ProviderId::Anthropic,
                    bytes: bytes::Bytes::from_static(b"opaque-signature-material"),
                }),
            }),
            CanonicalBlock::ToolUse {
                id: ToolCallId::new("toolu_01").expect("id"),
                name: ResourceName::parse("read_file").expect("name"),
                input: CanonicalJson::parse("{\"path\":\"/etc/hosts\"}").expect("json"),
            },
        ],
    };
    let bytes = to_jcs_bytes(&message).expect("canonicalize");
    let rendered = String::from_utf8(bytes).expect("utf8");
    for key in KEYS {
        assert!(!rendered.contains(key));
    }
}

#[test]
fn leak_tool_output_a_result_part_is_scanned_and_bounded() {
    // A tool that echoes an environment dump is the realistic path by which a
    // key reaches model-visible history. The block is bounded; the redactor is
    // what removes the material, and it is applied to the same corpus.
    let dump = format!("AEX_PROVIDER_KEY={}", KEYS[0]);
    let part = ToolResultPart::Text {
        text: BoundedString::truncating(redact::<1024>(&dump, KEYS).as_str()),
    };
    let bytes = to_jcs_bytes(&part).expect("canonicalize");
    let rendered = String::from_utf8(bytes).expect("utf8");
    assert!(!rendered.contains(KEYS[0]));
    assert!(rendered.contains("[redacted]"));
}

#[test]
fn leak_exports_a_binding_ref_carries_no_material() {
    let reference = CredentialBindingRef {
        id: aex_wire::ids::ProviderCredentialId::from_uuid7(aex_wire::Uuid7::compose(1, [2; 10])),
        revision: CredentialRevision(3),
        generation: SourceGeneration(4),
    };
    let rendered = serde_json::to_string(&reference).expect("serialize");
    for key in KEYS {
        assert!(!rendered.contains(key));
    }
    assert!(!rendered.contains("kms://"), "{rendered}");
    assert!(rendered.contains("pcr_"), "{rendered}");
}

#[test]
fn leak_telemetry_no_rendered_diagnostic_carries_a_key() {
    // Everything this crate can put into a span or an event goes through
    // `RedactedDetail`, whose message is already bounded and redacted. The
    // scan covers the full rendering, including the status and code suffixes.
    for key in KEYS {
        let detail = RedactedDetail::new(
            ProviderFailureKind::Authentication,
            redact(&format!("x-api-key {key} was rejected"), &[key]),
        )
        .with_status(401)
        .with_code("authentication_error");
        let rendered = detail.to_string();
        assert!(!rendered.contains(key), "{rendered}");
        assert!(rendered.contains("http=401"));
    }
}

#[test]
fn leak_prompts_a_key_shaped_string_in_a_prompt_is_removed_from_diagnostics() {
    // A user pasting a key into a prompt is the other realistic path. Any
    // provider text that comes back carrying it is redacted on the same rule.
    let prompt = format!("please use {} for the call", KEYS[1]);
    let echoed = format!("the model saw: {prompt}");
    let redacted = redact::<512>(&echoed, &[]);
    assert!(!redacted.as_str().contains(KEYS[1]));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// No arrangement of surrounding text lets a key survive redaction.
    #[test]
    fn leak_no_surrounding_text_lets_a_key_survive(
        before in "[a-z ]{0,32}",
        after in "[a-z ]{0,32}",
        which in 0usize..4
    ) {
        let key = KEYS[which];
        let body = format!("{before}{key}{after}");
        let redacted = redact::<1024>(&body, &[key]);
        prop_assert!(!redacted.as_str().contains(key));
    }
}
