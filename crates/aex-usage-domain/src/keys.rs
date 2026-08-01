//! The pure key grammar every usage authority table shares.
//!
//! No AWS types appear here. The three `aex-usage-*-aws` adapters each bind this
//! grammar to their own [`Category`] and convert the neutral item map to
//! `AttributeValue`; keeping the grammar itself in one place means the three
//! tables cannot drift into three key formats, while the adapters stay unable to
//! depend on each other.
//!
//! Format rules, all from plan 05 §2.0 and identical across every regional
//! table: `#` is the only separator, sequences are zero-padded to 20 digits so
//! lexical order equals numeric order, and every component is refused if it
//! could contain a separator.

use std::collections::BTreeMap;
use std::fmt;

use crate::frontier::AcceptedSequence;
use crate::identity::FactId;
use crate::meter::Category;
use crate::wire_pending::{Timestamp, WorkspaceId};

/// Width every sequence number is zero-padded to.
///
/// `u64::MAX` is 20 digits, so a padded sequence sorts lexically exactly as it
/// sorts numerically — which is what lets a range query walk a workspace's facts
/// in admission order.
pub const SEQUENCE_WIDTH: usize = 20;

/// Renders a sequence in the padded sort-key form.
#[must_use]
pub fn padded(sequence: u64) -> String {
    format!("{sequence:0SEQUENCE_WIDTH$}")
}

/// Every item type an authority table holds.
///
/// Written to the `itemType` attribute on every row and checked on decode, so a
/// row can never be read as the wrong shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemType {
    /// One immutable admitted fact.
    Fact,
    /// The deterministic-identity claim that fences admission.
    FactClaim,
    /// The four-stage frontier for one `(workspace, category)` sequence.
    Frontier,
    /// A fact awaiting delivery to the central settlement queue.
    Outbox,
    /// A committed settlement receipt.
    SettlementReceipt,
    /// The head of one target's correction chain.
    CorrectionHead,
    /// A record that could not be folded.
    Quarantine,
    /// A residence's durable minute cursor.
    StorageCursor,
    /// A run or operation's closure vector.
    Closure,
    /// A mux activation's crash-recovery cursor.
    ActivationCursor,
    /// The one-crossing-one-fact fence for a counter-mode egress fact.
    CrossingClaim,
}

impl ItemType {
    /// Every item type, in a stable order.
    pub const ALL: [Self; 11] = [
        Self::Fact,
        Self::FactClaim,
        Self::Frontier,
        Self::Outbox,
        Self::SettlementReceipt,
        Self::CorrectionHead,
        Self::Quarantine,
        Self::StorageCursor,
        Self::Closure,
        Self::ActivationCursor,
        Self::CrossingClaim,
    ];

    /// The stable `itemType` attribute value.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Fact => "usage_fact",
            Self::FactClaim => "usage_fact_claim",
            Self::Frontier => "usage_frontier",
            Self::Outbox => "usage_outbox",
            Self::SettlementReceipt => "usage_settlement_receipt",
            Self::CorrectionHead => "usage_correction_head",
            Self::Quarantine => "usage_quarantine",
            Self::StorageCursor => "usage_storage_cursor",
            Self::Closure => "usage_closure",
            Self::ActivationCursor => "usage_activation_cursor",
            Self::CrossingClaim => "usage_crossing_claim",
        }
    }

    /// Which categories may hold this item type.
    ///
    /// Three item types are category-specific by design: a storage cursor only
    /// exists where residences accrue, a closure vector has exactly one home,
    /// and a crossing claim only fences egress.
    #[must_use]
    pub const fn allowed_in(self, category: Category) -> bool {
        match self {
            Self::StorageCursor => matches!(category, Category::Storage),
            Self::Closure | Self::ActivationCursor => matches!(category, Category::Compute),
            Self::CrossingClaim => matches!(category, Category::Transfer),
            _ => true,
        }
    }
}

impl fmt::Display for ItemType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

/// Why a key or item could not be built or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// A component contained the key separator.
    #[error("`{component}` contains the `#` key separator and cannot be a key component")]
    Separator {
        /// The offending component.
        component: String,
    },
    /// An item type was used in a category that does not hold it.
    ///
    /// This is the structural fence: a storage cursor cannot be written into the
    /// compute authority even by a caller that has the table handle.
    #[error("`{item_type}` does not belong in the `{category}` authority")]
    ItemTypeEscape {
        /// The item type that was refused.
        item_type: &'static str,
        /// The category that refused it.
        category: &'static str,
    },
    /// A row was read as the wrong shape.
    #[error("expected item type `{expected}` but the row carries `{actual}`")]
    ItemTypeMismatch {
        /// What the caller asked for.
        expected: &'static str,
        /// What the row says it is.
        actual: String,
    },
    /// A required attribute was absent.
    #[error("row is missing required attribute `{attribute}`")]
    MissingAttribute {
        /// The attribute that was absent.
        attribute: &'static str,
    },
    /// An attribute could not be decoded.
    #[error("attribute `{attribute}` is malformed: {reason}")]
    MalformedAttribute {
        /// The attribute that was refused.
        attribute: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// Rejects a key component that could forge a separator.
fn component(value: &str) -> Result<&str, KeyError> {
    if value.contains('#') {
        return Err(KeyError::Separator {
            component: value.to_owned(),
        });
    }
    Ok(value)
}

/// One item's composite primary key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ItemKey {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

/// The key grammar for one authority category.
///
/// Every constructor is bound to the category the value was built with, so an
/// adapter cannot accidentally mint a key for a sibling authority: the
/// category-specific constructors refuse outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityKeys {
    category: Category,
}

impl AuthorityKeys {
    /// Binds the grammar to one category.
    #[must_use]
    pub const fn new(category: Category) -> Self {
        Self { category }
    }

    /// Which authority these keys address.
    #[must_use]
    pub const fn category(self) -> Category {
        self.category
    }

    /// The workspace partition every fact-scoped item lives in.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::Separator`] when the workspace identifier could forge
    /// a separator. The identifier grammar already refuses `#`, so this is a
    /// second fence rather than the only one.
    pub fn partition(self, workspace: &WorkspaceId) -> Result<String, KeyError> {
        Ok(format!("WS#{}", component(workspace.as_str())?))
    }

    /// `FACT#{accepted_sequence:020}` — the immutable fact row.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn fact(
        self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!("FACT#{}", padded(sequence.get())),
        })
    }

    /// `ID#{fact_id}` — the deterministic-identity claim.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn claim(self, workspace: &WorkspaceId, fact: &FactId) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!("ID#{}", component(fact.as_str())?),
        })
    }

    /// `FRONTIER` — the one mutable position row per sequence.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn frontier(self, workspace: &WorkspaceId) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: "FRONTIER".to_owned(),
        })
    }

    /// `OUTBOX#{accepted_sequence:020}` — the sweep's "not yet delivered" marker.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn outbox(
        self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!("OUTBOX#{}", padded(sequence.get())),
        })
    }

    /// `RECEIPT#{accepted_sequence:020}` — a committed settlement receipt.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn receipt(
        self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!("RECEIPT#{}", padded(sequence.get())),
        })
    }

    /// `CHEAD#{target_fact_id}` — the head of one correction chain.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn correction_head(
        self,
        workspace: &WorkspaceId,
        target: &FactId,
    ) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!("CHEAD#{}", component(target.as_str())?),
        })
    }

    /// `QUAR#{accepted_sequence:020}` — a record that could not be folded.
    ///
    /// # Errors
    ///
    /// As [`AuthorityKeys::partition`].
    pub fn quarantine(
        self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<ItemKey, KeyError> {
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!("QUAR#{}", padded(sequence.get())),
        })
    }

    /// `ACCRUAL#{owner_kind}#{owner_id}` / `GEN#{generation:020}` — the storage
    /// minute cursor. Storage authority only.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::ItemTypeEscape`] outside the storage authority.
    pub fn storage_cursor(
        self,
        owner_kind: &str,
        owner_id: &str,
        generation: u64,
    ) -> Result<ItemKey, KeyError> {
        self.guard(ItemType::StorageCursor)?;
        Ok(ItemKey {
            pk: format!(
                "ACCRUAL#{}#{}",
                component(owner_kind)?,
                component(owner_id)?
            ),
            sk: format!("GEN#{}", padded(generation)),
        })
    }

    /// `CLOSURE#{authority_kind}#{authority_id}` — a run or operation's closure
    /// vector. Compute authority only, so the vector has exactly one home.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::ItemTypeEscape`] outside the compute authority.
    pub fn closure(
        self,
        workspace: &WorkspaceId,
        authority_kind: &str,
        authority_id: &str,
    ) -> Result<ItemKey, KeyError> {
        self.guard(ItemType::Closure)?;
        Ok(ItemKey {
            pk: self.partition(workspace)?,
            sk: format!(
                "CLOSURE#{}#{}",
                component(authority_kind)?,
                component(authority_id)?
            ),
        })
    }

    /// `ACTIVATION#{session}#{agent}` / `GEN#{generation:020}` — a mux
    /// activation's crash-recovery cursor. Compute authority only.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::ItemTypeEscape`] outside the compute authority.
    pub fn activation_cursor(
        self,
        session: &str,
        agent: &str,
        generation: u64,
    ) -> Result<ItemKey, KeyError> {
        self.guard(ItemType::ActivationCursor)?;
        Ok(ItemKey {
            pk: format!("ACTIVATION#{}#{}", component(session)?, component(agent)?),
            sk: format!("GEN#{}", padded(generation)),
        })
    }

    /// `BOUNDARY#{boundary}#{epoch}` / `CROSS#{crossing_seq:020}` — the
    /// one-crossing-one-fact fence. Transfer authority only.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::ItemTypeEscape`] outside the transfer authority.
    pub fn crossing_claim(
        self,
        boundary: &str,
        epoch: u64,
        crossing_seq: u64,
    ) -> Result<ItemKey, KeyError> {
        self.guard(ItemType::CrossingClaim)?;
        Ok(ItemKey {
            pk: format!("BOUNDARY#{}#{epoch}", component(boundary)?),
            sk: format!("CROSS#{}", padded(crossing_seq)),
        })
    }

    /// Refuses an item type that does not belong in this authority.
    fn guard(self, item_type: ItemType) -> Result<(), KeyError> {
        if item_type.allowed_in(self.category) {
            return Ok(());
        }
        Err(KeyError::ItemTypeEscape {
            item_type: item_type.id(),
            category: self.category.id(),
        })
    }
}

/// A neutral attribute value.
///
/// The adapters convert this to `AttributeValue`; keeping the row shape free of
/// AWS types is what lets the codec be unit-tested with no client at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemValue {
    /// A string attribute.
    S(String),
    /// A number attribute, always written as a bare decimal integer.
    N(String),
    /// A boolean attribute.
    Bool(bool),
    /// A nested map.
    M(BTreeMap<String, ItemValue>),
    /// A list.
    L(Vec<ItemValue>),
}

impl ItemValue {
    /// A number attribute from an unsigned integer.
    #[must_use]
    pub fn number(value: impl Into<u128>) -> Self {
        Self::N(value.into().to_string())
    }

    /// A string attribute.
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::S(value.into())
    }

    /// A timestamp attribute in the canonical fixed-width form.
    #[must_use]
    pub fn instant(value: Timestamp) -> Self {
        Self::S(value.to_canonical())
    }
}

/// One row: its key, its declared type and its attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The composite primary key.
    pub key: ItemKey,
    /// What kind of row this is.
    pub item_type: ItemType,
    /// The attributes, excluding `pk`, `sk` and `itemType`.
    pub attributes: BTreeMap<String, ItemValue>,
}

impl Item {
    /// Builds a row, refusing an item type the category does not hold.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError::ItemTypeEscape`] when the item type does not belong
    /// in `category`.
    pub fn new(
        category: Category,
        key: ItemKey,
        item_type: ItemType,
        attributes: BTreeMap<String, ItemValue>,
    ) -> Result<Self, KeyError> {
        if !item_type.allowed_in(category) {
            return Err(KeyError::ItemTypeEscape {
                item_type: item_type.id(),
                category: category.id(),
            });
        }
        Ok(Self {
            key,
            item_type,
            attributes,
        })
    }

    /// The complete attribute map including `pk`, `sk` and `itemType`.
    #[must_use]
    pub fn to_attribute_map(&self) -> BTreeMap<String, ItemValue> {
        let mut map = self.attributes.clone();
        map.insert("pk".to_owned(), ItemValue::text(self.key.pk.clone()));
        map.insert("sk".to_owned(), ItemValue::text(self.key.sk.clone()));
        map.insert(
            "itemType".to_owned(),
            ItemValue::text(self.item_type.id().to_owned()),
        );
        map
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthorityKeys, Item, ItemType, ItemValue, KeyError, SEQUENCE_WIDTH, padded};
    use crate::frontier::AcceptedSequence;
    use crate::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use crate::meter::Category;
    use crate::wire_pending::{RegionId, WorkspaceId};
    use std::collections::BTreeMap;

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    fn fact_id() -> crate::identity::FactId {
        AuthorityKey {
            region: RegionId::parse("eu-west-1").expect("region"),
            category: Category::Compute,
            kind: AuthorityKind::Activation,
            authority_id: AuthorityId::parse("act-1").expect("id"),
            segment_ordinal: SegmentOrdinal::FIRST,
        }
        .fact_id()
    }

    fn sequence(value: u64) -> AcceptedSequence {
        AcceptedSequence::new(value).expect("positive")
    }

    #[test]
    fn sequences_pad_so_lexical_order_equals_numeric_order() {
        assert_eq!(padded(1), "00000000000000000001");
        assert_eq!(padded(1).len(), SEQUENCE_WIDTH);
        assert_eq!(padded(u64::MAX).len(), SEQUENCE_WIDTH);

        let mut keys = [padded(10), padded(2), padded(100), padded(1)];
        keys.sort();
        assert_eq!(keys, [padded(1), padded(2), padded(10), padded(100)]);
    }

    #[test]
    fn the_workspace_is_the_fact_partition() {
        let keys = AuthorityKeys::new(Category::Storage);
        let first = keys.fact(&workspace(), sequence(1)).expect("key");
        let second = keys.fact(&workspace(), sequence(2)).expect("key");
        assert_eq!(first.pk, "WS#ws-1");
        assert_eq!(
            first.pk, second.pk,
            "one workspace's facts share one partition, which is what gives the \
             stream per-workspace order"
        );
        assert_eq!(first.sk, "FACT#00000000000000000001");
    }

    #[test]
    fn every_fact_scoped_item_shares_the_workspace_partition() {
        let keys = AuthorityKeys::new(Category::Compute);
        let workspace = workspace();
        let built = [
            keys.fact(&workspace, sequence(1)).expect("fact"),
            keys.claim(&workspace, &fact_id()).expect("claim"),
            keys.frontier(&workspace).expect("frontier"),
            keys.outbox(&workspace, sequence(1)).expect("outbox"),
            keys.receipt(&workspace, sequence(1)).expect("receipt"),
            keys.correction_head(&workspace, &fact_id()).expect("head"),
            keys.quarantine(&workspace, sequence(1)).expect("quar"),
        ];
        for key in &built {
            assert_eq!(
                key.pk, "WS#ws-1",
                "the admission transaction must touch one partition"
            );
        }

        // Every sort key is distinct, so no two item types collide.
        let mut sort_keys: Vec<&str> = built.iter().map(|key| key.sk.as_str()).collect();
        sort_keys.sort_unstable();
        sort_keys.dedup();
        assert_eq!(sort_keys.len(), built.len());
    }

    #[test]
    fn a_category_specific_item_is_refused_by_a_sibling_authority() {
        // A storage cursor only exists where residences accrue.
        assert!(
            AuthorityKeys::new(Category::Storage)
                .storage_cursor("content_object", "obj-1", 3)
                .is_ok()
        );
        for foreign in [Category::Compute, Category::Transfer] {
            assert!(matches!(
                AuthorityKeys::new(foreign).storage_cursor("content_object", "obj-1", 3),
                Err(KeyError::ItemTypeEscape { .. })
            ));
        }

        // A closure vector has exactly one home.
        assert!(
            AuthorityKeys::new(Category::Compute)
                .closure(&workspace(), "run", "run-1")
                .is_ok()
        );
        for foreign in [Category::Storage, Category::Transfer] {
            assert!(matches!(
                AuthorityKeys::new(foreign).closure(&workspace(), "run", "run-1"),
                Err(KeyError::ItemTypeEscape { .. })
            ));
        }

        // A crossing claim only fences egress.
        assert!(
            AuthorityKeys::new(Category::Transfer)
                .crossing_claim("regional_http", 1, 7)
                .is_ok()
        );
        for foreign in [Category::Storage, Category::Compute] {
            assert!(matches!(
                AuthorityKeys::new(foreign).crossing_claim("regional_http", 1, 7),
                Err(KeyError::ItemTypeEscape { .. })
            ));
        }
    }

    #[test]
    fn a_component_that_could_forge_a_separator_is_refused() {
        let keys = AuthorityKeys::new(Category::Storage);
        assert!(matches!(
            keys.storage_cursor("content#object", "obj-1", 1),
            Err(KeyError::Separator { .. })
        ));
        assert!(matches!(
            keys.storage_cursor("content_object", "obj#1", 1),
            Err(KeyError::Separator { .. })
        ));
    }

    #[test]
    fn an_item_declares_its_type_and_refuses_a_foreign_one() {
        let keys = AuthorityKeys::new(Category::Compute);
        let key = keys.fact(&workspace(), sequence(5)).expect("key");
        let item = Item::new(
            Category::Compute,
            key,
            ItemType::Fact,
            BTreeMap::from([("quantity".to_owned(), ItemValue::number(42u32))]),
        )
        .expect("a fact belongs in every authority");

        let map = item.to_attribute_map();
        assert_eq!(map["pk"], ItemValue::text("WS#ws-1"));
        assert_eq!(map["sk"], ItemValue::text("FACT#00000000000000000005"));
        assert_eq!(map["itemType"], ItemValue::text("usage_fact"));
        assert_eq!(map["quantity"], ItemValue::N("42".to_owned()));

        // The same fence applies to the row, not only to the key.
        assert!(matches!(
            Item::new(
                Category::Transfer,
                keys.fact(&workspace(), sequence(5)).expect("key"),
                ItemType::Closure,
                BTreeMap::new(),
            ),
            Err(KeyError::ItemTypeEscape { .. })
        ));
    }

    #[test]
    fn every_item_type_has_a_distinct_stable_identifier() {
        let mut ids: Vec<&str> = ItemType::ALL.iter().map(|item| item.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "item type identifiers must be distinct");

        // Every identifier is namespaced, so a usage row is recognisable in a
        // stream filter without reading anything else.
        for item in ItemType::ALL {
            assert!(item.id().starts_with("usage_"), "{item}");
        }
    }

    #[test]
    fn a_number_attribute_is_always_a_bare_decimal_integer() {
        assert_eq!(ItemValue::number(0u32), ItemValue::N("0".to_owned()));
        assert_eq!(
            ItemValue::number(u64::MAX),
            ItemValue::N("18446744073709551615".to_owned())
        );
    }
}
