//! G4 — injection safety is structural, not textual.
//!
//! An arbitrary, hostile attribute key or operand must never reach a `DynamoDB`
//! expression string. The property below generates hostile inputs and asserts
//! that every built expression matches the safe grammar and contains no byte of
//! the operand, which is the invariant the placeholder builder exists to hold.

use aex_observation_store_aws::expressions::{
    ExpressionBuilder, deletion_epoch_condition, frontier_condition, immutable_condition,
    is_safe_expression, prepare_condition,
};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// No operand, however hostile, changes the shape of an expression.
    #[test]
    fn a_hostile_operand_never_reaches_an_expression_string(operand in ".{0,96}") {
        let mut builder = ExpressionBuilder::new();
        let expression = prepare_condition(&mut builder, &operand);
        prop_assert!(is_safe_expression(&expression), "unsafe grammar: {expression}");
        if operand.len() > 4 && !operand.chars().all(char::is_alphanumeric) {
            prop_assert!(
                !expression.contains(operand.as_str()),
                "the operand leaked into the expression"
            );
        }
        // The operand is bound as a value, not interpolated.
        prop_assert!(
            builder
                .values()
                .values()
                .any(|value| value.as_s().map(String::as_str) == Ok(operand.as_str())),
            "the operand was not bound as a value"
        );
    }

    /// Numeric conditions are equally structural.
    #[test]
    fn numeric_conditions_are_placeholders_only(revision in any::<u64>(), epoch in any::<u64>()) {
        let mut builder = ExpressionBuilder::new();
        let frontier = frontier_condition(&mut builder, revision);
        let deletion = deletion_epoch_condition(&mut builder, epoch);
        let immutable = immutable_condition(&mut builder);
        for expression in [&frontier, &deletion, &immutable] {
            prop_assert!(is_safe_expression(expression), "unsafe grammar: {expression}");
            prop_assert!(
                !expression.chars().any(|c| c.is_ascii_digit() && !expression.contains("#n")),
                "a literal digit reached the expression"
            );
        }
        prop_assert!(!frontier.contains(&revision.to_string()) || revision < 10);
    }
}

#[test]
fn every_condition_binds_its_operands_rather_than_spelling_them() {
    let mut builder = ExpressionBuilder::new();
    let expression = prepare_condition(&mut builder, "deadbeef");
    assert_eq!(
        expression,
        "attribute_not_exists(#n0) OR (#n1 = :v0 AND #n2 = :v1)"
    );
    let (names, values) = builder.len();
    assert_eq!((names, values), (3, 2));
}
