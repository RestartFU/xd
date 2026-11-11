//! Browser commands from a chat's agent, carried to an xd desktop app, which
//! runs them in that chat's integrated browser and sends back what happened.
//!
//! A desktop offers its browser with `browser-provider` once connected. The
//! agent's tool server asks with `browser`, which waits here while the newest
//! desktop still connected gets a `browser-command` event and answers it with
//! `browser-result`. Only the desktop that was asked can answer.

use std::{
    collections::HashMap,
    sync::{
        Mutex, MutexGuard, PoisonError,
        mpsc::{SyncSender, sync_channel},
    },
    time::Duration,
};

use serde_json::{Value, json};

use crate::EventBus;

/// How long the agent's tool waits for the desktop. The desktop gives up on a
/// page well before this, so reaching it means the desktop is stuck or gone.
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// What the tool says when no desktop can show the browser.
const NO_DESKTOP: &str = "No xd desktop app with the integrated browser is connected to this \
host. Ask the user to open xd on their computer, then try again.";

#[derive(Default)]
pub(crate) struct BrowserRelay {
    state: Mutex<RelayState>,
}

#[derive(Default)]
struct RelayState {
    /// Connections offering a browser, in the order they offered it.
    providers: Vec<u64>,
    next_request: u64,
    /// Commands sent and not yet answered, by request number.
    waiting: HashMap<u64, Waiting>,
}

struct Waiting {
    provider: u64,
    answer: SyncSender<Value>,
}

impl BrowserRelay {
    fn state(&self) -> MutexGuard<'_, RelayState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `subscriber` can run browser commands; it is asked before any desktop
    /// that offered earlier.
    pub(crate) fn offer(&self, subscriber: u64) {
        let mut state = self.state();
        state.providers.retain(|provider| *provider != subscriber);
        state.providers.push(subscriber);
    }

    /// The connection closed. Commands it had not answered fail at once
    /// instead of waiting out the timeout.
    pub(crate) fn forget(&self, subscriber: u64) {
        let mut state = self.state();
        state.providers.retain(|provider| *provider != subscriber);
        // Dropping the sender wakes the waiting call.
        state
            .waiting
            .retain(|_, waiting| waiting.provider != subscriber);
    }

    /// Sends `command` for `chat` to the newest connected desktop and waits
    /// for its answer, which becomes the daemon's reply.
    pub(crate) fn run(
        &self,
        events: &EventBus,
        chat: &str,
        command: Value,
        timeout: Duration,
    ) -> Value {
        let (answer, answered) = sync_channel(1);
        let request = {
            let mut state = self.state();
            state
                .providers
                .retain(|provider| events.subscribed(*provider));
            let Some(provider) = state.providers.last().copied() else {
                return json!({"ok": false, "error": NO_DESKTOP});
            };
            state.next_request += 1;
            let request = state.next_request;
            state.waiting.insert(request, Waiting { provider, answer });
            // Published under the lock, so a fast answer finds its request.
            events.publish_to(
                provider,
                json!({"event": "browser-command", "request": request, "chat": chat,
                    "command": command}),
            );
            request
        };
        let reply = answered.recv_timeout(timeout);
        self.state().waiting.remove(&request);
        match reply {
            Ok(answer) => answer,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => json!({"ok": false,
                "error": "The xd desktop app did not finish the browser command in time."}),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => json!({"ok": false,
                "error": "The xd desktop app disconnected before the browser command finished."}),
        }
    }

    /// The desktop on `subscriber` answered `request`. An answer from any
    /// other connection, or for a command no longer waiting, is refused.
    pub(crate) fn answer(&self, subscriber: u64, request: &Value) -> Value {
        let Some(number) = request.get("request").and_then(Value::as_u64) else {
            return json!({"ok": false, "error": "browser-result needs a request number."});
        };
        let waiting = {
            let mut state = self.state();
            match state.waiting.get(&number) {
                Some(waiting) if waiting.provider == subscriber => state.waiting.remove(&number),
                _ => None,
            }
        };
        let Some(waiting) = waiting else {
            return json!({"ok": false, "error": "No browser command is waiting for that answer."});
        };
        let answer = match (
            request.get("error").and_then(Value::as_str),
            request.get("result"),
        ) {
            (Some(error), _) => json!({"ok": false, "error": error}),
            (None, Some(result)) => json!({"ok": true, "result": result}),
            (None, None) => json!({"ok": false, "error": "The browser command had no result."}),
        };
        let _ = waiting.answer.try_send(answer);
        json!({"ok": true})
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc::Receiver},
        thread,
    };

    use super::*;

    fn subscriber(events: &EventBus) -> (u64, Receiver<Value>) {
        let (sender, receiver) = sync_channel(8);
        let (connection, _) = crate::UnixStream::pair().unwrap();
        (events.subscribe(sender, connection).unwrap(), receiver)
    }

    #[test]
    fn without_a_desktop_the_agent_is_told_to_open_xd() {
        let events = EventBus::default();
        let relay = BrowserRelay::default();
        let reply = relay.run(&events, "chat", json!({}), Duration::from_secs(1));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"], NO_DESKTOP);
        // A desktop that offered and then left counts as none.
        let (gone, _) = subscriber(&events);
        relay.offer(gone);
        events.unsubscribe(gone);
        let reply = relay.run(&events, "chat", json!({}), Duration::from_secs(1));
        assert_eq!(reply["error"], NO_DESKTOP);
    }

    #[test]
    fn the_newest_desktop_runs_the_command_and_only_it_can_answer() {
        let events = Arc::new(EventBus::default());
        let relay = Arc::new(BrowserRelay::default());
        let (older, older_events) = subscriber(&events);
        let (newer, newer_events) = subscriber(&events);
        relay.offer(older);
        relay.offer(newer);
        let running = {
            let (events, relay) = (events.clone(), relay.clone());
            thread::spawn(move || {
                relay.run(
                    &events,
                    "chat-1",
                    json!({"action": "snapshot"}),
                    Duration::from_secs(10),
                )
            })
        };
        let event = newer_events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(event["event"], "browser-command");
        assert_eq!(event["chat"], "chat-1");
        assert_eq!(event["command"], json!({"action": "snapshot"}));
        assert!(older_events.try_recv().is_err());
        let request = event["request"].clone();
        let refused = relay.answer(older, &json!({"request": request, "result": {}}));
        assert_eq!(refused["ok"], false);
        let accepted = relay.answer(
            newer,
            &json!({"request": request, "result": {"text": "hi"}}),
        );
        assert_eq!(accepted["ok"], true);
        assert_eq!(
            running.join().unwrap(),
            json!({"ok": true, "result": {"text": "hi"}})
        );
        // Answered once only.
        assert_eq!(
            relay.answer(newer, &json!({"request": request, "result": {}}))["ok"],
            false
        );
    }

    #[test]
    fn a_desktop_that_leaves_fails_its_commands_at_once() {
        let events = Arc::new(EventBus::default());
        let relay = Arc::new(BrowserRelay::default());
        let (desktop, desktop_events) = subscriber(&events);
        relay.offer(desktop);
        let running = {
            let (events, relay) = (events.clone(), relay.clone());
            thread::spawn(move || relay.run(&events, "c", json!({}), Duration::from_secs(60)))
        };
        desktop_events.recv_timeout(Duration::from_secs(5)).unwrap();
        relay.forget(desktop);
        let reply = running.join().unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("disconnected"));
    }

    #[test]
    fn a_desktop_error_reaches_the_agent() {
        let events = Arc::new(EventBus::default());
        let relay = Arc::new(BrowserRelay::default());
        let (desktop, desktop_events) = subscriber(&events);
        relay.offer(desktop);
        let running = {
            let (events, relay) = (events.clone(), relay.clone());
            thread::spawn(move || relay.run(&events, "c", json!({}), Duration::from_secs(60)))
        };
        let event = desktop_events.recv_timeout(Duration::from_secs(5)).unwrap();
        relay.answer(
            desktop,
            &json!({"request": event["request"], "error": "No element e9."}),
        );
        assert_eq!(
            running.join().unwrap(),
            json!({"ok": false, "error": "No element e9."})
        );
    }
}
