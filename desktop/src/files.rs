//! The working directory as a folding tree, and the files opened out of it.
//!
//! The twin of `mobile/shared/.../files/FileTree.kt` and `OpenFiles.kt`. Both
//! clients answer the same questions -- what order entries go in, what closing
//! a folder remembers, where closing a tab lands -- and answering them
//! differently would be two file trees that disagree about the same repository.
//! No GPUI here, for the same reason there is no Compose over there: this is
//! the half worth testing.

use std::collections::{HashMap, HashSet};

use crate::BrowseEntry;

/// One line of the drawn tree: an entry, and how deep it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Relative to the chat's working directory, which is what the daemon takes.
    pub path: String,
    pub name: String,
    pub directory: bool,
    pub depth: usize,
    /// Open, so its children are the rows below this one.
    pub expanded: bool,
    /// Its listing is in flight.
    pub loading: bool,
}

/// A directory tree that folds, over a daemon that lists one directory at a time.
///
/// Children arrive per directory and are kept; what is drawn is the walk from
/// the root through whichever directories are open. Folding is then free, and
/// reopening a directory costs nothing.
///
/// Paths are relative and never begin with a slash -- the root is `""` -- since
/// that is the only shape the daemon accepts.
#[derive(Clone, Debug, Default)]
pub struct FileTree {
    children: HashMap<String, Vec<BrowseEntry>>,
    expanded: HashSet<String>,
    loading: HashSet<String>,
    failed: HashSet<String>,
}

impl FileTree {
    /// The tree flattened for a list, parents before their children.
    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        self.walk("", 0, &mut rows);
        rows
    }

    /// Whether this directory's listing has been fetched.
    pub fn is_loaded(&self, path: &str) -> bool {
        self.children.contains_key(path)
    }

    pub fn is_loading(&self, path: &str) -> bool {
        self.loading.contains(path)
    }

    pub fn has_failed(&self, path: &str) -> bool {
        self.failed.contains(path)
    }

    /// Listings that were in flight when a connection disappeared. Reissuing
    /// these requests keeps expanded folders from being stranded on a spinner.
    pub fn loading_paths(&self) -> Vec<String> {
        let mut paths = self.loading.iter().cloned().collect::<Vec<_>>();
        paths.sort_by(|left, right| {
            left.matches('/')
                .count()
                .cmp(&right.matches('/').count())
                .then_with(|| left.cmp(right))
        });
        paths
    }

    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    /// Restores the folders that were open the last time this chat was shown.
    pub fn set_expanded(&mut self, paths: impl IntoIterator<Item = String>) {
        self.expanded = paths.into_iter().filter(|path| !path.is_empty()).collect();
    }

    /// A stable snapshot suitable for settings persistence.
    pub fn expanded_paths(&self) -> Vec<String> {
        let mut paths = self.expanded.iter().cloned().collect::<Vec<_>>();
        paths.sort();
        paths
    }

    /// Restored descendants are fetched as soon as their parent arrives.
    pub fn expanded_unloaded_children(&self, path: &str) -> Vec<String> {
        self.children
            .get(path)
            .into_iter()
            .flatten()
            .filter(|entry| entry.directory)
            .map(|entry| {
                if path.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{path}/{}", entry.name)
                }
            })
            .filter(|child| {
                self.expanded.contains(child)
                    && !self.children.contains_key(child)
                    && !self.loading.contains(child)
            })
            .collect()
    }

    /// Opens or closes a directory, keeping what was open inside it: a folder
    /// reopened should look the way it was left, not collapsed to one level.
    pub fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_owned());
        }
    }

    /// Marks a directory as being listed, which draws its spinner.
    pub fn set_loading(&mut self, path: &str) {
        self.loading.insert(path.to_owned());
        self.failed.remove(path);
    }

    /// Takes a directory's entries, sorted the way a tree reads.
    pub fn set_children(&mut self, path: &str, mut entries: Vec<BrowseEntry>) {
        // Directories first, then by name: the order every file tree uses.
        entries.sort_by(|left, right| {
            right
                .directory
                .cmp(&left.directory)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        self.children.insert(path.to_owned(), entries);
        self.loading.remove(path);
        self.failed.remove(path);
    }

    /// Gives up on a listing, leaving the directory open and empty.
    pub fn set_failed(&mut self, path: &str) {
        self.loading.remove(path);
        self.failed.insert(path.to_owned());
    }

    /// Forgets every listing, keeping what was open.
    ///
    /// A refresh has to re-fetch rather than redraw: the agent edits files, and
    /// a tree that trusted its cache would go on showing what is no longer there.
    pub fn invalidate(&mut self) {
        self.children.clear();
        self.loading.clear();
        self.failed.clear();
    }

    /// Opens every directory on the way to `path`, so a file can be revealed
    /// without clicking down to it.
    pub fn reveal(&mut self, path: &str) {
        for (at, _) in path.match_indices('/') {
            self.expanded.insert(path[..at].to_owned());
        }
    }

    fn walk(&self, path: &str, depth: usize, rows: &mut Vec<TreeRow>) {
        let Some(entries) = self.children.get(path) else {
            return;
        };
        for entry in entries {
            let child = if path.is_empty() {
                entry.name.clone()
            } else {
                format!("{path}/{}", entry.name)
            };
            let expanded = entry.directory && self.expanded.contains(&child);
            rows.push(TreeRow {
                name: entry.name.clone(),
                directory: entry.directory,
                depth,
                expanded,
                loading: self.loading.contains(&child),
                path: child.clone(),
            });
            if expanded {
                self.walk(&child, depth + 1, rows);
            }
        }
    }
}

/// One file open in a tab.
#[derive(Clone, Debug)]
pub struct OpenFile {
    /// Relative to the chat's working directory.
    pub path: String,
    /// What the daemon last handed over, and what a write is checked against.
    pub saved: String,
    /// What is on screen, which is `saved` until it is typed in.
    pub text: String,
    pub saving: bool,
    pub error: Option<String>,
}

impl OpenFile {
    /// Whether there is anything to save.
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    /// The tab's label: the file's own name, not the path it took to get here.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// What the main area is showing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FileTab {
    #[default]
    Chat,
    /// The tree itself, which is a tab here rather than a dock: the window
    /// already has a sidebar of chats and a panel of diffs, and a third region
    /// competing for the same width helps nobody.
    Tree,
    File(String),
}

/// The files open beside the chat, and which one is in front.
///
/// The chat is not in this list. It is always there and cannot be closed, so it
/// is [`FileTab::Chat`] rather than an entry every caller must remember not to
/// close.
#[derive(Clone, Debug, Default)]
pub struct OpenFiles {
    pub files: Vec<OpenFile>,
    pub active: FileTab,
}

impl OpenFiles {
    /// Whichever file is in front, or `None` when the chat is.
    pub fn current(&self) -> Option<&OpenFile> {
        let FileTab::File(path) = &self.active else {
            return None;
        };
        self.files.iter().find(|file| &file.path == path)
    }

    pub fn any_dirty(&self) -> bool {
        self.files.iter().any(OpenFile::dirty)
    }

    /// Brings a file to the front, opening it if it is not open yet.
    ///
    /// Opening the same file twice is the same tab: two tabs on one path would
    /// be two sets of edits to reconcile, and no editor offers that.
    pub fn open(&mut self, path: &str, saved: String) {
        if !self.files.iter().any(|file| file.path == path) {
            self.files.push(OpenFile {
                path: path.to_owned(),
                text: saved.clone(),
                saved,
                saving: false,
                error: None,
            });
        }
        self.active = FileTab::File(path.to_owned());
    }

    /// Closes a tab, landing on the one to its left -- or the chat, which is
    /// always there to land on.
    pub fn close(&mut self, path: &str) {
        let Some(at) = self.files.iter().position(|file| file.path == path) else {
            return;
        };
        self.files.remove(at);
        if self.active != FileTab::File(path.to_owned()) {
            return;
        }
        self.active = match self.files.get(at.saturating_sub(1)) {
            Some(file) => FileTab::File(file.path.clone()),
            None => FileTab::Chat,
        };
    }

    /// Takes a keystroke.
    pub fn edit(&mut self, path: &str, text: String) {
        if let Some(file) = self.file_mut(path) {
            file.text = text;
            file.error = None;
        }
    }

    pub fn set_saving(&mut self, path: &str) {
        if let Some(file) = self.file_mut(path) {
            file.saving = true;
            file.error = None;
        }
    }

    /// Accepts a write.
    ///
    /// The saved text is what was *sent*, not what is on screen now: typing
    /// carries on during the round trip, and taking the current text would mark
    /// those later keystrokes as already written.
    pub fn set_saved(&mut self, path: &str, written: String) {
        if let Some(file) = self.file_mut(path) {
            file.saved = written;
            file.saving = false;
            file.error = None;
        }
    }

    pub fn set_failed(&mut self, path: &str, message: String) {
        if let Some(file) = self.file_mut(path) {
            file.saving = false;
            file.error = Some(message);
        }
    }

    fn file_mut(&mut self, path: &str) -> Option<&mut OpenFile> {
        self.files.iter_mut().find(|file| file.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> BrowseEntry {
        BrowseEntry {
            name: name.into(),
            directory: true,
        }
    }

    fn file(name: &str) -> BrowseEntry {
        BrowseEntry {
            name: name.into(),
            directory: false,
        }
    }

    fn drawn(tree: &FileTree) -> Vec<String> {
        tree.rows()
            .into_iter()
            .map(|row| {
                format!(
                    "{}{}{}",
                    "  ".repeat(row.depth),
                    row.name,
                    if row.directory { "/" } else { "" }
                )
            })
            .collect()
    }

    #[test]
    fn directories_sort_before_files_and_case_does_not_reorder_them() {
        let mut tree = FileTree::default();
        tree.set_children(
            "",
            vec![
                file("zebra.txt"),
                dir("Widgets"),
                file("Alpha.md"),
                dir("apps"),
            ],
        );
        assert_eq!(drawn(&tree), ["apps/", "Widgets/", "Alpha.md", "zebra.txt"]);
    }

    #[test]
    fn opening_a_directory_puts_its_children_underneath_it() {
        let mut tree = FileTree::default();
        tree.set_children("", vec![dir("src"), file("README.md")]);
        tree.toggle("src");
        tree.set_children("src", vec![dir("voice"), file("main.rs")]);
        tree.toggle("src/voice");
        tree.set_children("src/voice", vec![file("wav.rs")]);

        assert_eq!(
            drawn(&tree),
            ["src/", "  voice/", "    wav.rs", "  main.rs", "README.md"]
        );
        // Paths stay relative and never lead with a slash: the daemon takes
        // exactly these.
        assert_eq!(
            tree.rows()
                .into_iter()
                .map(|row| row.path)
                .collect::<Vec<_>>(),
            [
                "src",
                "src/voice",
                "src/voice/wav.rs",
                "src/main.rs",
                "README.md"
            ]
        );
    }

    #[test]
    fn closing_a_directory_remembers_what_was_open_inside_it() {
        let mut tree = FileTree::default();
        tree.set_children("", vec![dir("src")]);
        tree.toggle("src");
        tree.set_children("src", vec![dir("voice")]);
        tree.toggle("src/voice");
        tree.set_children("src/voice", vec![file("wav.rs")]);

        tree.toggle("src");
        assert_eq!(drawn(&tree), ["src/"]);

        // Reopening shows it as it was left, and costs no round trip.
        tree.toggle("src");
        assert_eq!(drawn(&tree), ["src/", "  voice/", "    wav.rs"]);
        assert!(tree.is_loaded("src/voice"));
    }

    #[test]
    fn a_directory_being_listed_says_so_and_stops_when_it_lands() {
        let mut tree = FileTree::default();
        tree.set_children("", vec![dir("src")]);
        tree.toggle("src");
        tree.set_loading("src");
        assert!(tree.rows()[0].loading);
        assert!(!tree.is_loaded("src"));

        tree.set_children("src", vec![file("main.rs")]);
        assert!(!tree.rows()[0].loading);
        assert_eq!(drawn(&tree), ["src/", "  main.rs"]);
    }

    #[test]
    fn reconnect_can_retry_every_listing_that_was_in_flight() {
        let mut tree = FileTree::default();
        tree.set_loading("src/voice");
        tree.set_loading("");
        tree.set_loading("src");

        assert_eq!(tree.loading_paths(), ["", "src", "src/voice"]);
    }

    #[test]
    fn a_listing_that_failed_leaves_the_directory_open_and_empty() {
        let mut tree = FileTree::default();
        tree.set_children("", vec![dir("secret")]);
        tree.toggle("secret");
        tree.set_loading("secret");
        tree.set_failed("secret");

        let rows = tree.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].expanded);
        assert!(!rows[0].loading);
        assert!(tree.has_failed("secret"));

        tree.set_loading("secret");
        assert!(!tree.has_failed("secret"));
        assert!(tree.is_loading("secret"));

        tree.set_children("secret", vec![file("visible.txt")]);
        assert!(!tree.has_failed("secret"));
        assert!(!tree.is_loading("secret"));
    }

    #[test]
    fn refreshing_forgets_listings_but_not_what_was_open() {
        let mut tree = FileTree::default();
        tree.set_children("", vec![dir("src")]);
        tree.toggle("src");
        tree.set_children("src", vec![file("gone.rs")]);

        tree.invalidate();
        assert!(!tree.is_loaded("src"));
        assert!(tree.is_expanded("src"));
        assert!(tree.rows().is_empty());
    }

    #[test]
    fn restored_expansion_state_fetches_each_visible_level() {
        let mut tree = FileTree::default();
        tree.set_expanded(["desktop".into(), "desktop/src".into()]);
        assert_eq!(tree.expanded_paths(), ["desktop", "desktop/src"]);

        tree.set_children("", vec![dir("desktop"), dir("daemon-rs")]);
        assert_eq!(tree.expanded_unloaded_children(""), ["desktop"]);
        tree.set_loading("desktop");
        assert!(tree.expanded_unloaded_children("").is_empty());

        tree.set_children("desktop", vec![dir("src"), file("Cargo.toml")]);
        assert_eq!(tree.expanded_unloaded_children("desktop"), ["desktop/src"]);
    }

    #[test]
    fn revealing_a_file_opens_every_directory_above_it() {
        let mut tree = FileTree::default();
        tree.reveal("src/voice/wav.rs");
        assert!(tree.is_expanded("src"));
        assert!(tree.is_expanded("src/voice"));
        assert!(!tree.is_expanded("src/voice/wav.rs"));

        // A file at the root has nothing above it to open.
        let mut root = FileTree::default();
        root.reveal("README.md");
        assert!(root.rows().is_empty());
        assert!(!root.is_expanded("README.md"));
    }

    #[test]
    fn the_chat_is_where_it_starts_and_where_closing_the_last_file_lands() {
        let mut open = OpenFiles::default();
        assert_eq!(open.active, FileTab::Chat);
        assert!(open.current().is_none());

        open.open("src/wav.rs", "fn wav() {}".into());
        assert_eq!(open.active, FileTab::File("src/wav.rs".into()));
        assert_eq!(open.current().unwrap().text, "fn wav() {}");
        assert_eq!(open.current().unwrap().name(), "wav.rs");

        open.close("src/wav.rs");
        assert_eq!(open.active, FileTab::Chat);
        assert!(open.files.is_empty());
    }

    #[test]
    fn opening_the_same_file_twice_is_the_same_tab() {
        let mut open = OpenFiles::default();
        open.open("a.rs", "one".into());
        open.open("b.rs", "two".into());
        open.edit("a.rs", "one, edited".into());
        open.open("a.rs", "one".into());

        assert_eq!(open.files.len(), 2);
        assert_eq!(open.active, FileTab::File("a.rs".into()));
        // Reopening must not throw away what was typed into the tab already there.
        assert_eq!(open.current().unwrap().text, "one, edited");
    }

    #[test]
    fn closing_lands_on_the_tab_to_the_left() {
        let mut open = OpenFiles::default();
        open.open("a.rs", String::new());
        open.open("b.rs", String::new());
        open.open("c.rs", String::new());

        open.close("c.rs");
        assert_eq!(open.active, FileTab::File("b.rs".into()));

        // Closing the leftmost has nothing to its left, so it lands right.
        open.close("a.rs");
        assert_eq!(open.active, FileTab::File("b.rs".into()));

        // A tab that is not in front leaves the front alone.
        open.open("d.rs", String::new());
        open.active = FileTab::Chat;
        open.close("d.rs");
        assert_eq!(open.active, FileTab::Chat);
    }

    #[test]
    fn typing_during_a_save_is_not_marked_as_already_written() {
        let mut open = OpenFiles::default();
        open.open("a.rs", "one".into());
        open.edit("a.rs", "two".into());
        open.set_saving("a.rs");
        // The round trip is in flight and the reader keeps typing.
        open.edit("a.rs", "two and three".into());
        open.set_saved("a.rs", "two".into());

        // "two" reached the daemon; "two and three" did not, and is still unsaved.
        assert_eq!(open.current().unwrap().saved, "two");
        assert_eq!(open.current().unwrap().text, "two and three");
        assert!(open.any_dirty());
    }

    #[test]
    fn a_failed_write_keeps_the_edit_and_says_why() {
        let mut open = OpenFiles::default();
        open.open("a.rs", "one".into());
        open.edit("a.rs", "two".into());
        open.set_saving("a.rs");
        open.set_failed("a.rs", "The file changed on disk".into());

        let file = open.current().unwrap();
        assert_eq!(file.text, "two");
        assert_eq!(file.saved, "one");
        assert!(!file.saving);
        assert_eq!(file.error.as_deref(), Some("The file changed on disk"));
        assert!(open.any_dirty());
    }
}
