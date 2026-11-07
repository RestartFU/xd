//! Images an assistant sends to the user.
//!
//! A reply shows an image by putting `[image: /path.png]` on a line of its own.
//! The path means nothing to a client: the file is on the host, may be somewhere
//! a client must not read from, and may be gone by the time the chat is reopened.
//! So the host keeps its own copy as the line goes by and writes that copy's path
//! into the reply, which is the one kind of path `image-read` serves. A line it
//! cannot honor becomes text saying so, rather than a marker no client can draw.
//!
//! Text in a code fence is left alone, so a reply can explain the syntax.

use std::path::{Path, PathBuf};

const MARKER_PREFIX: &str = "[image: ";
/// Longer lines are not waited on: a real marker holds one path.
const MAX_MARKER_BYTES: usize = 8 * 1024;
/// Every image is a copy on disk, so a runaway reply must not be able to repeat
/// one without limit.
const MAX_IMAGES_PER_TURN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fence {
    marker: char,
    length: usize,
}

/// Rewrites a reply as it streams. A line that could still turn out to be a
/// marker is held back until it ends, so a client never sees the original path.
#[derive(Debug, Default)]
pub(crate) struct ReplyImages {
    line: String,
    released: usize,
    fence: Option<Fence>,
    kept: usize,
}

impl ReplyImages {
    /// Takes more of the reply and returns what is ready to pass on.
    pub(crate) fn push(
        &mut self,
        text: &str,
        keep: &mut impl FnMut(&str) -> Result<PathBuf, String>,
    ) -> String {
        let mut output = String::with_capacity(text.len());
        for segment in text.split_inclusive('\n') {
            match segment.strip_suffix('\n') {
                Some(rest) => {
                    self.line.push_str(rest);
                    self.end_line(&mut output, keep);
                    output.push('\n');
                }
                None => {
                    self.line.push_str(segment);
                    if !self.may_become_marker() {
                        output.push_str(&self.line[self.released..]);
                        self.released = self.line.len();
                    }
                }
            }
        }
        output
    }

    /// Ends a stored message. Each one is read on its own, so a fence left open
    /// does not reach into the next.
    pub(crate) fn finish(
        &mut self,
        keep: &mut impl FnMut(&str) -> Result<PathBuf, String>,
    ) -> String {
        let mut output = String::new();
        self.end_line(&mut output, keep);
        self.fence = None;
        output
    }

    /// Rewrites a reply that arrived whole.
    pub(crate) fn whole(
        &mut self,
        text: &str,
        keep: &mut impl FnMut(&str) -> Result<PathBuf, String>,
    ) -> String {
        let mut output = self.push(text, keep);
        output.push_str(&self.finish(keep));
        output
    }

    fn may_become_marker(&self) -> bool {
        self.fence.is_none()
            && self.released == 0
            && self.line.len() <= MAX_MARKER_BYTES
            && (self.line.starts_with(MARKER_PREFIX) || MARKER_PREFIX.starts_with(&self.line))
    }

    fn end_line(
        &mut self,
        output: &mut String,
        keep: &mut impl FnMut(&str) -> Result<PathBuf, String>,
    ) {
        let line = std::mem::take(&mut self.line);
        let released = std::mem::take(&mut self.released);
        match self.fence {
            Some(fence) => {
                if closes_fence(&line, fence) {
                    self.fence = None;
                }
            }
            None => self.fence = opens_fence(&line),
        }
        let marker = (released == 0 && self.fence.is_none())
            .then(|| marker_path(&line))
            .flatten();
        match marker {
            Some(path) => output.push_str(&self.keep_image(path, keep)),
            None => output.push_str(&line[released..]),
        }
    }

    fn keep_image(
        &mut self,
        path: &str,
        keep: &mut impl FnMut(&str) -> Result<PathBuf, String>,
    ) -> String {
        let kept = if self.kept < MAX_IMAGES_PER_TURN {
            keep(path)
        } else {
            Err("this reply has too many images".to_owned())
        };
        match kept {
            Ok(copy) => {
                self.kept += 1;
                format!("{MARKER_PREFIX}{}]", copy.display())
            }
            Err(reason) => format!("Image unavailable: {path} ({reason})"),
        }
    }
}

fn marker_path(line: &str) -> Option<&str> {
    line.strip_suffix('\r')
        .unwrap_or(line)
        .strip_prefix(MARKER_PREFIX)?
        .strip_suffix(']')
        .filter(|path| !path.is_empty())
}

fn opens_fence(line: &str) -> Option<Fence> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let length = trimmed.chars().take_while(|c| *c == marker).count();
    (length >= 3).then_some(Fence { marker, length })
}

fn closes_fence(line: &str, fence: Fence) -> bool {
    let trimmed = line.trim();
    trimmed.len() >= fence.length && trimmed.chars().all(|c| c == fence.marker)
}

/// Where a marker's path points. A relative path is the assistant's own, so it
/// starts from where the assistant is working.
pub(crate) fn source_path(workdir: &str, path: &str) -> PathBuf {
    Path::new(workdir).join(path)
}

/// The file extension for an image the clients can draw. SVG is deliberately
/// absent: it is a document, not pixels.
pub(crate) fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    Some(match image_mime(bytes)? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => "webp",
    })
}

pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(path: &str) -> Result<PathBuf, String> {
        match path {
            "/missing.png" => Err("the file was not found".into()),
            other => Ok(PathBuf::from("/store").join(other.trim_start_matches('/'))),
        }
    }

    fn stream(pieces: &[&str]) -> (Vec<String>, String) {
        let mut images = ReplyImages::default();
        let mut keep = kept;
        let mut passed = Vec::new();
        for piece in pieces {
            passed.push(images.push(piece, &mut keep));
        }
        passed.push(images.finish(&mut keep));
        let joined = passed.concat();
        (passed, joined)
    }

    #[test]
    fn a_marker_comes_out_pointing_at_the_kept_copy() {
        let (_, text) = stream(&["Look:\n[image: /shot.png]\nDone."]);
        assert_eq!(text, "Look:\n[image: /store/shot.png]\nDone.");
    }

    #[test]
    fn a_marker_split_across_deltas_never_shows_the_original_path() {
        let (passed, text) = stream(&["Look:\n[ima", "ge: /sh", "ot.png]", "\nDone."]);
        assert_eq!(text, "Look:\n[image: /store/shot.png]\nDone.");
        assert_eq!(passed[0], "Look:\n");
        assert_eq!(passed[1], "");
        assert_eq!(passed[2], "");
        assert!(passed.iter().all(|piece| !piece.contains("[image: /shot")));
    }

    #[test]
    fn prose_is_not_held_back_once_it_cannot_be_a_marker() {
        let (passed, text) = stream(&["[i", "mpossible", " to wait"]);
        assert_eq!(passed[0], "");
        assert_eq!(passed[1], "[impossible");
        assert_eq!(passed[2], " to wait");
        assert_eq!(text, "[impossible to wait");
    }

    #[test]
    fn a_marker_at_the_end_of_a_message_is_still_kept() {
        let (_, text) = stream(&["[image: /shot.png]"]);
        assert_eq!(text, "[image: /store/shot.png]");
    }

    #[test]
    fn a_mention_mid_sentence_stays_prose() {
        for line in [
            "See [image: /shot.png] there",
            "[image: /shot.png] there",
            " [image: /shot.png]",
            "[image: ]",
        ] {
            assert_eq!(stream(&[line]).1, line);
        }
    }

    #[test]
    fn a_fenced_marker_stays_literal_until_the_fence_closes() {
        let source = "```text\n[image: /a.png]\n```\n[image: /b.png]\n";
        assert_eq!(
            stream(&[source]).1,
            "```text\n[image: /a.png]\n```\n[image: /store/b.png]\n"
        );
    }

    #[test]
    fn a_shorter_or_different_fence_does_not_close_a_block() {
        let source = "````\n```\n~~~~\n[image: /a.png]\n````\n";
        assert_eq!(stream(&[source]).1, source);
    }

    #[test]
    fn a_fence_left_open_does_not_reach_the_next_message() {
        let mut images = ReplyImages::default();
        let mut keep = kept;
        assert_eq!(
            images.whole("```\n[image: /a.png]", &mut keep),
            "```\n[image: /a.png]"
        );
        assert_eq!(
            images.whole("[image: /b.png]", &mut keep),
            "[image: /store/b.png]"
        );
    }

    #[test]
    fn a_file_that_cannot_be_kept_becomes_text_saying_so() {
        assert_eq!(
            stream(&["[image: /missing.png]\n"]).1,
            "Image unavailable: /missing.png (the file was not found)\n"
        );
    }

    #[test]
    fn a_line_ending_in_a_carriage_return_is_still_a_marker() {
        assert_eq!(
            stream(&["[image: /a.png]\r\nnext"]).1,
            "[image: /store/a.png]\nnext"
        );
    }

    #[test]
    fn a_runaway_reply_stops_copying() {
        let source = "[image: /a.png]\n".repeat(MAX_IMAGES_PER_TURN + 1);
        let text = stream(&[&source]).1;
        assert_eq!(text.matches("/store/a.png").count(), MAX_IMAGES_PER_TURN);
        assert_eq!(
            text.lines().last(),
            Some("Image unavailable: /a.png (this reply has too many images)")
        );
    }

    #[test]
    fn a_marker_longer_than_any_path_is_released_as_text() {
        let long = format!("[image: /{}", "a".repeat(MAX_MARKER_BYTES));
        let (passed, text) = stream(&[&long, "]"]);
        assert_eq!(passed[0], long);
        assert_eq!(text, format!("{long}]"));
    }

    #[test]
    fn multibyte_text_passes_through_intact() {
        assert_eq!(stream(&["日本", "語\n[", "é"]).1, "日本語\n[é");
    }

    #[test]
    fn a_relative_path_starts_from_the_working_directory() {
        assert_eq!(
            source_path("/work", "out/a.png"),
            PathBuf::from("/work/out/a.png")
        );
        assert_eq!(
            source_path("/work", "/abs/a.png"),
            PathBuf::from("/abs/a.png")
        );
    }

    #[test]
    fn only_formats_the_clients_can_draw_are_recognized() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(image_mime(b"\xff\xd8\xff\xe0"), Some("image/jpeg"));
        assert_eq!(image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_mime(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(
            image_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            None
        );
        assert_eq!(image_mime(b""), None);
        assert_eq!(image_extension(b"GIF87a"), Some("gif"));
    }
}
