use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, Weak},
    thread,
    time::Duration,
};

use serde_json::json;

use crate::{EventBus, StateStore};

const MAX_CHATS: usize = 8;
const INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct Entry {
    signature: Option<String>,
    touched: u64,
}

#[derive(Default, Debug)]
struct Tracker {
    entries: HashMap<String, Entry>,
    clock: u64,
}

impl Tracker {
    fn watch(&mut self, chat_id: &str) {
        self.clock = self.clock.saturating_add(1);
        let signature = self
            .entries
            .get(chat_id)
            .and_then(|entry| entry.signature.clone());
        self.entries.insert(
            chat_id.to_owned(),
            Entry {
                signature,
                touched: self.clock,
            },
        );
        if self.entries.len() > MAX_CHATS
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touched)
                .map(|(chat_id, _)| chat_id.clone())
        {
            self.entries.remove(&oldest);
        }
    }

    fn reset(&mut self, chat_id: &str) {
        if let Some(entry) = self.entries.get_mut(chat_id) {
            entry.signature = None;
        }
    }

    fn chat_ids(&self) -> Vec<String> {
        self.entries.keys().cloned().collect()
    }

    fn apply(&mut self, chat_id: &str, signature: String) -> bool {
        let Some(entry) = self.entries.get_mut(chat_id) else {
            return false;
        };
        let changed = entry
            .signature
            .as_ref()
            .is_some_and(|previous| previous != &signature);
        entry.signature = Some(signature);
        changed
    }
}

struct Shared {
    tracker: Mutex<Tracker>,
    wake: Condvar,
    closed: Mutex<bool>,
}

pub(crate) struct RepositoryMonitor {
    shared: Option<Arc<Shared>>,
}

#[derive(Clone)]
pub(crate) struct RepositoryMonitorHandle(Weak<Shared>);

impl RepositoryMonitor {
    pub(crate) fn disabled() -> Self {
        Self { shared: None }
    }

    pub(crate) fn new(store: Arc<StateStore>, events: Arc<EventBus>) -> Self {
        let shared = Arc::new(Shared {
            tracker: Mutex::new(Tracker::default()),
            wake: Condvar::new(),
            closed: Mutex::new(false),
        });
        let worker_shared = shared.clone();
        let _ = thread::Builder::new()
            .name("xd-repository-monitor".into())
            .spawn(move || run(worker_shared, store, events));
        Self {
            shared: Some(shared),
        }
    }

    pub(crate) fn watch(&self, chat_id: &str) {
        let Some(shared) = &self.shared else {
            return;
        };
        if let Ok(mut tracker) = shared.tracker.lock() {
            tracker.watch(chat_id);
        }
        shared.wake.notify_one();
    }

    pub(crate) fn handle(&self) -> RepositoryMonitorHandle {
        RepositoryMonitorHandle(self.shared.as_ref().map(Arc::downgrade).unwrap_or_default())
    }
}

impl Drop for RepositoryMonitor {
    fn drop(&mut self) {
        let Some(shared) = &self.shared else {
            return;
        };
        if let Ok(mut closed) = shared.closed.lock() {
            *closed = true;
        }
        shared.wake.notify_one();
    }
}

impl RepositoryMonitorHandle {
    pub(crate) fn reset(&self, chat_id: &str) {
        let Some(shared) = self.0.upgrade() else {
            return;
        };
        if let Ok(mut tracker) = shared.tracker.lock() {
            tracker.reset(chat_id);
        }
    }
}

fn run(shared: Arc<Shared>, store: Arc<StateStore>, events: Arc<EventBus>) {
    loop {
        let Ok(closed) = shared.closed.lock() else {
            return;
        };
        let Ok((closed, _)) = shared.wake.wait_timeout(closed, INTERVAL) else {
            return;
        };
        if *closed {
            return;
        }
        drop(closed);

        let chat_ids = match shared.tracker.lock() {
            Ok(tracker) => tracker.chat_ids(),
            Err(_) => return,
        };
        for chat_id in chat_ids {
            let signature = store.repository_head_signature(&chat_id);
            let changed = match shared.tracker.lock() {
                Ok(mut tracker) => tracker.apply(&chat_id, signature),
                Err(_) => return,
            };
            if changed {
                events.publish(json!({"event": "repository-changed", "chat": chat_id}));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_signature_seeds_and_later_changes_emit() {
        let mut tracker = Tracker::default();
        tracker.watch("chat");
        assert!(!tracker.apply("chat", "one".into()));
        assert!(!tracker.apply("chat", "one".into()));
        assert!(tracker.apply("chat", "two".into()));
        tracker.reset("chat");
        assert!(!tracker.apply("chat", "three".into()));
    }

    #[test]
    fn watch_keeps_only_the_eight_most_recent_chats() {
        let mut tracker = Tracker::default();
        for index in 0..MAX_CHATS {
            tracker.watch(&format!("chat-{index}"));
        }
        tracker.watch("chat-0");
        tracker.watch("new-chat");
        let chats = tracker.chat_ids();
        assert!(chats.contains(&"chat-0".to_owned()));
        assert!(chats.contains(&"new-chat".to_owned()));
        assert!(!chats.contains(&"chat-1".to_owned()));
        assert_eq!(chats.len(), MAX_CHATS);
    }
}
