use std::collections::BTreeMap;

use aex_internal_contracts::usage::{AuthorityKind, FactAuthority, FactId, Meter};
use aex_wire::canonical::to_jcs_bytes;
use aex_wire::types::{DecimalU128, Region};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Signed as _;
use proptest::prelude::*;
use serde_json::{Value, json};
use sha2::Digest as _;
use time::OffsetDateTime;

use crate::{
    BookVerifier, PlaneBilling, RateContext, RatingError, SegmentKey, accumulate, allocate_segment,
    rate_quantity as try_rate_quantity, settle_segment,
};

struct Verifier {
    valid: bool,
    now: OffsetDateTime,
}

impl BookVerifier for Verifier {
    fn verify(&self, _canonical_payload: &[u8], signature: &str) -> bool {
        self.valid && signature == "fixture-signature"
    }

    fn now(&self) -> OffsetDateTime {
        self.now
    }
}

#[test]
fn exact_quanta_match_the_four_published_meters() {
    let ctx = context(&provisional_book(), PlaneBilling::Shadow).expect("fixture book");
    let expected = [
        (Meter::ComputeMillicpuMs, (1, 14_400)),
        (Meter::MemoryByteMs, (1, 128_849_018_880_i64)),
        (Meter::StorageByteMin, (5, 940_597_837_824_i64)),
        (Meter::DataTransferEgressByte, (3, 10_000)),
    ];
    for (index, (meter, (num, den))) in expected.into_iter().enumerate() {
        let contribution = rate_quantity(
            &ctx,
            meter,
            u128::try_from(den).expect("positive"),
            fact(index),
        );
        assert_eq!(
            contribution.exact,
            BigRational::from_integer(BigInt::from(num))
        );
    }
}

proptest! {
    #[test]
    fn rt01_order_and_rt02_segmentation_invariance(values in prop::collection::vec(0_u64..1_000_000, 1..50)) {
        let ctx = context(&provisional_book(), PlaneBilling::Shadow).expect("fixture book");
        let mut parts: Vec<_> = values.iter().enumerate().map(|(index, value)| {
            rate_quantity(&ctx, Meter::ComputeMillicpuMs, u128::from(*value), fact(index))
        }).collect();
        let whole = accumulate(parts.clone());
        parts.reverse();
        prop_assert_eq!(accumulate(parts.clone()), whole.clone());
        let midpoint = parts.len() / 2;
        let segmented = accumulate(parts[..midpoint].to_vec()) + accumulate(parts[midpoint..].to_vec());
        prop_assert_eq!(segmented, whole);
    }

    #[test]
    fn rt03_monotonic_and_rt05_homogeneous(value in 0_u64..1_000_000) {
        let ctx = context(&provisional_book(), PlaneBilling::Shadow).expect("fixture book");
        let one = rate_quantity(&ctx, Meter::DataTransferEgressByte, u128::from(value), fact(1));
        let more = rate_quantity(&ctx, Meter::DataTransferEgressByte, u128::from(value) + 1, fact(2));
        prop_assert!(more.exact >= one.exact);
        let doubled = rate_quantity(&ctx, Meter::DataTransferEgressByte, u128::from(value) * 2, fact(3));
        prop_assert_eq!(doubled.exact, one.exact * BigInt::from(2));
    }
}

#[test]
fn rt04_zero_book_handles_u128_max_without_overflow() {
    let ctx = context(&zero_book(false), PlaneBilling::Shadow).expect("zero book");
    for (index, meter) in Meter::ALL.into_iter().enumerate() {
        let part = rate_quantity(&ctx, meter, u128::MAX, fact(index));
        let rated = settle_segment(&ctx, key(meter), part.exact).expect("zero rates to zero");
        assert_eq!(rated.rounded.get(), 0);
    }
}

#[test]
fn rt06_million_fact_accumulation_rounds_once_without_drift() {
    let ctx = context(&provisional_book(), PlaneBilling::Shadow).expect("book");
    let exact = accumulate((0_u32..1_000_000).map(|index| {
        rate_quantity(
            &ctx,
            Meter::ComputeMillicpuMs,
            u128::from((index % 17) + 1),
            fact(usize::try_from(index).expect("u32 fits usize on supported targets")),
        )
    }));
    let segment = settle_segment(&ctx, key(Meter::ComputeMillicpuMs), exact)
        .expect("million facts stay bounded");
    assert!(segment.residual.abs() < BigRational::from_integer(BigInt::from(1)));
}

#[test]
fn rt07_rounding_ties_and_rt08_bound_are_checked() {
    let even = context(&provisional_book(), PlaneBilling::Shadow).expect("book");
    let half = BigRational::new(BigInt::from(5), BigInt::from(2));
    assert_eq!(
        settle_segment(&even, key(Meter::ComputeMillicpuMs), half)
            .expect("round")
            .rounded
            .get(),
        2
    );
    let ceil = context(&book_with_rounding("ceil_total"), PlaneBilling::Shadow).expect("book");
    assert_eq!(
        settle_segment(
            &ceil,
            key(Meter::ComputeMillicpuMs),
            BigRational::new(BigInt::from(1), BigInt::from(2))
        )
        .expect("round")
        .rounded
        .get(),
        1
    );
    let enormous = rate_quantity(&even, Meter::DataTransferEgressByte, u128::MAX, fact(0));
    assert_eq!(
        settle_segment(&even, key(Meter::DataTransferEgressByte), enormous.exact),
        Err(RatingError::AmountExceedsBusinessBound)
    );
}

#[test]
fn rt10_golden_invoice_and_rt13_allocation_are_exact() {
    let ctx = context(&provisional_book(), PlaneBilling::Shadow).expect("book");
    let compute = rate_quantity(&ctx, Meter::ComputeMillicpuMs, 500 * 3_600_000, fact(1));
    let memory = rate_quantity(
        &ctx,
        Meter::MemoryByteMs,
        1_073_741_824 * 3_600_000,
        fact(2),
    );
    let hands = settle_segment(
        &ctx,
        key(Meter::ComputeMillicpuMs),
        compute.exact.clone() + memory.exact.clone(),
    )
    .expect("hands invoice");
    let storage = settle_segment(
        &ctx,
        key(Meter::StorageByteMin),
        rate_quantity(&ctx, Meter::StorageByteMin, 1_073_741_824 * 43_800, fact(3)).exact,
    )
    .expect("storage");
    let transfer = settle_segment(
        &ctx,
        key(Meter::DataTransferEgressByte),
        rate_quantity(&ctx, Meter::DataTransferEgressByte, 1_000_000_000, fact(4)).exact,
    )
    .expect("transfer");
    insta::assert_json_snapshot!(json!({
        "handsOneHourMicrousd": hands.rounded.get(),
        "storageOneGibMonthMicrousd": storage.rounded.get(),
        "egressOneGbMicrousd": transfer.rounded.get(),
    }), @r###"
    {
      "egressOneGbMicrousd": 300000,
      "handsOneHourMicrousd": 155000,
      "storageOneGibMonthMicrousd": 250
    }
    "###);

    let pieces = vec![
        rate_quantity(&ctx, Meter::DataTransferEgressByte, 1, fact(5)),
        rate_quantity(&ctx, Meter::DataTransferEgressByte, 2, fact(6)),
    ];
    let allocation_segment = settle_segment(
        &ctx,
        key(Meter::DataTransferEgressByte),
        accumulate(pieces.clone()),
    )
    .expect("allocation segment");
    let allocated = allocate_segment(&allocation_segment, &pieces).expect("allocation");
    assert_eq!(
        allocated.iter().map(|(_, value)| value.get()).sum::<i64>(),
        allocation_segment.rounded.get()
    );
}

#[test]
fn rt11_correction_is_exact_before_the_single_rounding_boundary() {
    let ctx = context(&provisional_book(), PlaneBilling::Shadow).expect("book");
    let original = rate_quantity(&ctx, Meter::ComputeMillicpuMs, 123_456, fact(0));
    assert_eq!(
        original.exact.clone() + -original.exact,
        BigRational::from_integer(BigInt::from(0))
    );
}

#[test]
fn rt09_context_is_pinned_and_rt12_large_replay_is_byte_identical() {
    let priced = context(&provisional_book(), PlaneBilling::Shadow).expect("priced book");
    let zero = context(&zero_book(false), PlaneBilling::Shadow).expect("zero book");
    let priced_fact = rate_quantity(&priced, Meter::ComputeMillicpuMs, 14400, fact(0));
    let zero_fact = rate_quantity(&zero, Meter::ComputeMillicpuMs, 14400, fact(0));
    assert_ne!(priced.book_id(), zero.book_id());
    assert_ne!(priced_fact.exact, zero_fact.exact);

    let rate_corpus = || {
        accumulate((0_u32..100_000).map(|index| {
            rate_quantity(
                &priced,
                Meter::ComputeMillicpuMs,
                u128::from((index % 101) + 1),
                fact(usize::try_from(index).expect("u32 fits usize on supported targets")),
            )
        }))
    };
    let first = settle_segment(&priced, key(Meter::ComputeMillicpuMs), rate_corpus())
        .expect("bounded corpus");
    let second = settle_segment(&priced, key(Meter::ComputeMillicpuMs), rate_corpus())
        .expect("bounded corpus");
    assert_eq!(
        to_jcs_bytes(&first).expect("serializable result"),
        to_jcs_bytes(&second).expect("serializable result")
    );
}

#[test]
fn rate_book_open_fails_closed_for_signature_hash_expiry_schema_and_active_zero() {
    let now = OffsetDateTime::UNIX_EPOCH;
    let mut value = zero_book(false);
    assert_eq!(
        context_with(&value, PlaneBilling::Shadow, false, now).unwrap_err(),
        RatingError::UnsignedRateBook
    );
    value["rateCardHash"] = Value::String("sha256:00".into());
    assert_eq!(
        context_with(&value, PlaneBilling::Shadow, true, now).unwrap_err(),
        RatingError::HashMismatch
    );
    let mut value = zero_book(false);
    value["schemaVersion"] = json!(2);
    assert_eq!(
        context_with(&value, PlaneBilling::Shadow, true, now).unwrap_err(),
        RatingError::SchemaVersionUnsupported(2)
    );
    let mut value = zero_book(false);
    value["effectiveTo"] = json!("1969-12-31T23:59:59Z");
    resign(&mut value);
    assert_eq!(
        context_with(&value, PlaneBilling::Shadow, true, now).unwrap_err(),
        RatingError::ContextExpired
    );
    assert_eq!(
        context(&zero_book(true), PlaneBilling::Active).unwrap_err(),
        RatingError::ZeroBookOnActivePlane
    );
}

#[test]
fn one_module_owns_big_rational_to_microusd_conversion() {
    let modules = [
        ("exact", include_str!("exact.rs")),
        ("rate_card", include_str!("rate_card.rs")),
        ("rounding", include_str!("rounding.rs")),
        ("allocation", include_str!("allocation.rs")),
    ];
    let owners: Vec<_> = modules
        .into_iter()
        .filter_map(|(name, source)| source.contains("fn rational_to_microusd").then_some(name))
        .collect();
    assert_eq!(owners, ["rounding"]);
}

fn context(value: &Value, plane: PlaneBilling) -> Result<RateContext, RatingError> {
    context_with(value, plane, true, OffsetDateTime::UNIX_EPOCH)
}

fn context_with(
    value: &Value,
    plane: PlaneBilling,
    valid: bool,
    now: OffsetDateTime,
) -> Result<RateContext, RatingError> {
    RateContext::open(
        &to_jcs_bytes(value).expect("fixture canonical"),
        &Verifier { valid, now },
        plane,
    )
}

fn rate_quantity(
    context: &RateContext,
    meter: Meter,
    base_units: u128,
    fact: FactId,
) -> crate::Contribution {
    try_rate_quantity(context, meter, base_units, fact)
        .expect("fixture meter is in the verified book")
}

fn provisional_book() -> Value {
    book_with_rounding("half_even")
}

fn book_with_rounding(rounding: &str) -> Value {
    book(
        rounding,
        false,
        BTreeMap::from([
            (Meter::ComputeMillicpuMs.as_str(), ("1", "14400")),
            (Meter::MemoryByteMs.as_str(), ("1", "128849018880")),
            (Meter::StorageByteMin.as_str(), ("5", "940597837824")),
            (Meter::DataTransferEgressByte.as_str(), ("3", "10000")),
        ]),
    )
}

fn zero_book(active: bool) -> Value {
    book(
        "half_even",
        active,
        BTreeMap::from([
            (Meter::ComputeMillicpuMs.as_str(), ("0", "1")),
            (Meter::MemoryByteMs.as_str(), ("0", "1")),
            (Meter::StorageByteMin.as_str(), ("0", "1")),
            (Meter::DataTransferEgressByte.as_str(), ("0", "1")),
        ]),
    )
}

fn book(rounding: &str, active: bool, rates: BTreeMap<&str, (&str, &str)>) -> Value {
    let rates: serde_json::Map<String, Value> = rates
        .into_iter()
        .map(|(meter, (num, den))| {
            (
                meter.into(),
                json!({"numeratorMicrousd": num, "denominatorUnits": den}),
            )
        })
        .collect();
    let mut value = json!({
        "pricingVersion": if active { "private-fixture-v1" } else { "synthetic-zero-v1" },
        "schemaVersion": 1,
        "currency": "USD",
        "roundingRule": rounding,
        "billingActive": active,
        "effectiveFrom": "1969-01-01T00:00:00Z",
        "effectiveTo": "2030-01-01T00:00:00Z",
        "rateCardHash": "",
        "rates": Value::Object(rates),
        "signature": "fixture-signature"
    });
    resign(&mut value);
    value
}

fn resign(value: &mut Value) {
    let bytes = to_jcs_bytes(&value["rates"]).expect("rates canonical");
    value["rateCardHash"] = Value::String(format!(
        "sha256:{}",
        hex::encode(sha2::Sha256::digest(bytes))
    ));
}

fn fact(index: usize) -> FactId {
    FactId::derive(
        Region::EuWest1,
        Meter::ComputeMillicpuMs,
        &FactAuthority {
            kind: AuthorityKind::Compute,
            authority_id: format!("fixture-{index}").into_boxed_str(),
            segment_ordinal: DecimalU128::new(index as u128),
        },
    )
}

fn key(meter: Meter) -> SegmentKey {
    SegmentKey {
        organization: "org_fixture".into(),
        reservation: "res_fixture".into(),
        meter,
    }
}
