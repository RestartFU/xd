use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptWindow {
    pub overscan: usize,
}

impl TranscriptWindow {
    pub const fn new(overscan: usize) -> Self {
        Self { overscan }
    }

    pub fn around(self, total: usize, first_visible: usize, visible_count: usize) -> Range<usize> {
        let start = first_visible.saturating_sub(self.overscan).min(total);
        let end = first_visible
            .saturating_add(visible_count)
            .saturating_add(self.overscan)
            .min(total);
        start..end.max(start)
    }

    pub fn tail(self, total: usize, visible_count: usize) -> Range<usize> {
        let count = visible_count.saturating_add(self.overscan).min(total);
        total - count..total
    }
}

impl Default for TranscriptWindow {
    fn default() -> Self {
        Self::new(24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_a_large_conversation_to_the_viewport_and_overscan() {
        let range = TranscriptWindow::new(20).around(10_000, 5_000, 30);
        assert_eq!(range, 4_980..5_050);
        assert_eq!(range.len(), 70);
    }

    #[test]
    fn follows_the_tail_without_underflow() {
        let window = TranscriptWindow::new(20);
        assert_eq!(window.tail(10_000, 30), 9_950..10_000);
        assert_eq!(window.tail(8, 30), 0..8);
    }
}
