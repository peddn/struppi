//! A bounded buffer of output lines.

use std::collections::VecDeque;

use crate::line::Line;

/// Default number of lines kept.
pub const DEFAULT_CAPACITY: usize = 10_000;

/// A bounded buffer of lines.
///
/// Every line gets a sequence number that never changes, even when older
/// lines are evicted. Views anchor themselves to sequence numbers so that
/// they stay put while new output arrives.
#[derive(Debug)]
pub struct Scrollback {
    lines: VecDeque<Line>,
    capacity: usize,
    /// Sequence number of `lines[0]`.
    first: u64,
}

impl Scrollback {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            capacity: capacity.max(1),
            first: 0,
        }
    }

    pub fn push(&mut self, line: Line) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
            self.first += 1;
        }
        self.lines.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Sequence number of the oldest line still kept.
    pub fn first_seq(&self) -> u64 {
        self.first
    }

    /// Sequence number the next pushed line will get.
    pub fn end_seq(&self) -> u64 {
        self.first + self.lines.len() as u64
    }

    pub fn get(&self, seq: u64) -> Option<&Line> {
        let index = usize::try_from(seq.checked_sub(self.first)?).ok()?;
        self.lines.get(index)
    }

    pub fn last(&self) -> Option<&Line> {
        self.lines.back()
    }
}

impl Default for Scrollback {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::Style;

    fn line(text: &str) -> Line {
        Line::local(text, Style::default())
    }

    #[test]
    fn sequence_numbers_survive_eviction() {
        let mut scrollback = Scrollback::new(3);
        for i in 0..5 {
            scrollback.push(line(&i.to_string()));
        }
        assert_eq!(scrollback.len(), 3);
        assert_eq!(scrollback.first_seq(), 2);
        assert_eq!(scrollback.end_seq(), 5);
        assert!(scrollback.get(1).is_none());
        assert_eq!(scrollback.get(2).unwrap().text, "2");
        assert_eq!(scrollback.get(4).unwrap().text, "4");
        assert!(scrollback.get(5).is_none());
        assert_eq!(scrollback.last().unwrap().text, "4");
    }

    #[test]
    fn zero_capacity_still_keeps_one_line() {
        let mut scrollback = Scrollback::new(0);
        scrollback.push(line("a"));
        scrollback.push(line("b"));
        assert_eq!(scrollback.len(), 1);
        assert_eq!(scrollback.last().unwrap().text, "b");
    }
}
