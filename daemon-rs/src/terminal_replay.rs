use std::collections::VecDeque;

use xd_terminal::TerminalScreen;

pub(crate) const HISTORY_LIMIT: usize = 16 * 1024 * 1024;
pub(crate) const REPLAY_ITEM_LIMIT: usize = 65_536;
// A terminal may retain substantially more history in the daemon than should
// be sent in one protocol reply. Requests and keystrokes share a connection;
// multi-megabyte terminal-list replies otherwise make a remote CLI appear
// frozen until all of its scrollback has crossed the network.
const TRANSFER_LIMIT: usize = 1024 * 1024;
const TRANSFER_ITEM_LIMIT: usize = 4_096;

pub(crate) fn pasted_text_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReplayFrame {
    Output(Vec<u8>),
    Resize { columns: u16, rows: u16 },
    Checkpoint { exact: Vec<u8>, fallback: Vec<u8> },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RecordOutcome {
    Accepted(u64),
    Unchanged,
    Closing,
}

pub(crate) struct TerminalState {
    pub(crate) columns: u16,
    pub(crate) rows: u16,
    pub(crate) replay: VecDeque<ReplayFrame>,
    pub(crate) replay_bytes: usize,
    pub(crate) sequence: u64,
    pub(crate) working: bool,
    pub(crate) closing: bool,
    screen: TerminalScreen,
}

impl TerminalState {
    pub(crate) fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns,
            rows,
            replay: VecDeque::from([ReplayFrame::Resize { columns, rows }]),
            replay_bytes: 0,
            sequence: 0,
            working: false,
            closing: false,
            screen: TerminalScreen::new(usize::from(columns), usize::from(rows)),
        }
    }

    pub(crate) fn bracketed_paste(&self) -> bool {
        self.screen.bracketed_paste()
    }

    pub(crate) fn compact_for_transfer(&mut self) {
        if self.replay_bytes > TRANSFER_LIMIT || self.replay.len() > TRANSFER_ITEM_LIMIT {
            // A pathological viewport can itself exceed the transfer budget.
            // In that case preserve the reconstructable replay; ordinary CLI
            // geometries compact to a checkpoint plus a small ANSI fallback.
            let _ = self.compact_replay(TRANSFER_LIMIT, TRANSFER_ITEM_LIMIT);
        }
    }

    fn compact_replay(&mut self, byte_limit: usize, item_limit: usize) -> bool {
        if item_limit == 0 {
            return false;
        }

        // New clients restore the exact emulator state. Older clients ignore
        // that field and consume the safe ANSI fallback. Neither path ever
        // begins halfway through UTF-8 or an escape sequence.
        let delta_reserve = byte_limit / 4;
        let snapshot_budget = byte_limit.saturating_sub(delta_reserve) / 4;
        let Some(fallback) = self.screen.ansi_snapshot_bounded(snapshot_budget) else {
            return false;
        };
        let exact_limit = byte_limit
            .saturating_sub(delta_reserve)
            .saturating_sub(fallback.len());
        let Some(exact) = self.screen.checkpoint_bytes_bounded(exact_limit) else {
            // The raw replay is still reconstructable. Exceeding the soft
            // memory target is safer than replacing it with a blank or partial
            // snapshot for a pathological but valid terminal state.
            return false;
        };

        self.replay.clear();
        self.replay_bytes = exact.len() + fallback.len();
        if item_limit > 1 {
            self.replay.push_back(ReplayFrame::Resize {
                columns: self.columns,
                rows: self.rows,
            });
        }
        self.replay
            .push_back(ReplayFrame::Checkpoint { exact, fallback });
        true
    }

    fn output_fits(&self, bytes: usize, byte_limit: usize, item_limit: usize) -> bool {
        item_limit > 0
            && self.replay.len() < item_limit
            && bytes <= byte_limit.saturating_sub(self.replay_bytes)
    }

    fn item_fits(&self, item_limit: usize) -> bool {
        item_limit > 0 && self.replay.len() < item_limit
    }

    pub(crate) fn record_output_bounded(
        &mut self,
        data: Vec<u8>,
        byte_limit: usize,
        item_limit: usize,
    ) -> RecordOutcome {
        if self.closing {
            return RecordOutcome::Closing;
        }

        self.screen.feed(&data);
        if self.output_fits(data.len(), byte_limit, item_limit) {
            self.replay_bytes = self.replay_bytes.saturating_add(data.len());
            self.replay.push_back(ReplayFrame::Output(data));
        } else if !self.compact_replay(byte_limit, item_limit) {
            self.replay_bytes = self.replay_bytes.saturating_add(data.len());
            self.replay.push_back(ReplayFrame::Output(data));
        }
        self.sequence = self.sequence.saturating_add(1);
        RecordOutcome::Accepted(self.sequence)
    }

    pub(crate) fn record_resize_bounded(
        &mut self,
        columns: u16,
        rows: u16,
        item_limit: usize,
    ) -> RecordOutcome {
        if self.closing {
            return RecordOutcome::Closing;
        }
        if self.columns == columns && self.rows == rows {
            return RecordOutcome::Unchanged;
        }

        self.columns = columns;
        self.rows = rows;
        self.screen.resize(usize::from(columns), usize::from(rows));
        if self.item_fits(item_limit) {
            self.replay.push_back(ReplayFrame::Resize { columns, rows });
        } else if !self.compact_replay(HISTORY_LIMIT, item_limit) {
            self.replay.push_back(ReplayFrame::Resize { columns, rows });
        }
        self.sequence = self.sequence.saturating_add(1);
        RecordOutcome::Accepted(self.sequence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_text_uses_the_sessions_bracketed_paste_mode() {
        assert_eq!(
            pasted_text_bytes("/tmp/image.png", false),
            b"/tmp/image.png"
        );
        assert_eq!(
            pasted_text_bytes("/tmp/image.png", true),
            b"\x1b[200~/tmp/image.png\x1b[201~"
        );
    }

    #[test]
    fn compaction_preserves_an_exact_reconstructable_checkpoint() {
        let mut state = TerminalState::new(30, 4);
        assert_eq!(
            state.record_output_bounded(b"\x1b[31m\x1b[?20".to_vec(), 16_384, 1),
            RecordOutcome::Accepted(1)
        );
        assert_eq!(
            state.record_resize_bounded(32, 5, 1),
            RecordOutcome::Accepted(2)
        );
        assert_eq!(
            state.record_output_bounded(b"04hprompt> ".to_vec(), 16_384, 1),
            RecordOutcome::Accepted(3)
        );

        assert!(state.replay_bytes <= 12_288);
        assert_eq!(state.replay.len(), 1);
        assert_eq!(state.sequence, 3);
        assert!(!state.closing);

        let checkpoint = match state.replay.front() {
            Some(ReplayFrame::Checkpoint {
                exact: checkpoint, ..
            }) => checkpoint,
            _ => panic!("compacted terminal checkpoint"),
        };
        let mut rebuilt =
            TerminalScreen::from_checkpoint_bytes(&checkpoint).expect("checkpoint should decode");
        assert_eq!(rebuilt.geometry(), (32, 5));
        assert_eq!(
            rebuilt.rendered_with_cursor(),
            state.screen.rendered_with_cursor()
        );
        assert!(rebuilt.bracketed_paste());

        let continuation = b"\x1b[0m!";
        rebuilt.feed(continuation);
        state.screen.feed(continuation);
        assert_eq!(
            rebuilt.rendered_with_cursor(),
            state.screen.rendered_with_cursor()
        );
    }

    #[test]
    fn largest_supported_terminal_state_compacts_with_delta_headroom() {
        let mut state = TerminalState::new(500, 200);
        for index in 0..4_096 {
            let prefix = format!("https://example.com/{index:04}/");
            let url = format!("{prefix}{}", "x".repeat(256 - prefix.len()));
            state.screen.feed(format!("\x1b]8;;{url}\x1b\\").as_bytes());
        }
        state.screen.feed(b"\x1b[1;38;2;1;2;3;48;2;4;5;6m");
        let line = "\u{10348}\u{e0100}\u{e0100}".repeat(500);
        let fill = |screen: &mut TerminalScreen| {
            for row in 0..200 {
                screen.feed(line.as_bytes());
                if row + 1 < 200 {
                    screen.feed(b"\r\n");
                }
            }
        };
        fill(&mut state.screen);
        state
            .screen
            .feed(b"\x1b[?1049h\x1b[1;38;2;1;2;3;48;2;4;5;6m");
        fill(&mut state.screen);

        assert!(state.compact_replay(HISTORY_LIMIT, REPLAY_ITEM_LIMIT));
        assert!(state.replay_bytes <= HISTORY_LIMIT - HISTORY_LIMIT / 4);
        assert!(matches!(
            state.replay.back(),
            Some(ReplayFrame::Checkpoint { .. })
        ));
    }

    #[test]
    fn output_and_resize_sequences_remain_monotonic() {
        let mut state = TerminalState::new(80, 24);
        assert_eq!(
            state.record_output_bounded(b"one".to_vec(), 64, 8),
            RecordOutcome::Accepted(1)
        );
        assert_eq!(
            state.record_resize_bounded(100, 30, 8),
            RecordOutcome::Accepted(2)
        );
        assert_eq!(
            state.record_output_bounded(b"two".to_vec(), 64, 8),
            RecordOutcome::Accepted(3)
        );
        assert_eq!(state.sequence, 3);
    }

    #[test]
    fn closing_state_rejects_new_replay_without_changing_sequence() {
        let mut state = TerminalState::new(80, 24);
        state.closing = true;
        assert_eq!(
            state.record_output_bounded(b"ignored".to_vec(), 64, 8),
            RecordOutcome::Closing
        );
        assert_eq!(state.sequence, 0);
    }

    #[test]
    fn transfer_compaction_bounds_busy_cli_replies() {
        let mut state = TerminalState::new(120, 32);
        for _ in 0..2_000 {
            assert!(matches!(
                state.record_output_bounded(vec![b'x'; 1024], HISTORY_LIMIT, REPLAY_ITEM_LIMIT),
                RecordOutcome::Accepted(_)
            ));
        }
        assert!(state.replay_bytes > TRANSFER_LIMIT);

        state.compact_for_transfer();

        assert!(state.replay_bytes <= TRANSFER_LIMIT);
        assert!(state.replay.len() <= TRANSFER_ITEM_LIMIT);
        assert!(matches!(
            state.replay.back(),
            Some(ReplayFrame::Checkpoint { .. })
        ));
    }
}
