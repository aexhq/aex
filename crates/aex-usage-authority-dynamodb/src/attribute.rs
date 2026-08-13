//! The neutral-item to `AttributeValue` conversion and the strict row reader.
//!
//! Two directions, deliberately asymmetric:
//!
//! - **Writing** is total. [`ItemValue`] is a closed set that every variant of
//!   `AttributeValue` this table uses can represent, so an encode never fails.
//! - **Reading** is strict and fallible. Every accessor names the attribute it
//!   wanted and refuses a value of the wrong shape, because a money row read as
//!   the wrong shape is worse than a row that could not be read at all.
//!
//! The same reader serves a `GetItem` response, a `Query` page and a stream
//! `NEW_IMAGE`: [`from_stream_json`] converts the stream's DynamoDB-JSON
//! encoding into the identical `AttributeValue` map, so there is exactly one
//! decode path and a stream image cannot be admitted through a laxer one.

use std::collections::HashMap;

use aex_usage_domain::keys::{Item, ItemKey, ItemType, ItemValue};
use aex_usage_domain::wire_pending::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;

/// Why a stored row could not be read.
///
/// Every variant is terminal. A row that does not decode never decodes, so the
/// caller quarantines rather than retrying — the distinction the ports layer
/// draws between [`crate::PortError::Corrupt`] and
/// [`crate::PortError::Unavailable`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowError {
    /// A required attribute was absent.
    #[error("row is missing required attribute `{attribute}`")]
    Missing {
        /// The attribute that was absent.
        attribute: &'static str,
    },
    /// An attribute was present but unusable.
    #[error("attribute `{attribute}` is malformed: {reason}")]
    Malformed {
        /// The attribute that was refused.
        attribute: &'static str,
        /// Why it was refused.
        reason: String,
    },
    /// The row declares a different shape than the caller asked for.
    #[error("expected item type `{expected}` but the row carries `{actual}`")]
    ItemTypeMismatch {
        /// What the caller asked for.
        expected: &'static str,
        /// What the row says it is.
        actual: String,
    },
    /// A stream image was not valid `DynamoDB` JSON.
    #[error("stream image attribute `{attribute}` is not DynamoDB JSON: {reason}")]
    NotStreamJson {
        /// Where the decode stopped.
        attribute: String,
        /// Why it was refused.
        reason: String,
    },
}

/// The discriminator attribute every authority row carries.
pub const ITEM_TYPE: &str = "itemType";
/// The partition key attribute.
pub const PARTITION: &str = "pk";
/// The sort key attribute.
pub const SORT: &str = "sk";

/// Renders one neutral value as an `AttributeValue`.
#[must_use]
pub fn attribute(value: &ItemValue) -> AttributeValue {
    match value {
        ItemValue::S(text) => AttributeValue::S(text.clone()),
        ItemValue::N(number) => AttributeValue::N(number.clone()),
        ItemValue::Bool(flag) => AttributeValue::Bool(*flag),
        ItemValue::M(map) => AttributeValue::M(
            map.iter()
                .map(|(name, nested)| (name.clone(), attribute(nested)))
                .collect(),
        ),
        ItemValue::L(list) => AttributeValue::L(list.iter().map(attribute).collect()),
    }
}

/// Renders a whole row, including `pk`, `sk` and `itemType`.
#[must_use]
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn item(row: &Item) -> HashMap<String, AttributeValue> {
    row.to_attribute_map()
        .iter()
        .map(|(name, value)| (name.clone(), attribute(value)))
        .collect()
}

/// The two key attributes of one composite key.
#[must_use]
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn key(item_key: &ItemKey) -> HashMap<String, AttributeValue> {
    HashMap::from([
        (PARTITION.to_owned(), AttributeValue::S(item_key.pk.clone())),
        (SORT.to_owned(), AttributeValue::S(item_key.sk.clone())),
    ])
}

/// A string attribute value, for a condition or key binding.
#[must_use]
pub fn text(value: impl Into<String>) -> AttributeValue {
    AttributeValue::S(value.into())
}

/// A number attribute value, for a condition or key binding.
#[must_use]
pub fn number(value: impl Into<u128>) -> AttributeValue {
    AttributeValue::N(value.into().to_string())
}

/// Converts a DynamoDB-JSON stream image into the same map a `GetItem` returns.
///
/// The stream delivers `{"attr": {"S": "value"}}`; the SDK delivers
/// `AttributeValue::S`. Converting here rather than writing a second decoder is
/// what keeps the stream path and the read path on one set of strictness rules.
///
/// # Errors
///
/// Returns [`RowError::NotStreamJson`] for anything that is not a single-key
/// type-tagged object, for an unknown type tag, or for a tag whose payload is
/// the wrong JSON shape. Binary, set and null attributes are refused outright:
/// no usage row writes one, so encountering one means the image is not a usage
/// row.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn from_stream_json(
    image: &serde_json::Value,
) -> Result<HashMap<String, AttributeValue>, RowError> {
    let object = image.as_object().ok_or_else(|| RowError::NotStreamJson {
        attribute: "<image>".to_owned(),
        reason: "the image is not a JSON object".to_owned(),
    })?;
    let mut decoded = HashMap::with_capacity(object.len());
    for (name, value) in object {
        decoded.insert(name.clone(), tagged(name, value)?);
    }
    Ok(decoded)
}

/// Converts one type-tagged stream value.
fn tagged(name: &str, value: &serde_json::Value) -> Result<AttributeValue, RowError> {
    let refuse = |reason: &str| RowError::NotStreamJson {
        attribute: name.to_owned(),
        reason: reason.to_owned(),
    };
    let object = value.as_object().ok_or_else(|| refuse("not an object"))?;
    let mut entries = object.iter();
    let (tag, payload) = entries.next().ok_or_else(|| refuse("no type tag"))?;
    if entries.next().is_some() {
        return Err(refuse("more than one type tag"));
    }
    match tag.as_str() {
        "S" => Ok(AttributeValue::S(
            payload
                .as_str()
                .ok_or_else(|| refuse("`S` is not a string"))?
                .to_owned(),
        )),
        "N" => Ok(AttributeValue::N(
            payload
                .as_str()
                .ok_or_else(|| refuse("`N` is not a string-encoded number"))?
                .to_owned(),
        )),
        "BOOL" => Ok(AttributeValue::Bool(
            payload
                .as_bool()
                .ok_or_else(|| refuse("`BOOL` is not a bool"))?,
        )),
        "M" => {
            let nested = payload
                .as_object()
                .ok_or_else(|| refuse("`M` is not an object"))?;
            let mut map = HashMap::with_capacity(nested.len());
            for (inner, entry) in nested {
                map.insert(inner.clone(), tagged(inner, entry)?);
            }
            Ok(AttributeValue::M(map))
        }
        "L" => {
            let nested = payload
                .as_array()
                .ok_or_else(|| refuse("`L` is not an array"))?;
            let mut list = Vec::with_capacity(nested.len());
            for entry in nested {
                list.push(tagged(name, entry)?);
            }
            Ok(AttributeValue::L(list))
        }
        other => Err(RowError::NotStreamJson {
            attribute: name.to_owned(),
            reason: format!(
                "type tag `{other}` is not one this authority writes; a usage row \
                 carries strings, numbers, booleans, maps and lists only"
            ),
        }),
    }
}

/// A strict reader over one stored row.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    map: &'a HashMap<String, AttributeValue>,
}

impl<'a> Row<'a> {
    /// Binds a reader to a row, checking the declared item type first.
    ///
    /// The discriminator is checked before any attribute is read, so a row of
    /// the wrong shape fails as a mismatch rather than as a pile of missing
    /// attributes that hides which row was actually read.
    ///
    /// # Errors
    ///
    /// Returns [`RowError::Missing`] when the row carries no `itemType` and
    /// [`RowError::ItemTypeMismatch`] when it carries a different one.
    #[allow(
        clippy::implicit_hasher,
        reason = "the SDK hands over exactly this map type; generalizing buys nothing"
    )]
    pub fn bind(
        map: &'a HashMap<String, AttributeValue>,
        expected: ItemType,
    ) -> Result<Self, RowError> {
        let declared = map
            .get(ITEM_TYPE)
            .ok_or(RowError::Missing {
                attribute: ITEM_TYPE,
            })?
            .as_s()
            .map_err(|_| RowError::Malformed {
                attribute: ITEM_TYPE,
                reason: "the discriminator is not a string".to_owned(),
            })?;
        if declared != expected.id() {
            return Err(RowError::ItemTypeMismatch {
                expected: expected.id(),
                actual: declared.clone(),
            });
        }
        Ok(Self { map })
    }

    /// Binds a reader to a row read back through an `INCLUDE` index projection.
    ///
    /// Both this authority's indexes project `itemType` explicitly, so this is
    /// used only where the caller has already established the family from the
    /// index's own sparse key attribute.
    #[must_use]
    pub const fn bind_projected(map: &'a HashMap<String, AttributeValue>) -> Self {
        Self { map }
    }

    /// A required string attribute.
    ///
    /// # Errors
    ///
    /// [`RowError::Missing`] or [`RowError::Malformed`].
    pub fn string(&self, attribute: &'static str) -> Result<&'a str, RowError> {
        self.map
            .get(attribute)
            .ok_or(RowError::Missing { attribute })?
            .as_s()
            .map(String::as_str)
            .map_err(|_| RowError::Malformed {
                attribute,
                reason: "not a string".to_owned(),
            })
    }

    /// An optional string attribute.
    ///
    /// # Errors
    ///
    /// [`RowError::Malformed`] when it is present but not a string.
    pub fn optional_string(&self, attribute: &'static str) -> Result<Option<&'a str>, RowError> {
        match self.map.get(attribute) {
            None => Ok(None),
            Some(value) => {
                value
                    .as_s()
                    .map(|text| Some(text.as_str()))
                    .map_err(|_| RowError::Malformed {
                        attribute,
                        reason: "not a string".to_owned(),
                    })
            }
        }
    }

    /// A required unsigned integer attribute.
    ///
    /// # Errors
    ///
    /// [`RowError::Missing`] or [`RowError::Malformed`].
    pub fn u64(&self, attribute: &'static str) -> Result<u64, RowError> {
        self.raw_number(attribute)?
            .parse::<u64>()
            .map_err(|error| RowError::Malformed {
                attribute,
                reason: error.to_string(),
            })
    }

    /// A required unsigned 32-bit attribute.
    ///
    /// # Errors
    ///
    /// [`RowError::Missing`] or [`RowError::Malformed`].
    pub fn u32(&self, attribute: &'static str) -> Result<u32, RowError> {
        self.raw_number(attribute)?
            .parse::<u32>()
            .map_err(|error| RowError::Malformed {
                attribute,
                reason: error.to_string(),
            })
    }

    /// A required 128-bit unsigned attribute, for a quantity.
    ///
    /// # Errors
    ///
    /// [`RowError::Missing`] or [`RowError::Malformed`].
    pub fn u128(&self, attribute: &'static str) -> Result<u128, RowError> {
        self.raw_number(attribute)?
            .parse::<u128>()
            .map_err(|error| RowError::Malformed {
                attribute,
                reason: error.to_string(),
            })
    }

    /// A required signed integer attribute.
    ///
    /// Money is exact and may be negative when a correction reverses, so a
    /// rated amount is read through this rather than through the unsigned
    /// accessors, which would refuse a reversal as malformed.
    ///
    /// # Errors
    ///
    /// [`RowError::Missing`] or [`RowError::Malformed`].
    pub fn i128(&self, attribute: &'static str) -> Result<i128, RowError> {
        self.raw_number(attribute)?
            .parse::<i128>()
            .map_err(|error| RowError::Malformed {
                attribute,
                reason: error.to_string(),
            })
    }

    /// A required canonical instant.
    ///
    /// # Errors
    ///
    /// [`RowError::Missing`] or [`RowError::Malformed`].
    pub fn timestamp(&self, attribute: &'static str) -> Result<Timestamp, RowError> {
        Timestamp::parse(self.string(attribute)?).map_err(|error| RowError::Malformed {
            attribute,
            reason: error.to_string(),
        })
    }

    /// An optional canonical instant.
    ///
    /// # Errors
    ///
    /// [`RowError::Malformed`] when it is present but unparseable.
    pub fn optional_timestamp(
        &self,
        attribute: &'static str,
    ) -> Result<Option<Timestamp>, RowError> {
        match self.optional_string(attribute)? {
            None => Ok(None),
            Some(text) => Timestamp::parse(text)
                .map(Some)
                .map_err(|error| RowError::Malformed {
                    attribute,
                    reason: error.to_string(),
                }),
        }
    }

    /// Whether the row carries an attribute at all.
    #[must_use]
    pub fn has(&self, attribute: &str) -> bool {
        self.map.contains_key(attribute)
    }

    /// The raw decimal literal of a number attribute.
    fn raw_number(self, attribute: &'static str) -> Result<&'a str, RowError> {
        self.map
            .get(attribute)
            .ok_or(RowError::Missing { attribute })?
            .as_n()
            .map(String::as_str)
            .map_err(|_| RowError::Malformed {
                attribute,
                reason: "not a number".to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Row, RowError, attribute, from_stream_json, item, key};
    use aex_usage_domain::keys::{AuthorityKeys, Item, ItemType, ItemValue};
    use aex_usage_domain::meter::Category;
    use aex_usage_domain::wire_pending::{Timestamp, WorkspaceId};
    use aws_sdk_dynamodb::types::AttributeValue;
    use std::collections::{BTreeMap, HashMap};

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    fn row() -> Item {
        Item::new(
            Category::Compute,
            AuthorityKeys::new(Category::Compute)
                .frontier(&workspace())
                .expect("key"),
            ItemType::Frontier,
            BTreeMap::from([
                ("acceptedSequence".to_owned(), ItemValue::number(7u32)),
                ("state".to_owned(), ItemValue::text("advancing")),
                (
                    "serviceThrough".to_owned(),
                    ItemValue::instant(Timestamp::from_unix_millis(1_000).expect("instant")),
                ),
            ]),
        )
        .expect("a frontier belongs in every authority")
    }

    #[test]
    fn a_rendered_row_carries_its_key_and_discriminator() {
        let rendered = item(&row());
        assert_eq!(rendered["pk"], AttributeValue::S("WS#ws-1".to_owned()));
        assert_eq!(rendered["sk"], AttributeValue::S("FRONTIER".to_owned()));
        assert_eq!(
            rendered["itemType"],
            AttributeValue::S("usage_frontier".to_owned())
        );
        assert_eq!(
            rendered["acceptedSequence"],
            AttributeValue::N("7".to_owned())
        );
    }

    #[test]
    fn a_key_binding_carries_only_the_two_key_attributes() {
        let bound = key(&AuthorityKeys::new(Category::Compute)
            .frontier(&workspace())
            .expect("key"));
        assert_eq!(bound.len(), 2);
        assert!(bound.contains_key("pk") && bound.contains_key("sk"));
    }

    #[test]
    fn every_neutral_value_shape_round_trips_through_the_stream_encoding() {
        let neutral = ItemValue::M(BTreeMap::from([
            ("text".to_owned(), ItemValue::text("a")),
            ("count".to_owned(), ItemValue::number(3u32)),
            ("flag".to_owned(), ItemValue::Bool(true)),
            (
                "list".to_owned(),
                ItemValue::L(vec![ItemValue::text("b"), ItemValue::number(4u32)]),
            ),
        ]));
        let rendered = attribute(&neutral);
        // The stream delivers the same value in DynamoDB JSON; converting it
        // must land on exactly the value the SDK would have handed over.
        let stream = serde_json::json!({
            "value": {
                "M": {
                    "text": { "S": "a" },
                    "count": { "N": "3" },
                    "flag": { "BOOL": true },
                    "list": { "L": [{ "S": "b" }, { "N": "4" }] }
                }
            }
        });
        let decoded = from_stream_json(&stream).expect("decodes");
        assert_eq!(decoded["value"], rendered);
    }

    #[test]
    fn a_stream_image_that_is_not_dynamodb_json_is_refused_never_guessed() {
        for refused in [
            serde_json::json!({ "attr": "bare string" }),
            serde_json::json!({ "attr": { "S": 7 } }),
            serde_json::json!({ "attr": { "N": 7 } }),
            serde_json::json!({ "attr": {} }),
            serde_json::json!({ "attr": { "S": "a", "N": "1" } }),
            // Binary, sets and null are never written by this authority, so an
            // image carrying one is not a usage row.
            serde_json::json!({ "attr": { "B": "AAA=" } }),
            serde_json::json!({ "attr": { "NULL": true } }),
            serde_json::json!({ "attr": { "SS": ["a"] } }),
        ] {
            assert!(
                matches!(
                    from_stream_json(&refused),
                    Err(RowError::NotStreamJson { .. })
                ),
                "{refused} was accepted"
            );
        }
        assert!(matches!(
            from_stream_json(&serde_json::json!("not an object")),
            Err(RowError::NotStreamJson { .. })
        ));
    }

    #[test]
    fn a_row_is_refused_when_it_declares_a_different_shape() {
        let rendered = item(&row());
        assert!(Row::bind(&rendered, ItemType::Frontier).is_ok());
        assert!(matches!(
            Row::bind(&rendered, ItemType::Fact),
            Err(RowError::ItemTypeMismatch { .. })
        ));
        assert!(matches!(
            Row::bind(&HashMap::new(), ItemType::Fact),
            Err(RowError::Missing { .. })
        ));
    }

    #[test]
    fn every_accessor_names_the_attribute_it_refused() {
        let rendered = item(&row());
        let bound = Row::bind(&rendered, ItemType::Frontier).expect("binds");

        assert_eq!(bound.u64("acceptedSequence").expect("reads"), 7);
        assert_eq!(bound.u32("acceptedSequence").expect("reads"), 7);
        assert_eq!(bound.u128("acceptedSequence").expect("reads"), 7);
        assert_eq!(bound.string("state").expect("reads"), "advancing");
        assert_eq!(
            bound.timestamp("serviceThrough").expect("reads"),
            Timestamp::from_unix_millis(1_000).expect("instant")
        );
        assert_eq!(bound.optional_string("absent").expect("reads"), None);
        assert_eq!(bound.optional_timestamp("absent").expect("reads"), None);
        assert!(bound.has("state") && !bound.has("absent"));

        assert_eq!(
            bound.string("absent"),
            Err(RowError::Missing {
                attribute: "absent"
            })
        );
        // A number read as a string, and a string read as a number, are both
        // refused rather than coerced: a coerced money attribute is a wrong
        // answer that looks like a right one.
        assert!(matches!(
            bound.string("acceptedSequence"),
            Err(RowError::Malformed { .. })
        ));
        assert!(matches!(
            bound.u64("state"),
            Err(RowError::Malformed { .. })
        ));
        assert!(matches!(
            bound.timestamp("state"),
            Err(RowError::Malformed { .. })
        ));
    }
}
