//! Undo and redo for the desktop's own text boxes.
//!
//! Each step is a whole snapshot of the text and its selection, taken just
//! before an edit. Typing a word, or deleting one character after another,
//! makes a single step, the way other editors group them.

use std::ops::Range;

use gpui::SharedString;

/// The most steps one box keeps.
const MAX_STEPS: usize = 500;
/// The most text all of one box's snapshots may hold between them. A large
/// file keeps fewer steps rather than a copy of itself for every word typed.
const MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub text: SharedString,
    pub selection: Range<usize>,
    pub reversed: bool,
}

/// What an edit did, for grouping it with the edits around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    /// One character typed at the caret.
    Insert,
    /// Characters deleted next to the caret with Backspace or Delete.
    Delete,
    /// Anything else: pasting, cutting, a newline, replacing a selection.
    Other,
}

#[derive(Clone, Debug)]
struct Group {
    kind: EditKind,
    /// Where the caret was left by the group's latest edit.
    caret: usize,
    /// Whether the group's latest edit typed whitespace.
    ended_in_space: bool,
}

#[derive(Debug, Default)]
pub(crate) struct EditHistory {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<Group>,
}

impl EditHistory {
    /// Notes an edit about to replace `range` of `before.text` with
    /// `inserted`. An edit that carries on from the one before it joins that
    /// step instead of starting its own.
    pub fn record(
        &mut self,
        before: Snapshot,
        kind: EditKind,
        range: Range<usize>,
        inserted: &str,
    ) {
        let starts_with_space = inserted.starts_with(char::is_whitespace);
        let continues = self.group.as_ref().is_some_and(|group| {
            group.kind == kind
                && match kind {
                    // A word starts a new step once a space ends the last one.
                    EditKind::Insert => {
                        range.start == group.caret
                            && range.is_empty()
                            && (group.ended_in_space || !starts_with_space)
                    }
                    EditKind::Delete => range.end == group.caret || range.start == group.caret,
                    EditKind::Other => false,
                }
        });
        self.redo.clear();
        if !continues {
            self.push_undo(before);
        }
        self.group = (kind != EditKind::Other).then(|| Group {
            kind,
            caret: range.start + inserted.len(),
            ended_in_space: inserted.ends_with(char::is_whitespace),
        });
    }

    /// Ends the current step, so the next edit starts one of its own even if
    /// it carries on from the caret where the last one left it.
    pub fn break_group(&mut self) {
        self.group = None;
    }

    /// The state to go back to, given the state shown now.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.group = None;
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    /// The state an undo left, given the state shown now.
    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        self.group = None;
        let next = self.redo.pop()?;
        self.push_undo(current);
        Some(next)
    }

    /// Forgets every step, for a box given a different text to edit.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.group = None;
    }

    fn push_undo(&mut self, snapshot: Snapshot) {
        self.undo.push(snapshot);
        let mut bytes: usize = self
            .undo
            .iter()
            .chain(&self.redo)
            .map(|snapshot| snapshot.text.len())
            .sum();
        let mut drop = 0;
        while self.undo.len() - drop > 1
            && (self.undo.len() - drop > MAX_STEPS || bytes > MAX_BYTES)
        {
            bytes -= self.undo[drop].text.len();
            drop += 1;
        }
        self.undo.drain(..drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, caret: usize) -> Snapshot {
        Snapshot {
            text: text.to_owned().into(),
            selection: caret..caret,
            reversed: false,
        }
    }

    /// Types `text` one character at a time after `start`, recording each.
    fn type_text(history: &mut EditHistory, start: &str, text: &str) -> String {
        let mut content = start.to_owned();
        for character in text.chars() {
            let caret = content.len();
            history.record(
                at(&content, caret),
                EditKind::Insert,
                caret..caret,
                &character.to_string(),
            );
            content.push(character);
        }
        content
    }

    #[test]
    fn typed_words_undo_one_word_at_a_time() {
        let mut history = EditHistory::default();
        let text = type_text(&mut history, "", "hello world");
        let step = history.undo(at(&text, text.len())).unwrap();
        assert_eq!(step, at("hello", 5));
        let step = history.undo(step).unwrap();
        assert_eq!(step, at("", 0));
        assert!(history.undo(step).is_none());
    }

    #[test]
    fn redo_walks_back_to_the_latest_text() {
        let mut history = EditHistory::default();
        let text = type_text(&mut history, "", "one two");
        let first = history.undo(at(&text, text.len())).unwrap();
        let second = history.undo(first.clone()).unwrap();
        let again = history.redo(second).unwrap();
        assert_eq!(again, first);
        assert_eq!(history.redo(again).unwrap(), at("one two", 7));
    }

    #[test]
    fn a_new_edit_drops_what_could_be_redone() {
        let mut history = EditHistory::default();
        let text = type_text(&mut history, "", "abc");
        let back = history.undo(at(&text, 3)).unwrap();
        history.record(back, EditKind::Other, 0..0, "x");
        assert!(history.redo(at("x", 1)).is_none());
    }

    #[test]
    fn consecutive_backspaces_make_one_step() {
        let mut history = EditHistory::default();
        history.record(at("abcd", 4), EditKind::Delete, 3..4, "");
        history.record(at("abc", 3), EditKind::Delete, 2..3, "");
        history.record(at("ab", 2), EditKind::Delete, 1..2, "");
        assert_eq!(history.undo(at("a", 1)).unwrap(), at("abcd", 4));
    }

    #[test]
    fn typing_somewhere_else_starts_a_new_step() {
        let mut history = EditHistory::default();
        let text = type_text(&mut history, "", "ab");
        history.record(at(&text, 0), EditKind::Insert, 0..0, "x");
        assert_eq!(history.undo(at("xab", 1)).unwrap(), at("ab", 0));
    }

    #[test]
    fn pastes_never_join_the_typing_around_them() {
        let mut history = EditHistory::default();
        let text = type_text(&mut history, "", "ab");
        history.record(at(&text, 2), EditKind::Other, 2..2, "pasted");
        let after = type_text(&mut history, "abpasted", "c");
        let step = history.undo(at(&after, after.len())).unwrap();
        assert_eq!(step, at("abpasted", 8));
        assert_eq!(history.undo(step).unwrap(), at("ab", 2));
    }

    #[test]
    fn a_large_text_keeps_fewer_steps_but_always_the_last() {
        let mut history = EditHistory::default();
        let large = "x".repeat(MAX_BYTES);
        history.record(at(&large, 0), EditKind::Other, 0..0, "a");
        history.record(at(&large, 0), EditKind::Other, 0..0, "b");
        assert_eq!(history.undo.len(), 1);
        assert!(history.undo(at("", 0)).is_some());
    }
}
