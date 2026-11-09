//! Clickable desktop notifications through the freedesktop D-Bus service.
//!
//! A click can only come back to whoever is still on the bus, so xd owns the
//! notification rather than handing it to a short-lived `notify-send`. GNOME
//! also ties a notification naming its `desktop-entry` to the life of the
//! sender, and would take it away as soon as such a helper exited.

use std::{collections::HashMap, future::ready, num::NonZeroU32, pin::pin, thread, time::Duration};

use async_channel::{Receiver, Sender};
use futures_util::{StreamExt as _, stream};
use gpui::{BackgroundExecutor, Window};
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{ConnectionExt as _, PropMode},
    wrapper::ConnectionExt as _,
};
use zbus::{Connection, Message, MessageStream, message::Type, zvariant::Value};

const SERVICE: &str = "org.freedesktop.Notifications";
const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
const DEFAULT_ACTION: &str = "default";
const APP_NAME: &str = "xd";
/// How long a completion notification stays up, in milliseconds.
const EXPIRE_TIMEOUT: i32 = 10_000;
/// When xd closes a notification itself: some servers keep one past its
/// expiry, GNOME while the screen is locked for one.
const CLOSE_AFTER: Duration = Duration::from_secs(10);

/// A notification the user clicked.
#[derive(Debug, PartialEq)]
pub(crate) struct Activation<T> {
    pub(crate) target: T,
    /// Proof for the window manager that the user asked for this, if the
    /// notification server provided one.
    pub(crate) token: Option<String>,
}

struct Request<T> {
    body: String,
    target: T,
}

/// Shows notifications, and reports clicks on them to the receiver from [`Notifier::new`].
pub(crate) struct Notifier<T> {
    requests: Option<Sender<Request<T>>>,
    activations: Sender<Activation<T>>,
}

impl<T: Send + 'static> Notifier<T> {
    pub(crate) fn new() -> (Self, Receiver<Activation<T>>) {
        let (activations, clicked) = async_channel::unbounded();
        (
            Self {
                requests: None,
                activations,
            },
            clicked,
        )
    }

    /// `body` is notification markup; see [`turn_finished_body`].
    ///
    /// Connects on first use, and again after a connection has been lost.
    pub(crate) fn show(&mut self, body: String, target: T, executor: &BackgroundExecutor) {
        let mut request = Request { body, target };
        if let Some(requests) = &self.requests {
            match requests.try_send(request) {
                Ok(()) => return,
                Err(error) => request = error.into_inner(),
            }
        }
        let (requests, received) = async_channel::unbounded();
        let _ = requests.try_send(request);
        self.requests = Some(requests);
        let activations = self.activations.clone();
        executor
            .spawn(async move {
                // Notifications are a courtesy. Without a session bus, or once
                // it is gone, there is nowhere to show them or report it.
                let _ = run(received, activations).await;
            })
            .detach();
    }
}

enum Input<T> {
    Request(Request<T>),
    Bus(zbus::Result<Message>),
    BusClosed,
    /// A shown notification whose time is up, by its key and the `Notify`
    /// call that showed it.
    Close(Key, NonZeroU32),
}

async fn run<T>(
    requests: Receiver<Request<T>>,
    activations: Sender<Activation<T>>,
) -> zbus::Result<()> {
    serve(
        Connection::session().await?,
        requests,
        activations,
        CLOSE_AFTER,
    )
    .await
}

async fn serve<T>(
    connection: Connection,
    requests: Receiver<Request<T>>,
    activations: Sender<Activation<T>>,
    close_after: Duration,
) -> zbus::Result<()> {
    // One stream carries the replies to `Notify` as well as the signals, in bus
    // order, so a click can never be seen before the id it refers to.
    let messages = MessageStream::from(&connection);
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &format!("type='signal',sender='{SERVICE}',interface='{SERVICE}',path='{OBJECT_PATH}'"),
        )
        .await?;
    let desktop_entry = xd_desktop::channel::app_id();
    let mut tracker = Tracker::default();
    // The requests alone must not keep this going: `Notifier::show` can only
    // tell that the connection is gone once they have stopped being received.
    let messages = messages
        .map(Input::Bus)
        .chain(stream::once(ready(Input::BusClosed)));
    let (due, closes) = async_channel::unbounded();
    let mut inputs = pin!(stream::select(
        stream::select(requests.map(Input::Request), messages),
        closes.map(|(key, serial)| Input::Close(key, serial)),
    ));
    while let Some(input) = inputs.next().await {
        match input {
            Input::Request(Request { body, target }) => {
                let message = notify_message(&body, &desktop_entry)?;
                let serial = message.primary_header().serial_num();
                tracker.sent.insert(serial, target);
                connection.send(&message).await?;
            }
            Input::Bus(Ok(message)) => {
                let Some(event) = decode(&message) else {
                    continue;
                };
                // Ids are the server connection's that handed them out: one
                // that restarts, or takes over the service, hands them out
                // again, possibly to other applications.
                let sender = message.header().sender().map(|sender| sender.to_string());
                // A notification of ours was shown: close it when its time is up.
                if let BusEvent::Reply {
                    serial,
                    id: Some(id),
                } = &event
                    && tracker.sent.contains_key(serial)
                {
                    let (due, key, serial) = (due.clone(), (sender.clone(), *id), *serial);
                    thread::spawn(move || {
                        thread::sleep(close_after);
                        let _ = due.try_send((key, serial));
                    });
                }
                if let Some(activation) = tracker.apply(event, sender.as_deref()) {
                    let _ = activations.send(activation).await;
                }
            }
            Input::Bus(Err(error)) => return Err(error),
            Input::BusClosed => return Ok(()),
            // Only while the id is still that notification's: one clicked or
            // closed is gone. Sent to the server connection that showed it, so
            // a server that has taken over the service never closes its own.
            Input::Close(key, serial) => {
                if tracker.shows(&key, serial) {
                    tracker.shown.remove(&key);
                    let (owner, id) = key;
                    connection
                        .send(&close_message(id, owner.as_deref())?)
                        .await?;
                }
            }
        }
    }
    Ok(())
}

fn notify_message(body: &str, desktop_entry: &str) -> zbus::Result<Message> {
    // A `default` action is what makes clicking the notification itself report
    // back. Without one GNOME would only try to open the app on its own.
    let actions = [DEFAULT_ACTION, "Open"];
    // Transient: a completion alert stays out of the notification history.
    let hints = HashMap::from([
        ("desktop-entry", Value::from(desktop_entry)),
        ("transient", Value::from(true)),
    ]);
    Message::method_call(OBJECT_PATH, "Notify")?
        .destination(SERVICE)?
        .interface(SERVICE)?
        .build(&(
            APP_NAME,
            0u32,
            "",
            APP_NAME,
            body,
            &actions[..],
            hints,
            EXPIRE_TIMEOUT,
        ))
}

fn close_message(id: u32, owner: Option<&str>) -> zbus::Result<Message> {
    Message::method_call(OBJECT_PATH, "CloseNotification")?
        .destination(owner.unwrap_or(SERVICE))?
        .interface(SERVICE)?
        .build(&(id,))
}

#[derive(Debug, PartialEq)]
enum BusEvent {
    Reply { serial: NonZeroU32, id: Option<u32> },
    Token { id: u32, token: String },
    Action { id: u32, key: String },
    Closed { id: u32 },
}

fn decode(message: &Message) -> Option<BusEvent> {
    let header = message.header();
    let body = message.body();
    match message.message_type() {
        Type::MethodReturn => Some(BusEvent::Reply {
            serial: header.reply_serial()?,
            id: body.deserialize().ok(),
        }),
        Type::Error => Some(BusEvent::Reply {
            serial: header.reply_serial()?,
            id: None,
        }),
        Type::Signal if header.interface()?.as_str() == SERVICE => {
            match header.member()?.as_str() {
                "ActivationToken" => {
                    let (id, token) = body.deserialize().ok()?;
                    Some(BusEvent::Token { id, token })
                }
                "ActionInvoked" => {
                    let (id, key) = body.deserialize().ok()?;
                    Some(BusEvent::Action { id, key })
                }
                "NotificationClosed" => {
                    let (id, _reason): (u32, u32) = body.deserialize().ok()?;
                    Some(BusEvent::Closed { id })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

struct Shown<T> {
    target: T,
    token: Option<String>,
    /// The `Notify` call that showed it.
    serial: NonZeroU32,
}

/// A shown notification: the unique bus name of the server connection that
/// showed it, and the id that connection gave it.
type Key = (Option<String>, u32);

/// What is known about each notification between sending it and it closing.
struct Tracker<T> {
    /// `Notify` calls awaiting the id the server gives them, by call serial.
    sent: HashMap<NonZeroU32, T>,
    shown: HashMap<Key, Shown<T>>,
}

impl<T> Default for Tracker<T> {
    fn default() -> Self {
        Self {
            sent: HashMap::new(),
            shown: HashMap::new(),
        }
    }
}

impl<T> Tracker<T> {
    /// Whether `key` is still the notification the `Notify` call `serial` showed.
    fn shows(&self, key: &Key, serial: NonZeroU32) -> bool {
        self.shown
            .get(key)
            .is_some_and(|shown| shown.serial == serial)
    }

    /// `sender` is the unique bus name the event came from: a notification
    /// is only ever spoken for by the connection that showed it.
    fn apply(&mut self, event: BusEvent, sender: Option<&str>) -> Option<Activation<T>> {
        let shown_as = |id| (sender.map(str::to_owned), id);
        match event {
            BusEvent::Reply { serial, id } => {
                if let (Some(target), Some(id)) = (self.sent.remove(&serial), id) {
                    self.shown.insert(
                        shown_as(id),
                        Shown {
                            target,
                            token: None,
                            serial,
                        },
                    );
                }
                None
            }
            // The specification sends this just before the action it is for.
            BusEvent::Token { id, token } => {
                if let Some(shown) = self.shown.get_mut(&shown_as(id)) {
                    shown.token = Some(token);
                }
                None
            }
            BusEvent::Action { id, key } if key == DEFAULT_ACTION => {
                self.shown.remove(&shown_as(id)).map(|shown| Activation {
                    target: shown.target,
                    token: shown.token,
                })
            }
            BusEvent::Action { .. } => None,
            BusEvent::Closed { id } => {
                self.shown.remove(&shown_as(id));
                None
            }
        }
    }
}

/// Notification bodies are markup, and chat titles are not.
pub(crate) fn turn_finished_body(title: &str) -> String {
    let title = title
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!("{title} finished")
}

/// Brings the window to the front on behalf of a notification click.
pub(crate) fn raise_window(window: &Window, token: Option<&str>) {
    // Without a timestamp Mutter only focuses a window on the current workspace.
    // Elsewhere it would announce "xd is ready" and leave the user where they
    // are. Presenting the token as the window's startup id makes it a proper,
    // user-initiated activation, which also follows the window across workspaces.
    if let Some(token) = token
        && let Ok(x11_window) = crate::browser::find_x11_parent()
    {
        let _ = present_x11_startup_id(x11_window, token);
    }
    // Still needed: other window managers may not act on the startup id.
    window.activate_window();
}

fn present_x11_startup_id(
    window: u32,
    token: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (connection, _) = x11rb::connect(None)?;
    let property = connection
        .intern_atom(false, b"_NET_STARTUP_ID")?
        .reply()?
        .atom;
    let utf8 = connection.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
    connection
        .change_property8(PropMode::REPLACE, window, property, utf8, token.as_bytes())?
        .check()?;
    connection.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERFACE: &str = SERVICE;

    type NotifyArguments<'a> = (
        String,
        u32,
        String,
        String,
        String,
        Vec<String>,
        HashMap<String, Value<'a>>,
        i32,
    );

    fn signal<B>(member: &str, body: &B) -> Message
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        Message::signal(OBJECT_PATH, INTERFACE, member)
            .unwrap()
            .build(body)
            .unwrap()
    }

    fn serial(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }

    fn shown(id: u32, target: &'static str) -> Tracker<&'static str> {
        let mut tracker = Tracker::default();
        tracker.sent.insert(serial(1), target);
        assert_eq!(
            tracker.apply(
                BusEvent::Reply {
                    serial: serial(1),
                    id: Some(id),
                },
                None
            ),
            None
        );
        tracker
    }

    #[test]
    fn clicking_reports_the_target_with_its_activation_token() {
        let mut tracker = shown(7, "chat");
        tracker.apply(
            BusEvent::Token {
                id: 7,
                token: "token".into(),
            },
            None,
        );
        assert_eq!(
            tracker.apply(
                BusEvent::Action {
                    id: 7,
                    key: DEFAULT_ACTION.into(),
                },
                None
            ),
            Some(Activation {
                target: "chat",
                token: Some("token".into()),
            })
        );
        assert!(tracker.shown.is_empty());
    }

    #[test]
    fn clicking_without_a_token_still_reports_the_target() {
        let mut tracker = shown(7, "chat");
        assert_eq!(
            tracker.apply(
                BusEvent::Action {
                    id: 7,
                    key: DEFAULT_ACTION.into(),
                },
                None
            ),
            Some(Activation {
                target: "chat",
                token: None,
            })
        );
    }

    #[test]
    fn other_events_do_not_open_anything() {
        let mut tracker = shown(7, "chat");
        let other_action = BusEvent::Action {
            id: 7,
            key: "other".into(),
        };
        assert_eq!(tracker.apply(other_action, None), None);
        let someone_elses_click = BusEvent::Action {
            id: 8,
            key: DEFAULT_ACTION.into(),
        };
        assert_eq!(tracker.apply(someone_elses_click, None), None);

        tracker.apply(BusEvent::Closed { id: 7 }, None);
        let after_closing = BusEvent::Action {
            id: 7,
            key: DEFAULT_ACTION.into(),
        };
        assert_eq!(tracker.apply(after_closing, None), None);
    }

    #[test]
    fn a_rejected_notification_is_forgotten() {
        let mut tracker = Tracker::default();
        tracker.sent.insert(serial(1), "chat");
        tracker.apply(
            BusEvent::Reply {
                serial: serial(1),
                id: None,
            },
            None,
        );
        assert!(tracker.sent.is_empty());
        assert!(tracker.shown.is_empty());
    }

    #[test]
    fn decodes_the_signals_it_follows() {
        assert_eq!(
            decode(&signal("ActivationToken", &(7u32, "token"))),
            Some(BusEvent::Token {
                id: 7,
                token: "token".into(),
            })
        );
        assert_eq!(
            decode(&signal("ActionInvoked", &(7u32, "default"))),
            Some(BusEvent::Action {
                id: 7,
                key: "default".into(),
            })
        );
        assert_eq!(
            decode(&signal("NotificationClosed", &(7u32, 2u32))),
            Some(BusEvent::Closed { id: 7 })
        );
        assert_eq!(decode(&signal("Unrelated", &(7u32,))), None);
    }

    #[test]
    fn ignores_signals_of_other_interfaces() {
        let message = Message::signal(OBJECT_PATH, "org.example.Other", "ActionInvoked")
            .unwrap()
            .build(&(7u32, "default"))
            .unwrap();
        assert_eq!(decode(&message), None);
    }

    #[test]
    fn the_notification_asks_for_a_click_and_names_its_desktop_entry() {
        let message = notify_message("body", "com.restartfu.Xd").unwrap();
        let header = message.header();
        assert_eq!(header.member().unwrap().as_str(), "Notify");
        assert_eq!(header.interface().unwrap().as_str(), SERVICE);
        let body = message.body();
        let (app_name, replaces, _icon, _summary, text, actions, hints, timeout): NotifyArguments =
            body.deserialize().unwrap();
        assert_eq!(app_name, "xd");
        assert_eq!(replaces, 0);
        assert_eq!(text, "body");
        assert_eq!(actions, ["default", "Open"]);
        assert_eq!(
            hints.get("desktop-entry"),
            Some(&Value::from("com.restartfu.Xd"))
        );
        assert_eq!(hints.get("transient"), Some(&Value::from(true)));
        assert_eq!(timeout, 10_000);
    }

    /// A private bus, for as long as the machine has a `dbus-daemon` to run one.
    struct PrivateBus {
        daemon: std::process::Child,
        address: String,
    }

    impl PrivateBus {
        fn start() -> Option<Self> {
            let mut daemon = std::process::Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()?;
            let mut address = String::new();
            std::io::BufRead::read_line(
                &mut std::io::BufReader::new(daemon.stdout.take()?),
                &mut address,
            )
            .ok()?;
            Some(Self {
                daemon,
                address: address.trim().to_owned(),
            })
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    /// The conversation GNOME Shell has with the sender of a clicked notification.
    async fn click_like_gnome(address: &str) -> zbus::Result<Activation<&'static str>> {
        let server = zbus::connection::Builder::address(address)?
            .name(SERVICE)?
            .build()
            .await?;
        let mut calls = MessageStream::from(&server);
        let client = zbus::connection::Builder::address(address)?.build().await?;
        let (requests, received) = async_channel::unbounded();
        let (activations, clicked) = async_channel::unbounded();
        requests
            .send(Request {
                body: "body".to_owned(),
                target: "chat",
            })
            .await
            .unwrap();
        let notifier = Box::pin(serve(client, received, activations, CLOSE_AFTER));
        let shell = Box::pin(async {
            let notify = loop {
                let message = calls.next().await.unwrap()?;
                if message.header().member().is_some_and(|m| m == "Notify") {
                    break message;
                }
            };
            assert_eq!(
                notify.body().signature().to_string_no_parens(),
                "susssasa{sv}i"
            );
            let header = notify.header();
            let sender = header.sender().unwrap().to_owned();
            server.reply(&header, &42u32).await?;
            // The token goes to the sender alone. The action is broadcast here,
            // because only the match rule can bring that back to the notifier.
            server
                .emit_signal(
                    Some(sender.as_str()),
                    OBJECT_PATH,
                    SERVICE,
                    "ActivationToken",
                    &(42u32, "token"),
                )
                .await?;
            server
                .emit_signal(
                    None::<()>,
                    OBJECT_PATH,
                    SERVICE,
                    "ActionInvoked",
                    &(42u32, DEFAULT_ACTION),
                )
                .await?;
            Ok::<_, zbus::Error>(clicked.recv().await.unwrap())
        });
        match futures_util::future::select(notifier, shell).await {
            futures_util::future::Either::Left((result, _)) => panic!("notifier ended: {result:?}"),
            futures_util::future::Either::Right((activation, _)) => activation,
        }
    }

    const TEST_CLOSE_AFTER: Duration = Duration::from_millis(400);

    /// Waits without holding up the other futures on the same executor.
    async fn sleep(duration: Duration) {
        let (done, finished) = async_channel::bounded(1);
        thread::spawn(move || {
            thread::sleep(duration);
            let _ = done.try_send(());
        });
        let _ = finished.recv().await;
    }

    /// The next call the notification server receives with that name.
    async fn next_call(calls: &mut MessageStream, member: &str) -> zbus::Result<Message> {
        loop {
            let message = calls.next().await.unwrap()?;
            if message.header().member().is_some_and(|m| m == member) {
                return Ok(message);
            }
        }
    }

    /// The ids of every `CloseNotification` the server receives for a while.
    async fn closes_during(calls: &mut MessageStream, window: Duration) -> zbus::Result<Vec<u32>> {
        let mut closes = Vec::new();
        let mut deadline = Box::pin(sleep(window));
        loop {
            match futures_util::future::select(Box::pin(calls.next()), deadline.as_mut()).await {
                futures_util::future::Either::Left((Some(message), _)) => {
                    let message = message?;
                    if message
                        .header()
                        .member()
                        .is_some_and(|m| m == "CloseNotification")
                    {
                        closes.push(message.body().deserialize::<u32>()?);
                    }
                }
                futures_util::future::Either::Left((None, _))
                | futures_util::future::Either::Right(_) => return Ok(closes),
            }
        }
    }

    /// Runs `server` against a notifier on the bus at `address`, which xd has
    /// asked to show `shown` notifications, until `server` finishes.
    async fn against_notifier<F, R>(
        address: &str,
        server: impl FnOnce(Sender<Request<&'static str>>, Receiver<Activation<&'static str>>) -> F,
    ) -> zbus::Result<R>
    where
        F: std::future::Future<Output = zbus::Result<R>>,
    {
        let client = zbus::connection::Builder::address(address)?.build().await?;
        let (requests, received) = async_channel::unbounded();
        let (activations, clicked) = async_channel::unbounded();
        let notifier = Box::pin(serve(client, received, activations, TEST_CLOSE_AFTER));
        match futures_util::future::select(notifier, Box::pin(server(requests, clicked))).await {
            futures_util::future::Either::Left((result, _)) => panic!("notifier ended: {result:?}"),
            futures_util::future::Either::Right((outcome, _)) => outcome,
        }
    }

    fn request(target: &'static str) -> Request<&'static str> {
        Request {
            body: "body".to_owned(),
            target,
        }
    }

    async fn notification_server(address: &str) -> zbus::Result<(Connection, MessageStream)> {
        let server = zbus::connection::Builder::address(address)?
            .name(SERVICE)?
            .build()
            .await?;
        let calls = MessageStream::from(&server);
        Ok((server, calls))
    }

    fn on_a_private_bus<R: Send + 'static>(
        run: impl FnOnce(String) -> zbus::Result<R> + Send + 'static,
    ) -> Option<R> {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipped: dbus-daemon is not available");
            return None;
        };
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(run(bus.address.clone()).map_err(|e| e.to_string()));
            drop(bus);
        });
        Some(
            result
                .recv_timeout(std::time::Duration::from_secs(20))
                .expect("the notification server never finished")
                .unwrap(),
        )
    }

    #[test]
    fn a_notification_is_closed_when_its_own_time_is_up() {
        // A notification, then halfway to its close the next one under the
        // same id: the first one's timer must leave the id alone, and the
        // second is closed once, when its own time is up.
        let outcome = on_a_private_bus(|address| {
            zbus::block_on(async move {
                let (server, mut calls) = notification_server(&address).await?;
                against_notifier(&address, |requests, _clicked| async move {
                    requests.send(request("first")).await.unwrap();
                    let first = next_call(&mut calls, "Notify").await?;
                    server.reply(&first.header(), &7u32).await?;
                    sleep(TEST_CLOSE_AFTER / 2).await;
                    requests.send(request("second")).await.unwrap();
                    let second = next_call(&mut calls, "Notify").await?;
                    server.reply(&second.header(), &7u32).await?;
                    let shown = std::time::Instant::now();
                    let close = next_call(&mut calls, "CloseNotification").await?;
                    let since = shown.elapsed();
                    let mut closes = vec![close.body().deserialize::<u32>()?];
                    closes.extend(closes_during(&mut calls, TEST_CLOSE_AFTER).await?);
                    Ok((closes, since))
                })
                .await
            })
        });
        let Some((closes, since)) = outcome else {
            return;
        };
        assert_eq!(closes, [7]);
        assert!(
            since >= TEST_CLOSE_AFTER - Duration::from_millis(50),
            "{since:?}"
        );
    }

    #[test]
    fn notifications_of_a_server_that_restarted_are_left_alone() {
        // The ids a restarted server hands out may be another application's:
        // neither the first one's timer nor a click on the new server's
        // notification with the same id acts on it.
        let closes = on_a_private_bus(|address| {
            zbus::block_on(async move {
                let (server, mut calls) = notification_server(&address).await?;
                let restart_at = address.clone();
                against_notifier(&address, |requests, clicked| async move {
                    requests.send(request("first")).await.unwrap();
                    let first = next_call(&mut calls, "Notify").await?;
                    server.reply(&first.header(), &7u32).await?;
                    server.release_name(SERVICE).await?;
                    let (restarted, mut calls) = notification_server(&restart_at).await?;
                    // Its own notification 7, clicked.
                    restarted
                        .emit_signal(
                            None::<()>,
                            OBJECT_PATH,
                            SERVICE,
                            "ActionInvoked",
                            &(7u32, DEFAULT_ACTION),
                        )
                        .await?;
                    let closes = closes_during(&mut calls, TEST_CLOSE_AFTER * 2).await?;
                    Ok((closes, clicked.try_recv().ok()))
                })
                .await
            })
        });
        if let Some((closes, click)) = closes {
            assert_eq!(closes, Vec::<u32>::new());
            assert_eq!(click, None);
        }
    }

    #[test]
    fn overlapping_servers_keep_their_own_notifications() {
        // A server that took over the service gives id 7 to xd's second
        // notification; the one it replaced then answers the first, late,
        // with id 7 as well. Each stays its own server's.
        let outcome = on_a_private_bus(|address| {
            zbus::block_on(async move {
                let (old, mut old_calls) = notification_server(&address).await?;
                let takeover_at = address.clone();
                against_notifier(&address, |requests, clicked| async move {
                    requests.send(request("first")).await.unwrap();
                    let first = next_call(&mut old_calls, "Notify").await?;
                    old.release_name(SERVICE).await?;
                    let (new, mut new_calls) = notification_server(&takeover_at).await?;
                    requests.send(request("second")).await.unwrap();
                    let second = next_call(&mut new_calls, "Notify").await?;
                    new.reply(&second.header(), &7u32).await?;
                    old.reply(&first.header(), &7u32).await?;
                    new.emit_signal(
                        None::<()>,
                        OBJECT_PATH,
                        SERVICE,
                        "ActionInvoked",
                        &(7u32, DEFAULT_ACTION),
                    )
                    .await?;
                    let click = clicked.recv().await.ok();
                    let closes = closes_during(&mut new_calls, TEST_CLOSE_AFTER * 2).await?;
                    Ok((click.map(|click| click.target), closes))
                })
                .await
            })
        });
        let Some((click, closes)) = outcome else {
            return;
        };
        assert_eq!(click, Some("second"));
        // The new server's notification was clicked, so it is gone: its timer
        // has nothing left to close there, and the old one's goes to the old.
        assert_eq!(closes, Vec::<u32>::new());
    }

    #[test]
    fn a_click_comes_back_over_a_real_bus() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipped: dbus-daemon is not available");
            return;
        };
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done
                .send(zbus::block_on(click_like_gnome(&bus.address)).map_err(|e| e.to_string()));
        });
        let activation = result
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the click never came back")
            .unwrap();
        assert_eq!(
            activation,
            Activation {
                target: "chat",
                token: Some("token".to_owned()),
            }
        );
    }

    #[test]
    fn losing_the_bus_ends_the_worker_so_the_next_notification_reconnects() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipped: dbus-daemon is not available");
            return;
        };
        let address = bus.address.clone();
        let (requests, received) = async_channel::unbounded::<Request<()>>();
        let (activations, _clicked) = async_channel::unbounded();
        let (ended, worker_ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = zbus::block_on(async move {
                let connection = zbus::connection::Builder::address(address.as_str())?
                    .build()
                    .await?;
                serve(connection, received, activations, CLOSE_AFTER).await
            });
            let _ = ended.send(());
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(worker_ended.try_recv().is_err(), "the bus is still up");

        drop(bus);

        worker_ended
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the worker outlived the bus");
        // This failing is what sends `Notifier::show` to a new connection.
        assert!(requests.is_closed());
    }

    #[test]
    fn chat_titles_cannot_inject_markup() {
        assert_eq!(
            turn_finished_body("a <b>&\nc"),
            "a &lt;b&gt;&amp;c finished"
        );
    }
}
