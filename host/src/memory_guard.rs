//! A last line of defence against the kernel killing every process in the
//! daemon's container at once.
//!
//! A container's memory limit is a cgroup v2 `memory.max`. When the cgroup also
//! has `memory.oom.group` set to 1, running out of memory there does not kill one
//! process: the kernel kills every process in the cgroup together. For xd that is
//! the daemon, every agent CLI, every background agent and every terminal, all
//! because one chat ran a build that wanted more memory than was left.
//!
//! The limit and the group kill can each be set on the daemon's own cgroup or on
//! any cgroup above it, as when the daemon runs in a child cgroup of a limited
//! container: the kernel kills, together, the highest cgroup with
//! `memory.oom.group` between the process it picked and the cgroup that ran out.
//! The guard watches the tightest limit above the daemon whose breach would
//! kill a group that includes the daemon.
//!
//! Only in that setup, the guard watches the cgroup's memory that cannot be
//! reclaimed: anonymous memory, shared memory (tmpfs, shm), and kernel memory
//! other than reclaimable slab, all from `memory.stat`. Page cache is left out,
//! however large: the kernel drops it under pressure, so a cgroup sitting at
//! `memory.max` because of cached files is reclaiming, not about to be killed.
//! Swap could take some anonymous memory, but containers rarely have any, so it
//! is counted as unreclaimable. Pressure stall information is not used: its
//! averages lag by seconds, and a build can allocate the remaining headroom
//! faster than that.
//!
//! When that measure reaches a share of `memory.max` (92% unless
//! `XD_MEMORY_GUARD_PERCENT` says otherwise; `off` turns the guard off), the guard
//! stops the process tree holding the most memory that is not protected.
//! Protected are the daemon
//! and its ancestors, the agent processes it started, every process named like
//! an agent CLI or xd itself, the shells leading terminal sessions, and the
//! ancestors of any of them. What is left is the work they started: a compiler,
//! a build tool, a test run, a build daemon. The guard stops a whole subtree from
//! its topmost unprotected process, such as the `bash -c` an agent ran a build
//! in, and only processes of this user in the guarded cgroup.
//!
//! The tree is frozen with SIGSTOP before it is killed with SIGKILL: a process
//! that is stopped cannot start another, and a child whose parent has exited
//! belongs to init and can no longer be traced to the tree. A grace period after
//! SIGTERM would let a build do exactly that while it shuts down, and the kernel
//! gives a group it kills for lack of memory no grace either.
//!
//! After acting, it does nothing more until the measure falls a few points below
//! the threshold or a cooldown passes, so one spike stops one tree. What it
//! stopped is written to the host log and, when the tree belonged to a chat's
//! agent or terminal, to that chat.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// Overrides the threshold, as a percentage of `memory.max`, or turns the guard
/// off with `off` or `0`.
pub(crate) const PERCENT_VARIABLE: &str = "XD_MEMORY_GUARD_PERCENT";
const DEFAULT_PERCENT: u64 = 92;
/// Thresholds outside this range are refused: lower would stop ordinary work,
/// higher leaves no time to act.
const PERCENT_RANGE: std::ops::RangeInclusive<u64> = 50..=99;
/// Reading `memory.stat` costs one small file read, so this can be frequent.
pub(crate) const SAMPLE_EVERY: Duration = Duration::from_secs(1);
/// How far below the threshold, in points of `memory.max`, the measure has to
/// fall before another crossing counts as a new spike.
const REARM_POINTS: u64 = 5;
/// How long a spike that does not let up waits before another tree is stopped.
const COOLDOWN: Duration = Duration::from_secs(20);
/// Passes over `/proc` that freezing a tree may take. Each pass stops every
/// process found so far, so the tree is complete as soon as one finds nothing
/// new; the bound only guards against a tree that keeps eluding it.
const FREEZE_PASSES: usize = 50;
/// How long one pass waits for the processes it stopped to stop.
const FREEZE_WAIT: Duration = Duration::from_millis(200);
/// Processes never stopped, by command name, whoever started them: agent CLIs
/// another daemon or the user started, and xd itself.
const PROTECTED_NAMES: [&str; 5] = ["claude", "codex", "jcode", "xd-host", "xd"];

/// The cgroup, the daemon's own or one above it, whose memory limit would kill
/// the daemon's whole group if reached.
pub(crate) struct Cgroup {
    directory: PathBuf,
    /// Its path in the cgroup hierarchy, as `/proc/<pid>/cgroup` names it.
    path: String,
    max: u64,
}

impl Cgroup {
    /// The cgroup to guard, if the guard has anything to do; see
    /// [`guarded_limit`].
    pub(crate) fn discover() -> Option<Self> {
        let own = cgroup_path(&fs::read_to_string("/proc/self/cgroup").ok()?)?;
        let directory = |path: &str| Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/'));
        let read = |path: &str, file: &str| fs::read_to_string(directory(path).join(file)).ok();
        let levels: Vec<Level> = ancestors(&own)
            .into_iter()
            .map(|path| Level {
                max: read(&path, "memory.max"),
                oom_group: read(&path, "memory.oom.group"),
                path,
            })
            .collect();
        let (path, max) = guarded_limit(&levels)?;
        Some(Self {
            directory: directory(&path),
            path,
            max,
        })
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn max(&self) -> u64 {
        self.max
    }

    pub(crate) fn unreclaimable(&self) -> Option<u64> {
        unreclaimable_bytes(&fs::read_to_string(self.directory.join("memory.stat")).ok()?)
    }
}

/// One cgroup on the way from the daemon's to the root, with its
/// `memory.max` and `memory.oom.group` files, which the root does not have.
pub(crate) struct Level {
    pub(crate) path: String,
    pub(crate) max: Option<String>,
    pub(crate) oom_group: Option<String>,
}

/// `path` and every cgroup above it, nearest first, ending with `/`.
pub(crate) fn ancestors(path: &str) -> Vec<String> {
    let mut found = vec![path.to_owned()];
    let mut path = path.trim_end_matches('/');
    while let Some((parent, _)) = path.rsplit_once('/') {
        found.push(if parent.is_empty() { "/" } else { parent }.to_owned());
        path = parent;
    }
    found.dedup();
    found
}

/// The cgroup to guard and its limit, from `levels`, the daemon's cgroup and
/// those above it, nearest first. Reaching a finite `memory.max` kills a group
/// holding the daemon when that cgroup, or one between it and the daemon, has
/// `memory.oom.group` set. Of those limits, the smallest is reached first.
pub(crate) fn guarded_limit(levels: &[Level]) -> Option<(String, u64)> {
    let mut group_kill = false;
    let mut guarded: Option<(String, u64)> = None;
    for level in levels {
        group_kill |= level.oom_group.as_deref().map(str::trim) == Some("1");
        let max = level
            .max
            .as_deref()
            .and_then(|max| max.trim().parse::<u64>().ok())
            .filter(|max| *max > 0);
        if let Some(max) = max
            && group_kill
            && guarded.as_ref().is_none_or(|(_, smallest)| max < *smallest)
        {
            guarded = Some((level.path.clone(), max));
        }
    }
    guarded
}

/// The threshold percentage `setting` asks for, `None` to leave the guard off.
/// No setting, or one that cannot be used, means the default.
pub(crate) fn threshold_percent(setting: Option<&str>) -> Option<u64> {
    let Some(setting) = setting.map(str::trim) else {
        return Some(DEFAULT_PERCENT);
    };
    if setting.eq_ignore_ascii_case("off") || setting == "0" {
        return None;
    }
    match setting.parse() {
        Ok(percent) if PERCENT_RANGE.contains(&percent) => Some(percent),
        _ => {
            eprintln!(
                "xd-host: ignoring {PERCENT_VARIABLE}={setting}; expected off or {}-{}.",
                PERCENT_RANGE.start(),
                PERCENT_RANGE.end()
            );
            Some(DEFAULT_PERCENT)
        }
    }
}

/// The cgroup's memory that the kernel cannot reclaim, from `memory.stat`:
/// `anon + shmem + kernel - slab_reclaimable`. Kernels before 5.18 have no
/// `kernel` line, so there its unreclaimable parts are added up instead.
pub(crate) fn unreclaimable_bytes(stat: &str) -> Option<u64> {
    let fields: HashMap<&str, u64> = stat
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(' ')?;
            Some((name, value.trim().parse().ok()?))
        })
        .collect();
    let field = |name: &str| fields.get(name).copied().unwrap_or(0);
    let anon = *fields.get("anon")?;
    let kernel = match fields.get("kernel") {
        Some(kernel) => kernel.saturating_sub(field("slab_reclaimable")),
        None => [
            "slab_unreclaimable",
            "kernel_stack",
            "pagetables",
            "percpu",
            "sock",
            "vmalloc",
        ]
        .into_iter()
        .map(field)
        .sum(),
    };
    Some(anon + field("shmem") + kernel)
}

/// Decides when a reading calls for stopping something.
pub(crate) struct Trigger {
    threshold: u64,
    rearm: u64,
    /// No tree has been stopped since the measure was last well below the
    /// threshold.
    armed: bool,
    last: Option<Instant>,
}

impl Trigger {
    pub(crate) fn new(max: u64, percent: u64) -> Self {
        let share = |percent: u64| (max as u128 * percent as u128 / 100) as u64;
        Self {
            threshold: share(percent),
            rearm: share(percent.saturating_sub(REARM_POINTS)),
            armed: true,
            last: None,
        }
    }

    pub(crate) fn threshold(&self) -> u64 {
        self.threshold
    }

    /// Whether a reading of `usage` at `now` calls for stopping a tree.
    pub(crate) fn due(&mut self, usage: u64, now: Instant) -> bool {
        if usage < self.rearm {
            self.armed = true;
        }
        usage >= self.threshold
            && (self.armed
                || self
                    .last
                    .is_none_or(|last| now.duration_since(last) >= COOLDOWN))
    }

    /// Records that the guard acted at `now`, whether or not it found a tree.
    pub(crate) fn fired(&mut self, now: Instant) {
        self.armed = false;
        self.last = Some(now);
    }
}

/// One process, as `/proc` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Process {
    pub(crate) pid: u32,
    pub(crate) parent: u32,
    /// When it started, in clock ticks after boot, which tells it apart from a
    /// later process given the same pid.
    pub(crate) start: u64,
    pub(crate) uid: u32,
    pub(crate) cgroup: String,
    /// Its own unreclaimable memory: `RssAnon + RssShmem`.
    pub(crate) bytes: u64,
    /// Its process group.
    pub(crate) group: u32,
    /// The kernel's short command name, never its arguments, which can hold
    /// secrets.
    pub(crate) name: String,
}

/// The tree to stop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Victim {
    pub(crate) pid: u32,
    /// Its topmost process's command name.
    pub(crate) name: String,
    /// The command name of the process in it holding the most memory.
    pub(crate) largest: String,
    /// What the whole tree holds.
    pub(crate) bytes: u64,
    /// Every process in it, with its start time.
    pub(crate) members: Vec<(u32, u64)>,
    /// Process groups only the tree's processes are in. A process in one of
    /// them belongs to the tree even after its parent exits and it is handed
    /// to init.
    pub(crate) groups: HashSet<u32>,
    /// The nearest of `owners` above it: the agent or terminal it ran under.
    pub(crate) owner: Option<u32>,
}

/// Who may be stopped and who may not.
pub(crate) struct Scope<'a> {
    /// The daemon.
    pub(crate) me: u32,
    pub(crate) uid: u32,
    pub(crate) cgroup: &'a str,
    /// Agent processes and terminal session leaders, which are protected and
    /// own what runs below them.
    pub(crate) owners: &'a HashSet<u32>,
}

impl Scope<'_> {
    /// Whether `process` is this user's, in this cgroup or one below it, and
    /// is not the daemon, an owner, or named like an agent CLI or xd. Its
    /// ancestors are not considered.
    fn may_stop(&self, process: &Process) -> bool {
        let in_cgroup = self.cgroup == "/"
            || process.cgroup == self.cgroup
            || process
                .cgroup
                .strip_prefix(self.cgroup)
                .is_some_and(|rest| rest.starts_with('/'));
        process.uid == self.uid
            && in_cgroup
            && process.pid != self.me
            && process.pid > 2
            && process.parent != 2
            && !self.owners.contains(&process.pid)
            && !PROTECTED_NAMES.contains(&process.name.as_str())
    }
}

/// The unprotected tree holding the most memory, if any.
pub(crate) fn choose_victim(processes: &[Process], scope: &Scope) -> Option<Victim> {
    let by_pid: HashMap<u32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
    // Protected processes and everything above them, since stopping a parent
    // can take its children with it.
    let mut protected = HashSet::new();
    let seeds = processes
        .iter()
        .filter(|process| {
            process.pid == scope.me
                || process.pid <= 2
                || process.parent == 2
                || scope.owners.contains(&process.pid)
                || PROTECTED_NAMES.contains(&process.name.as_str())
        })
        .map(|process| process.pid)
        .chain([scope.me]);
    for seed in seeds {
        let mut pid = seed;
        while protected.insert(pid) {
            match by_pid.get(&pid) {
                Some(process) if process.parent != 0 => pid = process.parent,
                _ => break,
            }
        }
    }
    let candidates: HashSet<u32> = processes
        .iter()
        .filter(|process| scope.may_stop(process) && !protected.contains(&process.pid))
        .map(|process| process.pid)
        .collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for process in processes {
        if candidates.contains(&process.pid) {
            children
                .entry(process.parent)
                .or_default()
                .push(process.pid);
        }
    }
    processes
        .iter()
        .filter(|process| {
            candidates.contains(&process.pid) && !candidates.contains(&process.parent)
        })
        .map(|root| {
            let mut members = Vec::new();
            let mut bytes = 0;
            let mut largest = root;
            let mut pending = vec![root.pid];
            while let Some(pid) = pending.pop() {
                let process = by_pid[&pid];
                members.push((pid, process.start));
                bytes += process.bytes;
                if process.bytes > largest.bytes {
                    largest = process;
                }
                pending.extend(children.get(&pid).into_iter().flatten());
            }
            let member_pids: HashSet<u32> = members.iter().map(|(pid, _)| *pid).collect();
            let shared_groups: HashSet<u32> = processes
                .iter()
                .filter(|process| !member_pids.contains(&process.pid))
                .map(|process| process.group)
                .collect();
            let groups = member_pids
                .iter()
                .map(|pid| by_pid[pid].group)
                .filter(|group| !shared_groups.contains(group))
                .collect();
            // Nothing at or above the daemon is an agent or a terminal. The
            // bound only guards against a snapshot that loops.
            let mut owner = root.parent;
            for _ in 0..by_pid.len() {
                if scope.owners.contains(&owner) || owner == scope.me {
                    break;
                }
                match by_pid.get(&owner) {
                    Some(process) if process.parent != 0 => owner = process.parent,
                    _ => break,
                }
            }
            Victim {
                pid: root.pid,
                name: root.name.clone(),
                largest: largest.name.clone(),
                bytes,
                members,
                groups,
                owner: scope.owners.contains(&owner).then_some(owner),
            }
        })
        .filter(|victim| victim.bytes > 0)
        .max_by_key(|victim| victim.bytes)
}

/// `doomed` and every process that belongs with it in `processes`: their
/// children, and processes in `groups`, the tree's own process groups. Only
/// processes `scope` lets the guard stop are added.
pub(crate) fn enclose(
    processes: &[Process],
    doomed: &HashSet<(u32, u64)>,
    groups: &HashSet<u32>,
    scope: &Scope,
) -> HashSet<(u32, u64)> {
    let by_pid: HashMap<u32, &Process> = processes.iter().map(|p| (p.pid, p)).collect();
    let mut found = doomed.clone();
    loop {
        let before = found.len();
        for process in processes {
            if !scope.may_stop(process) {
                continue;
            }
            let parent_doomed = by_pid
                .get(&process.parent)
                .is_some_and(|parent| found.contains(&(parent.pid, parent.start)));
            if parent_doomed || groups.contains(&process.group) {
                found.insert((process.pid, process.start));
            }
        }
        if found.len() == before {
            return found;
        }
    }
}

/// The cgroup path in a `/proc/<pid>/cgroup` file, from its cgroup v2 line.
pub(crate) fn cgroup_path(cgroup: &str) -> Option<String> {
    cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| path.trim().to_owned())
}

/// What `/proc/<pid>/stat` says about a process.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Stat {
    pub(crate) pid: u32,
    pub(crate) name: String,
    pub(crate) state: char,
    pub(crate) parent: u32,
    pub(crate) group: u32,
    pub(crate) start: u64,
}

impl Stat {
    fn running(&self) -> bool {
        !matches!(self.state, 'Z' | 'X')
    }
}

/// Reads a `/proc/<pid>/stat` line. The command name is free text in
/// parentheses, so fields count from the last one.
pub(crate) fn parse_stat(stat: &str) -> Option<Stat> {
    let (head, fields) = stat.rsplit_once(')')?;
    let (pid, name) = head.split_once(" (")?;
    let fields: Vec<&str> = fields.split_ascii_whitespace().collect();
    // Fields 3, 4, 5 and 22 of proc(5); `fields` starts at field 3.
    Some(Stat {
        pid: pid.trim().parse().ok()?,
        name: name.to_owned(),
        state: fields.first()?.chars().next()?,
        parent: fields.get(1)?.parse().ok()?,
        group: fields.get(2)?.parse().ok()?,
        start: fields.get(19)?.parse().ok()?,
    })
}

/// The real uid and the unreclaimable resident memory, in bytes, from
/// `/proc/<pid>/status`. Kernel threads have no memory lines, so they hold none.
pub(crate) fn parse_status(status: &str) -> Option<(u32, u64)> {
    let mut uid = None;
    let mut bytes = 0;
    for line in status.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let mut words = value.split_ascii_whitespace();
        match name {
            "Uid" => uid = words.next().and_then(|uid| uid.parse().ok()),
            "RssAnon" | "RssShmem" => {
                bytes += words.next().and_then(|kib| kib.parse::<u64>().ok())? * 1024;
            }
            _ => {}
        }
    }
    Some((uid?, bytes))
}

/// Every process `/proc` lists that can still be read.
pub(crate) fn read_processes() -> Vec<Process> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let stat = parse_stat(&fs::read_to_string(path.join("stat")).ok()?)?;
            if !stat.running() {
                return None;
            }
            let (uid, bytes) = parse_status(&fs::read_to_string(path.join("status")).ok()?)?;
            let cgroup = cgroup_path(&fs::read_to_string(path.join("cgroup")).ok()?)?;
            Some(Process {
                pid: stat.pid,
                parent: stat.parent,
                start: stat.start,
                uid,
                cgroup,
                bytes,
                group: stat.group,
                name: stat.name,
            })
        })
        .collect()
}

/// Whether the process that had `pid` when it started at `start` can no longer
/// start another: it has stopped, or it has gone.
fn frozen(pid: u32, start: u64) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parse_stat(&stat))
        .is_none_or(|stat| {
            stat.start != start || !stat.running() || matches!(stat.state, 'T' | 't')
        })
}

/// Whether the process that had `pid` when it started at `start` still runs.
fn still_running(pid: u32, start: u64) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parse_stat(&stat))
        .is_some_and(|stat| stat.start == start && stat.running())
}

fn signal<'a>(members: impl IntoIterator<Item = &'a (u32, u64)>, signal: libc::c_int) {
    for &(pid, start) in members {
        if still_running(pid, start)
            && let Ok(pid) = libc::pid_t::try_from(pid)
        {
            // SAFETY: kill has no memory effects; the start time was checked
            // just before, so the pid still names the process chosen.
            unsafe { libc::kill(pid, signal) };
        }
    }
}

/// Stops `victim`'s tree: freezes it with SIGSTOP, pass by pass, until every
/// process of it has stopped and a pass over `/proc` finds none it started
/// meanwhile, then kills all of it with SIGKILL. SIGSTOP takes effect when the
/// process next runs, so a pass also waits for that.
pub(crate) fn stop(victim: &Victim, scope: &Scope) {
    let mut doomed: HashSet<(u32, u64)> = victim.members.iter().copied().collect();
    let all_frozen =
        |doomed: &HashSet<(u32, u64)>| doomed.iter().all(|&(pid, start)| frozen(pid, start));
    for _ in 0..FREEZE_PASSES {
        signal(&doomed, libc::SIGSTOP);
        let deadline = Instant::now() + FREEZE_WAIT;
        while !all_frozen(&doomed) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let found = enclose(&read_processes(), &doomed, &victim.groups, scope);
        if found.len() == doomed.len() && all_frozen(&doomed) {
            break;
        }
        doomed = found;
    }
    signal(&doomed, libc::SIGKILL);
}

/// What the user is told about a stopped tree.
/// It names commands, never their arguments, which can hold secrets.
pub(crate) fn notice(victim: &Victim, usage: u64, max: u64) -> String {
    let gib = |bytes: u64| bytes as f64 / (1u64 << 30) as f64;
    let others = victim.members.len().saturating_sub(1);
    let tree = match others {
        0 => String::new(),
        1 => " and the process it started".to_owned(),
        others => format!(" and the {others} processes it started"),
    };
    let mostly = if victim.largest != victim.name {
        format!(", mostly in `{}`", victim.largest)
    } else {
        String::new()
    };
    format!(
        "xd stopped `{}` (pid {}){tree}, holding {:.1} GiB{mostly}, because the container was about to run out of memory: {:.1} of its {:.1} GiB could not be freed, and running out would have stopped every chat and terminal.",
        victim.name,
        victim.pid,
        gib(victim.bytes),
        gib(usage),
        gib(max),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    fn process(pid: u32, parent: u32, name: &str, gib: u64) -> Process {
        Process {
            pid,
            parent,
            start: u64::from(pid) * 10,
            uid: 1000,
            cgroup: "/".into(),
            bytes: gib * GIB,
            group: pid,
            name: name.into(),
        }
    }

    /// `process`, in the process group of `group`.
    fn in_group(group: u32, process: Process) -> Process {
        Process { group, ..process }
    }

    /// A container: init, the daemon, two agents, a terminal shell, and the work
    /// they started.
    fn container() -> Vec<Process> {
        vec![
            process(1, 0, "tini", 0),
            process(10, 1, "xd-host", 1),
            // An agent running a build through its shell tool.
            process(20, 10, "claude", 1),
            // A shell tool's command stays in the agent's process group.
            in_group(20, process(21, 20, "bash", 0)),
            in_group(20, process(22, 21, "cargo", 1)),
            in_group(20, process(23, 22, "rustc", 3)),
            in_group(20, process(24, 22, "rustc", 2)),
            // Another agent with a small tool server.
            process(30, 10, "claude", 1),
            process(31, 30, "node", 1),
            // A terminal running a test.
            process(40, 10, "bash", 0),
            process(41, 40, "pytest", 4),
            // A build daemon that detached to init.
            process(50, 1, "java", 5),
        ]
    }

    fn owners() -> HashSet<u32> {
        HashSet::from([20, 30, 40])
    }

    fn scope(owners: &HashSet<u32>) -> Scope<'_> {
        Scope {
            me: 10,
            uid: 1000,
            cgroup: "/",
            owners,
        }
    }

    #[test]
    fn only_a_finite_limit_that_kills_the_whole_group_is_guarded() {
        let level = |path: &str, max: Option<&str>, oom_group: Option<&str>| Level {
            path: path.into(),
            max: max.map(str::to_owned),
            oom_group: oom_group.map(str::to_owned),
        };
        // A container whose own cgroup is the root of its namespace.
        assert_eq!(
            guarded_limit(&[level("/", Some("17179869184\n"), Some("1\n"))]),
            Some(("/".into(), 16 * GIB))
        );
        assert_eq!(
            guarded_limit(&[level("/", Some("max\n"), Some("1\n"))]),
            None
        );
        assert_eq!(
            guarded_limit(&[level("/", Some("17179869184\n"), Some("0\n"))]),
            None
        );
        // The daemon in a child cgroup of a limited container.
        assert_eq!(
            guarded_limit(&[
                level("/pod/xd", Some("max\n"), Some("0\n")),
                level("/pod", Some("17179869184\n"), Some("1\n")),
                level("/", None, None),
            ]),
            Some(("/pod".into(), 16 * GIB))
        );
        // Group killing set below the limit still takes the daemon's group.
        assert_eq!(
            guarded_limit(&[
                level("/pod/xd", Some("max\n"), Some("1\n")),
                level("/pod", Some("17179869184\n"), Some("0\n")),
            ]),
            Some(("/pod".into(), 16 * GIB))
        );
        // A limit below any group kill kills one process, not the group; of
        // two that would, the smaller is reached first.
        assert_eq!(
            guarded_limit(&[
                level("/pod/xd", Some("1073741824\n"), Some("0\n")),
                level("/pod", Some("34359738368\n"), Some("1\n")),
                level("/", Some("17179869184\n"), Some("0\n")),
            ]),
            Some(("/".into(), 16 * GIB))
        );
        assert_eq!(ancestors("/"), ["/"]);
        assert_eq!(ancestors("/pod/xd"), ["/pod/xd", "/pod", "/"]);
    }

    #[test]
    fn the_threshold_comes_from_the_setting_or_the_default() {
        assert_eq!(threshold_percent(None), Some(92));
        assert_eq!(threshold_percent(Some("85")), Some(85));
        assert_eq!(threshold_percent(Some("off")), None);
        assert_eq!(threshold_percent(Some("0")), None);
        assert_eq!(threshold_percent(Some("100")), Some(92));
        assert_eq!(threshold_percent(Some("lots")), Some(92));
    }

    #[test]
    fn page_cache_never_counts_toward_the_threshold() {
        let stat = "anon 5755305984\nfile 9108017152\nkernel 427618304\nkernel_stack 1000\n\
                    shmem 4096\nfile_mapped 1\nslab_reclaimable 397395392\n";
        assert_eq!(
            unreclaimable_bytes(stat),
            Some(5755305984 + 4096 + 427618304 - 397395392)
        );
        let older = "anon 100\nfile 900000\nkernel_stack 2\npagetables 3\npercpu 4\nsock 5\n\
                     vmalloc 6\nshmem 7\nslab_reclaimable 1000\nslab_unreclaimable 8\n";
        assert_eq!(
            unreclaimable_bytes(older),
            Some(100 + 7 + 2 + 3 + 4 + 5 + 6 + 8)
        );
        assert_eq!(unreclaimable_bytes("file 900000\n"), None);
    }

    #[test]
    fn one_spike_stops_one_tree_until_it_passes_or_the_cooldown_ends() {
        let start = Instant::now();
        let mut trigger = Trigger::new(100 * GIB, 92);
        assert!(!trigger.due(91 * GIB, start));
        assert!(trigger.due(92 * GIB, start));
        trigger.fired(start);
        assert!(!trigger.due(95 * GIB, start + Duration::from_secs(1)));
        assert!(
            !trigger.due(88 * GIB, start + Duration::from_secs(2)),
            "not far enough below to count as over"
        );
        assert!(!trigger.due(93 * GIB, start + Duration::from_secs(3)));
        assert!(trigger.due(93 * GIB, start + COOLDOWN));
        trigger.fired(start + COOLDOWN);
        let later = start + COOLDOWN + Duration::from_secs(1);
        assert!(!trigger.due(86 * GIB, later));
        assert!(trigger.due(92 * GIB, later), "a new spike");
    }

    #[test]
    fn the_largest_unprotected_tree_is_stopped_from_its_top() {
        let owners = owners();
        let victim = choose_victim(&container(), &scope(&owners)).unwrap();
        assert_eq!(victim.pid, 21, "the shell the agent ran the build in");
        assert_eq!(victim.bytes, 6 * GIB);
        let mut members: Vec<u32> = victim.members.iter().map(|(pid, _)| *pid).collect();
        members.sort_unstable();
        assert_eq!(members, [21, 22, 23, 24]);
        assert_eq!(victim.members[0], (21, 210));
        assert_eq!(victim.owner, Some(20));
        assert_eq!(
            (victim.name.as_str(), victim.largest.as_str()),
            ("bash", "rustc")
        );
        assert!(
            victim.groups.is_empty(),
            "the agent's own process group does not mark the tree"
        );
    }

    #[test]
    fn a_tree_is_frozen_with_what_it_started_since_and_what_it_left_behind() {
        let owners = owners();
        let mut processes = container();
        processes.retain(|process| ![21, 22, 23, 24, 50].contains(&process.pid));
        let victim = choose_victim(&processes, &scope(&owners)).unwrap();
        assert_eq!(victim.pid, 41);
        assert_eq!(victim.groups, HashSet::from([41]));
        let doomed: HashSet<(u32, u64)> = victim.members.iter().copied().collect();

        // Since the snapshot, the test started a worker, and a helper whose
        // parent exited at once, so init holds it. Neither may escape. An agent
        // started inside the tree is still never stopped.
        processes.push(process(42, 41, "pytest", 2));
        processes.push(in_group(41, process(43, 1, "helper", 1)));
        processes.push(process(44, 42, "claude", 1));
        // Nor does one under another user or in another cgroup.
        let mut stranger = process(45, 41, "sudo", 1);
        stranger.uid = 0;
        let mut elsewhere = in_group(41, process(46, 1, "make", 1));
        elsewhere.cgroup = "/other".into();
        processes.extend([stranger, elsewhere]);
        for process in &mut processes {
            if process.cgroup == "/" {
                process.cgroup = "/system.slice/xd.service".into();
            }
        }
        let scope = Scope {
            cgroup: "/system.slice/xd.service",
            ..scope(&owners)
        };
        let found = enclose(&processes, &doomed, &victim.groups, &scope);
        assert_eq!(found, HashSet::from([(41, 410), (42, 420), (43, 430)]));
        assert_eq!(enclose(&processes, &found, &victim.groups, &scope), found);
    }

    #[test]
    fn a_detached_build_daemon_or_a_terminal_command_can_be_stopped_too() {
        let owners = owners();
        let mut processes = container();
        processes.retain(|process| ![21, 22, 23, 24].contains(&process.pid));
        let victim = choose_victim(&processes, &scope(&owners)).unwrap();
        assert_eq!((victim.pid, victim.owner), (50, None));

        processes.retain(|process| process.pid != 50);
        let victim = choose_victim(&processes, &scope(&owners)).unwrap();
        assert_eq!((victim.pid, victim.owner), (41, Some(40)));
    }

    #[test]
    fn agents_the_daemon_and_their_ancestors_are_never_stopped() {
        let owners = HashSet::new();
        let processes = vec![
            process(1, 0, "init", 0),
            process(5, 1, "sh", 0),
            process(10, 5, "xd-host", 9),
            // An agent another daemon started, under a shell of its own.
            process(60, 1, "bash", 0),
            process(61, 60, "codex", 9),
            process(70, 1, "jcode", 9),
        ];
        assert_eq!(choose_victim(&processes, &scope(&owners)), None);
    }

    #[test]
    fn only_this_users_processes_in_this_cgroup_are_considered() {
        let owners = owners();
        let mut processes = container();
        for process in &mut processes {
            process.cgroup = "/system.slice/xd.service".into();
        }
        let elsewhere = processes.iter_mut().find(|p| p.pid == 50).unwrap();
        elsewhere.cgroup = "/user.slice".into();
        let mut stranger = process(80, 1, "postgres", 9);
        stranger.uid = 70;
        stranger.cgroup = "/system.slice/xd.service".into();
        let mut nested = process(90, 1, "make", 7);
        nested.cgroup = "/system.slice/xd.service/build".into();
        let mut sibling = process(91, 1, "make", 8);
        sibling.cgroup = "/system.slice/xd.service-other".into();
        processes.extend([stranger, nested, sibling]);
        let scope = Scope {
            cgroup: "/system.slice/xd.service",
            ..scope(&owners)
        };
        assert_eq!(choose_victim(&processes, &scope).unwrap().pid, 90);
    }

    #[test]
    fn proc_files_are_read_as_the_kernel_writes_them() {
        assert_eq!(
            parse_stat(
                "4242 (odd) name) S 7 4200 4242 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 98765 1 2"
            ),
            Some(Stat {
                pid: 4242,
                name: "odd) name".into(),
                state: 'S',
                parent: 7,
                group: 4200,
                start: 98765,
            })
        );
        assert_eq!(parse_stat("damaged"), None);
        assert_eq!(
            parse_status(
                "Name:\tcargo\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t  900 kB\nRssAnon:\t  600 kB\n\
                 RssFile:\t  280 kB\nRssShmem:\t  20 kB\n"
            ),
            Some((1000, 620 * 1024))
        );
        assert_eq!(
            parse_status("Name:\tkthreadd\nUid:\t0\t0\t0\t0\n"),
            Some((0, 0))
        );
        assert_eq!(cgroup_path("0::/\n"), Some("/".into()));
        assert_eq!(
            cgroup_path("1:name=systemd:/x\n0::/system.slice/xd.service\n"),
            Some("/system.slice/xd.service".into())
        );
    }

    #[test]
    fn this_process_is_read_from_proc() {
        let me = std::process::id();
        let found = read_processes()
            .into_iter()
            .find(|process| process.pid == me)
            .expect("this process");
        // SAFETY: neither call has preconditions.
        assert_eq!(found.parent, unsafe { libc::getppid() } as u32);
        assert_eq!(found.uid, unsafe { libc::getuid() });
        assert!(found.bytes > 0);
        assert!(still_running(me, found.start));
        assert!(!still_running(me, found.start + 1));
    }

    /// Only processes this test starts, in a process group of their own, are
    /// signalled.
    #[test]
    fn stopping_a_tree_kills_the_children_it_started_after_it_was_chosen() {
        use std::os::unix::process::CommandExt;

        let mut shell = std::process::Command::new("sh")
            .args(["-c", "sleep 30 & sleep 31 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let root = shell.id();
        let children = || -> Vec<(u32, u64)> {
            read_processes()
                .into_iter()
                .filter(|process| process.parent == root)
                .map(|process| (process.pid, process.start))
                .collect()
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while children().len() < 2 {
            assert!(
                Instant::now() < deadline,
                "the shell never started its sleeps"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = children();
        let start = read_processes()
            .into_iter()
            .find(|process| process.pid == root)
            .unwrap()
            .start;
        // Chosen before its children existed: only the shell is listed.
        let victim = Victim {
            pid: root,
            name: "sh".into(),
            largest: "sh".into(),
            bytes: 0,
            members: vec![(root, start)],
            groups: HashSet::new(),
            owner: None,
        };
        let owners = HashSet::new();
        let cgroup = read_processes()
            .into_iter()
            .find(|process| process.pid == root)
            .unwrap()
            .cgroup;
        stop(
            &victim,
            &Scope {
                me: std::process::id(),
                // SAFETY: getuid has no preconditions.
                uid: unsafe { libc::getuid() },
                cgroup: &cgroup,
                owners: &owners,
            },
        );
        shell.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while left.iter().any(|&(pid, start)| still_running(pid, start)) {
            assert!(Instant::now() < deadline, "a child survived");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_notice_says_what_was_stopped_and_why() {
        let victim = Victim {
            pid: 21,
            name: "bash".into(),
            largest: "rustc".into(),
            bytes: 6 * GIB,
            members: vec![(21, 1), (22, 2), (23, 3)],
            groups: HashSet::new(),
            owner: None,
        };
        assert_eq!(
            notice(&victim, 15 * GIB, 16 * GIB),
            "xd stopped `bash` (pid 21) and the 2 processes it started, holding 6.0 GiB, mostly in `rustc`, because the container was about to run out of memory: 15.0 of its 16.0 GiB could not be freed, and running out would have stopped every chat and terminal."
        );
        let alone = Victim {
            name: "java".into(),
            largest: "java".into(),
            members: vec![(21, 1)],
            ..victim
        };
        assert_eq!(
            notice(&alone, 15 * GIB, 16 * GIB),
            "xd stopped `java` (pid 21), holding 6.0 GiB, because the container was about to run out of memory: 15.0 of its 16.0 GiB could not be freed, and running out would have stopped every chat and terminal."
        );
    }
}
