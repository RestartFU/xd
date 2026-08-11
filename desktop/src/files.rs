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

    /// Whether this directory's listing has been fetched.
    pub fn is_loaded(&self, path: &str) -> bool {
        self.children.contains_key(path)
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

    /// Restores the folders that were open the last time this chat was shown.

    /// A stable snapshot suitable for settings persistence.

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
}

/// What the main area is showing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FileTab {
    #[default]
    Chat,
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

    /// Closes a tab. Closing the active file returns to the chat; closing a
    /// background file leaves the current tab alone.

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
