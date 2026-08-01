//! Bounded page assembly that never drops a split item.

use serde::Serialize;

/// Contract hard maximum.
pub const MAX_PAGE_ITEMS: usize = 1_000;
/// Response body hard maximum.
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// One assembled page and the source index of its next item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// Items included in this page.
    pub items: Vec<T>,
    /// `None` only when all source items were consumed.
    pub next_index: Option<usize>,
    /// Exact serialized JSON byte length of `items`.
    pub encoded_bytes: usize,
}

/// Validated item and byte budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paginator {
    items: usize,
    bytes: usize,
}

impl Paginator {
    /// Refuses invalid bounds instead of clamping them.
    ///
    /// # Errors
    ///
    /// Returns [`PageError`] for an item or byte cap outside the contract.
    pub const fn new(items: usize, bytes: usize) -> Result<Self, PageError> {
        let items = if items == 0 { 100 } else { items };
        if items > MAX_PAGE_ITEMS {
            return Err(PageError::ItemLimitTooLarge {
                found: items,
                maximum: MAX_PAGE_ITEMS,
            });
        }
        if bytes == 0 || bytes > MAX_PAGE_BYTES {
            return Err(PageError::ByteLimitInvalid {
                found: bytes,
                maximum: MAX_PAGE_BYTES,
            });
        }
        Ok(Self { items, bytes })
    }

    /// Assembles from `start`, stopping before either bound.
    ///
    /// # Errors
    ///
    /// Returns [`PageError`] for an invalid start, oversize item or serialization failure.
    pub fn page<T>(&self, source: &[T], start: usize) -> Result<Page<T>, PageError>
    where
        T: Clone + Serialize,
    {
        if start > source.len() {
            return Err(PageError::InvalidStart);
        }
        let mut items = Vec::new();
        let mut encoded_bytes = 2;
        for item in source.iter().skip(start).take(self.items) {
            let mut candidate = items.clone();
            candidate.push(item.clone());
            let candidate_bytes = serde_json::to_vec(&candidate)
                .map_err(|_| PageError::Serialization)?
                .len();
            if candidate_bytes > self.bytes {
                if items.is_empty() {
                    return Err(PageError::ItemTooLarge {
                        maximum: self.bytes,
                    });
                }
                break;
            }
            items.push(item.clone());
            encoded_bytes = candidate_bytes;
        }
        let consumed = start + items.len();
        Ok(Page {
            items,
            next_index: (consumed < source.len()).then_some(consumed),
            encoded_bytes,
        })
    }
}

/// Why a requested page could not be assembled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    /// Requested item cap exceeded the contract maximum.
    #[error("page item cap {found} exceeds {maximum}")]
    ItemLimitTooLarge {
        /// Requested cap.
        found: usize,
        /// Hard cap.
        maximum: usize,
    },
    /// Byte limit was zero or above the contract maximum.
    #[error("page byte cap {found} is invalid; maximum is {maximum}")]
    ByteLimitInvalid {
        /// Requested cap.
        found: usize,
        /// Hard cap.
        maximum: usize,
    },
    /// Start position exceeded the source length.
    #[error("page start is outside the source")]
    InvalidStart,
    /// One item cannot fit in an otherwise empty page.
    #[error("one item exceeds page byte cap {maximum}")]
    ItemTooLarge {
        /// Enforced cap.
        maximum: usize,
    },
    /// An item failed JSON serialization.
    #[error("page item is not JSON serializable")]
    Serialization,
}
