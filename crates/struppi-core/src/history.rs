//! Input history with shell-like navigation.

use std::collections::VecDeque;

/// Default number of entries kept.
pub const DEFAULT_CAPACITY: usize = 1000;

/// Previously entered lines.
///
/// While navigating, the text that was being typed is kept as a draft and
/// comes back when moving past the newest entry.
#[derive(Debug)]
pub struct History {
    entries: VecDeque<String>,
    capacity: usize,
    cursor: Option<usize>,
    draft: String,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: capacity.max(1),
            cursor: None,
            draft: String::new(),
        }
    }

    /// Records an entered line and ends any navigation. Empty lines and
    /// immediate repetitions are not recorded.
    pub fn push(&mut self, line: &str) {
        self.reset();
        if line.is_empty() || self.entries.back().is_some_and(|last| last == line) {
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(line.to_owned());
    }

    /// Moves to the next older entry. `current` is the text in the input
    /// line, saved as the draft when navigation starts. Returns `None` when
    /// there is nothing older.
    pub fn older(&mut self, current: &str) -> Option<&str> {
        let index = match self.cursor {
            None => {
                let index = self.entries.len().checked_sub(1)?;
                self.draft = current.to_owned();
                index
            }
            Some(index) => index.checked_sub(1)?,
        };
        self.cursor = Some(index);
        self.entries.get(index).map(String::as_str)
    }

    /// Moves to the next newer entry, or back to the draft. Returns `None`
    /// when not navigating.
    pub fn newer(&mut self) -> Option<&str> {
        let index = self.cursor? + 1;
        if index < self.entries.len() {
            self.cursor = Some(index);
            self.entries.get(index).map(String::as_str)
        } else {
            self.cursor = None;
            Some(self.draft.as_str())
        }
    }

    /// Ends navigation, for example because the line was edited and sent.
    pub fn reset(&mut self) {
        self.cursor = None;
        self.draft.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for History {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_walks_back_and_returns_to_the_draft() {
        let mut history = History::default();
        history.push("north");
        history.push("look");

        assert_eq!(history.older("dra"), Some("look"));
        assert_eq!(history.older("look"), Some("north"));
        assert_eq!(history.older("north"), None, "stays on the oldest entry");
        assert_eq!(history.newer(), Some("look"));
        assert_eq!(history.newer(), Some("dra"));
        assert_eq!(history.newer(), None);
    }

    #[test]
    fn empty_history_has_nothing() {
        let mut history = History::default();
        assert_eq!(history.older("x"), None);
        assert_eq!(history.newer(), None);
    }

    #[test]
    fn empty_lines_and_repeats_are_skipped() {
        let mut history = History::default();
        history.push("look");
        history.push("look");
        history.push("");
        assert_eq!(history.len(), 1);
        history.push("north");
        history.push("look");
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn capacity_evicts_the_oldest() {
        let mut history = History::new(2);
        history.push("a");
        history.push("b");
        history.push("c");
        assert_eq!(history.older(""), Some("c"));
        assert_eq!(history.older(""), Some("b"));
        assert_eq!(history.older(""), None);
    }

    #[test]
    fn push_ends_navigation() {
        let mut history = History::default();
        history.push("a");
        history.older("draft");
        history.push("b");
        assert_eq!(history.newer(), None);
        assert_eq!(history.older(""), Some("b"));
    }
}
