//! Browser commands from a chat's agent, carried to an xd desktop app, which
//! runs them in that chat's integrated browser and sends back what happened.
//!
//! A desktop offers its browser with `browser-provider` once connected. The
//! agent's tool server asks with `browser`, which waits here while the newest
//! desktop still connected gets a `browser-command` event and answers it with
//! `browser-result`. Only the desktop that was asked can answer. A command the
//! relay stops waiting for, or whose agent's connection goes, gets
//! `browser-cancel`, so the desktop does no more of it once nobody waits.

use std::{
    collections::{HashMap, HashSet},
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
    /// Connections that have stopped sending, whose commands that asked to
    /// stop with them are refused, even one read just before they stopped.
    gone: HashSet<u64>,
}

struct Waiting {
    provider: u64,
    /// The connection that asked, which the answer goes back on.
    requester: u64,
    /// Whether the command stops when that connection stops sending. A
    /// client may instead close its sending side and read on, so only one
    /// that asked counts its end as giving up.
    cancel_on_close: bool,
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
    /// instead of waiting out the timeout, and those it asked for stop.
    pub(crate) fn forget(&self, events: &EventBus, subscriber: u64) {
        {
            let mut state = self.state();
            state.providers.retain(|provider| *provider != subscriber);
            // Dropping the sender wakes the waiting call.
            state
                .waiting
                .retain(|_, waiting| waiting.provider != subscriber);
        }
        self.abandon(events, subscriber);
        // Gone for good, so nothing more of it can come.
        self.state().gone.remove(&subscriber);
    }

    /// The connection `requester` has stopped sending, as when an agent's
    /// tool server exits mid-command: the commands it asked to stop with it
    /// stop on the desktop, and any it sent last are refused.
    pub(crate) fn abandon(&self, events: &EventBus, requester: u64) {
        let mut abandoned = Vec::new();
        let mut state = self.state();
        state.gone.insert(requester);
        state.waiting.retain(|request, waiting| {
            let keep = waiting.requester != requester || !waiting.cancel_on_close;
            if !keep {
                abandoned.push((*request, waiting.provider));
            }
            keep
        });
        drop(state);
        for (request, provider) in abandoned {
            events.publish_to(
                provider,
                json!({"event": "browser-cancel", "request": request}),
            );
        }
    }

    /// Sends `command` for `chat`, asked on connection `requester`, to the
    /// newest connected desktop and waits for its answer, which becomes the
    /// daemon's reply. With `cancel_on_close`, the command stops once the
    /// requester stops sending.
    pub(crate) fn run(
        &self,
        events: &EventBus,
        requester: u64,
        cancel_on_close: bool,
        chat: &str,
        command: Value,
        timeout: Duration,
    ) -> Value {
        let (answer, answered) = sync_channel(1);
        let (request, provider) = {
            let mut state = self.state();
            if cancel_on_close && state.gone.contains(&requester) {
                return json!({"ok": false, "error": "The agent's connection closed."});
            }
            state
                .providers
                .retain(|provider| events.subscribed(*provider));
            let Some(provider) = state.providers.last().copied() else {
                return json!({"ok": false, "error": NO_DESKTOP});
            };
            state.next_request += 1;
            let request = state.next_request;
            state.waiting.insert(
                request,
                Waiting {
                    provider,
                    requester,
                    cancel_on_close,
                    answer,
                },
            );
            // Published under the lock, so a fast answer finds its request.
            events.publish_to(
                provider,
                json!({"event": "browser-command", "request": request, "chat": chat,
                    "command": command}),
            );
            (request, provider)
        };
        let reply = answered.recv_timeout(timeout);
        self.state().waiting.remove(&request);
        match reply {
            Ok(answer) => answer,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                events.publish_to(
                    provider,
                    json!({"event": "browser-cancel", "request": request}),
                );
                json!({"ok": false,
                    "error": "The xd desktop app did not finish the browser command in time, \
                        so it was stopped."})
            }
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
        let reply = relay.run(&events, 0, true, "chat", json!({}), Duration::from_secs(1));
        assert_eq!(reply["ok"], false);
        assert_eq!(reply["error"], NO_DESKTOP);
        // A desktop that offered and then left counts as none.
        let (gone, _) = subscriber(&events);
        relay.offer(gone);
        events.unsubscribe(gone);
        let reply = relay.run(&events, 0, true, "chat", json!({}), Duration::from_secs(1));
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
                    0,
                    true,
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
            thread::spawn(move || {
                relay.run(&events, 0, true, "c", json!({}), Duration::from_secs(60))
            })
        };
        desktop_events.recv_timeout(Duration::from_secs(5)).unwrap();
        relay.forget(&events, desktop);
        let reply = running.join().unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("disconnected"));
    }

    #[test]
    fn a_command_not_answered_in_time_is_cancelled_on_the_desktop() {
        let events = EventBus::default();
        let relay = BrowserRelay::default();
        let (desktop, desktop_events) = subscriber(&events);
        relay.offer(desktop);
        let reply = relay.run(&events, 0, true, "c", json!({}), Duration::from_millis(50));
        assert_eq!(reply["ok"], false);
        let command = desktop_events.try_recv().unwrap();
        let cancel = desktop_events.try_recv().unwrap();
        assert_eq!(cancel["event"], "browser-cancel");
        assert_eq!(cancel["request"], command["request"]);
        // Too late to answer now.
        assert_eq!(
            relay.answer(
                desktop,
                &json!({"request": command["request"], "result": {}})
            )["ok"],
            false
        );
    }

    #[test]
    fn an_agent_that_goes_mid_command_stops_it_on_the_desktop() {
        let events = Arc::new(EventBus::default());
        let relay = Arc::new(BrowserRelay::default());
        let (desktop, desktop_events) = subscriber(&events);
        relay.offer(desktop);
        let (agent, _) = subscriber(&events);
        let running = {
            let (events, relay) = (events.clone(), relay.clone());
            thread::spawn(move || {
                relay.run(
                    &events,
                    agent,
                    true,
                    "c",
                    json!({}),
                    Duration::from_secs(60),
                )
            })
        };
        let command = desktop_events.recv_timeout(Duration::from_secs(5)).unwrap();
        // Another connection's leaving touches nothing.
        relay.abandon(&events, desktop + agent + 1);
        assert!(desktop_events.try_recv().is_err());
        relay.abandon(&events, agent);
        let cancel = desktop_events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(cancel["event"], "browser-cancel");
        assert_eq!(cancel["request"], command["request"]);
        assert_eq!(running.join().unwrap()["ok"], false);
        // A command it sent just before going, read only now, never starts.
        let late = relay.run(&events, agent, true, "c", json!({}), Duration::from_secs(1));
        assert_eq!(late["error"], "The agent's connection closed.");
        assert!(desktop_events.try_recv().is_err());
        // Once the connection is gone for good, its number is forgotten.
        relay.forget(&events, agent);
        assert!(!relay.state().gone.contains(&agent));
    }

    #[test]
    fn a_client_that_only_stops_sending_still_gets_its_answer() {
        let events = Arc::new(EventBus::default());
        let relay = Arc::new(BrowserRelay::default());
        let (desktop, desktop_events) = subscriber(&events);
        relay.offer(desktop);
        let (client, _) = subscriber(&events);
        let running = {
            let (events, relay) = (events.clone(), relay.clone());
            thread::spawn(move || {
                relay.run(
                    &events,
                    client,
                    false,
                    "c",
                    json!({}),
                    Duration::from_secs(60),
                )
            })
        };
        let command = desktop_events.recv_timeout(Duration::from_secs(5)).unwrap();
        // It closed its sending side, and reads on.
        relay.abandon(&events, client);
        assert!(desktop_events.try_recv().is_err());
        relay.answer(
            desktop,
            &json!({"request": command["request"], "result": {"text": "Done."}}),
        );
        assert_eq!(running.join().unwrap()["ok"], true);
    }

    #[test]
    fn a_desktop_error_reaches_the_agent() {
        let events = Arc::new(EventBus::default());
        let relay = Arc::new(BrowserRelay::default());
        let (desktop, desktop_events) = subscriber(&events);
        relay.offer(desktop);
        let running = {
            let (events, relay) = (events.clone(), relay.clone());
            thread::spawn(move || {
                relay.run(&events, 0, true, "c", json!({}), Duration::from_secs(60))
            })
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
