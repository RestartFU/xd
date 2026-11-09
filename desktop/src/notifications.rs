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
    /// A shown notification whose time is up, by its id and the `Notify`
    /// call that showed it.
    Close(u32, NonZeroU32),
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
        closes.map(|(id, serial)| Input::Close(id, serial)),
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
                // A notification of ours was shown: close it when its time is up.
                if let BusEvent::Reply {
                    serial,
                    id: Some(id),
                } = &event
                    && tracker.sent.contains_key(serial)
                {
                    let (due, id, serial) = (due.clone(), *id, *serial);
                    thread::spawn(move || {
                        thread::sleep(close_after);
                        let _ = due.try_send((id, serial));
                    });
                }
                if let Some(activation) = tracker.apply(event) {
                    let _ = activations.send(activation).await;
                }
            }
            Input::Bus(Err(error)) => return Err(error),
            Input::BusClosed => return Ok(()),
            // Only while the id is still that notification's: one clicked or
            // closed is gone, and a notification server that restarted can
            // give the same id to a newer one.
            Input::Close(id, serial) => {
                if tracker.shows(id, serial) {
                    connection.send(&close_message(id)?).await?;
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

fn close_message(id: u32) -> zbus::Result<Message> {
    Message::method_call(OBJECT_PATH, "CloseNotification")?
        .destination(SERVICE)?
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

/// What is known about each notification between sending it and it closing.
struct Tracker<T> {
    /// `Notify` calls awaiting the id the server gives them, by call serial.
    sent: HashMap<NonZeroU32, T>,
    shown: HashMap<u32, Shown<T>>,
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
    /// Whether `id` is still the notification the `Notify` call `serial` showed.
    fn shows(&self, id: u32, serial: NonZeroU32) -> bool {
        self.shown
            .get(&id)
            .is_some_and(|shown| shown.serial == serial)
    }

    fn apply(&mut self, event: BusEvent) -> Option<Activation<T>> {
        match event {
            BusEvent::Reply { serial, id } => {
                if let (Some(target), Some(id)) = (self.sent.remove(&serial), id) {
                    self.shown.insert(
                        id,
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
                if let Some(shown) = self.shown.get_mut(&id) {
                    shown.token = Some(token);
                }
                None
            }
            BusEvent::Action { id, key } if key == DEFAULT_ACTION => {
                self.shown.remove(&id).map(|shown| Activation {
                    target: shown.target,
                    token: shown.token,
                })
            }
            BusEvent::Action { .. } => None,
            BusEvent::Closed { id } => {
                self.shown.remove(&id);
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
            tracker.apply(BusEvent::Reply {
                serial: serial(1),
                id: Some(id),
            }),
            None
        );
        tracker
    }

    #[test]
    fn clicking_reports_the_target_with_its_activation_token() {
        let mut tracker = shown(7, "chat");
        tracker.apply(BusEvent::Token {
            id: 7,
            token: "token".into(),
        });
        assert_eq!(
            tracker.apply(BusEvent::Action {
                id: 7,
                key: DEFAULT_ACTION.into(),
            }),
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
            tracker.apply(BusEvent::Action {
                id: 7,
                key: DEFAULT_ACTION.into(),
            }),
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
        assert_eq!(tracker.apply(other_action), None);
        let someone_elses_click = BusEvent::Action {
            id: 8,
            key: DEFAULT_ACTION.into(),
        };
        assert_eq!(tracker.apply(someone_elses_click), None);

        tracker.apply(BusEvent::Closed { id: 7 });
        let after_closing = BusEvent::Action {
            id: 7,
            key: DEFAULT_ACTION.into(),
        };
        assert_eq!(tracker.apply(after_closing), None);
    }

    #[test]
    fn a_rejected_notification_is_forgotten() {
        let mut tracker = Tracker::default();
        tracker.sent.insert(serial(1), "chat");
        tracker.apply(BusEvent::Reply {
            serial: serial(1),
            id: None,
        });
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

    /// A notification server that shows a notification, restarts, and gives
    /// the next one the same id, as a restarted server may. It reports each
    /// close xd asks for, with how long after the second was shown it came.
    async fn close_like_a_restarted_server(address: &str) -> zbus::Result<Vec<(u32, Duration)>> {
        let server = zbus::connection::Builder::address(address)?
            .name(SERVICE)?
            .build()
            .await?;
        let mut calls = MessageStream::from(&server);
        let client = zbus::connection::Builder::address(address)?.build().await?;
        let (requests, received) = async_channel::unbounded();
        let (activations, _clicked) = async_channel::unbounded();
        let request = |target| Request {
            body: "body".to_owned(),
            target,
        };
        requests.send(request("first")).await.unwrap();
        let notifier = Box::pin(serve(client, received, activations, TEST_CLOSE_AFTER));
        let shell = Box::pin(async {
            let mut shown = 0;
            let mut second_shown = None;
            let mut closes = Vec::new();
            loop {
                let message = calls.next().await.unwrap()?;
                match message.header().member().map(|m| m.to_string()).as_deref() {
                    Some("Notify") => {
                        server.reply(&message.header(), &7u32).await?;
                        shown += 1;
                        if shown == 1 {
                            // Halfway to the first one's close, a restarted
                            // server shows the next under the same id.
                            sleep(TEST_CLOSE_AFTER / 2).await;
                            requests.send(request("second")).await.unwrap();
                        } else {
                            second_shown = Some(std::time::Instant::now());
                        }
                    }
                    Some("CloseNotification") => {
                        let id = message.body().deserialize::<u32>()?;
                        let since = second_shown.map(|at| at.elapsed()).unwrap_or_default();
                        closes.push((id, since));
                        // Long enough for a second close, if one were coming.
                        sleep(TEST_CLOSE_AFTER).await;
                        return Ok::<_, zbus::Error>(closes);
                    }
                    _ => {}
                }
            }
        });
        match futures_util::future::select(notifier, shell).await {
            futures_util::future::Either::Left((result, _)) => panic!("notifier ended: {result:?}"),
            futures_util::future::Either::Right((closes, _)) => closes,
        }
    }

    #[test]
    fn a_notification_is_closed_when_its_own_time_is_up() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipped: dbus-daemon is not available");
            return;
        };
        let (done, result) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(
                zbus::block_on(close_like_a_restarted_server(&bus.address))
                    .map_err(|e| e.to_string()),
            );
        });
        let closes = result
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the notification was never closed")
            .unwrap();
        // The first one's timer finds its id taken by the second and leaves
        // it alone; the second is closed only once its own time is up.
        assert_eq!(closes.len(), 1, "{closes:?}");
        let (id, since) = closes[0];
        assert_eq!(id, 7);
        assert!(
            since >= TEST_CLOSE_AFTER - Duration::from_millis(50),
            "{since:?}"
        );
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
