//! Item size preflight.
//!
//! `DynamoDB` rejects an over-large item with a `400`, which reaches the caller as
//! an opaque validation failure with no measurement in it. Measuring first turns
//! that into [`StoreError::ItemTooLarge`] carrying the actual size, which is the
//! difference between "something was too big" and "your 300 KiB tool result must
//! go to content".

use aws_sdk_dynamodb::types::AttributeValue;

use crate::attr::Item;
use crate::error::StoreError;

/// The provider's hard item ceiling.
pub const DYNAMODB_ITEM_CEILING: usize = 400 * 1024;

/// The application ceiling. Below the provider ceiling so a row can still gain
/// an attribute in a later revision without becoming unwritable.
pub const APPLICATION_ITEM_CEILING: usize = 256 * 1024;

/// The target size of a Merkle tree page or a fold snapshot page.
pub const PAGE_TARGET_BYTES: usize = 192 * 1024;

/// The largest canonical plaintext that is stored inline rather than in the
/// object store (`CANONICAL_PLAINTEXT_DYNAMODB_MAX_BYTES`).
///
/// The threshold is named configuration rather than a literal at the call site
/// precisely so the pending 4/8/16/32 KiB benchmark can move it without a code
/// change (plan 05 G-9).
pub const INLINE_PLAINTEXT_CEILING: usize = 32_768;

/// Measures the encoded size of one item, as `DynamoDB` counts it.
///
/// `DynamoDB` charges the UTF-8 bytes of every attribute name plus the bytes of
/// every value: a string is its UTF-8 length, a number is roughly one byte per
/// two significant digits plus one, a binary value is its raw length, and a
/// boolean or null is one byte. The measurement is deliberately an
/// over-estimate for numbers, because under-estimating is what produces the
/// opaque `400` this function exists to prevent.
#[must_use]
pub fn item_bytes(item: &Item) -> usize {
    item.iter()
        .map(|(name, value)| name.len() + value_bytes(value))
        .sum()
}

#[allow(
    clippy::match_same_arms,
    reason = "the wildcard charges an unknown future type the same one byte on purpose"
)]
fn value_bytes(value: &AttributeValue) -> usize {
    match value {
        AttributeValue::S(text) => text.len(),
        AttributeValue::N(text) => text.len().div_ceil(2) + 1,
        AttributeValue::B(blob) => blob.as_ref().len(),
        AttributeValue::Bool(_) | AttributeValue::Null(_) => 1,
        AttributeValue::L(items) => {
            3 + items
                .iter()
                .map(|item| 1 + value_bytes(item))
                .sum::<usize>()
        }
        AttributeValue::M(members) => {
            3 + members
                .iter()
                .map(|(name, member)| name.len() + value_bytes(member) + 1)
                .sum::<usize>()
        }
        AttributeValue::Ss(values) => values.iter().map(String::len).sum(),
        AttributeValue::Ns(values) => values.iter().map(|text| text.len().div_ceil(2) + 1).sum(),
        AttributeValue::Bs(values) => values.iter().map(|blob| blob.as_ref().len()).sum(),
        // A future attribute type is charged one byte rather than zero, because
        // the measurement exists to be an over-estimate.
        _ => 1,
    }
}

/// Rejects an item above the application ceiling.
///
/// # Errors
///
/// [`StoreError::ItemTooLarge`] carrying the measurement.
pub fn check_item(item: &Item) -> Result<(), StoreError> {
    check_against(item, APPLICATION_ITEM_CEILING)
}

/// Rejects a page above the page target.
///
/// # Errors
///
/// [`StoreError::ItemTooLarge`] carrying the measurement.
pub fn check_page(item: &Item) -> Result<(), StoreError> {
    check_against(item, PAGE_TARGET_BYTES)
}

fn check_against(item: &Item, ceiling: usize) -> Result<(), StoreError> {
    let measured = item_bytes(item);
    if measured <= ceiling {
        return Ok(());
    }
    Err(StoreError::ItemTooLarge { measured, ceiling })
}

/// Where a canonical plaintext of `bytes` belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placement {
    /// Small enough to live in the authority row.
    Inline,
    /// Large enough to live in the object store.
    ObjectStore,
}

impl Placement {
    /// The wire spelling stored on a content descriptor.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::ObjectStore => "s3",
        }
    }
}

/// Selects a placement for a canonical plaintext of `bytes`.
///
/// The boundary is inclusive: exactly [`INLINE_PLAINTEXT_CEILING`] bytes is
/// still inline.
#[must_use]
pub const fn placement(bytes: usize) -> Placement {
    if bytes <= INLINE_PLAINTEXT_CEILING {
        Placement::Inline
    } else {
        Placement::ObjectStore
    }
}

#[cfg(test)]
mod tests {
    use super::{
        APPLICATION_ITEM_CEILING, INLINE_PLAINTEXT_CEILING, PAGE_TARGET_BYTES, Placement,
        check_item, check_page, item_bytes, placement,
    };
    use crate::attr::{ItemBuilder, b, s};
    use crate::error::StoreError;

    fn sized(bytes: usize) -> crate::attr::Item {
        // `itemType` is nine bytes of name plus four of value; the payload
        // attribute name is seven. The blob makes up the rest exactly.
        let overhead = "itemType".len() + "run".len() + "payload".len();
        ItemBuilder::new("run")
            .set("payload", b(vec![0u8; bytes.saturating_sub(overhead)]))
            .build()
    }

    #[test]
    fn a_string_costs_its_utf8_length_and_a_name_costs_its_own() {
        let item = ItemBuilder::new("run").set("k", s("abcd")).build();
        assert_eq!(item_bytes(&item), "itemType".len() + "run".len() + 1 + 4);
    }

    #[test]
    fn the_item_ceiling_is_exact_at_n_minus_one_n_and_n_plus_one() {
        assert!(check_item(&sized(APPLICATION_ITEM_CEILING - 1)).is_ok());
        assert!(check_item(&sized(APPLICATION_ITEM_CEILING)).is_ok());
        let error = check_item(&sized(APPLICATION_ITEM_CEILING + 1))
            .expect_err("one byte over the ceiling");
        assert!(
            matches!(
                error,
                StoreError::ItemTooLarge {
                    ceiling: APPLICATION_ITEM_CEILING,
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn the_page_target_is_exact_at_n_minus_one_n_and_n_plus_one() {
        assert!(check_page(&sized(PAGE_TARGET_BYTES - 1)).is_ok());
        assert!(check_page(&sized(PAGE_TARGET_BYTES)).is_ok());
        assert!(check_page(&sized(PAGE_TARGET_BYTES + 1)).is_err());
    }

    #[test]
    fn the_placement_boundary_sits_exactly_at_the_named_threshold() {
        assert_eq!(placement(INLINE_PLAINTEXT_CEILING - 1), Placement::Inline);
        assert_eq!(placement(INLINE_PLAINTEXT_CEILING), Placement::Inline);
        assert_eq!(
            placement(INLINE_PLAINTEXT_CEILING + 1),
            Placement::ObjectStore
        );
        assert_eq!(Placement::ObjectStore.as_str(), "s3");
    }

    #[test]
    fn the_measurement_reaches_inside_a_list_and_a_map() {
        let nested = ItemBuilder::new("work")
            .set(
                "payload",
                aws_sdk_dynamodb::types::AttributeValue::M(std::collections::HashMap::from([(
                    "cursor".to_owned(),
                    s("abcdefgh"),
                )])),
            )
            .build();
        assert!(item_bytes(&nested) > "itemType".len() + "work".len() + "payload".len() + 8);
    }
}
