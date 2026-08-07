//! The monotonic gap-change hint one workspace publishes.
//!
//! A follow socket must notice that gap history changed, but reading the whole
//! ledger every idle cycle costs a query per cycle per socket to learn, almost
//! always, that nothing changed. The hint is one counter row that every gap
//! append increments, so a reader can carry it in the frontier bundle it already
//! reads and skip the ledger while the counter stands still.
//!
//! # The hint is not an authority
//!
//! Gap history remains the only authority on gap state; the hint decides only
//! *when to look*. Two properties keep that true:
//!
//! - it only ever **runs ahead**. The value counts append attempts, not durable
//!   revisions, so a write that fails after the counter moved costs one wasted
//!   ledger read and never a missed one;
//! - it is never *evidence of no change*. An absent, unreadable or regressed
//!   value means "unknown", which forces the reader to read the ledger, and a
//!   reader that trusts a value at all still re-reads on its own bounded
//!   recovery interval.

use std::collections::HashMap;
use std::hash::BuildHasher;

use aex_observation_domain::keys::{self, GAP_HINT_SK};
use aex_wire::ids::WorkspaceId;
use aws_sdk_dynamodb::types::{AttributeValue, TransactWriteItem, Update};

use crate::expressions::{ExpressionBuilder, ITEM_TYPE, PK, SK};
use crate::gap::GapCodecError;

/// The durable item discriminator of the gap-change hint row.
pub const GAP_HINT_ITEM_TYPE: &str = "gap_change_hint";

/// The attribute carrying the monotonic count.
const APPENDS: &str = "gapAppends";

/// One workspace's published gap-append count.
///
/// The counter is opaque to a reader beyond one question: is it the same value
/// it was last cycle? Its magnitude is deliberately not a revision number of
/// anything — no reader may derive gap state from it.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct GapAppendCount(u64);

impl GapAppendCount {
    /// The raw published count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The exact update one gap append publishes, independent of its envelope.
///
/// The transactional reconciler path and the direct `PutItem` path publish the
/// same row the same way, so the expression is built once here rather than
/// mirrored at each writer.
#[derive(Clone, Debug)]
pub struct HintUpdate {
    /// The hint row's partition key.
    pub pk: String,
    /// The hint row's sort key.
    pub sk: &'static str,
    /// The update expression.
    pub expression: String,
    /// The bound attribute names.
    pub names: HashMap<String, String>,
    /// The bound attribute values.
    pub values: HashMap<String, AttributeValue>,
}

/// Builds the update that publishes `appended` new gap revisions.
///
/// `ADD` rather than a read-modify-write: two concurrent appends must both be
/// counted, and an item that does not exist yet is created by the same call.
#[must_use]
pub fn hint_update(workspace: WorkspaceId, appended: u64) -> HintUpdate {
    let mut builder = ExpressionBuilder::new();
    let discriminator = builder.name(ITEM_TYPE);
    let kind = builder.string(GAP_HINT_ITEM_TYPE);
    let appends = builder.name(APPENDS);
    let delta = builder.number(appended);
    HintUpdate {
        pk: keys::gap_hint_pk(workspace),
        sk: GAP_HINT_SK,
        expression: format!("SET {discriminator} = {kind} ADD {appends} {delta}"),
        names: builder.names(),
        values: builder.values(),
    }
}

/// The same update as one action of a larger transaction.
///
/// # Errors
///
/// Returns [`GapCodecError`] when the provider builder refuses the action,
/// which can only mean the update is not a legal `Update` at all.
pub fn hint_update_action(
    table: &str,
    workspace: WorkspaceId,
    appended: u64,
) -> Result<TransactWriteItem, GapCodecError> {
    let update = hint_update(workspace, appended);
    let action = Update::builder()
        .table_name(table)
        .key(PK, AttributeValue::S(update.pk))
        .key(SK, AttributeValue::S(update.sk.to_owned()))
        .update_expression(update.expression)
        .set_expression_attribute_names(Some(update.names))
        .set_expression_attribute_values(Some(update.values))
        .build()
        .map_err(|error| GapCodecError::Inconsistent {
            attribute: PK,
            reason: error.to_string().into_boxed_str(),
        })?;
    Ok(TransactWriteItem::builder().update(action).build())
}

/// Decodes one returned hint row.
///
/// # Errors
///
/// Returns [`GapCodecError`] when the row is not a hint row or carries no
/// readable count. Nothing is defaulted: a reader treats the failure as
/// "unknown" and reads gap history, which is the answer that cannot be wrong.
pub fn decode<S: BuildHasher>(
    item: &HashMap<String, AttributeValue, S>,
) -> Result<GapAppendCount, GapCodecError> {
    let kind = item
        .get(ITEM_TYPE)
        .and_then(|value| value.as_s().ok())
        .ok_or(GapCodecError::Attribute {
            attribute: ITEM_TYPE,
        })?;
    if kind != GAP_HINT_ITEM_TYPE {
        return Err(GapCodecError::Inconsistent {
            attribute: ITEM_TYPE,
            reason: "expected gap_change_hint".into(),
        });
    }
    let appends = item
        .get(APPENDS)
        .and_then(|value| value.as_n().ok())
        .and_then(|text| text.parse::<u64>().ok())
        .ok_or(GapCodecError::Attribute { attribute: APPENDS })?;
    Ok(GapAppendCount(appends))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use aex_wire::ids::{PrefixedId as _, WorkspaceId};
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{APPENDS, GAP_HINT_ITEM_TYPE, decode, hint_update, hint_update_action};
    use crate::expressions::ITEM_TYPE;
    use crate::gap::GapCodecError;

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("wsp_0000000001e40r2081040g2081").expect("workspace")
    }

    fn row(appends: &str) -> HashMap<String, AttributeValue> {
        HashMap::from([
            (
                ITEM_TYPE.to_owned(),
                AttributeValue::S(GAP_HINT_ITEM_TYPE.to_owned()),
            ),
            (APPENDS.to_owned(), AttributeValue::N(appends.to_owned())),
        ])
    }

    #[test]
    fn a_hint_update_adds_to_the_count_rather_than_replacing_it() {
        let update = hint_update(workspace(), 3);

        assert!(
            update.expression.contains("ADD"),
            "two concurrent appends must both be counted: {}",
            update.expression
        );
        assert!(
            !update.expression.contains(&format!("SET {APPENDS}")),
            "a read-modify-write would lose a concurrent append: {}",
            update.expression
        );
        assert_eq!(update.pk, format!("GAPV#{}", workspace()));
        assert_eq!(update.sk, "CHANGE");
        assert!(
            update
                .values
                .values()
                .any(|value| value.as_n().is_ok_and(|number| number == "3")),
            "the update carries the number of appended revisions"
        );
    }

    #[test]
    fn a_hint_action_targets_the_hint_row_of_the_named_table() {
        let action =
            hint_update_action("observation-authority", workspace(), 1).expect("the action builds");
        let update = action.update().expect("the action is an update");

        assert_eq!(update.table_name(), "observation-authority");
        assert_eq!(
            update.key().get("pk").and_then(|key| key.as_s().ok()),
            Some(&format!("GAPV#{}", workspace()))
        );
        assert_eq!(
            update.key().get("sk").and_then(|key| key.as_s().ok()),
            Some(&"CHANGE".to_owned())
        );
    }

    #[test]
    fn a_published_count_round_trips_through_the_row_it_is_stored_in() {
        assert_eq!(decode(&row("7")).expect("decodes").get(), 7);
    }

    #[test]
    fn a_row_that_is_not_a_hint_row_is_refused_rather_than_counted() {
        let mut foreign = row("7");
        foreign.insert(
            ITEM_TYPE.to_owned(),
            AttributeValue::S("telemetry_gap".to_owned()),
        );

        assert!(matches!(
            decode(&foreign),
            Err(GapCodecError::Inconsistent {
                attribute: "itemType",
                ..
            })
        ));
        assert!(matches!(
            decode(&HashMap::<String, AttributeValue>::new()),
            Err(GapCodecError::Attribute {
                attribute: "itemType"
            })
        ));
    }

    #[test]
    fn a_count_that_is_not_a_number_is_refused_rather_than_treated_as_zero() {
        let mut mistyped = row("7");
        mistyped.insert(APPENDS.to_owned(), AttributeValue::S("7".to_owned()));
        assert!(matches!(
            decode(&mistyped),
            Err(GapCodecError::Attribute {
                attribute: "gapAppends"
            })
        ));

        assert!(matches!(
            decode(&row("not-a-number")),
            Err(GapCodecError::Attribute {
                attribute: "gapAppends"
            })
        ));
    }
}
