use std::{collections::HashSet, fs, path::PathBuf, time::Duration};

use gpui::{
    App, Application, Bounds, Context, Entity, Focusable, FontStyle, FontWeight, HighlightStyle,
    KeyBinding, ListAlignment, ListState, ObjectFit, PathPromptOptions, Render, StyledText, Timer,
    Window, WindowBounds, WindowOptions, div, img, list, prelude::*, px, rgb, size,
};
use serde_json::Value;
use xd_desktop::{
    activity::{ActivityCard, ActivityKind},
    daemon::{DaemonHandle, DaemonUpdate, RequestKind, StartedDaemon},
    markdown::{self, Block, CodeKind, InlineKind, InlineText},
    model::{AppModel, Attachment, Message},
};

mod input;

use input::{
    Backspace, ComposerEvent, ComposerInput, Copy, Cut, Delete, End, Home, Left, Paste, Right,
    SelectAll, SelectLeft, SelectRight, ShowCharacterPalette, Submit,
};

const BG: u32 = 0x111318;
const SURFACE: u32 = 0x191c22;
const SURFACE_HIGH: u32 = 0x232730;
const BORDER: u32 = 0x303641;
const TEXT: u32 = 0xe8eaf0;
const MUTED: u32 = 0x969daa;
const ACCENT: u32 = 0x6b8cff;
const MAX_ATTACHMENTS: usize = 4;
const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_TOTAL_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

struct PendingSend {
    text: String,
    attachments: Vec<Attachment>,
    restore: bool,
}

#[derive(Clone)]
struct QueueEdit {
    chat_id: String,
    index: usize,
    original: String,
    text: String,
    submitting: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SidebarTarget {
    Folder(String),
    Chat(String),
}

#[derive(Clone)]
struct SidebarEdit {
    target: SidebarTarget,
    original: String,
    text: String,
    submitting: bool,
}

struct XdDesktop {
    model: AppModel,
    daemon: Option<DaemonHandle>,
    _started_daemon: Option<StartedDaemon>,
    transcript: ListState,
    composer_input: Entity<ComposerInput>,
    queue_edit_input: Entity<ComposerInput>,
    sidebar_edit_input: Entity<ComposerInput>,
    composer: String,
    queue_edit: Option<QueueEdit>,
    sidebar_edit: Option<SidebarEdit>,
    pending_sidebar_delete: Option<SidebarTarget>,
    sidebar_delete_submitting: bool,
    draft_generation: u64,
    draft_dirty: bool,
    attachments_dirty: bool,
    attachment_generation: u64,
    sending: bool,
    pending_send: Option<PendingSend>,
    expanded_activity: HashSet<String>,
}

impl XdDesktop {
    fn new(cx: &mut Context<Self>) -> Self {
        let composer_input = cx.new(|cx| ComposerInput::new(cx, "Message xd…"));
        cx.subscribe(&composer_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.composer_changed(text.clone(), cx),
            ComposerEvent::Submit => this.send_composer(cx),
        })
        .detach();
        let queue_edit_input = cx.new(|cx| ComposerInput::new(cx, "Edit queued message…"));
        cx.subscribe(&queue_edit_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.queue_edit_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_queue_edit(cx),
        })
        .detach();
        let sidebar_edit_input = cx.new(|cx| ComposerInput::new(cx, "Name…"));
        cx.subscribe(&sidebar_edit_input, |this, _, event, cx| match event {
            ComposerEvent::Changed(text) => this.sidebar_edit_changed(text.clone(), cx),
            ComposerEvent::Submit => this.save_sidebar_edit(cx),
        })
        .detach();
        let mut desktop = Self {
            model: AppModel {
                draft_revision: -1,
                ..Default::default()
            },
            daemon: None,
            _started_daemon: None,
            transcript: ListState::new(0, ListAlignment::Bottom, px(700.0)),
            composer_input,
            queue_edit_input,
            sidebar_edit_input,
            composer: String::new(),
            queue_edit: None,
            sidebar_edit: None,
            pending_sidebar_delete: None,
            sidebar_delete_submitting: false,
            draft_generation: 0,
            draft_dirty: false,
            attachments_dirty: false,
            attachment_generation: 0,
            sending: false,
            pending_send: None,
            expanded_activity: HashSet::new(),
        };
        desktop.connect(cx);
        desktop
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        match DaemonHandle::connect_or_start() {
            Ok((daemon, updates, started_daemon)) => {
                self.daemon = Some(daemon);
                self._started_daemon = started_daemon;
                self.model.connection_error = None;
                cx.spawn(async move |this, cx| {
                    while let Ok(update) = updates.recv().await {
                        if this
                            .update(cx, |this, cx| this.handle_daemon_update(update, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => {
                self.model.connected = false;
                self.model.connection_error = Some(error.to_string());
            }
        }
    }

    fn handle_daemon_update(&mut self, update: DaemonUpdate, cx: &mut Context<Self>) {
        match update {
            DaemonUpdate::Connected { .. } => {
                self.model.connected = true;
                self.model.connection_error = None;
                self.request_tree();
                self.request_agent_catalog();
            }
            DaemonUpdate::Disconnected { message } => {
                self.model.connected = false;
                self.model.connection_error = Some(message);
                self.sending = false;
                self.restore_pending_send(cx);
            }
            DaemonUpdate::Reply {
                kind,
                body,
                attachments,
            } => self.handle_reply(kind, body, attachments, cx),
            DaemonUpdate::Event {
                name,
                body,
                attachments,
            } => self.handle_event(&name, Value::Object(body), attachments, cx),
        }
        cx.notify();
    }

    fn handle_reply(
        &mut self,
        kind: RequestKind,
        body: serde_json::Map<String, Value>,
        attachments: Option<Vec<Attachment>>,
        cx: &mut Context<Self>,
    ) {
        let value = Value::Object(body);
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            self.model.connection_error = Some(
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("The xd daemon rejected the request.")
                    .to_owned(),
            );
            match &kind {
                RequestKind::Send { .. } => {
                    self.sending = false;
                    self.restore_pending_send(cx);
                }
                RequestKind::EditQueue {
                    chat_id,
                    index,
                    old_text,
                    new_text,
                } => {
                    if let Some(edit) = &mut self.queue_edit
                        && edit.chat_id == *chat_id
                        && edit.index == *index
                        && edit.original == *old_text
                        && edit.submitting.as_deref() == Some(new_text.as_str())
                    {
                        edit.submitting = None;
                    }
                }
                RequestKind::RenameFolder { folder_id, .. } => {
                    if let Some(edit) = &mut self.sidebar_edit
                        && edit.target == SidebarTarget::Folder(folder_id.clone())
                    {
                        edit.submitting = false;
                    }
                }
                RequestKind::RenameChat { chat_id, .. } => {
                    if let Some(edit) = &mut self.sidebar_edit
                        && edit.target == SidebarTarget::Chat(chat_id.clone())
                    {
                        edit.submitting = false;
                    }
                }
                RequestKind::TrashFolder { folder_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Folder(folder_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
                }
                RequestKind::DeleteChat { chat_id } => {
                    if self.pending_sidebar_delete.as_ref()
                        == Some(&SidebarTarget::Chat(chat_id.clone()))
                    {
                        self.sidebar_delete_submitting = false;
                    }
                }
                _ => {}
            }
            return;
        }

        match kind {
            RequestKind::Tree => {
                if let Err(error) = self.model.apply_tree(&value) {
                    self.model.connection_error = Some(format!("Invalid tree response: {error}"));
                    return;
                }
                if self
                    .sidebar_edit
                    .as_ref()
                    .is_some_and(|edit| !self.sidebar_target_exists(&edit.target))
                {
                    self.cancel_sidebar_edit(cx);
                }
                if self
                    .pending_sidebar_delete
                    .as_ref()
                    .is_some_and(|target| !self.sidebar_target_exists(target))
                {
                    self.pending_sidebar_delete = None;
                    self.sidebar_delete_submitting = false;
                }
                if self.model.selected_chat.is_none() {
                    if let Some(chat_id) = self.model.chats.first().map(|chat| chat.id.clone()) {
                        self.select_chat(chat_id, cx);
                    }
                }
            }
            RequestKind::AgentCatalog => {
                if let Err(error) = self.model.apply_agent_catalog(&value) {
                    self.model.connection_error =
                        Some(format!("Invalid assistant catalog response: {error}"));
                }
            }
            RequestKind::Shortcuts { folder_id } => {
                if self
                    .model
                    .selected_summary()
                    .is_some_and(|chat| chat.folder == folder_id)
                {
                    self.model.apply_shortcuts(&value);
                }
            }
            RequestKind::NewFolder => {
                let Some(folder_id) = value.get("id").and_then(Value::as_str) else {
                    self.model.connection_error =
                        Some("The daemon returned no workspace id.".into());
                    return;
                };
                if let Some(daemon) = &self.daemon
                    && let Err(error) = daemon.new_chat(folder_id)
                {
                    self.model.connection_error = Some(error);
                }
            }
            RequestKind::NewChat { folder_id } => {
                let Some(chat_id) = value.get("id").and_then(Value::as_str) else {
                    self.model.connection_error = Some("The daemon returned no chat id.".into());
                    return;
                };
                let _ = folder_id;
                self.request_tree();
                self.select_chat(chat_id.to_owned(), cx);
            }
            RequestKind::RenameFolder { folder_id, name } => {
                if self.sidebar_edit.as_ref().is_some_and(|edit| {
                    edit.target == SidebarTarget::Folder(folder_id) && edit.text.trim() == name
                }) {
                    self.cancel_sidebar_edit(cx);
                }
            }
            RequestKind::TrashFolder { folder_id } => {
                if self.pending_sidebar_delete.as_ref() == Some(&SidebarTarget::Folder(folder_id)) {
                    self.pending_sidebar_delete = None;
                    self.sidebar_delete_submitting = false;
                }
            }
            RequestKind::RenameChat { chat_id, title } => {
                if self.sidebar_edit.as_ref().is_some_and(|edit| {
                    edit.target == SidebarTarget::Chat(chat_id) && edit.text.trim() == title
                }) {
                    self.cancel_sidebar_edit(cx);
                }
            }
            RequestKind::DeleteChat { chat_id } => {
                if self.pending_sidebar_delete.as_ref() == Some(&SidebarTarget::Chat(chat_id)) {
                    self.pending_sidebar_delete = None;
                    self.sidebar_delete_submitting = false;
                }
            }
            RequestKind::Chat { chat_id } if self.chat_is_active(&chat_id) => {
                let local_attachments = self
                    .attachments_dirty
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_chat(&value);
                if let Some(local_attachments) = local_attachments {
                    self.model.draft_attachments = local_attachments;
                } else if let Some(attachments) = attachments {
                    self.model.draft_attachments = attachments;
                }
                if !self.draft_dirty {
                    let draft = self.model.draft.clone();
                    self.set_composer_text(draft, cx);
                }
            }
            RequestKind::Messages { chat_id } if self.chat_is_active(&chat_id) => {
                if let Err(error) = self.model.apply_messages(&value) {
                    self.model.connection_error =
                        Some(format!("Invalid transcript response: {error}"));
                    return;
                }
                if !self.model.working {
                    self.model.live_text.clear();
                    self.model.live_activity.clear();
                }
                self.transcript.reset(self.model.display_message_count());
            }
            RequestKind::Send { chat_id, text } if self.chat_is_active(&chat_id) => {
                self.sending = false;
                self.pending_send = None;
                let queued = value
                    .get("queued")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !queued {
                    self.model.working = true;
                    self.request_messages(&chat_id);
                }
                if self.composer.is_empty() {
                    self.draft_dirty = false;
                }
                let _ = text;
            }
            // Queue events carry the authoritative complete queue. Mutation
            // replies are acknowledgements only, so they never refetch chat.
            RequestKind::QueueMutation { chat_id } if self.chat_is_active(&chat_id) => {}
            RequestKind::EditQueue {
                chat_id,
                index,
                old_text,
                new_text,
            } if self.chat_is_active(&chat_id) => {
                if self.queue_edit.as_ref().is_some_and(|edit| {
                    edit.chat_id == chat_id
                        && edit.index == index
                        && edit.original == old_text
                        && edit.submitting.as_deref() == Some(new_text.as_str())
                }) {
                    self.cancel_queue_edit(cx);
                }
            }
            RequestKind::Cancel { chat_id } if self.chat_is_active(&chat_id) => {}
            RequestKind::SetOption { chat_id } if self.chat_is_active(&chat_id) => {
                self.request_chat(&chat_id);
            }
            // The authoritative shortcuts-changed event follows this reply
            // and refetches the active workspace's merged shortcut state.
            RequestKind::SetShortcuts => {}
            RequestKind::RemoveWorktree { chat_id } if self.chat_is_active(&chat_id) => {
                self.request_chat(&chat_id);
            }
            RequestKind::SetDraft {
                chat_id,
                text,
                attachment_generation,
            } if self.chat_is_active(&chat_id) => {
                let attachment_reply_is_current = attachment_generation
                    .is_some_and(|generation| generation == self.attachment_generation);
                let local_attachments = (!attachment_reply_is_current && self.attachments_dirty)
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_draft_snapshot(&value);
                if let Some(local_attachments) = local_attachments {
                    self.model.draft_attachments = local_attachments;
                } else if attachment_reply_is_current && let Some(attachments) = attachments {
                    self.model.draft_attachments = attachments;
                }
                if self.composer == text {
                    self.draft_dirty = false;
                }
                if attachment_reply_is_current {
                    self.attachments_dirty = false;
                }
            }
            _ => {}
        }
    }

    fn handle_event(
        &mut self,
        name: &str,
        body: Value,
        attachments: Option<Vec<Attachment>>,
        cx: &mut Context<Self>,
    ) {
        match name {
            "tree" => self.request_tree(),
            "changed" if self.event_is_active(&body) => {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
                }
            }
            "draft" if self.event_is_active(&body) => {
                let local_attachments = self
                    .attachments_dirty
                    .then(|| self.model.draft_attachments.clone());
                self.model.apply_event(name, &body);
                if let Some(local_attachments) = local_attachments {
                    self.model.draft_attachments = local_attachments;
                } else if let Some(attachments) = attachments {
                    self.model.draft_attachments = attachments;
                }
                if !self.draft_dirty {
                    let draft = self.model.draft.clone();
                    self.set_composer_text(draft, cx);
                }
            }
            "turn-started" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                self.sync_transcript_count(false);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "text" | "tool" if self.event_is_active(&body) => {
                let old_count = self.model.display_message_count();
                self.model.apply_event(name, &body);
                let new_count = self.model.display_message_count();
                if new_count > old_count {
                    self.transcript
                        .splice(old_count..old_count, new_count - old_count);
                } else if new_count > 0 {
                    self.transcript.splice(new_count - 1..new_count, 1);
                }
            }
            "turn-finished" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_messages(&chat_id);
                    self.request_chat(&chat_id);
                }
            }
            "queued" if self.event_is_active(&body) => {
                self.model.apply_event(name, &body);
                let edit_is_stale = self.queue_edit.as_ref().is_some_and(|edit| {
                    edit.submitting.is_none()
                        && self.model.queue.get(edit.index) != Some(&edit.original)
                });
                if edit_is_stale {
                    self.cancel_queue_edit(cx);
                    self.model.connection_error =
                        Some("That queued message changed on another client.".into());
                }
            }
            "shortcuts-changed" => self.request_shortcuts(),
            "agent-auth-changed"
                if body.get("provider").and_then(Value::as_str)
                    == Some(self.model.backend.as_str()) =>
            {
                if let Some(chat_id) = self.model.selected_chat.clone() {
                    self.request_chat(&chat_id);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn request_tree(&mut self) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.tree() {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn request_agent_catalog(&mut self) {
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.agent_catalog()
        {
            self.model.connection_error = Some(error);
        }
    }

    fn request_shortcuts(&mut self) {
        let Some(folder_id) = self
            .model
            .selected_summary()
            .map(|chat| chat.folder.clone())
        else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.shortcuts(&folder_id)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn create_workspace(&mut self) {
        let name = format!("Workspace {}", self.model.folders.len() + 1);
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.new_folder(&name)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn create_chat(&mut self, folder_id: &str) {
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.new_chat(folder_id)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn begin_sidebar_edit(
        &mut self,
        target: SidebarTarget,
        current: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_sidebar_delete = None;
        self.sidebar_delete_submitting = false;
        self.sidebar_edit = Some(SidebarEdit {
            target,
            original: current.clone(),
            text: current.clone(),
            submitting: false,
        });
        self.sidebar_edit_input
            .update(cx, |input, cx| input.set_text(current, cx));
        cx.notify();
    }

    fn sidebar_edit_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.sidebar_edit
            && !edit.submitting
        {
            edit.text = text;
            cx.notify();
        }
    }

    fn save_sidebar_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.sidebar_edit.clone() else {
            return;
        };
        if edit.submitting {
            return;
        }
        let text = edit.text.trim();
        if text.is_empty() {
            self.model.connection_error = Some("A name cannot be empty.".into());
            cx.notify();
            return;
        }
        if text == edit.original {
            self.cancel_sidebar_edit(cx);
            return;
        }
        let result = self.daemon.as_ref().map(|daemon| match &edit.target {
            SidebarTarget::Folder(folder_id) => daemon.rename_folder(folder_id, text),
            SidebarTarget::Chat(chat_id) => daemon.rename_chat(chat_id, text),
        });
        match result {
            Some(Ok(())) => {
                if let Some(active) = &mut self.sidebar_edit
                    && active.target == edit.target
                {
                    active.text = text.to_owned();
                    active.submitting = true;
                }
            }
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => {
                self.model.connection_error = Some("xd-dev is not connected to a daemon.".into())
            }
        }
        cx.notify();
    }

    fn cancel_sidebar_edit(&mut self, cx: &mut Context<Self>) {
        self.sidebar_edit = None;
        self.sidebar_edit_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn delete_sidebar_item(&mut self, target: SidebarTarget, cx: &mut Context<Self>) {
        if self.sidebar_delete_submitting {
            return;
        }
        if self.pending_sidebar_delete.as_ref() != Some(&target) {
            self.cancel_sidebar_edit(cx);
            self.pending_sidebar_delete = Some(target);
            cx.notify();
            return;
        }
        let result = self.daemon.as_ref().map(|daemon| match &target {
            SidebarTarget::Folder(folder_id) => daemon.trash_folder(folder_id),
            SidebarTarget::Chat(chat_id) => daemon.delete_chat(chat_id),
        });
        match result {
            Some(Ok(())) => self.sidebar_delete_submitting = true,
            Some(Err(error)) => self.model.connection_error = Some(error),
            None => {
                self.model.connection_error = Some("xd-dev is not connected to a daemon.".into())
            }
        }
        cx.notify();
    }

    fn toggle_new_worktree(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.has_messages || self.model.working {
            return;
        }
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_new_worktree(&chat_id, !self.model.new_worktree)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_model(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        let choices = self
            .model
            .agent_backends
            .iter()
            .flat_map(|backend| {
                backend
                    .models
                    .iter()
                    .map(|model| (backend.id.clone(), model.id.clone()))
            })
            .collect::<Vec<_>>();
        if choices.is_empty() {
            return;
        }
        let current = choices
            .iter()
            .position(|(backend, model)| {
                backend == &self.model.backend
                    && Some(model.as_str()) == self.model.model.as_deref()
            })
            .unwrap_or(choices.len() - 1);
        let (backend, model) = &choices[(current + 1) % choices.len()];
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_model(&chat_id, backend, model)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_effort(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        let Some(backend) = self
            .model
            .agent_backends
            .iter()
            .find(|backend| backend.id == self.model.backend)
        else {
            return;
        };
        if backend.efforts.is_empty() {
            return;
        }
        let current = backend
            .efforts
            .iter()
            .position(|effort| effort == &self.model.effort)
            .unwrap_or(backend.efforts.len() - 1);
        let effort = backend.efforts[(current + 1) % backend.efforts.len()].clone();
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_effort(&chat_id, &effort)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_access(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        const ACCESS: [&str; 3] = ["read-only", "edit", "full"];
        let current = ACCESS
            .iter()
            .position(|access| *access == self.model.access)
            .unwrap_or(ACCESS.len() - 1);
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_access(&chat_id, ACCESS[(current + 1) % ACCESS.len()])
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_plan(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.working {
            return;
        }
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_plan(&chat_id, !self.model.plan)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cycle_workspace(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.has_messages || self.model.working || self.model.worktrees.len() < 2 {
            return;
        }
        let current = self
            .model
            .worktrees
            .iter()
            .position(|worktree| worktree.current)
            .unwrap_or(0);
        let next = (current + 1) % self.model.worktrees.len();
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_workspace(&chat_id, &self.model.worktrees[next].path)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn remove_selected_worktree(&mut self) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if self.model.has_messages || self.model.working {
            return;
        }
        let Some(worktree) = self
            .model
            .worktrees
            .iter()
            .find(|worktree| worktree.current && !worktree.main)
        else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.remove_worktree(&chat_id, &worktree.path)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn request_chat(&mut self, chat_id: &str) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.chat(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn request_messages(&mut self, chat_id: &str) {
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.messages(chat_id) {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn select_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        if self.model.selected_chat.as_deref() == Some(chat_id.as_str()) {
            return;
        }
        self.model.select_chat(chat_id.clone());
        self.set_composer_text(String::new(), cx);
        self.draft_dirty = false;
        self.attachments_dirty = false;
        self.pending_send = None;
        self.cancel_queue_edit(cx);
        self.sending = false;
        self.transcript.reset(0);
        self.request_chat(&chat_id);
        self.request_messages(&chat_id);
        self.request_shortcuts();
        cx.notify();
    }

    fn send_composer(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let text = self.composer.trim().to_owned();
        let attachments = self.model.draft_attachments.clone();
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if text.is_empty() && attachments.is_empty() {
            return;
        }
        let Some(daemon) = self.daemon.clone() else {
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.send_message(&chat_id, &text, &attachments) {
            self.model.connection_error = Some(error);
            return;
        }

        self.sending = true;
        self.pending_send = Some(PendingSend {
            text,
            attachments,
            restore: true,
        });
        self.set_composer_text(String::new(), cx);
        self.model.draft_attachments.clear();
        self.draft_dirty = true;
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
        self.draft_generation = self.draft_generation.saturating_add(1);
        let _ = daemon.set_draft(&chat_id, "", Some(&[]), Some(self.attachment_generation));
        cx.notify();
    }

    fn restore_pending_send(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_send.take() else {
            return;
        };
        if !pending.restore {
            return;
        }
        let restored = match (pending.text.is_empty(), self.composer.is_empty()) {
            (false, false) => format!("{}\n{}", pending.text, self.composer),
            (false, true) => pending.text,
            (true, false) => self.composer.clone(),
            (true, true) => String::new(),
        };
        self.set_composer_text(restored, cx);
        self.model.draft_attachments = pending.attachments;
        self.draft_dirty = true;
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
    }

    fn send_shortcut(&mut self, prompt: String) {
        if self.sending || prompt.trim().is_empty() {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        let Some(daemon) = self.daemon.clone() else {
            self.model.connection_error = Some("xd-dev is not connected to a daemon.".into());
            return;
        };
        if let Err(error) = daemon.send_message(&chat_id, &prompt, &[]) {
            self.model.connection_error = Some(error);
            return;
        }
        self.sending = true;
        self.pending_send = Some(PendingSend {
            text: prompt,
            attachments: Vec::new(),
            restore: false,
        });
    }

    fn drop_queued(&mut self, index: usize) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.drop_queue(chat_id, index)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn begin_queue_edit(&mut self, index: usize, prompt: String, cx: &mut Context<Self>) {
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        self.queue_edit = Some(QueueEdit {
            chat_id,
            index,
            original: prompt.clone(),
            text: prompt.clone(),
            submitting: None,
        });
        self.queue_edit_input
            .update(cx, |input, cx| input.set_text(prompt, cx));
        cx.notify();
    }

    fn queue_edit_changed(&mut self, text: String, cx: &mut Context<Self>) {
        if let Some(edit) = &mut self.queue_edit
            && edit.submitting.is_none()
        {
            edit.text = text;
            cx.notify();
        }
    }

    fn save_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.clone() else {
            return;
        };
        let text = edit.text.trim();
        if text.is_empty() {
            self.model.connection_error = Some("A queued message cannot be empty.".into());
            cx.notify();
            return;
        }
        if text == edit.original {
            self.cancel_queue_edit(cx);
            return;
        }
        if let Some(daemon) = &self.daemon {
            if let Err(error) = daemon.edit_queue(&edit.chat_id, edit.index, &edit.original, text) {
                self.model.connection_error = Some(error);
            } else if let Some(active) = &mut self.queue_edit
                && active.chat_id == edit.chat_id
                && active.index == edit.index
                && active.original == edit.original
            {
                active.submitting = Some(text.to_owned());
            }
        }
    }

    fn cancel_queue_edit(&mut self, cx: &mut Context<Self>) {
        self.queue_edit = None;
        self.queue_edit_input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        cx.notify();
    }

    fn steer_queued(&mut self, index: usize, text: &str) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.steer_queue(chat_id, index, text)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn cancel_turn(&mut self) {
        let Some(chat_id) = self.model.selected_chat.as_deref() else {
            return;
        };
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.cancel(chat_id)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn toggle_shortcut(&mut self, workspace: bool) {
        let prompt = self.composer.trim();
        if prompt.is_empty() {
            return;
        }
        let mut shortcuts = if workspace {
            self.model.workspace_shortcuts.clone()
        } else {
            self.model.global_shortcuts.clone()
        };
        if let Some(index) = shortcuts.iter().position(|shortcut| shortcut == prompt) {
            shortcuts.remove(index);
        } else {
            shortcuts.push(prompt.to_owned());
        }
        let folder_id = workspace
            .then(|| {
                self.model
                    .selected_summary()
                    .map(|chat| chat.folder.clone())
            })
            .flatten();
        if workspace && folder_id.is_none() {
            return;
        }
        if let Some(daemon) = &self.daemon
            && let Err(error) = daemon.set_shortcuts(folder_id.as_deref(), &shortcuts)
        {
            self.model.connection_error = Some(error);
        }
    }

    fn set_composer_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer.clone_from(&text);
        self.composer_input
            .update(cx, |input, cx| input.set_text(text, cx));
    }

    fn attach_images(&mut self, cx: &mut Context<Self>) {
        let available = MAX_ATTACHMENTS.saturating_sub(self.model.draft_attachments.len());
        if available == 0 || self.model.selected_chat.is_none() {
            return;
        }
        let existing_bytes = self
            .model
            .draft_attachments
            .iter()
            .map(|attachment| attachment.preview.bytes.len())
            .sum();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach PNG images".into()),
        });
        let load = cx.background_executor().spawn(async move {
            match receiver.await {
                Ok(Ok(Some(paths))) => load_png_attachments(paths, available, existing_bytes),
                Ok(Ok(None)) => Ok(Vec::new()),
                Ok(Err(error)) => Err(format!("Cannot open the image picker: {error}")),
                Err(_) => Err("The image picker closed unexpectedly.".into()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(attachments) if !attachments.is_empty() => {
                    this.model.draft_attachments.extend(attachments);
                    this.attachments_dirty = true;
                    this.attachment_generation = this.attachment_generation.saturating_add(1);
                    this.schedule_draft_sync(cx);
                    cx.notify();
                }
                Ok(_) => {}
                Err(error) => {
                    this.model.connection_error = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn remove_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.model.draft_attachments.len() {
            return;
        }
        self.model.draft_attachments.remove(index);
        self.attachments_dirty = true;
        self.attachment_generation = self.attachment_generation.saturating_add(1);
        self.schedule_draft_sync(cx);
        cx.notify();
    }

    fn composer_changed(&mut self, text: String, cx: &mut Context<Self>) {
        self.composer = text;
        self.draft_dirty = true;
        self.schedule_draft_sync(cx);
        cx.notify();
    }

    fn schedule_draft_sync(&mut self, cx: &mut Context<Self>) {
        self.draft_generation = self.draft_generation.saturating_add(1);
        let generation = self.draft_generation;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(250)).await;
            let _ = this.update(cx, |this, _cx| {
                if this.draft_generation == generation {
                    this.sync_draft();
                }
            });
        })
        .detach();
    }

    fn sync_draft(&mut self) {
        if !self.draft_dirty && !self.attachments_dirty {
            return;
        }
        let Some(chat_id) = self.model.selected_chat.clone() else {
            return;
        };
        if let Some(daemon) = &self.daemon {
            let attachments = self
                .attachments_dirty
                .then_some(self.model.draft_attachments.as_slice());
            let attachment_generation = attachments.map(|_| self.attachment_generation);
            if let Err(error) =
                daemon.set_draft(&chat_id, &self.composer, attachments, attachment_generation)
            {
                self.model.connection_error = Some(error);
            }
        }
    }

    fn event_is_active(&self, body: &Value) -> bool {
        body.get("chat").and_then(Value::as_str) == self.model.selected_chat.as_deref()
    }

    fn chat_is_active(&self, chat_id: &str) -> bool {
        self.model.selected_chat.as_deref() == Some(chat_id)
    }

    fn sidebar_target_exists(&self, target: &SidebarTarget) -> bool {
        match target {
            SidebarTarget::Folder(folder_id) => self
                .model
                .folders
                .iter()
                .any(|folder| &folder.id == folder_id),
            SidebarTarget::Chat(chat_id) => self.model.chats.iter().any(|chat| &chat.id == chat_id),
        }
    }

    fn sync_transcript_count(&self, reset: bool) {
        let count = self.model.display_message_count();
        if reset {
            self.transcript.reset(count);
            return;
        }
        let old_count = self.transcript.item_count();
        if count > old_count {
            self.transcript
                .splice(old_count..old_count, count - old_count);
        } else if count < old_count {
            self.transcript.splice(count..old_count, 0);
        }
    }

    fn message_row(
        message: &Message,
        index: usize,
        expanded: bool,
        desktop: Entity<Self>,
    ) -> gpui::AnyElement {
        let is_user = message.role == "user";
        let is_tool = message.role == "tool";
        let label = message
            .label
            .clone()
            .unwrap_or_else(|| match message.role.as_str() {
                "user" => "You".into(),
                "assistant" => "Assistant".into(),
                "tool" => "Activity".into(),
                role => role.to_owned(),
            });

        if is_tool {
            let key = message
                .id
                .map(|id| format!("message-{id}"))
                .unwrap_or_else(|| format!("live-{index}"));
            return Self::activity_card(
                ActivityCard::parse(&message.content),
                key,
                index,
                expanded,
                desktop,
            );
        }

        div()
            .w_full()
            .px_6()
            .py_2()
            .child(
                div()
                    .w_full()
                    .max_w(px(920.0))
                    .mx_auto()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(if is_user { 0x3c4b78 } else { BORDER }))
                    .bg(rgb(if is_user { 0x202944 } else { SURFACE }))
                    .text_color(rgb(TEXT))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(if is_user { 0xaec0ff } else { MUTED }))
                            .mb_2()
                            .child(label),
                    )
                    .child(
                        Self::markdown_content(message.markdown())
                            .text_sm()
                            .line_height(px(21.0)),
                    ),
            )
            .into_any_element()
    }

    fn activity_card(
        card: ActivityCard,
        key: String,
        index: usize,
        expanded: bool,
        desktop: Entity<Self>,
    ) -> gpui::AnyElement {
        let status_color = match card.kind {
            ActivityKind::Running => 0x91a7ff,
            ActivityKind::Success => 0x8bd5a0,
            ActivityKind::Failure => 0xff8f8f,
            ActivityKind::Finished => 0xaab2c0,
        };
        let toggle_key = key.clone();
        let mut body = div()
            .w_full()
            .max_w(px(920.0))
            .mx_auto()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343b48))
            .bg(rgb(0x171a20))
            .overflow_hidden()
            .child(
                div()
                    .id(("activity-card", index))
                    .w_full()
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x1d222b)))
                    .on_click(move |_, _, cx| {
                        desktop.update(cx, |this, cx| {
                            if !this.expanded_activity.remove(&toggle_key) {
                                this.expanded_activity.insert(toggle_key.clone());
                            }
                            cx.notify();
                        });
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(MUTED))
                                    .child(card.title.clone()),
                            )
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(rgb(MUTED)).child(if expanded {
                                "▾"
                            } else {
                                "▸"
                            })),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(rgb(status_color)).child("●"))
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(card.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(status_color))
                                    .child(card.status.clone()),
                            ),
                    ),
            );
        if expanded {
            body = body.child(
                div()
                    .w_full()
                    .px_4()
                    .pb_3()
                    .pt_1()
                    .border_t_1()
                    .border_color(rgb(0x2a303b))
                    .text_sm()
                    .text_color(rgb(0xb8bfcc))
                    .child(card.detail),
            );
            if let Some(footer) = card.footer {
                body = body.child(
                    div()
                        .px_4()
                        .pb_3()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(footer),
                );
            }
        }
        div()
            .w_full()
            .px_6()
            .py_2()
            .text_color(rgb(TEXT))
            .child(body)
            .into_any_element()
    }

    fn markdown_content(document: std::sync::Arc<markdown::Document>) -> gpui::Div {
        let mut content = div().w_full().flex().flex_col().gap_2();
        for block in document.blocks.iter().cloned() {
            let element = match block {
                Block::Heading { level, content } => {
                    let heading = div()
                        .mt_1()
                        .font_weight(FontWeight::BOLD)
                        .child(Self::inline_text(content));
                    match level {
                        1 => heading.text_xl().line_height(px(30.0)),
                        2 => heading.text_lg().line_height(px(27.0)),
                        _ => heading.text_base().line_height(px(24.0)),
                    }
                    .into_any_element()
                }
                Block::Paragraph(content) => div()
                    .whitespace_normal()
                    .child(Self::inline_text(content))
                    .into_any_element(),
                Block::Quote(content) => div()
                    .pl_3()
                    .py_1()
                    .border_l_2()
                    .border_color(rgb(0x59647a))
                    .text_color(rgb(0xb8bfcc))
                    .child(Self::inline_text(content))
                    .into_any_element(),
                Block::ListItem { ordered, content } => div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(
                        div()
                            .w(px(18.0))
                            .flex_none()
                            .text_color(rgb(MUTED))
                            .child(if ordered { "1." } else { "•" }),
                    )
                    .child(div().flex_1().child(Self::inline_text(content)))
                    .into_any_element(),
                Block::Rule => div()
                    .w_full()
                    .h(px(1.0))
                    .my_2()
                    .bg(rgb(BORDER))
                    .into_any_element(),
                Block::Code(code) => {
                    let language = code.language.unwrap_or_else(|| "text".into());
                    let highlights = code.spans.into_iter().map(|span| {
                        let color = match span.kind {
                            CodeKind::Keyword => 0xc792ea,
                            CodeKind::String => 0xc3e88d,
                            CodeKind::Comment => 0x758195,
                            CodeKind::Number => 0xf78c6c,
                        };
                        (
                            span.range,
                            HighlightStyle {
                                color: Some(rgb(color).into()),
                                ..Default::default()
                            },
                        )
                    });
                    div()
                        .w_full()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(0x343b48))
                        .bg(rgb(0x11141a))
                        .overflow_hidden()
                        .child(
                            div()
                                .w_full()
                                .px_3()
                                .py_1()
                                .bg(rgb(0x1d222b))
                                .text_xs()
                                .text_color(rgb(0xaab2c0))
                                .child(language),
                        )
                        .child(
                            div()
                                .w_full()
                                .p_3()
                                .font_family("monospace")
                                .text_sm()
                                .line_height(px(20.0))
                                .whitespace_normal()
                                .child(StyledText::new(code.code).with_highlights(highlights)),
                        )
                        .into_any_element()
                }
            };
            content = content.child(element);
        }
        content
    }

    fn inline_text(content: InlineText) -> StyledText {
        let highlights = content.spans.into_iter().map(|span| {
            let style = match span.kind {
                InlineKind::Strong => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..Default::default()
                },
                InlineKind::Emphasis => HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..Default::default()
                },
                InlineKind::Code => HighlightStyle {
                    color: Some(rgb(0xd8b4fe).into()),
                    background_color: Some(rgb(0x292331).into()),
                    ..Default::default()
                },
                InlineKind::Link => HighlightStyle {
                    color: Some(rgb(0x91a7ff).into()),
                    ..Default::default()
                },
            };
            (span.range, style)
        });
        StyledText::new(content.text).with_highlights(highlights)
    }
}

impl Render for XdDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let messages = self.model.display_messages();
        let queue_count = self.model.queue.len();
        let working = self.model.working;
        let selected = self.model.selected_summary().cloned();
        let new_worktree = self.model.new_worktree;
        let can_change_worktree = selected.is_some() && !self.model.has_messages && !working;
        let can_cycle_workspace =
            can_change_worktree && !new_worktree && self.model.worktrees.len() > 1;
        let can_remove_worktree = can_change_worktree
            && !new_worktree
            && self
                .model
                .worktrees
                .iter()
                .any(|worktree| worktree.current && !worktree.main);
        let workspace_label = self
            .model
            .worktrees
            .iter()
            .find(|worktree| worktree.current)
            .map(|worktree| {
                worktree.branch.clone().unwrap_or_else(|| {
                    if worktree.main {
                        "main checkout".into()
                    } else {
                        "detached worktree".into()
                    }
                })
            })
            .unwrap_or_else(|| "workspace".into());
        let model_label = self
            .model
            .selected_model_name()
            .unwrap_or(if self.model.backend.is_empty() {
                "Assistant"
            } else {
                &self.model.backend
            })
            .to_owned();
        let effort_label = if self.model.effort.is_empty() {
            "high"
        } else {
            &self.model.effort
        }
        .to_owned();
        let can_change_agent =
            selected.is_some() && !working && !self.model.agent_backends.is_empty();
        let access_label = match self.model.access.as_str() {
            "full" => "Full access",
            "edit" => "Edit",
            _ => "Read only",
        };
        let status_text = if self.model.connected {
            "connected"
        } else {
            "offline"
        };
        let status_color = if self.model.connected {
            0x92d5a5
        } else {
            0xe49a9a
        };

        let sidebar_edit = self.sidebar_edit.clone();
        let sidebar_edit_input = self.sidebar_edit_input.clone();
        let sidebar_edit_focus = self.sidebar_edit_input.read(cx).focus_handle(cx);
        let pending_sidebar_delete = self.pending_sidebar_delete.clone();
        let sidebar_delete_submitting = self.sidebar_delete_submitting;
        let mut tree_rows = Vec::new();
        let mut chat_row_index = 0_usize;
        for (folder_row_index, folder) in self.model.folders.clone().into_iter().enumerate() {
            let indent = if folder.parent.is_some() { 22.0 } else { 12.0 };
            let folder_id = folder.id.clone();
            let folder_name = folder.name.clone();
            let folder_target = SidebarTarget::Folder(folder.id.clone());
            let editing_folder = sidebar_edit
                .as_ref()
                .is_some_and(|edit| edit.target == folder_target);
            if editing_folder {
                let can_save = sidebar_edit.as_ref().is_some_and(|edit| {
                    !edit.submitting
                        && !edit.text.trim().is_empty()
                        && edit.text.trim() != edit.original
                });
                let saving = sidebar_edit.as_ref().is_some_and(|edit| edit.submitting);
                tree_rows.push(
                    div()
                        .ml(px(indent))
                        .px_2()
                        .pt_2()
                        .pb_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(("folder-name-editor", folder_row_index))
                                .track_focus(&sidebar_edit_focus)
                                .h(px(30.0))
                                .min_w_0()
                                .flex_1()
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if sidebar_edit_focus.is_focused(window) {
                                    ACCENT
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(BG))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.sidebar_edit_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(sidebar_edit_input.clone()),
                        )
                        .child(
                            div()
                                .id(("save-folder-name", folder_row_index))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(if can_save { TEXT } else { MUTED }))
                                .when(can_save, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if can_save {
                                        this.save_sidebar_edit(cx);
                                    }
                                }))
                                .child(if saving { "…" } else { "Save" }),
                        )
                        .child(
                            div()
                                .id(("cancel-folder-name", folder_row_index))
                                .px_1()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_sidebar_edit(cx);
                                }))
                                .child("×"),
                        )
                        .into_any_element(),
                );
            } else {
                let rename_target = folder_target.clone();
                let delete_target = folder_target.clone();
                let confirming_delete = pending_sidebar_delete.as_ref() == Some(&folder_target);
                tree_rows.push(
                    div()
                        .px_3()
                        .ml(px(indent))
                        .pt_2()
                        .pb_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_sm()
                        .text_color(rgb(TEXT))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .child(format!("▾  {folder_name}")),
                        )
                        .child(
                            div()
                                .id(("new-chat", folder_row_index))
                                .px_2()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                                .on_click(
                                    cx.listener(move |this, _, _, _| this.create_chat(&folder_id)),
                                )
                                .child("+"),
                        )
                        .child(
                            div()
                                .id(("rename-folder", folder_row_index))
                                .px_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_sidebar_edit(
                                        rename_target.clone(),
                                        folder_name.clone(),
                                        cx,
                                    );
                                    let focus = this.sidebar_edit_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child("Edit"),
                        )
                        .child(
                            div()
                                .id(("trash-folder", folder_row_index))
                                .px_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(if confirming_delete { 0xefaaaa } else { MUTED }))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x3b282e)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.delete_sidebar_item(delete_target.clone(), cx);
                                }))
                                .child(if confirming_delete {
                                    if sidebar_delete_submitting {
                                        "…"
                                    } else {
                                        "Confirm"
                                    }
                                } else {
                                    "Trash"
                                }),
                        )
                        .into_any_element(),
                );
            }
            for chat in self
                .model
                .chats
                .iter()
                .filter(|chat| chat.folder == folder.id)
                .cloned()
            {
                let chat_id = chat.id.clone();
                let row_id = chat_row_index;
                chat_row_index += 1;
                let is_selected = self.model.selected_chat.as_deref() == Some(chat.id.as_str());
                let title = chat.title.unwrap_or_else(|| "New Chat".into());
                let chat_target = SidebarTarget::Chat(chat.id.clone());
                let editing_chat = sidebar_edit
                    .as_ref()
                    .is_some_and(|edit| edit.target == chat_target);
                if editing_chat {
                    let can_save = sidebar_edit.as_ref().is_some_and(|edit| {
                        !edit.submitting
                            && !edit.text.trim().is_empty()
                            && edit.text.trim() != edit.original
                    });
                    let saving = sidebar_edit.as_ref().is_some_and(|edit| edit.submitting);
                    tree_rows.push(
                        div()
                            .mx_2()
                            .ml(px(indent + 10.0))
                            .mb_1()
                            .p_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .child(
                                div()
                                    .id(("chat-title-editor", row_id))
                                    .track_focus(&sidebar_edit_focus)
                                    .h(px(30.0))
                                    .min_w_0()
                                    .flex_1()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(rgb(if sidebar_edit_focus.is_focused(window) {
                                        ACCENT
                                    } else {
                                        BORDER
                                    }))
                                    .bg(rgb(BG))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let focus =
                                            this.sidebar_edit_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child(sidebar_edit_input.clone()),
                            )
                            .child(
                                div()
                                    .id(("save-chat-title", row_id))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(if can_save { TEXT } else { MUTED }))
                                    .when(can_save, |button| {
                                        button
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0x303c52)))
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if can_save {
                                            this.save_sidebar_edit(cx);
                                        }
                                    }))
                                    .child(if saving { "…" } else { "Save" }),
                            )
                            .child(
                                div()
                                    .id(("cancel-chat-title", row_id))
                                    .px_1()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(MUTED))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.cancel_sidebar_edit(cx);
                                    }))
                                    .child("×"),
                            )
                            .into_any_element(),
                    );
                    continue;
                }
                let rename_target = chat_target.clone();
                let delete_target = chat_target.clone();
                let confirming_delete = pending_sidebar_delete.as_ref() == Some(&chat_target);
                tree_rows.push(
                    div()
                        .id(("chat", row_id))
                        .mx_2()
                        .ml(px(indent + 10.0))
                        .mb_1()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(if is_selected { SURFACE_HIGH } else { SURFACE }))
                        .text_color(rgb(if is_selected { TEXT } else { MUTED }))
                        .text_sm()
                        .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id(("select-chat", row_id))
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_chat(chat_id.clone(), cx)
                                }))
                                .child(if chat.working {
                                    format!("●  {title}")
                                } else {
                                    format!("   {title}")
                                }),
                        )
                        .child(
                            div()
                                .id(("rename-chat", row_id))
                                .px_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x303c52)).text_color(rgb(TEXT)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_sidebar_edit(
                                        rename_target.clone(),
                                        title.clone(),
                                        cx,
                                    );
                                    let focus = this.sidebar_edit_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child("Edit"),
                        )
                        .child(
                            div()
                                .id(("delete-chat", row_id))
                                .px_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(rgb(if confirming_delete { 0xefaaaa } else { MUTED }))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x3b282e)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.delete_sidebar_item(delete_target.clone(), cx);
                                }))
                                .child(if confirming_delete {
                                    if sidebar_delete_submitting {
                                        "…"
                                    } else {
                                        "Confirm"
                                    }
                                } else {
                                    "Delete"
                                }),
                        )
                        .into_any_element(),
                );
            }
        }

        let sidebar = div()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(58.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().text_lg().text_color(rgb(TEXT)).child("xd-dev"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(status_color))
                            .child(status_text),
                    ),
            )
            .child(
                div()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("WORKSPACES")
                    .child(
                        div()
                            .id("new-workspace")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SURFACE_HIGH)).text_color(rgb(TEXT)))
                            .on_click(cx.listener(|this, _, _, _| this.create_workspace()))
                            .child("+ New"),
                    ),
            )
            .child(
                div()
                    .id("workspace-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(tree_rows),
            );

        let title = selected
            .as_ref()
            .and_then(|chat| chat.title.clone())
            .unwrap_or_else(|| "Select a chat".into());
        let context = selected
            .as_ref()
            .map(|chat| {
                let state = match self.model.auth_state.as_str() {
                    "signed-in" => "signed in",
                    "signed-out" => "signed out",
                    "checking" => "checking sign-in",
                    "failed" => "sign-in check failed",
                    _ => "sign-in unknown",
                };
                format!("{} · {state}", chat.backend)
            })
            .unwrap_or_else(|| "xd daemon".into());
        let header = div()
            .h(px(102.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(58.0))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .px_5()
                    .child(div().text_sm().text_color(rgb(TEXT)).child(title))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(context)),
            )
            .child(
                div()
                    .h(px(44.0))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_5()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .id("assistant-cycle")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(SURFACE_HIGH))
                            .text_xs()
                            .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                            .when(can_change_agent, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                if can_change_agent {
                                    this.cycle_model();
                                }
                            }))
                            .child(model_label),
                    )
                    .child(
                        div()
                            .id("effort-cycle")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(SURFACE_HIGH))
                            .text_xs()
                            .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                            .when(can_change_agent, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                if can_change_agent {
                                    this.cycle_effort();
                                }
                            }))
                            .child(format!("Effort: {effort_label}")),
                    )
                    .child(
                        div()
                            .id("access-cycle")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(SURFACE_HIGH))
                            .text_xs()
                            .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                            .when(can_change_agent, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                if can_change_agent {
                                    this.cycle_access();
                                }
                            }))
                            .child(access_label),
                    )
                    .child(
                        div()
                            .id("plan-toggle")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(if self.model.plan {
                                0x26354d
                            } else {
                                SURFACE_HIGH
                            }))
                            .text_xs()
                            .text_color(rgb(if can_change_agent { TEXT } else { MUTED }))
                            .when(can_change_agent, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                if can_change_agent {
                                    this.toggle_plan();
                                }
                            }))
                            .child(if self.model.plan {
                                "Plan: on"
                            } else {
                                "Plan: off"
                            }),
                    )
                    .child(
                        div()
                            .id("workspace-cycle")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(SURFACE_HIGH))
                            .text_xs()
                            .text_color(rgb(if can_cycle_workspace { TEXT } else { MUTED }))
                            .when(can_cycle_workspace, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                if can_cycle_workspace {
                                    this.cycle_workspace();
                                }
                            }))
                            .child(format!("Workspace: {workspace_label}")),
                    )
                    .when(can_remove_worktree, |controls| {
                        controls.child(
                            div()
                                .id("remove-worktree")
                                .px_3()
                                .py_1()
                                .rounded_full()
                                .bg(rgb(0x3c292d))
                                .text_xs()
                                .text_color(rgb(0xf1b3ba))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x513038)))
                                .on_click(
                                    cx.listener(|this, _, _, _| this.remove_selected_worktree()),
                                )
                                .child("Remove worktree"),
                        )
                    })
                    .child(
                        div()
                            .id("new-worktree-toggle")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(if new_worktree { 0x26354d } else { SURFACE_HIGH }))
                            .text_xs()
                            .text_color(rgb(if can_change_worktree { TEXT } else { MUTED }))
                            .when(can_change_worktree, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x303c52)))
                            })
                            .on_click(cx.listener(move |this, _, _, _| {
                                if can_change_worktree {
                                    this.toggle_new_worktree();
                                }
                            }))
                            .child(if new_worktree {
                                "New worktree: on"
                            } else {
                                "New worktree: off"
                            }),
                    )
                    .child(
                        div()
                            .id("turn-status")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .bg(rgb(if working { 0x26354d } else { SURFACE_HIGH }))
                            .text_xs()
                            .text_color(rgb(if working { 0xaec0ff } else { MUTED }))
                            .when(working, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x31435f)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if working {
                                    this.cancel_turn();
                                    cx.notify();
                                }
                            }))
                            .child(if working { "■ Stop turn" } else { "Ready" }),
                    ),
            );

        let expanded_activity = self.expanded_activity.clone();
        let desktop = cx.entity();
        let transcript = list(self.transcript.clone(), move |index, _window, _cx| {
            let message = &messages[index];
            let key = message
                .id
                .map(|id| format!("message-{id}"))
                .unwrap_or_else(|| format!("live-{index}"));
            Self::message_row(
                message,
                index,
                expanded_activity.contains(&key),
                desktop.clone(),
            )
        })
        .size_full();

        let composer_focus = self.composer_input.read(cx).focus_handle(cx);
        let attachment_count = self.model.draft_attachments.len();
        let can_attach = attachment_count < MAX_ATTACHMENTS
            && self.model.selected_chat.is_some()
            && self.model.connected;
        let can_send = (!self.composer.trim().is_empty() || attachment_count > 0)
            && self.model.selected_chat.is_some()
            && self.model.connected
            && !self.sending;
        let shortcuts_enabled =
            self.model.selected_chat.is_some() && self.model.connected && !self.sending;
        let shortcut_buttons = self
            .model
            .shortcuts
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, prompt)| {
                let label = compact_label(&prompt, 42);
                div()
                    .id(("shortcut", index))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .text_xs()
                    .text_color(rgb(if shortcuts_enabled { TEXT } else { MUTED }))
                    .when(shortcuts_enabled, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if shortcuts_enabled {
                            this.send_shortcut(prompt.clone());
                            cx.notify();
                        }
                    }))
                    .child(label)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let queue_edit = self.queue_edit.clone();
        let queue_edit_input = self.queue_edit_input.clone();
        let queue_edit_focus = self.queue_edit_input.read(cx).focus_handle(cx);
        let selected_chat_id = self.model.selected_chat.clone().unwrap_or_default();
        let queue_rows = self
            .model
            .queue
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, prompt)| {
                let editing = queue_edit.as_ref().is_some_and(|edit| {
                    edit.chat_id == selected_chat_id
                        && edit.index == index
                        && edit.original == prompt
                });
                if editing {
                    let can_save = queue_edit.as_ref().is_some_and(|edit| {
                        edit.submitting.is_none()
                            && !edit.text.trim().is_empty()
                            && edit.text.trim() != edit.original
                    });
                    let saving = queue_edit
                        .as_ref()
                        .is_some_and(|edit| edit.submitting.is_some());
                    return div()
                        .w_full()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(0x66557d))
                        .bg(rgb(0x211c2a))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(rgb(0xc8b6e8))
                                        .child(format!("Editing queued {}", index + 1)),
                                )
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .id(("save-queue", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(if can_save { 0xb9c7ff } else { MUTED }))
                                        .when(can_save, |button| {
                                            button
                                                .cursor_pointer()
                                                .hover(|style| style.bg(rgb(0x302b3a)))
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if can_save {
                                                this.save_queue_edit(cx);
                                            }
                                        }))
                                        .child(if saving { "Saving…" } else { "Save" }),
                                )
                                .child(
                                    div()
                                        .id(("cancel-queue-edit", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x302b3a)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_queue_edit(cx);
                                        }))
                                        .child("Cancel"),
                                ),
                        )
                        .child(
                            div()
                                .id(("queue-editor", index))
                                .track_focus(&queue_edit_focus)
                                .mt_2()
                                .w_full()
                                .h(px(36.0))
                                .px_2()
                                .flex()
                                .items_center()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(if queue_edit_focus.is_focused(window) {
                                    ACCENT
                                } else {
                                    BORDER
                                }))
                                .bg(rgb(SURFACE))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let focus = this.queue_edit_input.read(cx).focus_handle(cx);
                                    window.focus(&focus);
                                }))
                                .child(queue_edit_input.clone()),
                        )
                        .into_any_element();
                }
                let steer_prompt = prompt.clone();
                let edit_prompt = prompt.clone();
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(0x3a3348))
                    .bg(rgb(0x1d1a25))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xc8b6e8))
                                    .child(format!("Queued {}", index + 1)),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(("edit-queue", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xd7cede))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x302b3a)))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.begin_queue_edit(index, edit_prompt.clone(), cx);
                                        let focus = this.queue_edit_input.read(cx).focus_handle(cx);
                                        window.focus(&focus);
                                    }))
                                    .child("Edit"),
                            )
                            .child(
                                div()
                                    .id(("steer-queue", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xb9c7ff))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x302b3a)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.steer_queued(index, &steer_prompt);
                                        cx.notify();
                                    }))
                                    .child("Send now"),
                            )
                            .child(
                                div()
                                    .id(("drop-queue", index))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(rgb(0xefaaaa))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x3b282e)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.drop_queued(index);
                                        cx.notify();
                                    }))
                                    .child("Remove"),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .max_h(px(63.0))
                            .overflow_hidden()
                            .text_sm()
                            .line_height(px(21.0))
                            .text_color(rgb(0xd7cede))
                            .child(prompt),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let draft_prompt = self.composer.trim();
        let global_saved = !draft_prompt.is_empty()
            && self
                .model
                .global_shortcuts
                .iter()
                .any(|prompt| prompt == draft_prompt);
        let workspace_saved = !draft_prompt.is_empty()
            && self
                .model
                .workspace_shortcuts
                .iter()
                .any(|prompt| prompt == draft_prompt);
        let can_edit_shortcuts = !draft_prompt.is_empty() && selected.is_some();
        let send_label = if self.sending {
            "Sending…"
        } else if working {
            "Queue"
        } else {
            "Send"
        };
        let attachment_previews = self
            .model
            .draft_attachments
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, attachment)| {
                div()
                    .w(px(88.0))
                    .p_1()
                    .rounded_lg()
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .relative()
                            .h(px(60.0))
                            .overflow_hidden()
                            .rounded_md()
                            .bg(rgb(SURFACE_HIGH))
                            .child(
                                img(attachment.preview)
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            )
                            .child(
                                div()
                                    .id(("remove-attachment", index))
                                    .absolute()
                                    .top_1()
                                    .right_1()
                                    .size(px(20.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(rgb(0x1a1d24))
                                    .text_xs()
                                    .text_color(rgb(TEXT))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_attachment(index, cx)
                                    }))
                                    .child("×"),
                            ),
                    )
                    .child(
                        div()
                            .mt_1()
                            .px_1()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .overflow_hidden()
                            .child(attachment.name),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let composer = div()
            .flex_shrink_0()
            .px_5()
            .pt_2()
            .pb_4()
            .bg(rgb(BG))
            .when_some(self.model.connection_error.clone(), |element, error| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgb(0x382126))
                        .text_xs()
                        .text_color(rgb(0xefb1b1))
                        .child(error),
                )
            })
            .when(queue_count > 0, |element| {
                element.child(
                    div()
                        .id("queue-panel")
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .max_h(px(230.0))
                        .overflow_y_scroll()
                        .p_2()
                        .rounded_lg()
                        .bg(rgb(0x24212f))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_1()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(rgb(0xc8b6e8))
                                .child(format!(
                                    "{queue_count} queued message{}",
                                    if queue_count == 1 { "" } else { "s" }
                                )),
                        )
                        .children(queue_rows),
                )
            })
            .when(!shortcut_buttons.is_empty(), |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(shortcut_buttons),
                )
            })
            .when(can_edit_shortcuts, |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .id("toggle-global-shortcut")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(SURFACE_HIGH))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x303c52)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_shortcut(false);
                                    cx.notify();
                                }))
                                .child(if global_saved {
                                    "Remove global shortcut"
                                } else {
                                    "Save as global shortcut"
                                }),
                        )
                        .child(
                            div()
                                .id("toggle-workspace-shortcut")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgb(SURFACE_HIGH))
                                .text_xs()
                                .text_color(rgb(TEXT))
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0x303c52)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_shortcut(true);
                                    cx.notify();
                                }))
                                .child(if workspace_saved {
                                    "Remove workspace shortcut"
                                } else {
                                    "Save as workspace shortcut"
                                }),
                        ),
                )
            })
            .when(attachment_count > 0, |element| {
                element.child(
                    div()
                        .w_full()
                        .max_w(px(920.0))
                        .mx_auto()
                        .mb_2()
                        .flex()
                        .gap_2()
                        .children(attachment_previews),
                )
            })
            .child(
                div()
                    .id("composer")
                    .track_focus(&composer_focus)
                    .w_full()
                    .max_w(px(920.0))
                    .mx_auto()
                    .min_h(px(74.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(if composer_focus.is_focused(window) {
                        ACCENT
                    } else {
                        BORDER
                    }))
                    .bg(rgb(SURFACE))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let focus = this.composer_input.read(cx).focus_handle(cx);
                        window.focus(&focus);
                    }))
                    .child(
                        div()
                            .id("attach")
                            .px_2()
                            .py_2()
                            .rounded_lg()
                            .text_sm()
                            .text_color(rgb(if can_attach { TEXT } else { MUTED }))
                            .when(can_attach, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(SURFACE_HIGH)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_attach {
                                    this.attach_images(cx);
                                }
                            }))
                            .child("+ Attach"),
                    )
                    .child(self.composer_input.clone())
                    .child(
                        div()
                            .id("send")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(rgb(if can_send { ACCENT } else { SURFACE_HIGH }))
                            .text_sm()
                            .text_color(rgb(if can_send { 0xffffff } else { MUTED }))
                            .when(can_send, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x7b98ff)))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if can_send {
                                    this.send_composer(cx);
                                }
                            }))
                            .child(send_label),
                    ),
            );

        div()
            .size_full()
            .flex()
            .bg(rgb(BG))
            .font_family("Inter")
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(transcript))
                    .child(composer),
            )
    }
}

fn load_png_attachments(
    paths: Vec<PathBuf>,
    available: usize,
    mut total_bytes: usize,
) -> Result<Vec<Attachment>, String> {
    if paths.len() > available {
        return Err(format!(
            "A message can contain at most {MAX_ATTACHMENTS} images."
        ));
    }
    let mut attachments = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        if metadata.len() > MAX_ATTACHMENT_BYTES as u64 {
            return Err("Each PNG must be 10 MiB or smaller.".into());
        }
        let bytes =
            fs::read(&path).map_err(|error| format!("Cannot read {}: {error}", path.display()))?;
        if total_bytes > MAX_TOTAL_ATTACHMENT_BYTES.saturating_sub(bytes.len()) {
            return Err("Attached images must stay under 20 MiB total.".into());
        }
        total_bytes += bytes.len();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image.png")
            .to_owned();
        attachments.push(Attachment::from_png(name, bytes)?);
    }
    Ok(attachments)
}

fn compact_label(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= limit {
        return compact;
    }
    let mut shortened = compact
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    shortened.push('…');
    shortened
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some("ComposerInput")),
            KeyBinding::new("delete", Delete, Some("ComposerInput")),
            KeyBinding::new("left", Left, Some("ComposerInput")),
            KeyBinding::new("right", Right, Some("ComposerInput")),
            KeyBinding::new("shift-left", SelectLeft, Some("ComposerInput")),
            KeyBinding::new("shift-right", SelectRight, Some("ComposerInput")),
            KeyBinding::new("home", Home, Some("ComposerInput")),
            KeyBinding::new("end", End, Some("ComposerInput")),
            KeyBinding::new("ctrl-a", SelectAll, Some("ComposerInput")),
            KeyBinding::new("ctrl-c", Copy, Some("ComposerInput")),
            KeyBinding::new("ctrl-x", Cut, Some("ComposerInput")),
            KeyBinding::new("ctrl-v", Paste, Some("ComposerInput")),
            KeyBinding::new("cmd-a", SelectAll, Some("ComposerInput")),
            KeyBinding::new("cmd-c", Copy, Some("ComposerInput")),
            KeyBinding::new("cmd-x", Cut, Some("ComposerInput")),
            KeyBinding::new("cmd-v", Paste, Some("ComposerInput")),
            KeyBinding::new("enter", Submit, Some("ComposerInput")),
            KeyBinding::new(
                "ctrl-cmd-space",
                ShowCharacterPalette,
                Some("ComposerInput"),
            ),
        ]);
        let bounds = Bounds::centered(None, size(px(1180.0), px(780.0)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(XdDesktop::new),
        )
        .expect("open xd GPUI window");
        cx.activate(true);
    });
}
