//! History that pairs model input with host-owned annotations.
//!
//! Only [`Item`] is projected to providers; annotation `A` holds storage identifiers
//! and input provenance. Cloning, truncation, and restoration keep each pair together.
//! The caller's append callback owns persistence; this container does not.

use serde::{Deserialize, Serialize};

use crate::Item;

/// One conversation-history entry with host-owned annotation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry<A = ()> {
    /// Provider/model-visible conversation item.
    pub item: Item,
    /// Host-domain metadata kept with the item and never projected to providers.
    pub annotation: A,
}

impl<A> HistoryEntry<A> {
    /// Build an entry from an item and its annotation.
    pub fn new(item: Item, annotation: A) -> Self {
        Self { item, annotation }
    }

    /// Split the entry into its item and annotation.
    pub fn into_parts(self) -> (Item, A) {
        (self.item, self.annotation)
    }
}

impl HistoryEntry<()> {
    /// Build a unit-annotated entry.
    pub fn from_item(item: Item) -> Self {
        Self {
            item,
            annotation: (),
        }
    }
}

/// Ordered history of model input and host-owned annotations.
///
/// [`History::append_with`] runs the callback before adding an item. A host that
/// persists in that callback keeps failed writes out of subsequent model requests.
/// Batch appends commit one item at a time; later failures do not roll back earlier
/// items. Direct append and replacement APIs do not persist, so the host must
/// distinguish restoration from normal appends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct History<A = ()> {
    entries: Vec<HistoryEntry<A>>,
}

impl<A> History<A> {
    /// Create an empty history.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Build history from already annotated entries, preserving order.
    pub fn from_entries(entries: Vec<HistoryEntry<A>>) -> Self {
        Self { entries }
    }

    /// Replace all entries as one restore/rebuild operation and return the old entries.
    pub fn replace_entries(&mut self, entries: Vec<HistoryEntry<A>>) -> Vec<HistoryEntry<A>> {
        std::mem::replace(&mut self.entries, entries)
    }

    /// Borrow annotated entries.
    pub fn entries(&self) -> &[HistoryEntry<A>] {
        &self.entries
    }

    /// Mutably borrow annotated entries for host-owned rebuild operations.
    pub fn entries_mut(&mut self) -> &mut [HistoryEntry<A>] {
        &mut self.entries
    }

    /// Consume the history into annotated entries.
    pub fn into_entries(self) -> Vec<HistoryEntry<A>> {
        self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over annotated entries.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &HistoryEntry<A>> {
        self.entries.iter()
    }

    /// Iterate over provider-visible items only.
    pub fn items(&self) -> impl ExactSizeIterator<Item = &Item> {
        self.entries.iter().map(|entry| &entry.item)
    }

    /// Clone provider-visible items into a request-local projection.
    pub fn items_cloned(&self) -> Vec<Item> {
        self.items().cloned().collect()
    }

    /// Append an already annotated entry.
    pub fn push_entry(&mut self, entry: HistoryEntry<A>) {
        self.entries.push(entry);
    }

    /// Append many already annotated entries.
    pub fn extend_entries(&mut self, entries: impl IntoIterator<Item = HistoryEntry<A>>) {
        self.entries.extend(entries);
    }

    /// Commit one item through a trusted annotation callback before it becomes live.
    ///
    /// The callback may durably persist the item and returns the annotation that
    /// must be stored with it. If the callback fails, the history is left unchanged.
    pub fn append_with(
        &mut self,
        item: Item,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<(), String> {
        let annotation = annotate(&item)?;
        self.entries.push(HistoryEntry { item, annotation });
        Ok(())
    }

    /// Commit items through a trusted annotation callback before they become live.
    ///
    /// Items before a failure remain appended; the failing item and later items do
    /// not enter history. This mirrors append-only durable logs where each accepted
    /// item is already committed before the next item is attempted.
    pub fn extend_with(
        &mut self,
        items: impl IntoIterator<Item = Item>,
        annotate: &mut impl FnMut(&Item) -> Result<A, String>,
    ) -> Result<(), String> {
        for item in items {
            self.append_with(item, annotate)?;
        }
        Ok(())
    }

    /// Truncate entries, preserving item+annotation pairing for retained entries.
    pub fn truncate(&mut self, len: usize) {
        self.entries.truncate(len);
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl History<()> {
    /// Build unit-annotated history from provider-visible items.
    pub fn from_items(items: Vec<Item>) -> Self {
        Self {
            entries: items.into_iter().map(HistoryEntry::from_item).collect(),
        }
    }

    /// Replace history from provider-visible items using unit annotations.
    pub fn replace_items(&mut self, items: Vec<Item>) -> Vec<HistoryEntry<()>> {
        self.replace_entries(items.into_iter().map(HistoryEntry::from_item).collect())
    }

    /// Append one item with unit annotation.
    pub fn push(&mut self, item: Item) {
        self.entries.push(HistoryEntry::from_item(item));
    }

    /// Append items with unit annotations.
    pub fn extend_items(&mut self, items: impl IntoIterator<Item = Item>) {
        self.entries
            .extend(items.into_iter().map(HistoryEntry::from_item));
    }
}

impl<A> IntoIterator for History<A> {
    type Item = HistoryEntry<A>;
    type IntoIter = std::vec::IntoIter<HistoryEntry<A>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<'a, A> IntoIterator for &'a History<A> {
    type Item = &'a HistoryEntry<A>;
    type IntoIter = std::slice::Iter<'a, HistoryEntry<A>>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}
