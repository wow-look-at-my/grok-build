//! The event ids of the agent text chunks this view applied.
//! The scalar dedup highwater cannot tell a duplicate from a chunk that arrived late. This set can.

use std::collections::BTreeSet;

/// The most ids the set keeps. An id below the window is unknown, and the caller treats it as a duplicate.
const WINDOW: usize = 4096;

#[derive(Debug, Clone, Default)]
pub struct AppliedTextSeqs {
    seqs: BTreeSet<u64>,
}

/// What the set knows about one event id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSeqStatus {
    /// The view applied this id before.
    Applied,
    /// The id is in the window and the view never applied it: a late chunk.
    Unseen,
    /// The id is below the window, so the set cannot tell.
    BelowWindow,
}

impl AppliedTextSeqs {
    pub fn status(&self, seq: u64) -> TextSeqStatus {
        if self.seqs.contains(&seq) {
            TextSeqStatus::Applied
        } else if self.seqs.len() >= WINDOW && self.seqs.first().is_some_and(|&low| seq < low) {
            TextSeqStatus::BelowWindow
        } else {
            TextSeqStatus::Unseen
        }
    }

    pub fn insert(&mut self, seq: u64) {
        self.seqs.insert(seq);
        while self.seqs.len() > WINDOW {
            self.seqs.pop_first();
        }
    }

    pub fn clear(&mut self) {
        self.seqs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_chunk_is_unseen_and_duplicate_is_applied() {
        let mut set = AppliedTextSeqs::default();
        set.insert(5);
        set.insert(9);
        assert_eq!(set.status(9), TextSeqStatus::Applied);
        assert_eq!(set.status(7), TextSeqStatus::Unseen);
        assert_eq!(set.status(3), TextSeqStatus::Unseen);
    }

    #[test]
    fn window_evicts_oldest_and_reports_below_window() {
        let mut set = AppliedTextSeqs::default();
        for seq in 100..100 + WINDOW as u64 + 1 {
            set.insert(seq);
        }
        assert_eq!(set.status(100), TextSeqStatus::BelowWindow);
        assert_eq!(set.status(101), TextSeqStatus::Applied);
    }
}
