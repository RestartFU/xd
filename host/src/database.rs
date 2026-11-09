//! SQLite connection handling for the state store.
//!
//! All writes go through one connection behind a mutex, as before. The database
//! runs in write-ahead-log mode so that reads can use their own connections:
//! a reader sees the last committed state and never waits for the writer,
//! even while a commit is stuck in `fsync` because other programs are flooding
//! the same disk. Checkpoints, which copy the log back into the database file
//! and sync it, run on a background connection instead of inside a commit.

use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags};

/// Connections that may read at the same time as the writer and each other.
pub(crate) const MAX_READERS: usize = 4;
/// Log size in pages at which the writer checkpoints inline. The background
/// checkpointer normally keeps the log far below this; it is only a backstop.
const WRITER_AUTOCHECKPOINT_PAGES: i64 = 10_000;
/// How often the background connection checkpoints the log.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);
/// Size the log file is truncated back to once it has been checkpointed.
const JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// Switches a freshly opened writable connection to write-ahead logging and
/// reports whether that worked. Opening the database first replays any
/// rollback journal left by a crash, so the switch never discards a commit.
///
/// Commits keep `synchronous = FULL`: every commit is on disk before it
/// returns.
pub(crate) fn enable_write_ahead_log(
    connection: &Connection,
    path: &Path,
) -> rusqlite::Result<bool> {
    let mode = match connection
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
    {
        Ok(mode) => mode,
        // Another process holding the database in rollback mode can keep the
        // switch from happening. Keep working in the old mode rather than
        // failing to start; a later start will switch.
        Err(rusqlite::Error::SqliteFailure(error, _))
            if matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            eprintln!(
                "xd-host: {} is busy; keeping its rollback journal for now",
                path.display()
            );
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    connection.pragma_update(None, "synchronous", "FULL")?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Ok(false);
    }
    connection.pragma_update(None, "wal_autocheckpoint", WRITER_AUTOCHECKPOINT_PAGES)?;
    connection.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
    Ok(true)
}

/// A bounded set of read-only connections to one database.
pub(crate) struct ReaderPool {
    path: PathBuf,
    busy_timeout: Duration,
    state: Mutex<PoolState>,
    returned: Condvar,
}

struct PoolState {
    idle: Vec<Connection>,
    open: usize,
}

impl ReaderPool {
    pub(crate) fn new(path: &Path, busy_timeout: Duration) -> Self {
        Self {
            path: path.to_owned(),
            busy_timeout,
            state: Mutex::new(PoolState {
                idle: Vec::new(),
                open: 0,
            }),
            returned: Condvar::new(),
        }
    }

    /// Lends out a connection inside a read transaction, so several queries
    /// see one consistent snapshot. Waits only for other readers, and only
    /// when all of them are busy.
    pub(crate) fn snapshot(&self) -> rusqlite::Result<Snapshot<'_>> {
        let connection = self.checkout()?;
        let mut snapshot = Snapshot {
            pool: self,
            connection: Some(connection),
        };
        snapshot.connection()?.execute_batch("BEGIN DEFERRED")?;
        Ok(snapshot)
    }

    fn checkout(&self) -> rusqlite::Result<Connection> {
        let mut state = self.lock();
        loop {
            if let Some(connection) = state.idle.pop() {
                return Ok(connection);
            }
            if state.open < MAX_READERS {
                state.open += 1;
                drop(state);
                return self.open_reader().inspect_err(|_| self.forget());
            }
            state = self
                .returned
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn open_reader(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(self.busy_timeout)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        // A read path that tries to write fails loudly instead of racing the
        // writer connection.
        connection.pragma_update(None, "query_only", true)?;
        Ok(connection)
    }

    fn give_back(&self, connection: Connection) {
        self.lock().idle.push(connection);
        self.returned.notify_one();
    }

    fn forget(&self) {
        self.lock().open -= 1;
        self.returned.notify_one();
    }

    fn lock(&self) -> MutexGuard<'_, PoolState> {
        // The state is a list and a count, both valid after any panic.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// A pooled connection with an open read transaction, ended when dropped.
pub(crate) struct Snapshot<'a> {
    pool: &'a ReaderPool,
    connection: Option<Connection>,
}

impl Snapshot<'_> {
    fn connection(&mut self) -> rusqlite::Result<&Connection> {
        self.connection
            .as_ref()
            .ok_or(rusqlite::Error::InvalidQuery)
    }
}

impl Deref for Snapshot<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("a snapshot holds its connection until dropped")
    }
}

impl Drop for Snapshot<'_> {
    fn drop(&mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        // Ending the transaction releases the snapshot, so checkpoints can
        // reuse the log behind it. A connection that cannot end it is closed.
        if connection.execute_batch("ROLLBACK").is_ok() && connection.is_autocommit() {
            self.pool.give_back(connection);
        } else {
            drop(connection);
            self.pool.forget();
        }
    }
}

/// A connection to read through: a pooled snapshot, or the writer itself when
/// the database could not switch to write-ahead logging.
pub(crate) enum Reader<'a> {
    Snapshot(Snapshot<'a>),
    Writer(MutexGuard<'a, Connection>),
}

impl Deref for Reader<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Self::Snapshot(snapshot) => snapshot,
            Self::Writer(writer) => writer,
        }
    }
}

/// Checkpoints the write-ahead log from its own connection and thread, so the
/// syncs a checkpoint needs never happen while the writer is locked.
pub(crate) struct Checkpointer {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Checkpointer {
    /// Starts checkpointing `path`. Without it the writer's own, rarer
    /// checkpoints still keep the log bounded, so a failure is only logged.
    pub(crate) fn start(path: &Path, busy_timeout: Duration) -> Option<Self> {
        Self::try_start(path, busy_timeout)
            .inspect_err(|error| {
                eprintln!("xd-host: cannot start background checkpoints: {error}");
            })
            .ok()
    }

    fn try_start(path: &Path, busy_timeout: Duration) -> Result<Self, Box<dyn std::error::Error>> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(busy_timeout)?;
        connection.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("xd-sqlite-checkpoint".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    thread::park_timeout(CHECKPOINT_EVERY);
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    // PASSIVE copies what it can without blocking the writer
                    // or readers. A busy result just means trying again.
                    let _ = connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}
