//! Attribute construction and the typed row reader every regional codec uses.
//!
//! Decoding never coerces. A missing attribute, a wrong attribute type, an
//! unexpected `itemType` or a row whose ownership attributes name another
//! tenant is [`CodecError`], never a best-effort default, because a silently
//! defaulted authority field is indistinguishable from a correct one until it
//! decides something.

use std::collections::HashMap;

use aex_internal_contracts::RunId;
use aex_wire::ids::{IdParseError, PrefixedId};
use aex_wire::types::{Timestamp, ValueError};
use aws_sdk_dynamodb::types::AttributeValue;

/// A decoded `DynamoDB` item.
pub type Item = HashMap<String, AttributeValue>;

/// The `itemType` discriminator attribute, present on every regional item.
pub const ITEM_TYPE: &str = "itemType";

/// The partition key attribute name. There is no other one anywhere.
pub const PK: &str = "pk";

/// The sort key attribute name. There is no other one anywhere.
pub const SK: &str = "sk";

/// Why a stored row could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    /// A required attribute was absent.
    #[error("`{item_type}` is missing required attribute `{attribute}`")]
    Missing {
        /// The item type being decoded.
        item_type: &'static str,
        /// The attribute that was absent.
        attribute: &'static str,
    },
    /// An attribute carried a different `DynamoDB` type than the codec declares.
    #[error("`{item_type}`.`{attribute}` is {found}, not {expected}")]
    WrongType {
        /// The item type being decoded.
        item_type: &'static str,
        /// The offending attribute.
        attribute: &'static str,
        /// The `DynamoDB` type the codec declares.
        expected: &'static str,
        /// The `DynamoDB` type that was stored.
        found: &'static str,
    },
    /// An attribute carried the right type but an unusable value.
    #[error("`{item_type}`.`{attribute}` is malformed: {reason}")]
    Malformed {
        /// The item type being decoded.
        item_type: &'static str,
        /// The offending attribute.
        attribute: &'static str,
        /// Why the value is unusable.
        reason: String,
    },
    /// The row's discriminator named a different item type.
    #[error("expected `itemType` `{expected}` but the row declares `{found}`")]
    UnexpectedItemType {
        /// What the caller asked to decode.
        expected: &'static str,
        /// What the row actually is.
        found: String,
    },
    /// The row belongs to another tenant.
    ///
    /// `DynamoDB` `dynamodb:LeadingKeys` cannot express tenancy here, because
    /// partitions are session, agent and content scoped. Tenancy is enforced by
    /// the signed regional assertion plus this post-read check; IAM separates
    /// roles, not tenants.
    #[error("`{item_type}` belongs to `{found}`, not to the asserted `{expected}`")]
    WrongTenant {
        /// The item type being decoded.
        item_type: &'static str,
        /// The ownership value the caller asserted.
        expected: String,
        /// The ownership value stored on the row.
        found: String,
    },
}

/// A string attribute.
#[must_use]
pub fn s(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

/// A numeric attribute rendered from an unsigned integer.
#[must_use]
pub fn n(value: u64) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

/// A numeric attribute rendered from a signed integer.
#[must_use]
pub fn n_i64(value: i64) -> AttributeValue {
    AttributeValue::N(value.to_string())
}

/// A binary attribute.
#[must_use]
pub fn b(bytes: Vec<u8>) -> AttributeValue {
    AttributeValue::B(aws_smithy_types::Blob::new(bytes))
}

/// A boolean attribute.
#[must_use]
pub const fn boolean(value: bool) -> AttributeValue {
    AttributeValue::Bool(value)
}

/// A list-of-strings attribute.
#[must_use]
pub fn string_list(values: impl IntoIterator<Item = String>) -> AttributeValue {
    AttributeValue::L(values.into_iter().map(AttributeValue::S).collect())
}

/// A timestamp attribute in the one fixed-width spelling.
///
/// Fixed width is a correctness requirement rather than formatting taste: these
/// values appear inside sort keys, and a variable-width rendering would break
/// lexicographic ordering (D-03).
#[must_use]
pub fn stamp(value: Timestamp) -> AttributeValue {
    AttributeValue::S(value.to_wire())
}

/// The `DynamoDB` type name of a stored value, for a diagnostic.
const fn type_name(value: &AttributeValue) -> &'static str {
    match value {
        AttributeValue::S(_) => "S",
        AttributeValue::N(_) => "N",
        AttributeValue::B(_) => "B",
        AttributeValue::Bool(_) => "BOOL",
        AttributeValue::Null(_) => "NULL",
        AttributeValue::L(_) => "L",
        AttributeValue::M(_) => "M",
        AttributeValue::Ss(_) => "SS",
        AttributeValue::Ns(_) => "NS",
        AttributeValue::Bs(_) => "BS",
        _ => "unknown",
    }
}

/// A borrowed view over one stored row, bound to the item type being decoded.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    item: &'a Item,
    item_type: &'static str,
}

impl<'a> Row<'a> {
    /// Binds `item` to `item_type` after checking the discriminator.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::UnexpectedItemType`] when the row declares another
    /// type, and [`CodecError::Missing`] when it declares none at all. A row
    /// with an unexpected discriminator is never coerced.
    pub fn bind(item: &'a Item, item_type: &'static str) -> Result<Self, CodecError> {
        let declared = item
            .get(ITEM_TYPE)
            .ok_or(CodecError::Missing {
                item_type,
                attribute: ITEM_TYPE,
            })?
            .as_s()
            .map_err(|_| CodecError::WrongType {
                item_type,
                attribute: ITEM_TYPE,
                expected: "S",
                found: type_name(&item[ITEM_TYPE]),
            })?;
        if declared != item_type {
            return Err(CodecError::UnexpectedItemType {
                expected: item_type,
                found: declared.clone(),
            });
        }
        Ok(Self { item, item_type })
    }

    /// Binds a row read from a secondary index whose projection omits the
    /// discriminator.
    ///
    /// A `DynamoDB` `INCLUDE` or `KEYS_ONLY` projection can omit the
    /// discriminator, so [`Row::bind`] can never succeed against one: the
    /// discriminator is not there to check. Use this only where a **sparse** index key already
    /// restricts the partition to one row family, which is what makes the
    /// missing check safe rather than merely convenient. Every typed accessor
    /// still reports `item_type` in its errors, so a projection missing an
    /// attribute names the family it was decoding.
    #[must_use]
    pub const fn bind_projected(item: &'a Item, item_type: &'static str) -> Self {
        Self { item, item_type }
    }

    /// The underlying item.
    #[must_use]
    pub const fn item(&self) -> &'a Item {
        self.item
    }

    /// A required string attribute.
    ///
    /// # Errors
    ///
    /// [`CodecError::Missing`] or [`CodecError::WrongType`].
    pub fn string(&self, attribute: &'static str) -> Result<&'a str, CodecError> {
        let value = self.present(attribute)?;
        value
            .as_s()
            .map(String::as_str)
            .map_err(|_| self.wrong_type(attribute, "S", value))
    }

    /// An optional string attribute.
    ///
    /// # Errors
    ///
    /// [`CodecError::WrongType`] when present with another type.
    pub fn opt_string(&self, attribute: &'static str) -> Result<Option<&'a str>, CodecError> {
        match self.item.get(attribute) {
            None => Ok(None),
            Some(value) => value
                .as_s()
                .map(|text| Some(text.as_str()))
                .map_err(|_| self.wrong_type(attribute, "S", value)),
        }
    }

    /// A required unsigned numeric attribute.
    ///
    /// # Errors
    ///
    /// [`CodecError::Missing`], [`CodecError::WrongType`], or
    /// [`CodecError::Malformed`] for a fractional or out-of-range value —
    /// never a silent truncation.
    pub fn u64(&self, attribute: &'static str) -> Result<u64, CodecError> {
        let value = self.present(attribute)?;
        let text = value
            .as_n()
            .map_err(|_| self.wrong_type(attribute, "N", value))?;
        text.parse::<u64>().map_err(|error| CodecError::Malformed {
            item_type: self.item_type,
            attribute,
            reason: format!("`{text}` is not a u64: {error}"),
        })
    }

    /// An optional unsigned numeric attribute.
    ///
    /// # Errors
    ///
    /// As [`Row::u64`], minus the missing case.
    pub fn opt_u64(&self, attribute: &'static str) -> Result<Option<u64>, CodecError> {
        if self.item.contains_key(attribute) {
            self.u64(attribute).map(Some)
        } else {
            Ok(None)
        }
    }

    /// A required boolean attribute.
    ///
    /// # Errors
    ///
    /// [`CodecError::Missing`] or [`CodecError::WrongType`].
    pub fn boolean(&self, attribute: &'static str) -> Result<bool, CodecError> {
        let value = self.present(attribute)?;
        value
            .as_bool()
            .copied()
            .map_err(|_| self.wrong_type(attribute, "BOOL", value))
    }

    /// A required binary attribute.
    ///
    /// # Errors
    ///
    /// [`CodecError::Missing`] or [`CodecError::WrongType`].
    pub fn bytes(&self, attribute: &'static str) -> Result<&'a [u8], CodecError> {
        let value = self.present(attribute)?;
        value
            .as_b()
            .map(aws_smithy_types::Blob::as_ref)
            .map_err(|_| self.wrong_type(attribute, "B", value))
    }

    /// An optional binary attribute.
    ///
    /// # Errors
    ///
    /// [`CodecError::WrongType`] when present with another type.
    pub fn opt_bytes(&self, attribute: &'static str) -> Result<Option<&'a [u8]>, CodecError> {
        if self.item.contains_key(attribute) {
            self.bytes(attribute).map(Some)
        } else {
            Ok(None)
        }
    }

    /// A required binary attribute of an exact width.
    ///
    /// Key material has one length. A codec that accepted any width would let a
    /// truncated or padded verifier reach a comparison, where it would simply
    /// fail to match and look like a wrong credential rather than a corrupt row.
    ///
    /// # Errors
    ///
    /// [`CodecError::Missing`], [`CodecError::WrongType`], or
    /// [`CodecError::Malformed`] when the stored value is another width.
    pub fn fixed_bytes<const N: usize>(
        &self,
        attribute: &'static str,
    ) -> Result<[u8; N], CodecError> {
        let bytes = self.bytes(attribute)?;
        <[u8; N]>::try_from(bytes).map_err(|_| CodecError::Malformed {
            item_type: self.item_type,
            attribute,
            reason: format!("expected exactly {N} bytes, found {}", bytes.len()),
        })
    }

    /// A required list-of-strings attribute.
    ///
    /// Every member must be a string. A list holding another type is a corrupt
    /// row rather than a member to skip: skipping one would silently narrow a
    /// scope or an audience set, and narrowing is only safe when it is intended.
    ///
    /// # Errors
    ///
    /// [`CodecError::Missing`], [`CodecError::WrongType`] for a non-list or a
    /// non-string member.
    pub fn string_list(&self, attribute: &'static str) -> Result<Vec<&'a str>, CodecError> {
        let value = self.present(attribute)?;
        let members = value
            .as_l()
            .map_err(|_| self.wrong_type(attribute, "L", value))?;
        members
            .iter()
            .map(|member| {
                member
                    .as_s()
                    .map(String::as_str)
                    .map_err(|_| self.wrong_type(attribute, "L of S", member))
            })
            .collect()
    }

    /// A required timestamp in the one fixed-width spelling.
    ///
    /// # Errors
    ///
    /// [`CodecError::Malformed`] for any other RFC 3339 spelling, including a
    /// wider or absent fractional part.
    pub fn timestamp(&self, attribute: &'static str) -> Result<Timestamp, CodecError> {
        let text = self.string(attribute)?;
        Timestamp::parse(text).map_err(|error: ValueError| CodecError::Malformed {
            item_type: self.item_type,
            attribute,
            reason: error.to_string(),
        })
    }

    /// An optional timestamp.
    ///
    /// # Errors
    ///
    /// As [`Row::timestamp`], minus the missing case.
    pub fn opt_timestamp(&self, attribute: &'static str) -> Result<Option<Timestamp>, CodecError> {
        if self.item.contains_key(attribute) {
            self.timestamp(attribute).map(Some)
        } else {
            Ok(None)
        }
    }

    /// A required prefixed identifier.
    ///
    /// # Errors
    ///
    /// [`CodecError::Malformed`] when the stored text is not an identifier of
    /// exactly the requested kind. An id of one kind never decodes as another.
    pub fn id<T: PrefixedId>(&self, attribute: &'static str) -> Result<T, CodecError> {
        let text = self.string(attribute)?;
        T::parse(text).map_err(|error: IdParseError| CodecError::Malformed {
            item_type: self.item_type,
            attribute,
            reason: error.to_string(),
        })
    }

    /// An optional prefixed identifier.
    ///
    /// # Errors
    ///
    /// As [`Row::id`], minus the missing case.
    pub fn opt_id<T: PrefixedId>(&self, attribute: &'static str) -> Result<Option<T>, CodecError> {
        if self.item.contains_key(attribute) {
            self.id(attribute).map(Some)
        } else {
            Ok(None)
        }
    }

    /// A required private execution identifier.
    ///
    /// This is deliberately separate from [`Row::id`]: `RunId` is persisted in
    /// internal rows but no longer belongs to the public ID registry.
    ///
    /// # Errors
    ///
    /// [`CodecError::Malformed`] when the attribute is absent, not a string, or
    /// does not contain a canonical private run identifier.
    pub fn run_id(&self, attribute: &'static str) -> Result<RunId, CodecError> {
        let text = self.string(attribute)?;
        RunId::parse(text).map_err(|error| CodecError::Malformed {
            item_type: self.item_type,
            attribute,
            reason: error.to_string(),
        })
    }

    /// An optional private execution identifier.
    ///
    /// # Errors
    ///
    /// As [`Row::run_id`], minus the missing case.
    pub fn opt_run_id(&self, attribute: &'static str) -> Result<Option<RunId>, CodecError> {
        if self.item.contains_key(attribute) {
            self.run_id(attribute).map(Some)
        } else {
            Ok(None)
        }
    }

    /// A required member of a closed vocabulary.
    ///
    /// # Errors
    ///
    /// [`CodecError::Malformed`] when the stored value is outside `permitted`.
    /// An unknown enumerated value is a corrupt row, never a new variant.
    pub fn enumerated(
        &self,
        attribute: &'static str,
        permitted: &[&'static str],
    ) -> Result<&'static str, CodecError> {
        let text = self.string(attribute)?;
        permitted
            .iter()
            .find(|candidate| **candidate == text)
            .copied()
            .ok_or_else(|| CodecError::Malformed {
                item_type: self.item_type,
                attribute,
                reason: format!("`{text}` is outside the closed vocabulary {permitted:?}"),
            })
    }

    /// Re-checks an ownership attribute against the asserted value.
    ///
    /// # Errors
    ///
    /// [`CodecError::WrongTenant`] when the row names another owner.
    pub fn owned_by(&self, attribute: &'static str, asserted: &str) -> Result<(), CodecError> {
        let stored = self.string(attribute)?;
        if stored == asserted {
            return Ok(());
        }
        Err(CodecError::WrongTenant {
            item_type: self.item_type,
            expected: asserted.to_owned(),
            found: stored.to_owned(),
        })
    }

    fn present(&self, attribute: &'static str) -> Result<&'a AttributeValue, CodecError> {
        self.item.get(attribute).ok_or(CodecError::Missing {
            item_type: self.item_type,
            attribute,
        })
    }

    fn wrong_type(
        &self,
        attribute: &'static str,
        expected: &'static str,
        value: &AttributeValue,
    ) -> CodecError {
        CodecError::WrongType {
            item_type: self.item_type,
            attribute,
            expected,
            found: type_name(value),
        }
    }
}

/// Builds one item, attribute by attribute, without ever writing an empty
/// optional as `NULL`.
///
/// `DynamoDB` treats a `NULL` attribute as present, so writing an absent optional
/// as `NULL` would make `attribute_not_exists` and `attribute_exists` disagree
/// with the domain's notion of absence. The builder simply omits it.
#[derive(Debug, Default, Clone)]
pub struct ItemBuilder {
    item: Item,
}

impl ItemBuilder {
    /// An empty builder.
    #[must_use]
    pub fn new(item_type: &'static str) -> Self {
        let mut item = Item::new();
        item.insert(ITEM_TYPE.to_owned(), s(item_type));
        Self { item }
    }

    /// Sets one attribute.
    #[must_use]
    pub fn set(mut self, attribute: &str, value: AttributeValue) -> Self {
        self.item.insert(attribute.to_owned(), value);
        self
    }

    /// Sets one attribute when it is present, and omits it otherwise.
    #[must_use]
    pub fn set_opt(self, attribute: &str, value: Option<AttributeValue>) -> Self {
        match value {
            Some(value) => self.set(attribute, value),
            None => self,
        }
    }

    /// The finished item.
    #[must_use]
    pub fn build(self) -> Item {
        self.item
    }
}

#[cfg(test)]
mod tests {
    use super::{CodecError, ITEM_TYPE, Item, ItemBuilder, Row, n, s};
    use aws_sdk_dynamodb::types::AttributeValue;

    fn row() -> Item {
        ItemBuilder::new("session_head")
            .set("sessionId", s("ses_01j0000000000000000000000"))
            .set("workspaceId", s("ws_1"))
            .set("revision", n(7))
            .build()
    }

    #[test]
    fn binding_rejects_an_unexpected_item_type() {
        let error = Row::bind(&row(), "run").expect_err("a run is not a session head");
        assert!(
            matches!(
                error,
                CodecError::UnexpectedItemType {
                    expected: "run",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_projected_row_binds_without_a_discriminator_and_still_decodes() {
        // A secondary-index projection can omit the discriminator, so it is
        // absent by construction rather than by corruption.
        let mut item = row();
        item.remove(ITEM_TYPE);
        let bound = Row::bind_projected(&item, "session_head");
        assert_eq!(bound.u64("revision").expect("revision decodes"), 7);
    }

    #[test]
    fn a_projected_row_still_names_its_family_when_an_attribute_is_absent() {
        let mut item = row();
        item.remove(ITEM_TYPE);
        item.remove("revision");
        let error = Row::bind_projected(&item, "session_head")
            .u64("revision")
            .expect_err("the projection omits it");
        assert!(
            matches!(
                error,
                CodecError::Missing {
                    item_type: "session_head",
                    attribute: "revision"
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn binding_rejects_a_row_with_no_discriminator_at_all() {
        let mut item = row();
        item.remove(ITEM_TYPE);
        let error = Row::bind(&item, "session_head").expect_err("no discriminator");
        assert!(matches!(error, CodecError::Missing { .. }), "{error}");
    }

    #[test]
    fn a_missing_attribute_is_never_defaulted() {
        let item = row();
        let bound = Row::bind(&item, "session_head").expect("binds");
        let error = bound.u64("cancelEpoch").expect_err("absent");
        assert!(
            matches!(
                error,
                CodecError::Missing {
                    attribute: "cancelEpoch",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_wrong_type_is_never_coerced() {
        let item = row();
        let bound = Row::bind(&item, "session_head").expect("binds");
        let error = bound
            .u64("workspaceId")
            .expect_err("a string is not a number");
        assert!(
            matches!(
                error,
                CodecError::WrongType {
                    expected: "N",
                    found: "S",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_fractional_or_out_of_range_number_is_rejected_rather_than_truncated() {
        let mut item = row();
        item.insert("revision".to_owned(), AttributeValue::N("7.5".to_owned()));
        let bound = Row::bind(&item, "session_head").expect("binds");
        assert!(matches!(
            bound.u64("revision"),
            Err(CodecError::Malformed { .. })
        ));
        item.insert(
            "revision".to_owned(),
            AttributeValue::N("184467440737095516150".to_owned()),
        );
        let bound = Row::bind(&item, "session_head").expect("binds");
        assert!(matches!(
            bound.u64("revision"),
            Err(CodecError::Malformed { .. })
        ));
    }

    #[test]
    fn a_row_from_another_tenant_is_rejected_after_read() {
        let item = row();
        let bound = Row::bind(&item, "session_head").expect("binds");
        bound.owned_by("workspaceId", "ws_1").expect("same tenant");
        let error = bound
            .owned_by("workspaceId", "ws_2")
            .expect_err("another tenant");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }

    #[test]
    fn a_value_outside_the_closed_vocabulary_is_corrupt_not_a_new_variant() {
        let item = ItemBuilder::new("run")
            .set("status", s("teleported"))
            .build();
        let bound = Row::bind(&item, "run").expect("binds");
        let error = bound
            .enumerated("status", &["queued", "running", "succeeded"])
            .expect_err("outside the vocabulary");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }

    #[test]
    fn an_absent_optional_is_omitted_rather_than_written_as_null() {
        let item = ItemBuilder::new("run").set_opt("startedAt", None).build();
        assert!(!item.contains_key("startedAt"));
    }
}
