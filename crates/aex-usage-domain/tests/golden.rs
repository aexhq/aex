//! Golden vectors pinning every wire-visible encoding this crate owns.
//!
//! These are the strings a settlement receipt, a `MessageDeduplicationId`, a
//! `business_key` and a `DynamoDB` sort key are built from. Changing one is a
//! breaking change to an already-published identity, so each is pinned as a
//! literal here rather than recomputed from the implementation.
//!
//! The `SHA-256` digests below were produced independently of this crate (a
//! plain `sha256sum` over the canonical string) and then pinned. A drift in
//! either the canonical form or the digest construction fails this file.

use aex_usage_domain::fact::{ResourceKind, SCHEMA_VERSION};
use aex_usage_domain::identity::{
    AuthorityId, AuthorityKey, AuthorityKind, FACT_ID_LEN, FACT_ID_PREFIX, SegmentOrdinal,
};
use aex_usage_domain::interval::storage::{StorageClose, StorageOwnerKind, StorageSource};
use aex_usage_domain::measurement::{BoundaryId, FactBasis, ReceiptKind};
use aex_usage_domain::meter::{
    BaseUnit, Category, Meter, ObservabilityMeter, PublicCategory, TokenClass,
};
use aex_usage_domain::quantity::{MAX_QUANTITY, Quantity};
use aex_usage_domain::shape::HandsShape;
use aex_usage_domain::wire_pending::{RegionId, Timestamp};

fn key(category: Category, kind: AuthorityKind, id: &str, ordinal: u64) -> AuthorityKey {
    AuthorityKey {
        region: RegionId::parse("eu-west-1").expect("region"),
        category,
        kind,
        authority_id: AuthorityId::parse(id).expect("id"),
        segment_ordinal: SegmentOrdinal::new(ordinal),
    }
}

/// `{region}/{category}/{authority_kind}/{authority_id}/{segment_ordinal}`,
/// and `usage_` + the lowercase hex `SHA-256` of exactly that string.
#[test]
fn the_authority_key_and_fact_id_golden_vectors_hold() {
    let vectors = [
        (
            key(Category::Compute, AuthorityKind::Activation, "act-1", 0),
            "eu-west-1/compute/activation/act-1/0",
            "usage_d84bd0bb820e044435c12c368e3218792fc8840ed2b76863e351261be118dc03",
        ),
        (
            key(
                Category::Storage,
                AuthorityKind::ContentOwner,
                "content-1",
                7,
            ),
            "eu-west-1/storage/content_owner/content-1/7",
            "usage_063e39374f12b9ff7f03e9ce7a283f42ac2835c1f7ad594242e389f0e640e8fd",
        ),
        (
            key(
                Category::Transfer,
                AuthorityKind::EgressCrossing,
                "cross-1",
                0,
            ),
            "eu-west-1/transfer/egress_crossing/cross-1/0",
            "usage_a43ec71e462af6b2083a0918faa060a4d9b66f1cd644d11092af7684980515c4",
        ),
        (
            key(
                Category::Compute,
                AuthorityKind::LambdaInvocation,
                "req-1",
                0,
            ),
            "eu-west-1/compute/lambda_invocation/req-1/0",
            "usage_76a1c4e7954d74cd16f1dc67e66a3f24b62c2783a0d14ee2a1f796bf174cc0c0",
        ),
        (
            key(
                Category::Compute,
                AuthorityKind::HandsGeneration,
                "gen-1",
                3,
            ),
            "eu-west-1/compute/hands_generation/gen-1/3",
            "usage_80e90405b21f555bba823dc1eabfca78c6cfb373768d2a12a56bc519ae03cc63",
        ),
    ];

    for (authority, canonical, fact_id) in vectors {
        assert_eq!(authority.canonical(), canonical);
        assert_eq!(authority.fact_id().as_str(), fact_id);
        assert_eq!(fact_id.len(), FACT_ID_LEN, "70 ASCII characters");
        assert!(fact_id.starts_with(FACT_ID_PREFIX));
    }
}

/// A component that could otherwise contribute a separator is percent-encoded,
/// so two different keys cannot render the same canonical string.
#[test]
fn reserved_characters_cannot_be_forged_into_a_canonical_key() {
    // The identifier grammar refuses `#` and `/` outright.
    assert!(AuthorityId::parse("a/b").is_err());
    assert!(AuthorityId::parse("a#b").is_err());
    assert!(AuthorityId::parse("a\0b").is_err());
    assert!(AuthorityId::parse("").is_err());

    // Anything else that is not unreserved is percent-encoded.
    let encoded = key(Category::Compute, AuthorityKind::Run, "a b+c%d", 0).canonical();
    assert_eq!(encoded, "eu-west-1/compute/run/a%20b%2Bc%25d/0");
    assert_eq!(encoded.split('/').count(), 5);
}

/// The four priced meter identifiers and their units. These strings appear in
/// every rating request and every settlement receipt.
#[test]
fn the_priced_meter_identifiers_are_pinned() {
    assert_eq!(Meter::ALL.len(), 4, "four priced meters, and nothing else");

    let expected = [
        (
            Meter::ComputeMillicpuMs,
            "compute.millicpu_ms.v1",
            BaseUnit::MillicpuMs,
            "millicpu_ms",
            Category::Compute,
            PublicCategory::Compute,
        ),
        (
            Meter::MemoryByteMs,
            "memory.byte_ms.v1",
            BaseUnit::ByteMs,
            "byte_ms",
            Category::Compute,
            PublicCategory::Memory,
        ),
        (
            Meter::StorageByteMin,
            "storage.byte_min.v1",
            BaseUnit::ByteMin,
            "byte_min",
            Category::Storage,
            PublicCategory::Storage,
        ),
        (
            Meter::DataTransferEgressByte,
            "data_transfer.egress_byte.v1",
            BaseUnit::Byte,
            "byte",
            Category::Transfer,
            PublicCategory::DataTransfer,
        ),
    ];

    for (meter, id, unit, unit_id, category, public) in expected {
        assert_eq!(meter.id(), id);
        assert_eq!(meter.base_unit(), unit);
        assert_eq!(meter.base_unit().id(), unit_id);
        assert_eq!(meter.category(), category);
        assert_eq!(meter.public(), public);
    }
}

/// Model tokens are zero-dollar observability under their own identifiers, and
/// the two vocabularies never overlap.
#[test]
fn the_observability_identifiers_are_pinned_and_disjoint() {
    let expected = [
        (
            ObservabilityMeter::ModelTokens,
            "observability.model_tokens.v1",
        ),
        (
            ObservabilityMeter::ProviderCalls,
            "observability.provider_calls.v1",
        ),
        (
            ObservabilityMeter::ToolInvocations,
            "observability.tool_invocations.v1",
        ),
    ];
    for (meter, id) in expected {
        assert_eq!(meter.id(), id);
        assert_eq!(meter.category(), Category::Compute);
        assert!(
            id.parse::<Meter>().is_err(),
            "an observability id must never resolve to a priced meter"
        );
    }

    assert_eq!(
        TokenClass::ALL.map(TokenClass::id),
        ["input", "output", "cache_read", "cache_write", "reasoning"]
    );
}

/// The three authority tables and the four public categories.
#[test]
fn the_category_vocabularies_are_pinned() {
    assert_eq!(
        Category::ALL.map(Category::id),
        ["storage", "compute", "transfer"]
    );
    assert_eq!(
        PublicCategory::ALL.map(PublicCategory::id),
        ["storage", "compute", "memory", "data_transfer"]
    );
    // Memory is a discriminated fact inside the compute authority.
    assert_eq!(PublicCategory::Memory.category(), Category::Compute);
}

/// The five public Hands baseline tokens and their provider-derived triples.
///
/// The arity is five, not six. `references/limits-and-ceilings-decision-2026-07-30.md`
/// line 126 pins the tokens and line 312 calls them "the five public baseline
/// tokens"; a six-row table is wrong.
#[test]
fn the_hands_shape_table_has_five_rows() {
    const GIB: u64 = 1024 * 1024 * 1024;

    assert_eq!(HandsShape::ALL.len(), 5);
    assert_eq!(
        HandsShape::ALL.map(HandsShape::id),
        ["512mb", "1gb", "2gb", "4gb", "8gb"]
    );

    let expected = [
        (
            HandsShape::Mb512,
            250u32,
            GIB / 2,
            1_000u32,
            2 * GIB,
            8 * GIB,
        ),
        (HandsShape::Gb1, 500, GIB, 2_000, 4 * GIB, 8 * GIB),
        (HandsShape::Gb2, 1_000, 2 * GIB, 4_000, 8 * GIB, 8 * GIB),
        (HandsShape::Gb4, 2_000, 4 * GIB, 8_000, 16 * GIB, 16 * GIB),
        (HandsShape::Gb8, 4_000, 8 * GIB, 16_000, 32 * GIB, 32 * GIB),
    ];
    for (shape, millicpu, memory, peak_millicpu, peak_memory, disk) in expected {
        assert_eq!(shape.baseline().millicpu, millicpu, "{shape} baseline cpu");
        assert_eq!(
            shape.baseline().memory_bytes,
            memory,
            "{shape} baseline mem"
        );
        assert_eq!(shape.peak().millicpu, peak_millicpu, "{shape} peak cpu");
        assert_eq!(shape.peak().memory_bytes, peak_memory, "{shape} peak mem");
        assert_eq!(shape.disk_bytes(), disk, "{shape} disk");
    }

    // A token from the retired six-shape and Fargate-era vocabularies is refused.
    for retired in ["16gb", "256mb", "0.25cpu-1gb", "4cpu-12gb"] {
        assert!(
            retired.parse::<HandsShape>().is_err(),
            "`{retired}` is not a public baseline token"
        );
    }
}

/// The fixed-width instant every regional sort key depends on.
#[test]
fn the_canonical_timestamp_form_is_fixed_width() {
    let stamp = Timestamp::parse("2026-08-01T12:34:56.789Z").expect("canonical");
    assert_eq!(stamp.to_canonical(), "2026-08-01T12:34:56.789Z");
    assert_eq!(stamp.to_canonical().len(), 24);
    assert_eq!(stamp.month_bucket(), "2026-08");
    assert_eq!(stamp.day_bucket(), "2026-08-01");
    assert_eq!(stamp.hour_bucket(), "2026-08-01T12");

    // Exactly one spelling. A second spelling of the same instant would give a
    // second sort key for the same fact.
    for rejected in [
        "2026-08-01T12:34:56Z",
        "2026-08-01T12:34:56.7Z",
        "2026-08-01T12:34:56.7891Z",
        "2026-08-01T12:34:56.789+00:00",
        "2026-08-01 12:34:56.789Z",
    ] {
        assert!(
            Timestamp::parse(rejected).is_err(),
            "`{rejected}` must be refused"
        );
    }
}

/// The remaining row vocabularies, each of which appears verbatim on a fact row.
#[test]
fn the_row_vocabularies_are_pinned() {
    assert_eq!(FactBasis::ALL.map(FactBasis::id), ["consumed", "reserved"]);
    assert_eq!(
        ReceiptKind::ALL.map(ReceiptKind::id),
        [
            "provider_lifecycle",
            "lambda_report",
            "cgroup_interval",
            "reservation_token",
            "boundary_counter",
            "delivery_log",
            "storage_commit",
            "correction_case",
        ]
    );
    assert_eq!(
        BoundaryId::ALL.map(BoundaryId::id),
        [
            "regional_http",
            "regional_stream",
            "content_download",
            "provider_http",
            "hands_egress",
            "vercel_authenticated",
        ]
    );
    assert_eq!(
        StorageOwnerKind::ALL.map(StorageOwnerKind::id),
        [
            "content_object",
            "content_root",
            "public_observation",
            "temporary_export",
            "microvm_snapshot",
        ]
    );
    assert_eq!(
        StorageSource::ALL.map(StorageSource::id),
        ["s3", "dynamodb", "aurora"]
    );
    assert_eq!(
        [StorageClose::InteriorFloor, StorageClose::TerminalCeil].map(StorageClose::id),
        ["interior_floor", "terminal_ceil"]
    );
    assert_eq!(
        ResourceKind::ALL.map(ResourceKind::id),
        [
            "mux_task",
            "hands_generation",
            "lambda_function",
            "stream_connection",
            "content_root",
            "observation_export",
        ]
    );
    assert_eq!(AuthorityKind::ALL.len(), 16);
    assert_eq!(SCHEMA_VERSION.get(), 1);
}

/// `DynamoDB`'s `N` type carries 38 significant digits, so the ceiling is
/// `10^38 - 1` and a quantity is written as bare decimal with no separators,
/// no sign and no exponent.
#[test]
fn the_quantity_decimal_form_is_pinned() {
    assert_eq!(MAX_QUANTITY, 10u128.pow(38) - 1);
    assert_eq!(MAX_QUANTITY.to_string().len(), 38);

    let ceiling = Quantity::new(MAX_QUANTITY).expect("representable");
    assert_eq!(
        ceiling.to_string(),
        "99999999999999999999999999999999999999"
    );
    assert!(Quantity::new(MAX_QUANTITY + 1).is_err());
    assert_eq!(Quantity::ZERO.to_string(), "0");

    for rejected in ["-1", "01", "1.0", "1e3", "+1", "1_000", " 1", ""] {
        assert!(
            Quantity::parse(rejected).is_err(),
            "`{rejected}` must be refused"
        );
    }
}
