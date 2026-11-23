//! Programs a runner runs on the origin, for an origin's MCP servers and
//! hooks that cannot run anywhere else: a Python environment, a checkout too
//! large to carry, a server that drives something only the origin has.
//!
//! The origin holds a link to the runner (`origin-link`) while it is online.
//! On the runner, the profile's configuration starts such a program through
//! `xd-host origin-exec`, which asks for it with `origin-exec-start` by the id
//! the origin gave it. The runner never names a program: the origin looks the
//! id up in its own configuration and refuses any it did not offer. Standard
//! input travels to the origin as `origin-exec-input` and `origin-exec-eof`;
//! output comes back as `origin-exec-output` and the exit as
//! `origin-exec-exit`, each delivered only to the connection it belongs to.
//!
//! When either end goes, the other is told: a program whose origin left exits
//! with an error, and one whose requester left is killed on the origin.

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, PoisonError},
};

use serde_json::{Value, json};

use crate::EventBus;

const NO_ORIGIN: &str = "The origin that runs this is not connected to the runner. Open xd on \
that device (or run `xd-host profile link` there), then try again.";

#[derive(Default)]
pub(crate) struct OriginRelay {
    state: Mutex<RelayState>,
}

#[derive(Default)]
struct RelayState {
    /// The connection each origin is linked through.
    links: HashMap<String, u64>,
    next_exec: u64,
    execs: HashMap<u64, Exec>,
}

struct Exec {
    link: u64,
    requester: u64,
}

impl OriginRelay {
    fn state(&self) -> MutexGuard<'_, RelayState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `subscriber` now speaks for `origin`; the newest link wins.
    pub(crate) fn link(&self, origin: &str, subscriber: u64) -> Value {
        self.state().links.insert(origin.to_owned(), subscriber);
        json!({"ok": true, "origin": origin})
    }

    pub(crate) fn linked(&self, events: &EventBus, origin: &str) -> bool {
        self.state()
            .links
            .get(origin)
            .is_some_and(|link| events.subscribed(*link))
    }

    /// Starts the program the origin offered as `command` for `requester`.
    pub(crate) fn start(
        &self,
        events: &EventBus,
        requester: u64,
        origin: &str,
        command: &str,
    ) -> Value {
        if command.is_empty()
            || command.len() > 64
            || !command.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return json!({"ok": false, "error": "That is not a program id the origin offered."});
        }
        let mut state = self.state();
        let Some(link) = state
            .links
            .get(origin)
            .copied()
            .filter(|link| events.subscribed(*link))
        else {
            return json!({"ok": false, "error": NO_ORIGIN});
        };
        state.next_exec += 1;
        let exec = state.next_exec;
        state.execs.insert(exec, Exec { link, requester });
        // Under the lock, so output that comes back at once finds its exec.
        events.publish_to(
            link,
            json!({"event": "origin-exec-start", "exec": exec, "command": command}),
        );
        json!({"ok": true, "exec": exec})
    }

    /// Standard input from the requester, or its end with `data` absent.
    pub(crate) fn input(&self, events: &EventBus, requester: u64, request: &Value) -> Value {
        let Some(exec) = request["exec"].as_u64() else {
            return json!({"ok": false, "error": "origin-exec-input needs an exec."});
        };
        let state = self.state();
        let Some(running) = state
            .execs
            .get(&exec)
            .filter(|running| running.requester == requester)
        else {
            return json!({"ok": false, "error": "That program is not running."});
        };
        let event = match request["data"].as_str() {
            Some(data) => json!({"event": "origin-exec-input", "exec": exec, "data": data}),
            None => json!({"event": "origin-exec-eof", "exec": exec}),
        };
        events.publish_to(running.link, event);
        json!({"ok": true})
    }

    /// Output or the exit from the origin running `exec`.
    pub(crate) fn output(&self, events: &EventBus, link: u64, request: &Value) -> Value {
        let Some(exec) = request["exec"].as_u64() else {
            return json!({"ok": false, "error": "origin-exec-output needs an exec."});
        };
        let exited = request["op"] == "origin-exec-exit";
        let mut state = self.state();
        let Some(running) = state
            .execs
            .get(&exec)
            .filter(|running| running.link == link)
        else {
            // Ended at the other side already; nothing waits for it.
            return json!({"ok": true, "dropped": true});
        };
        let requester = running.requester;
        let event = if exited {
            state.execs.remove(&exec);
            let mut event = json!({"event": "origin-exec-exit", "exec": exec,
                "code": request["code"].as_i64().unwrap_or(1)});
            if let Some(error) = request["error"].as_str() {
                event["error"] = json!(error);
            }
            event
        } else {
            json!({"event": "origin-exec-output", "exec": exec,
                "stream": if request["stream"] == "stderr" { "stderr" } else { "stdout" },
                "data": request["data"].as_str().unwrap_or_default()})
        };
        events.publish_to(requester, event);
        json!({"ok": true})
    }

    /// `subscriber` is gone: programs on an origin it linked exit, and
    /// programs it asked for stop on their origin.
    pub(crate) fn closed(&self, events: &EventBus, subscriber: u64) {
        let mut state = self.state();
        state.links.retain(|_, link| *link != subscriber);
        let mut exits = Vec::new();
        let mut kills = Vec::new();
        state.execs.retain(|exec, running| {
            if running.link == subscriber {
                exits.push((*exec, running.requester));
                false
            } else if running.requester == subscriber {
                kills.push((*exec, running.link));
                false
            } else {
                true
            }
        });
        drop(state);
        for (exec, requester) in exits {
            events.publish_to(
                requester,
                json!({"event": "origin-exec-exit", "exec": exec, "code": 1,
                    "error": "The origin disconnected."}),
            );
        }
        for (exec, link) in kills {
            events.publish_to(link, json!({"event": "origin-exec-kill", "exec": exec}));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, sync_channel};

    use super::*;

    fn subscriber(events: &EventBus) -> (u64, Receiver<Value>) {
        let (sender, receiver) = sync_channel(16);
        let (connection, _) = crate::UnixStream::pair().unwrap();
        (events.subscribe(sender, connection).unwrap(), receiver)
    }

    #[test]
    fn a_program_runs_on_the_linked_origin_and_its_output_reaches_only_its_requester() {
        let events = EventBus::default();
        let relay = OriginRelay::default();
        let (origin, from_runner) = subscriber(&events);
        let (shim, to_shim) = subscriber(&events);
        let (other, to_other) = subscriber(&events);
        assert_eq!(relay.start(&events, shim, "laptop", "abc")["ok"], false);
        relay.link("laptop", origin);
        let started = relay.start(&events, shim, "laptop", "abc");
        let exec = started["exec"].as_u64().unwrap();
        let start = from_runner.try_recv().unwrap();
        assert_eq!(start["event"], "origin-exec-start");
        assert_eq!(start["command"], "abc");

        relay.input(&events, shim, &json!({"exec": exec, "data": "aGk="}));
        assert_eq!(from_runner.try_recv().unwrap()["data"], "aGk=");
        // Only the requester feeds it, and only the origin answers for it.
        assert_eq!(relay.input(&events, other, &json!({"exec": exec, "data": "eA=="}))["ok"], false);
        relay.output(&events, other, &json!({"op": "origin-exec-output", "exec": exec, "data": "eA=="}));
        assert!(to_shim.try_recv().is_err());

        relay.output(&events, origin, &json!({"op": "origin-exec-output", "exec": exec, "data": "b2s="}));
        assert_eq!(to_shim.try_recv().unwrap()["data"], "b2s=");
        relay.input(&events, shim, &json!({"exec": exec}));
        assert_eq!(from_runner.try_recv().unwrap()["event"], "origin-exec-eof");
        relay.output(&events, origin, &json!({"op": "origin-exec-exit", "exec": exec, "code": 3}));
        let exit = to_shim.try_recv().unwrap();
        assert_eq!(exit["event"], "origin-exec-exit");
        assert_eq!(exit["code"], 3);
        assert!(to_other.try_recv().is_err());
    }

    #[test]
    fn either_end_leaving_ends_the_program_at_the_other() {
        let events = EventBus::default();
        let relay = OriginRelay::default();
        let (origin, from_runner) = subscriber(&events);
        let (shim, to_shim) = subscriber(&events);
        relay.link("laptop", origin);
        let first = relay.start(&events, shim, "laptop", "a")["exec"].as_u64().unwrap();
        from_runner.try_recv().unwrap();
        relay.closed(&events, shim);
        let kill = from_runner.try_recv().unwrap();
        assert_eq!(kill["event"], "origin-exec-kill");
        assert_eq!(kill["exec"], first);

        let (shim, to_shim2) = subscriber(&events);
        drop(to_shim);
        relay.start(&events, shim, "laptop", "a");
        relay.closed(&events, origin);
        let exit = to_shim2.try_recv().unwrap();
        assert_eq!(exit["event"], "origin-exec-exit");
        assert_eq!(exit["error"], "The origin disconnected.");
        assert_eq!(relay.start(&events, shim, "laptop", "a")["ok"], false);
    }

    #[test]
    fn refuses_ids_that_could_name_a_program() {
        let events = EventBus::default();
        let relay = OriginRelay::default();
        let (origin, _from_runner) = subscriber(&events);
        relay.link("laptop", origin);
        for command in ["", "/bin/sh", "a b", "../x", &"a".repeat(65)] {
            assert_eq!(relay.start(&events, 7, "laptop", command)["ok"], false, "{command}");
        }
    }
}
