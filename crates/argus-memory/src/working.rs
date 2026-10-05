//! Working memory: the bounded, in-process evidence context of one
//! investigation (ADR-0038 §1). Not persisted long-term.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};

/// One item of evidence under an investigator's working attention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingItem {
    pub text: String,
    pub recorded_at: DateTime<Utc>,
}

/// A bounded FIFO of evidence items. When `capacity` is exceeded the oldest
/// item is dropped: working memory is attention, not an archive. The default
/// capacity is 64 items.
#[derive(Debug)]
pub struct WorkingMemory {
    items: VecDeque<WorkingItem>,
    capacity: usize,
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

/// The default working-memory capacity.
pub const DEFAULT_CAPACITY: usize = 64;

impl WorkingMemory {
    pub fn new(capacity: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// Push one evidence item, evicting the oldest beyond capacity.
    pub fn push(&mut self, text: impl Into<String>, at: DateTime<Utc>) {
        if self.items.len() == self.capacity {
            self.items.pop_front();
        }
        self.items.push_back(WorkingItem {
            text: text.into(),
            recorded_at: at,
        });
    }

    /// The current items, oldest first.
    pub fn snapshot(&self) -> Vec<&WorkingItem> {
        self.items.iter().collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Clear the context (end of an investigation).
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 5, 12, minute, 0)
            .unwrap()
    }

    #[test]
    fn capacity_bounds_and_evicts_oldest() {
        let mut w = WorkingMemory::new(3);
        for m in 0..5 {
            w.push(format!("evidence-{m}"), at(m));
        }
        assert_eq!(w.len(), 3);
        let snap = w.snapshot();
        assert_eq!(snap[0].text, "evidence-2");
        assert_eq!(snap[2].text, "evidence-4");
    }

    #[test]
    fn clear_empties_the_context() {
        let mut w = WorkingMemory::new(3);
        w.push("x", at(0));
        w.clear();
        assert!(w.is_empty());
    }
}
