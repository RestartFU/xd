use std::{
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};

#[cfg(any(unix, windows, test))]
use std::io::{Read, Write};

#[cfg(unix)]
use std::{
    env,
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    os::windows::io::AsRawHandle,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

#[cfg(any(unix, windows, test))]
use serde_json::json;

#[cfg(any(unix, windows, test))]
const APPLICATION_ID: &str = "1531361363522490489";
const DEFAULT_STATE: &str = "Browsing workspaces";
#[cfg(any(unix, windows, test))]
const DETAILS: &str = "Building with AI";
#[cfg(any(unix, windows, test))]
const MAX_FRAME: usize = 1024 * 1024;
#[cfg(any(unix, windows, test))]
const IO_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(any(unix, windows, test))]
const RETRY_INTERVAL: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[cfg(any(unix, windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Opcode {
    Handshake = 0,
    Frame = 1,
    Close = 2,
    Ping = 3,
    Pong = 4,
}

#[cfg(any(unix, windows, test))]
impl Opcode {
    fn from_raw(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Handshake,
            1 => Self::Frame,
            2 => Self::Close,
            3 => Self::Ping,
            4 => Self::Pong,
            _ => return None,
        })
    }
}

struct PresenceState {
    text: String,
    version: u64,
    closed: bool,
}

struct SharedPresence {
    state: Mutex<PresenceState>,
    changed: Condvar,
}

pub struct DiscordPresence {
    shared: Arc<SharedPresence>,
}

impl Default for DiscordPresence {
    fn default() -> Self {
        let shared = Arc::new(SharedPresence {
            state: Mutex::new(PresenceState {
                text: DEFAULT_STATE.into(),
                version: 1,
                closed: false,
            }),
            changed: Condvar::new(),
        });
        let worker = shared.clone();
        let _ = thread::Builder::new()
            .name("xd-discord-presence".into())
            .spawn(move || run_presence(worker));
        Self { shared }
    }
}

impl DiscordPresence {
    pub fn set_state(&self, state: &str) {
        if state.is_empty() {
            return;
        }
        let Ok(mut current) = self.shared.state.lock() else {
            return;
        };
        if current.closed || current.text == state {
            return;
        }
        current.text.clear();
        current.text.push_str(state);
        current.version = current.version.wrapping_add(1);
        self.shared.changed.notify_one();
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        if let Ok(mut current) = self.shared.state.lock() {
            current.closed = true;
            current.version = current.version.wrapping_add(1);
            self.shared.changed.notify_one();
        }
    }
}

#[cfg(any(unix, windows, test))]
fn activity(state: &str, started_at: u64, process_id: u32, nonce: u64) -> String {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": process_id,
            "activity": {
                "details": DETAILS,
                "state": state,
                "timestamps": {"start": started_at},
            },
        },
        "nonce": nonce.to_string(),
    })
    .to_string()
}

#[cfg(any(unix, windows, test))]
fn handshake() -> String {
    json!({"v": 1, "client_id": APPLICATION_ID}).to_string()
}

#[cfg(any(unix, windows, test))]
fn clear_activity(process_id: u32, nonce: u64) -> String {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": {"pid": process_id, "activity": null},
        "nonce": nonce.to_string(),
    })
    .to_string()
}

#[cfg(any(unix, windows))]
fn run_presence(shared: Arc<SharedPresence>) {
    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let process_id = std::process::id();
    let mut connection = None;
    let mut sent_version = 0;
    let mut nonce = 1_u64;
    let mut next_retry = Instant::now();
    let mut next_refresh = Instant::now();

    loop {
        let (state, version, closed) = snapshot(&shared);
        if closed {
            if let Some(active) = &mut connection {
                let _ = send_frame(active, Opcode::Frame, &clear_activity(process_id, nonce));
            }
            return;
        }

        if connection.is_none() && Instant::now() >= next_retry {
            connection = connect();
            if let Some(active) = &mut connection {
                if send_frame(active, Opcode::Handshake, &handshake()) && read_reply(active) {
                    sent_version = 0;
                    next_refresh = Instant::now();
                } else {
                    connection = None;
                    next_retry = Instant::now() + RETRY_INTERVAL;
                }
            } else {
                next_retry = Instant::now() + RETRY_INTERVAL;
            }
        }

        if let Some(active) = &mut connection
            && (version != sent_version || Instant::now() >= next_refresh)
        {
            let payload = activity(&state, started_at, process_id, nonce);
            nonce = nonce.wrapping_add(1);
            if send_frame(active, Opcode::Frame, &payload) && read_reply(active) {
                sent_version = version;
                next_refresh = Instant::now() + RETRY_INTERVAL;
            } else {
                connection = None;
                next_retry = Instant::now() + RETRY_INTERVAL;
            }
        }
        wait_for_change(&shared);
    }
}

#[cfg(not(any(unix, windows)))]
fn run_presence(shared: Arc<SharedPresence>) {
    loop {
        if snapshot(&shared).2 {
            return;
        }
        wait_for_change(&shared);
    }
}

fn snapshot(shared: &SharedPresence) -> (String, u64, bool) {
    shared
        .state
        .lock()
        .map(|state| (state.text.clone(), state.version, state.closed))
        .unwrap_or_else(|_| (DEFAULT_STATE.into(), 0, true))
}

fn wait_for_change(shared: &SharedPresence) {
    if let Ok(state) = shared.state.lock() {
        let _ = shared.changed.wait_timeout(state, POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn connect() -> Option<UnixStream> {
    let mut candidates = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .into_iter()
        .filter_map(env::var_os)
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    candidates.push(PathBuf::from("/tmp"));
    let mut directories = Vec::new();
    for candidate in candidates {
        if !directories.contains(&candidate) {
            directories.push(candidate);
        }
    }
    for directory in directories {
        for index in 0..10 {
            let path = directory.join(format!("discord-ipc-{index}"));
            let Ok(stream) = UnixStream::connect(path) else {
                continue;
            };
            if stream.set_read_timeout(Some(IO_TIMEOUT)).is_ok()
                && stream.set_write_timeout(Some(IO_TIMEOUT)).is_ok()
            {
                return Some(stream);
            }
        }
    }
    None
}

#[cfg(windows)]
fn connect() -> Option<File> {
    (0..10).find_map(|index| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(format!(r"\\?\pipe\discord-ipc-{index}"))
            .ok()
    })
}

#[cfg(any(unix, windows, test))]
fn send_frame(connection: &mut impl Write, opcode: Opcode, payload: &str) -> bool {
    let Ok(length) = u32::try_from(payload.len()) else {
        return false;
    };
    if payload.len() > MAX_FRAME {
        return false;
    }
    connection.write_all(&(opcode as u32).to_le_bytes()).is_ok()
        && connection.write_all(&length.to_le_bytes()).is_ok()
        && connection.write_all(payload.as_bytes()).is_ok()
        && connection.flush().is_ok()
}

#[cfg(any(unix, windows, test))]
fn read_frame(connection: &mut impl Read) -> Option<(Opcode, String)> {
    let mut header = [0_u8; 8];
    connection.read_exact(&mut header).ok()?;
    let opcode = Opcode::from_raw(u32::from_le_bytes(header[..4].try_into().ok()?))?;
    let length = u32::from_le_bytes(header[4..].try_into().ok()?) as usize;
    if length > MAX_FRAME {
        return None;
    }
    let mut payload = vec![0_u8; length];
    connection.read_exact(&mut payload).ok()?;
    Some((opcode, String::from_utf8(payload).ok()?))
}

#[cfg(any(unix, test))]
fn read_reply(connection: &mut (impl Read + Write)) -> bool {
    for _ in 0..8 {
        let Some((opcode, payload)) = read_frame(connection) else {
            return false;
        };
        if opcode == Opcode::Ping {
            if !send_frame(connection, Opcode::Pong, &payload) {
                return false;
            }
            continue;
        }
        if opcode != Opcode::Frame {
            return false;
        }
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&payload) else {
            return false;
        };
        return root.get("evt").and_then(serde_json::Value::as_str) != Some("ERROR");
    }
    false
}

#[cfg(windows)]
fn read_reply(connection: &mut File) -> bool {
    for _ in 0..8 {
        if !wait_for_frame(connection) {
            return false;
        }
        let Some((opcode, payload)) = read_frame(connection) else {
            return false;
        };
        if opcode == Opcode::Ping {
            if !send_frame(connection, Opcode::Pong, &payload) {
                return false;
            }
            continue;
        }
        if opcode != Opcode::Frame {
            return false;
        }
        let Ok(root) = serde_json::from_str::<serde_json::Value>(&payload) else {
            return false;
        };
        return root.get("evt").and_then(serde_json::Value::as_str) != Some("ERROR");
    }
    false
}

#[cfg(windows)]
fn wait_for_frame(connection: &File) -> bool {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        let mut header = [0_u8; 8];
        let mut header_bytes = 0_u32;
        let mut available = 0_u32;
        let success = unsafe {
            PeekNamedPipe(
                connection.as_raw_handle(),
                header.as_mut_ptr().cast(),
                header.len() as u32,
                &mut header_bytes,
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if success == 0 {
            return false;
        }
        if header_bytes as usize == header.len() {
            let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
            if length > MAX_FRAME {
                return false;
            }
            if 8_usize
                .checked_add(length)
                .is_some_and(|frame_bytes| available as usize >= frame_bytes)
            {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn payloads_match_the_established_private_metadata_contract() {
        let payload: serde_json::Value =
            serde_json::from_str(&activity("Agent working", 10, 20, 30)).unwrap();
        assert_eq!(payload["args"]["pid"], 20);
        assert_eq!(payload["args"]["activity"]["details"], DETAILS);
        assert_eq!(payload["args"]["activity"]["state"], "Agent working");
        assert_eq!(payload["args"]["activity"]["timestamps"]["start"], 10);
        assert_eq!(payload["nonce"], "30");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&handshake()).unwrap()["client_id"],
            APPLICATION_ID
        );
    }

    #[test]
    fn frames_are_little_endian_bounded_and_ping_is_answered() {
        let mut bytes = Vec::new();
        assert!(send_frame(&mut bytes, Opcode::Frame, "ready"));
        assert_eq!(&bytes[..4], &1_u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &5_u32.to_le_bytes());
        assert_eq!(
            read_frame(&mut Cursor::new(bytes)),
            Some((Opcode::Frame, "ready".into()))
        );

        let mut ping = Vec::new();
        assert!(send_frame(&mut ping, Opcode::Ping, "hello"));
        let reply = json!({"evt": "READY"}).to_string();
        assert!(send_frame(&mut ping, Opcode::Frame, &reply));
        let mut connection = Duplex {
            input: Cursor::new(ping),
            output: Vec::new(),
        };
        assert!(read_reply(&mut connection));
        assert!(
            connection
                .output
                .windows(5)
                .any(|window| window == b"hello")
        );
        assert!(!send_frame(
            &mut Vec::new(),
            Opcode::Frame,
            &"x".repeat(MAX_FRAME + 1)
        ));
    }
}
