//! The generic collection envelope.
//!
//! Every generated collection response is a concrete struct, because JSON Schema
//! has no generics. [`Page<T>`] is the Rust-side convenience the server and
//! client use to talk about all of them uniformly; it serializes identically to
//! the generated concrete shapes.

use serde::{Deserialize, Serialize};

use crate::cursor::Cursor;

/// One page of a collection. Collections never carry an entity tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Page<T> {
    /// The items on this page, in the route's declared order.
    pub items: Vec<T>,
    /// The continuation token; absent exactly on the last page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Cursor>,
}

impl<T> Page<T> {
    /// A page that ends the collection.
    #[must_use]
    pub const fn last(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
        }
    }

    /// A page with a continuation.
    #[must_use]
    pub const fn with_cursor(items: Vec<T>, cursor: Cursor) -> Self {
        Self {
            items,
            next_cursor: Some(cursor),
        }
    }

    /// Whether this is the last page.
    #[must_use]
    pub const fn is_last(&self) -> bool {
        self.next_cursor.is_none()
    }
}
