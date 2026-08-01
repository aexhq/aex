//! The hostile frame corpus, plus the proof that a declared payload length is
//! never allocated for.
//!
//! # How "no allocation before the bound check" is proven
//!
//! Not by counting bytes: a `#[global_allocator]` needs `unsafe impl`, and this
//! workspace forbids `unsafe_code`. The stronger structural proof is used instead —
//! the decoder returns a payload that **is a subslice of the caller's buffer**,
//! checked by comparing addresses. A decoder that copied, or that reserved capacity
//! from `payload_len` before checking it, could not return a pointer inside the
//! input, so the assertion fails the moment the property is lost.
//!
//! That is paired with the ordering assertion in `wire`: `payload_len` is refused
//! before the real frame length is even looked at, which is the only step where a
//! copy could plausibly appear.

use aex_hands_agent::wire::{
    FrameError, FrameExpectation, PROTOCOL_V1, REQUEST_PREAMBLE_LEN, RequestPreamble, Verb,
    decode_request, encode_request,
};
use aex_hands_protocol::rpc::Fence;
use aex_wire::ids::{GenerationId, PrefixedId as _, Uuid7};

fn generation(tag: u64) -> GenerationId {
    GenerationId::from_uuid7(Uuid7::compose(
        tag,
        [u8::try_from(tag & 0xff).unwrap_or(0); 10],
    ))
}

fn expectation() -> FrameExpectation {
    FrameExpectation {
        generation: generation(1),
        min_fence: Fence(4),
        schema_version: PROTOCOL_V1,
        max_frame_bytes: 1_024,
    }
}

/// The payload length as the preamble field type.
fn len32(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).expect("a test payload is small")
}

fn preamble(payload_len: u32) -> RequestPreamble {
    RequestPreamble {
        schema_version: PROTOCOL_V1,
        verb: Verb::Start,
        flags: 0,
        generation: generation(1),
        fence: Fence(4),
        payload_len,
    }
}

#[test]
fn an_oversize_declared_payload_is_refused_before_the_frame_is_even_measured() {
    // A frame that claims four gibibytes in a forty-byte body. If the bound were
    // checked after a `Vec::with_capacity(payload_len)`, this would try to reserve
    // four gibibytes.
    let hostile = encode_request(&preamble(u32::MAX), &[]);
    assert_eq!(hostile.len(), REQUEST_PREAMBLE_LEN);
    assert!(matches!(
        decode_request(&hostile, &expectation()),
        Err(FrameError::Oversize {
            limit: 1_024,
            actual: 4_294_967_295
        })
    ));

    // And the refusal is the *bound*, not the length disagreement: a frame whose
    // declared length is inside the bound but wrong reports the other error.
    let inconsistent = encode_request(&preamble(1_000), b"short");
    assert!(matches!(
        decode_request(&inconsistent, &expectation()),
        Err(FrameError::LengthMismatch { .. })
    ));
}

#[test]
fn a_decoded_payload_is_a_subslice_of_the_callers_buffer_and_never_a_copy() {
    let payload = vec![b'x'; 512];
    let encoded = encode_request(&preamble(512), &payload);

    let start = encoded.as_ptr() as usize;
    let end = start + encoded.len();

    let frame = decode_request(&encoded, &expectation()).expect("a well-formed frame decodes");
    let decoded = frame.payload.as_ptr() as usize;

    assert_eq!(frame.payload.len(), 512);
    assert!(
        decoded >= start && decoded + frame.payload.len() <= end,
        "the payload was copied out of the caller's buffer; the decoder must borrow"
    );
    assert_eq!(
        decoded,
        start + REQUEST_PREAMBLE_LEN,
        "the payload begins exactly after the preamble"
    );
}

#[test]
fn every_truncation_of_a_valid_frame_is_refused() {
    let payload = b"{\"operation\":\"exec\"}";
    let encoded = encode_request(&preamble(len32(payload)), payload);
    for cut in 0..encoded.len() {
        assert!(
            decode_request(&encoded[..cut], &expectation()).is_err(),
            "a frame truncated to {cut} bytes decoded"
        );
    }
    assert!(decode_request(&encoded, &expectation()).is_ok());
}

#[test]
fn every_single_byte_corruption_of_the_preamble_is_refused() {
    let payload = b"{}";
    let encoded = encode_request(&preamble(len32(payload)), payload);
    for index in 0..REQUEST_PREAMBLE_LEN {
        for delta in [1u8, 0x7f, 0xff] {
            let mut corrupt = encoded.clone();
            corrupt[index] = corrupt[index].wrapping_add(delta);
            assert!(
                decode_request(&corrupt, &expectation()).is_err(),
                "byte {index} + {delta} still decoded"
            );
        }
    }
}

#[test]
fn a_payload_corruption_is_not_refused_because_the_preamble_check_does_not_cover_it() {
    // Stated rather than assumed: `header_crc32c` covers the header only. Payload
    // integrity is the JSON decode's job for a metadata frame and the `blake3`
    // digest's job for a terminal body. Pretending otherwise would be a false
    // guarantee.
    let payload = b"{\"a\":1}";
    let mut encoded = encode_request(&preamble(len32(payload)), payload);
    let last = encoded.len() - 1;
    encoded[last] = b'!';
    let frame = decode_request(&encoded, &expectation())
        .expect("the header is still intact, so the frame decodes");
    assert_eq!(frame.payload.last(), Some(&b'!'));
}

#[test]
fn arbitrary_bytes_never_panic_and_never_decode_by_accident() {
    // A deterministic pseudo-random sweep: the codec must return a typed error for
    // every input, never panic, and never accept something it did not encode.
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut reached_magic = 0u32;
    for _ in 0..20_000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let len = usize::try_from(state % 96).unwrap_or(0);
        let mut bytes = vec![0u8; len];
        for (index, slot) in bytes.iter_mut().enumerate() {
            *slot = u8::try_from((state >> ((index % 8) * 8)) & 0xff).unwrap_or(0);
        }
        // Give a fraction of the corpus a valid magic so the later decode steps are
        // actually reached rather than every case dying at step two.
        if len >= 4 && state.is_multiple_of(3) {
            bytes[..4].copy_from_slice(b"AEXH");
            reached_magic += 1;
        }
        assert!(
            decode_request(&bytes, &expectation()).is_err(),
            "random bytes decoded as a valid frame: {bytes:?}"
        );
    }
    assert!(
        reached_magic > 1_000,
        "the corpus never got past the magic check, so it proved nothing"
    );
}
