use crate::HostOptions;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, UdpSocket},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use xd_host::{Engine, PeerEndpoint, StateStore, serve_daemon_connection};
use xd_transport::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};

const START_TIMEOUT: Duration = Duration::from_secs(15);
/// The daemon flag for a daemon the xd app started, which stops when it closes.
pub(super) const STOP_WITH_APP: &str = "--stop-with-app";
/// Set by the xd app on the bridge it runs, so the bridge tells the daemon
/// which connection is the app's and starts it with `--stop-with-app`.
const APP_BRIDGE: &str = "XD_APP_BRIDGE";
/// How long a daemon the app started waits for the app to come back once its
/// last connection closed. It covers a reconnect and an app restart.
const APP_CLOSE_GRACE: Duration = Duration::from_secs(10);
const MAX_CLIENTS: usize = 64;
/// A child forked just before its daemon died keeps the lock and the listening
/// socket until its `exec` closes them, so a busy lock can outlive the daemon.
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_HOLDER_ANSWER_WAIT: Duration = Duration::from_millis(200);
/// How often the daemon checks that its socket and lock files still exist.
const FILE_CHECK_INTERVAL: Duration = Duration::from_secs(1);
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Release);
}

struct Paths {
    socket: PathBuf,
    lock: PathBuf,
    data: PathBuf,
}
fn paths(options: &HostOptions) -> io::Result<Paths> {
    let parent = options
        .database
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let data = fs::canonicalize(parent)?;
    let database = data.join(
        options
            .database
            .file_name()
            .ok_or_else(|| io::Error::other("Invalid database path"))?,
    );
    let digest = format!(
        "{:x}",
        Sha256::digest(database.as_os_str().as_encoded_bytes())
    );
    let runtime = PathBuf::from(format!("/tmp/xd-{}", unsafe { libc::geteuid() }));
    private_directory(&runtime)?;
    Ok(Paths {
        socket: runtime.join(format!("daemon-{}.sock", &digest[..24])),
        lock: runtime.join(format!("daemon-{}.lock", &digest[..24])),
        data,
    })
}
fn private_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "Daemon directory must be owned by you and private (0700).",
        ));
    }
    // A restrictive umask may remove owner access at creation. Every caller
    // restores it after verifying ownership and rejecting public directories.
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
struct Singleton {
    lock: File,
    lock_path: PathBuf,
    socket: PathBuf,
    /// The socket file this daemon listens on, once it listens.
    listening: Option<(u64, u64)>,
}
fn identity(metadata: &fs::Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}
fn names(path: &Path, expected: Option<(u64, u64)>) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| Some(identity(&metadata)) == expected)
}
impl Singleton {
    fn listen(&mut self) -> io::Result<UnixListener> {
        let listener = UnixListener::bind(&self.socket)?;
        fs::set_permissions(&self.socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        self.listening = Some(identity(&fs::symlink_metadata(&self.socket)?));
        Ok(listener)
    }
    /// Clearing `/tmp` removes the socket and lock files while the daemon
    /// keeps running. Clients could no longer reach it, and the next one to
    /// start a daemon would serve the same database a second time. This puts
    /// both back and returns the listener on the new socket. It returns
    /// nothing when both are intact.
    fn restore(&mut self) -> io::Result<Option<UnixListener>> {
        let lock = self
            .lock
            .metadata()
            .map(|metadata| identity(&metadata))
            .ok();
        let lock_intact = names(&self.lock_path, lock);
        if lock_intact && names(&self.socket, self.listening) {
            return Ok(None);
        }
        if let Some(runtime) = self.socket.parent() {
            private_directory(runtime)?;
        }
        if !lock_intact {
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&self.lock_path)?;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(io::Error::other(
                    "another daemon took over this database's lock after its files were removed",
                ));
            }
            self.lock = lock;
        }
        // Holding the lock, anything left at the socket path is stale.
        match fs::symlink_metadata(&self.socket) {
            Ok(metadata) => {
                use std::os::unix::fs::FileTypeExt;
                if !metadata.file_type().is_socket() {
                    return Err(io::Error::other(
                        "Daemon socket path is occupied by a non-socket file.",
                    ));
                }
                fs::remove_file(&self.socket)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.listen().map(Some)
    }
}
impl Drop for Singleton {
    fn drop(&mut self) {
        // Never another daemon's socket, after this one's was removed.
        if self.listening.is_none() || names(&self.socket, self.listening) {
            let _ = fs::remove_file(&self.socket);
        }
    }
}
fn singleton(paths: &Paths, wait: Duration) -> io::Result<Option<Singleton>> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&paths.lock)?;
    let deadline = Instant::now() + wait;
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock {
            return Err(error);
        }
        // Only a daemon that is really running can answer, and then there is
        // nothing to wait for. Staying could even start a second one after it
        // is asked to stop.
        if Instant::now() >= deadline || daemon_answers(&paths.socket) {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
    match fs::symlink_metadata(&paths.socket) {
        Ok(metadata) => {
            use std::os::unix::fs::FileTypeExt;
            if !metadata.file_type().is_socket() {
                return Err(io::Error::other(
                    "Daemon socket path is occupied by a non-socket file.",
                ));
            }
            fs::remove_file(&paths.socket)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(Some(Singleton {
        lock,
        lock_path: paths.lock.clone(),
        socket: paths.socket.clone(),
        listening: None,
    }))
}

fn daemon_answers(socket: &Path) -> bool {
    // A Unix socket has no connect timeout, and connecting to a full queue
    // blocks. A thread left waiting ends with the listener or with this process.
    let socket = socket.to_owned();
    let (answered, answer) = mpsc::channel();
    thread::spawn(move || {
        let _ = answered.send(ask_for_an_answer(&socket));
    });
    answer
        .recv_timeout(LOCK_HOLDER_ANSWER_WAIT)
        .unwrap_or(false)
}

fn ask_for_an_answer(socket: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return false;
    };
    let mut answer = [0];
    stream
        .set_read_timeout(Some(LOCK_HOLDER_ANSWER_WAIT))
        .is_ok()
        && writeln!(stream, "{}", json!({"op": "ping"})).is_ok()
        && stream.read(&mut answer).is_ok_and(|read| read > 0)
}

pub(super) fn run(options: HostOptions, stop_with_app: bool) -> Result<(), String> {
    run_inner(options, stop_with_app).map_err(|e| e.to_string())
}
fn app_close_grace() -> Duration {
    // Tests shorten it.
    std::env::var("XD_APP_CLOSE_GRACE_MS")
        .ok()
        .and_then(|millis| millis.parse().ok())
        .map_or(APP_CLOSE_GRACE, Duration::from_millis)
}
fn run_inner(options: HostOptions, stop_with_app: bool) -> io::Result<()> {
    let paths = paths(&options)?;
    let Some(mut singleton) = singleton(&paths, LOCK_WAIT)? else {
        return Ok(());
    };
    // Read now, while the path names this binary: an update replaces it there.
    if let Ok(executable) = std::env::current_exe().and_then(fs::canonicalize) {
        let database = paths
            .data
            .join(options.database.file_name().unwrap_or_default());
        let workspaces =
            fs::canonicalize(&options.workspaces).unwrap_or(options.workspaces.clone());
        xd_host::offer_link_tool(executable, database, workspaces);
    }
    let store =
        StateStore::open(&options.database, &options.workspaces).map_err(io::Error::other)?;
    store
        .recover_interrupted_turns()
        .map_err(io::Error::other)?;
    let engine = Arc::new(Engine::with_store_and_data(store, Some(paths.data.clone())));
    engine.start_auto_settle();
    engine.start_account_switching();
    engine.start_chat_quotas();
    engine.start_idle_compaction();
    engine.start_reporting_agents();
    engine.deliver_waiting_agent_reports();
    #[cfg(target_os = "linux")]
    engine.start_memory_guard();
    if stop_with_app {
        engine.stop_with_app();
    }
    let mut listener = singleton.listen()?;
    let peers = Arc::new(PeerServer::new(&paths.data, Arc::downgrade(&engine))?);
    let peer_server = peers.clone();
    engine
        .set_peer_listener(move |bind, port| {
            peer_server.listen(bind, port).map_err(|e| e.to_string())
        })
        .map_err(io::Error::other)?;
    if let Some((bind, port)) = engine.remote_listener()
        && let Err(error) = peers.listen(&bind, port)
    {
        eprintln!("xd-host: cannot restore remote listener: {error}");
    }
    unsafe {
        libc::signal(
            libc::SIGTERM,
            stop_signal as *const () as libc::sighandler_t,
        );
        libc::signal(libc::SIGINT, stop_signal as *const () as libc::sighandler_t);
    }
    let mut clients = Vec::<thread::JoinHandle<()>>::new();
    let app_grace = app_close_grace();
    let mut installed_at = None::<Instant>;
    let mut checked_files = Instant::now();
    let mut restore_error = None::<String>;
    while !STOP.load(Ordering::Acquire) && !engine.shutdown_requested() {
        clients.retain(|client| !client.is_finished());
        if checked_files.elapsed() >= FILE_CHECK_INTERVAL {
            checked_files = Instant::now();
            match singleton.restore() {
                Ok(None) => restore_error = None,
                Ok(Some(restored)) => {
                    eprintln!("xd-host: its socket was removed, so it listens on a new one");
                    listener = restored;
                    restore_error = None;
                }
                Err(error) => {
                    let error = error.to_string();
                    if restore_error.as_ref() != Some(&error) {
                        eprintln!("xd-host: cannot restore its removed socket: {error}");
                        restore_error = Some(error);
                    }
                }
            }
        }
        if stop_with_app {
            // An update the app asked for finishes before the host stops.
            if engine.installing_update() {
                installed_at = Some(Instant::now());
            }
            // The bridge that started this daemon connects right after it
            // listens, and the engine counts from its start until then.
            if engine
                .app_absent_for()
                .is_some_and(|absent| absent >= app_grace)
                && installed_at.is_none_or(|at| at.elapsed() >= app_grace)
            {
                eprintln!("xd-host: stopping, because the xd app that started it closed");
                break;
            }
        }
        match listener.accept() {
            Ok((stream, _)) if clients.len() < MAX_CLIENTS => {
                // Accepted sockets inherit nonblocking mode on macOS.
                // macOS also refuses socket options once the peer has closed.
                // That ends this connection only, never the daemon.
                if stream
                    .set_nonblocking(false)
                    .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(15))))
                    .is_err()
                {
                    continue;
                }
                let engine = engine.clone();
                clients.push(thread::spawn(move || {
                    let _ = serve_daemon_connection(stream, engine, false);
                }));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            // A client that gave up while waiting to be accepted.
            Err(error) if error.kind() == io::ErrorKind::ConnectionAborted => {}
            Err(error) => return Err(error),
        }
    }
    // Let the stop acknowledgement reach its caller before disconnecting it.
    thread::sleep(Duration::from_millis(100));
    peers.stop.store(true, Ordering::Release);
    engine.shutdown();
    for client in clients {
        let _ = client.join();
    }
    Ok(())
}

/// Connects to the daemon, starting it when `start` is set. `app` starts it so
/// that it stops when the xd app closes.
pub(super) fn connect(options: &HostOptions, start: bool, app: bool) -> io::Result<UnixStream> {
    let paths = paths(options)?;
    if let Ok(stream) = UnixStream::connect(&paths.socket) {
        return Ok(stream);
    }
    if !start {
        return Err(io::Error::other("The xd daemon is not running."));
    }
    let log = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(paths.data.join("daemon.log"))?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["daemon", "--database"])
        .arg(&options.database)
        .arg("--workspaces")
        .arg(&options.workspaces)
        .args(app.then_some(STOP_WITH_APP))
        // Agents and shells the daemon starts are not the app.
        .env_remove(APP_BRIDGE)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .current_dir(&paths.data);
    // Detach from the window/terminal session so closing the client cannot kill work.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.socket) {
            thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(stream);
        }
        if let Some(status) = child.try_wait()? {
            // Another concurrent launcher may have won the singleton lock.
            if !status.success() {
                return Err(io::Error::other(format!(
                    "Daemon exited with {status}. See {}",
                    paths.data.join("daemon.log").display()
                )));
            }
        }
        if Instant::now() > deadline {
            return Err(io::Error::other(format!(
                "Daemon startup timed out. See {}",
                paths.data.join("daemon.log").display()
            )));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn bridge(options: HostOptions) -> Result<(), String> {
    let app = std::env::var_os(APP_BRIDGE).is_some_and(|value| value == "1");
    let mut stream = connect(&options, true, app).map_err(|e| e.to_string())?;
    let mut info = rpc(&mut stream, "daemon-info").map_err(|e| e.to_string())?;
    // A restart keeps the daemon's arguments, so it cannot fix this mismatch.
    check_workspaces(&options, &info)?;
    // Only a daemon the app started stops when it closes. One started any
    // other way keeps running, and an older one refuses this. It is sent
    // before any replacement: a busy daemon refuses to restart, and the app's
    // retries must keep it running until it can.
    if app {
        let _ = rpc(&mut stream, "app-client");
    }
    if !matches_this_build(&info) {
        (stream, info) = replace_stale_daemon(&options, stream, &info)?;
        check_workspaces(&options, &info)?;
        if app {
            let _ = rpc(&mut stream, "app-client");
        }
    }
    let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
    let close = stream.try_clone().map_err(|e| e.to_string())?;
    thread::spawn(move || {
        let _ = io::copy(&mut io::stdin(), &mut stream);
        let _ = stream.shutdown(Shutdown::Write);
    });
    let result = io::copy(&mut reader, &mut io::stdout())
        .map(|_| ())
        .map_err(|e| e.to_string());
    let _ = close.shutdown(Shutdown::Both);
    result
}
fn check_workspaces(options: &HostOptions, info: &Value) -> Result<(), String> {
    let expected = fs::canonicalize(&options.workspaces)
        .map_err(|_| "The running daemon uses a different workspace directory.".to_owned())?;
    if info["workspaces"].as_str() != expected.to_str() {
        return Err("This database is already served with a different workspace directory. Stop that daemon before changing its workspace root.".into());
    }
    Ok(())
}
fn matches_this_build(info: &Value) -> bool {
    info["protocol"] == 2
        && info["version"] == env!("CARGO_PKG_VERSION")
        && info["build"] == option_env!("XD_COMMIT").unwrap_or("development")
}
/// An update replaces the installed files but leaves the previous daemon
/// running, so the next client finds a build it cannot talk to. The daemon's
/// own update restart refuses while turns, background agents or shells are
/// live, which keeps this from interrupting work. Clients retry, so the upgrade
/// completes once idle.
fn replace_stale_daemon(
    options: &HostOptions,
    mut stale: UnixStream,
    info: &Value,
) -> Result<(UnixStream, Value), String> {
    // A daemon this has to start while the old one re-executes takes the old
    // one's place, so it keeps its lifetime rather than this bridge's.
    let stops_with_app = info["stops_with_app"] == true;
    let stop = stop_command(options);
    let status = request(&mut stale, json!({"op": "host-update"})).unwrap_or_default();
    // The Windows app's daemon in WSL belongs to no installation. A daemon
    // of this version restarts from the copy the app replaced all the same;
    // an earlier one cannot, and is stopped instead, once it is idle. Only one
    // the app started: that daemon already stops whenever the app stays away,
    // so its work was never promised to outlive the app. One started by hand
    // from the same path keeps running until its owner stops it.
    let stops_to_be_replaced = stops_with_app
        && app_runtime_host(options).is_some_and(|host| daemon_runs_from(info, &host));
    if !stops_to_be_replaced && !restarts_into_this_host(info, &status) {
        return Err(format!(
            "A different xd daemon version is running. Stop it with `{stop}` and reconnect."
        ));
    }
    if status["state"] == "installing" {
        return Err("xd is installing an update. It reconnects when the update finishes.".into());
    }
    let reply = request(
        &mut stale,
        json!({"op": "host-update", "action": "restart"}),
    )
    .map_err(|e| e.to_string())?;
    // An earlier daemon checks for live work before it finds it has nothing
    // to restart into, so that refusal says it is idle. Any other refusal says
    // it is not. Work that starts between this answer and the stop is cut off,
    // as it is when a daemon restarts itself a moment after the same check;
    // daemons already released offer nothing tighter to ask.
    let idle_without_restart =
        stops_to_be_replaced && reply["error"].as_str() == Some(xd_host::CANNOT_RESTART);
    if reply["ok"] != true && !idle_without_restart {
        return Err(format!(
            "xd was updated, but the previous daemon is still running because chats, their background agents and shells, or terminals are active. It restarts by itself when they finish. To restart it now and interrupt them, run `{stop}`."
        ));
    }
    if idle_without_restart
        && request(&mut stale, json!({"op": "daemon-stop"}))
            .map_or(true, |reply| reply["ok"] != true)
    {
        return Err(format!(
            "The previous xd daemon did not stop for the update. Stop it with `{stop}` and reconnect."
        ));
    }
    drop(stale);
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        // The old process answers until it re-executes, then refuses briefly.
        if let Ok(mut stream) = connect(options, true, stops_with_app)
            && let Ok(info) = rpc(&mut stream, "daemon-info")
            && matches_this_build(&info)
        {
            return Ok((stream, info));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "The xd daemon did not restart after the update. Stop it with `{stop}` and reconnect."
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}
/// Restarting only helps when the daemon re-executes the binary running here.
/// Development builds and other installs sharing this data must not disturb it.
fn restarts_into_this_host(info: &Value, status: &Value) -> bool {
    let Some(host) = xd_host::installed_host_executable() else {
        return false;
    };
    status["supported"] == true && daemon_runs_from(info, &host)
}

/// Whether the daemon was started from `host`, the binary running here.
fn daemon_runs_from(info: &Value, host: &Path) -> bool {
    match info["executable"].as_str() {
        Some(daemon) => Some(daemon) == host.to_str(),
        // Older daemons don't report it, and neither does one outside an
        // installation. Their command line still names the path they were
        // started from, even after an update replaces that file.
        None => info["pid"]
            .as_u64()
            .and_then(process_command)
            .is_some_and(|command| command.starts_with(&format!("{} daemon ", host.display()))),
    }
}

/// This binary, when it is the copy the Windows app keeps inside WSL.
///
/// The app replaces `runtime/v1/xd-host` in its data directory with its own
/// matching host each time it starts, then runs the bridge from there. A
/// daemon the previous version started from that path is still running the
/// old file, and the app could never connect again while it lived: the daemon
/// refused to restart, the bridge refused to talk to it, and every retry kept
/// it from noticing that the app had gone.
fn app_runtime_host(options: &HostOptions) -> Option<PathBuf> {
    let executable = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let data = fs::canonicalize(options.database.parent()?).ok()?;
    (executable == data.join("runtime/v1/xd-host")).then_some(executable)
}
#[cfg(target_os = "linux")]
fn process_command(pid: u64) -> Option<String> {
    let command = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(String::from_utf8_lossy(&command).replace('\0', " "))
}
#[cfg(not(target_os = "linux"))]
fn process_command(pid: u64) -> Option<String> {
    let output = Command::new("ps")
        .args(["-ww", "-o", "args=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}
fn stop_command(options: &HostOptions) -> String {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let host = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("xd-host"));
    let host = quote(&host);
    match options.database.parent() {
        Some(data)
            if options.database == data.join("chats.db")
                && options.workspaces == data.join("Workspaces") =>
        {
            format!("{host} daemon-stop --data {}", quote(data))
        }
        _ => format!(
            "{host} daemon-stop --database {} --workspaces {}",
            quote(&options.database),
            quote(&options.workspaces)
        ),
    }
}
fn rpc(stream: &mut UnixStream, operation: &str) -> io::Result<Value> {
    request(stream, json!({"op": operation}))
}
pub(super) fn request(stream: &mut UnixStream, body: Value) -> io::Result<Value> {
    request_within(stream, body, Duration::from_secs(10))
}

/// [`request`], waiting up to `timeout` for the reply.
pub(super) fn request_within(
    stream: &mut UnixStream,
    mut body: Value,
    timeout: Duration,
) -> io::Result<Value> {
    body["_xd_request"] = json!(0);
    stream.set_read_timeout(Some(timeout))?;
    writeln!(stream, "{body}")?;
    loop {
        // One byte at a time: the bridge passes on what follows the reply, so
        // nothing past its line may be read here.
        let mut line = Vec::new();
        let mut byte = [0];
        while byte[0] != b'\n' {
            if stream.read(&mut byte)? == 0 {
                return Err(io::Error::other("Daemon disconnected."));
            }
            line.push(byte[0]);
        }
        let frame: Value = serde_json::from_slice(&line)?;
        if frame["_xd_request"] == 0 {
            // A restarting daemon can close right after replying, and macOS then
            // rejects socket options with EINVAL. The reply is already in hand.
            let _ = stream.set_read_timeout(None);
            return Ok(frame);
        }
    }
}
pub(super) fn control(options: HostOptions, operation: &str) -> Result<(), String> {
    let mut stream = connect(&options, false, false).map_err(|e| e.to_string())?;
    let reply = rpc(&mut stream, operation).map_err(|e| e.to_string())?;
    println!("{reply}");
    Ok(())
}

struct PeerServer {
    config: Arc<rustls::ServerConfig>,
    fingerprint: String,
    endpoint: Mutex<Option<(String, u16, PeerEndpoint)>>,
    engine: Weak<Engine>,
    stop: Arc<AtomicBool>,
}
impl PeerServer {
    fn new(data: &Path, engine: Weak<Engine>) -> io::Result<Self> {
        let directory = data.join("daemon");
        private_directory(&directory)?;
        let identity = directory.join("identity.json");
        let value = match fs::read(&identity) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let generated = rcgen::generate_simple_self_signed(vec!["xd.local".into()])
                    .map_err(io::Error::other)?;
                let value = json!({"certificate": STANDARD.encode(generated.cert.der()), "key": STANDARD.encode(generated.signing_key.serialize_der())});
                let temporary = directory.join(format!(".identity-{}.tmp", uuid::Uuid::new_v4()));
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&temporary)?;
                serde_json::to_writer(&mut file, &value)?;
                file.sync_all()?;
                fs::rename(&temporary, &identity)?;
                value
            }
            Err(error) => return Err(error),
        };
        let certificate = STANDARD
            .decode(
                value["certificate"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("Invalid daemon identity"))?,
            )
            .map_err(io::Error::other)?;
        let key = STANDARD
            .decode(
                value["key"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("Invalid daemon identity"))?,
            )
            .map_err(io::Error::other)?;
        let fingerprint = xd_transport::fingerprint(&certificate);
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certificate)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .map_err(io::Error::other)?;
        Ok(Self {
            config: Arc::new(config),
            fingerprint,
            endpoint: Mutex::new(None),
            engine,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }
    fn listen(&self, bind: &str, port: u16) -> io::Result<PeerEndpoint> {
        let mut endpoint = self
            .endpoint
            .lock()
            .map_err(|_| io::Error::other("Remote listener unavailable"))?;
        if let Some((current_bind, current_port, current)) = &*endpoint {
            if current_bind == bind && (*current_port == port || port == 0) {
                return Ok(current.clone());
            }
            return Err(io::Error::other(
                "Remote listener already runs on a different address. Restart the daemon to change it.",
            ));
        }
        let listener = TcpListener::bind((bind, port))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let host = if address.ip().is_unspecified() {
            UdpSocket::bind("0.0.0.0:0")
                .and_then(|socket| {
                    socket.connect("192.0.2.1:80")?;
                    Ok(socket.local_addr()?.ip().to_string())
                })
                .unwrap_or_else(|_| "127.0.0.1".into())
        } else {
            address.ip().to_string()
        };
        let result = PeerEndpoint {
            host,
            port: address.port(),
            fingerprint: self.fingerprint.clone(),
        };
        let config = self.config.clone();
        let engine = self.engine.clone();
        let stop = self.stop.clone();
        thread::spawn(move || {
            let mut clients = Vec::<thread::JoinHandle<()>>::new();
            while !stop.load(Ordering::Acquire) {
                clients.retain(|client| !client.is_finished());
                match listener.accept() {
                    Ok((socket, _)) if clients.len() < MAX_CLIENTS => {
                        let config = config.clone();
                        let Some(engine) = engine.upgrade() else {
                            break;
                        };
                        clients.push(thread::spawn(move || {
                            let result = (|| -> io::Result<()> {
                                socket.set_nonblocking(false)?;
                                let tls = rustls::ServerConnection::new(config)
                                    .map_err(io::Error::other)?;
                                let (mut input, mut output, session) =
                                    xd_transport::split(rustls::Connection::Server(tls), socket)?;
                                let (stream, mut bridge) = UnixStream::pair()?;
                                stream.set_write_timeout(Some(Duration::from_secs(15)))?;
                                stream.set_read_timeout(Some(Duration::from_secs(30)))?;
                                let engine_for_client = engine.clone();
                                let server = thread::spawn(move || {
                                    let _ =
                                        serve_daemon_connection(stream, engine_for_client, true);
                                });
                                let mut incoming = bridge.try_clone()?;
                                thread::spawn(move || {
                                    let _ = io::copy(&mut input, &mut incoming);
                                    let _ = incoming.shutdown(Shutdown::Both);
                                });
                                let _ = io::copy(&mut bridge, &mut output);
                                session.close();
                                let _ = bridge.shutdown(Shutdown::Both);
                                let _ = server.join();
                                Ok(())
                            })();
                            if let Err(error) = result {
                                eprintln!("xd-host: remote connection: {error}");
                            }
                        }));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        *endpoint = Some((bind.to_owned(), address.port(), result.clone()));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn lock_fixture() -> (PathBuf, Paths, File) {
        // Not the temporary directory: on macOS its path is too long for a socket.
        let root = PathBuf::from(format!("/tmp/xd-lock-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let paths = Paths {
            socket: root.join("socket"),
            lock: root.join("lock"),
            data: root.clone(),
        };
        let holder = File::create(&paths.lock).unwrap();
        assert_eq!(
            unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        (root, paths, holder)
    }

    #[test]
    fn a_new_daemon_outlasts_a_lock_that_is_about_to_be_released() {
        let (root, paths, holder) = lock_fixture();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            drop(holder);
        });
        assert!(
            singleton(&paths, Duration::from_secs(10))
                .unwrap()
                .is_some()
        );
        release.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    fn running_daemon() -> (PathBuf, Paths, Singleton, UnixListener) {
        let (root, paths, holder) = lock_fixture();
        drop(holder);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let mut singleton = singleton(&paths, Duration::ZERO).unwrap().unwrap();
        let listener = singleton.listen().unwrap();
        (root, paths, singleton, listener)
    }

    #[test]
    fn a_daemon_whose_files_were_removed_can_be_reached_again() {
        let (root, paths, mut singleton, _old) = running_daemon();
        assert!(singleton.restore().unwrap().is_none());
        fs::remove_dir_all(&root).unwrap();
        assert!(UnixStream::connect(&paths.socket).is_err());

        let listener = singleton.restore().unwrap().unwrap();
        let _client = UnixStream::connect(&paths.socket).unwrap();
        listener.set_nonblocking(false).unwrap();
        listener.accept().unwrap();
        assert!(singleton.restore().unwrap().is_none());
        // The lock is back too, so no second daemon can start.
        assert!(singleton_now(&paths).is_none());
        drop(singleton);
        assert!(!paths.socket.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_daemon_leaves_the_files_of_one_that_took_over() {
        let (root, paths, mut singleton, _old) = running_daemon();
        fs::remove_file(&paths.lock).unwrap();
        fs::remove_file(&paths.socket).unwrap();
        let (_other, _other_listener) = {
            let mut other = singleton_now(&paths).unwrap();
            let listener = other.listen().unwrap();
            // Kept alive by forgetting it, as if it were another process.
            (std::mem::ManuallyDrop::new(other), listener)
        };

        assert!(singleton.restore().is_err());
        drop(singleton);
        assert!(UnixStream::connect(&paths.socket).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    fn singleton_now(paths: &Paths) -> Option<Singleton> {
        singleton(paths, Duration::ZERO).unwrap()
    }

    #[test]
    fn a_new_daemon_is_not_fooled_by_a_listener_nobody_serves() {
        let (root, paths, holder) = lock_fixture();
        let doomed = UnixListener::bind(&paths.socket).unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop((holder, doomed));
        });
        assert!(
            singleton(&paths, Duration::from_secs(10))
                .unwrap()
                .is_some()
        );
        release.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_daemon_leaves_at_once_when_the_running_one_answers() {
        let (root, paths, _holder) = lock_fixture();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        let running = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut request)
                .unwrap();
            stream.write_all(b"{\"ok\":true}\n").unwrap();
        });
        let began = Instant::now();
        assert!(
            singleton(&paths, Duration::from_secs(10))
                .unwrap()
                .is_none()
        );
        assert!(began.elapsed() < Duration::from_secs(5));
        running.join().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn asking_a_listener_with_a_full_queue_does_not_hang() {
        let (root, paths, _holder) = lock_fixture();
        let listener = UnixListener::bind(&paths.socket).unwrap();
        // Never accepting, with the shortest queue, so further connects block.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
        for _ in 0..4 {
            let socket = paths.socket.clone();
            thread::spawn(move || {
                let _connection = UnixStream::connect(socket);
                thread::sleep(Duration::from_secs(30));
            });
        }
        thread::sleep(Duration::from_millis(100));

        let (answered, answer) = mpsc::channel();
        let socket = paths.socket.clone();
        thread::spawn(move || {
            let _ = answered.send(daemon_answers(&socket));
        });
        assert_eq!(answer.recv_timeout(Duration::from_secs(5)), Ok(false));
        drop(listener);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_daemon_leaves_a_lock_that_stays_held() {
        let (root, paths, _holder) = lock_fixture();
        assert!(
            singleton(&paths, Duration::from_millis(50))
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_daemons_create_a_private_runtime_directory() {
        let root = std::env::temp_dir().join(format!("xd-runtime-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        if std::env::var_os("XD_TEST_RESTRICTIVE_UMASK").is_some() {
            // Only the subprocess test sets this process-wide mask.
            unsafe { libc::umask(0o700) };
        }
        for round in 0..32 {
            let runtime = root.join(round.to_string());
            let barrier = std::sync::Barrier::new(16);
            thread::scope(|scope| {
                for _ in 0..16 {
                    let runtime = &runtime;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        barrier.wait();
                        private_directory(runtime).unwrap();
                    });
                }
            });
            assert_eq!(fs::metadata(&runtime).unwrap().mode() & 0o777, 0o700);
            File::create(runtime.join("lock")).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_daemons_restore_owner_access_under_a_restrictive_umask() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::tests::concurrent_daemons_create_a_private_runtime_directory",
                "--nocapture",
            ])
            .env("XD_TEST_RESTRICTIVE_UMASK", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
