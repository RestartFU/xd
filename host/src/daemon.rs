use crate::HostOptions;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    net::{Shutdown, TcpListener, UdpSocket},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
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
const MAX_CLIENTS: usize = 64;
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
    match fs::create_dir(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700))?,
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
    Ok(())
}
struct Singleton {
    _lock: File,
    socket: PathBuf,
}
impl Drop for Singleton {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}
fn singleton(paths: &Paths) -> io::Result<Option<Singleton>> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&paths.lock)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(error);
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
        _lock: lock,
        socket: paths.socket.clone(),
    }))
}

pub(super) fn run(options: HostOptions) -> Result<(), String> {
    run_inner(options).map_err(|e| e.to_string())
}
fn run_inner(options: HostOptions) -> io::Result<()> {
    let paths = paths(&options)?;
    let Some(_singleton) = singleton(&paths)? else {
        return Ok(());
    };
    let store =
        StateStore::open(&options.database, &options.workspaces).map_err(io::Error::other)?;
    store
        .recover_interrupted_turns()
        .map_err(io::Error::other)?;
    let engine = Arc::new(Engine::with_store_and_data(store, Some(paths.data.clone())));
    let listener = UnixListener::bind(&paths.socket)?;
    fs::set_permissions(&paths.socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let peers = Arc::new(PeerServer::new(&paths.data, Arc::downgrade(&engine))?);
    let peer_server = peers.clone();
    engine
        .set_peer_listener(move |bind, port| {
            peer_server.listen(bind, port).map_err(|e| e.to_string())
        })
        .map_err(io::Error::other)?;
    if let Some((bind, port)) = engine.remote_listener() {
        if let Err(error) = peers.listen(&bind, port) {
            eprintln!("xd-host: cannot restore remote listener: {error}");
        }
    }
    unsafe {
        libc::signal(
            libc::SIGTERM,
            stop_signal as *const () as libc::sighandler_t,
        );
        libc::signal(libc::SIGINT, stop_signal as *const () as libc::sighandler_t);
    }
    let mut clients = Vec::<thread::JoinHandle<()>>::new();
    while !STOP.load(Ordering::Acquire) && !engine.shutdown_requested() {
        clients.retain(|client| !client.is_finished());
        match listener.accept() {
            Ok((stream, _)) if clients.len() < MAX_CLIENTS => {
                // Accepted sockets inherit nonblocking mode on macOS.
                stream.set_nonblocking(false)?;
                stream.set_write_timeout(Some(Duration::from_secs(15)))?;
                let engine = engine.clone();
                clients.push(thread::spawn(move || {
                    let _ = serve_daemon_connection(stream, engine, false);
                }));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
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

fn connect(options: &HostOptions, start: bool) -> io::Result<UnixStream> {
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
    let mut stream = connect(&options, true).map_err(|e| e.to_string())?;
    // Reject incompatible live daemons without interrupting their running turns.
    let info = rpc(&mut stream, "daemon-info").map_err(|e| e.to_string())?;
    if info["protocol"] != 2
        || info["version"] != env!("CARGO_PKG_VERSION")
        || info["build"] != option_env!("XD_COMMIT").unwrap_or("development")
    {
        return Err(
            "A different xd daemon version is running. Stop it with daemon-stop and reconnect."
                .into(),
        );
    }
    let expected = fs::canonicalize(&options.workspaces)
        .map_err(|_| "The running daemon uses a different workspace directory.".to_owned())?;
    if info["workspaces"].as_str() != expected.to_str() {
        return Err("This database is already served with a different workspace directory. Stop that daemon before changing its workspace root.".into());
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
fn rpc(stream: &mut UnixStream, operation: &str) -> io::Result<Value> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    writeln!(stream, "{}", json!({"op": operation, "_xd_request": 0}))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::other("Daemon disconnected."));
        }
        let frame: Value = serde_json::from_str(&line)?;
        if frame["_xd_request"] == 0 {
            stream.set_read_timeout(None)?;
            return Ok(frame);
        }
    }
}
pub(super) fn control(options: HostOptions, operation: &str) -> Result<(), String> {
    let mut stream = connect(&options, false).map_err(|e| e.to_string())?;
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
