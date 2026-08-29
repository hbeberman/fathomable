// @okf-doc: /decisions/0041-session-bonds.md
//! Session bonds: how a headless `--mcp` learns which harness session it
//! serves (ADR 0041).
//!
//! A harness runs its hooks and its MCP servers as children of one
//! process. The `hello` hook, which is handed the session id, records
//! the young ancestors of its own process as a [`Bond`]; the MCP server
//! later looks for the nearest of its own ancestors in those bonds and
//! takes that session's id. Neither side names the harness: whatever
//! process both descend from *is* the session.
//!
//! Everything here is best effort on Linux `/proc`. A denied read, a
//! PID-namespace boundary, or a detached server shortens the chain or
//! empties it, and [`session_for`] answers `None`; nothing is ever
//! guessed.

use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How young an ancestor must be for `hello` to record it.
///
/// A session-scoped process (the harness, its shell wrapper) starts
/// seconds before the session-start hook; the user's shell and
/// multiplexer, or an outer harness that spawned this one, are older
/// and must stay out of the bond, or a hand-run `--mcp` under that shell
/// would sign as this session.
pub const BOND_WINDOW: Duration = Duration::from_secs(30);

/// `/proc` reports process start times in `USER_HZ` ticks, which Linux
/// fixes at 100 on every architecture this project runs on, independent
/// of the kernel's own `HZ`.
const TICKS_PER_SECOND: u64 = 100;

/// Ancestor chains longer than this are cut; a real one is a handful.
const MAX_DEPTH: usize = 64;

/// A process identified well enough to survive pid reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Process {
    pid: u32,
    /// Start time in `USER_HZ` ticks since boot, from `/proc/<pid>/stat`.
    started: u64,
}

impl Process {
    /// A process by `pid` that started at `started` ticks since boot.
    #[must_use]
    pub fn new(pid: u32, started: u64) -> Self {
        Self { pid, started }
    }

    /// The process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Start time in `USER_HZ` ticks since boot.
    #[must_use]
    pub fn started(&self) -> u64 {
        self.started
    }
}

/// The processes a `hello` hook ran under, recorded for its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bond {
    id: String,
    processes: Vec<Process>,
    created: u64,
}

impl Bond {
    /// Bond session `id` to `processes`, recorded at `created` (Unix seconds).
    #[must_use]
    pub fn new(id: impl Into<String>, processes: Vec<Process>, created: u64) -> Self {
        Self {
            id: id.into(),
            processes,
            created,
        }
    }

    /// The harness session id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The recorded ancestors, nearest the hook first.
    #[must_use]
    pub fn processes(&self) -> &[Process] {
        &self.processes
    }

    /// When the bond was recorded, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }
}

/// The ancestors of this process, nearest first, up to but excluding
/// pid 1; empty when `/proc` cannot be read.
#[must_use]
pub fn ancestors() -> Vec<Process> {
    let mut chain = Vec::new();
    let Some((mut parent, _)) = stat(std::process::id()) else {
        return chain;
    };
    while parent > 1 && chain.len() < MAX_DEPTH {
        let Some((grandparent, started)) = stat(parent) else {
            break;
        };
        chain.push(Process::new(parent, started));
        parent = grandparent;
    }
    chain
}

/// The ancestors of this process that started within `window` before
/// it did, nearest first: the ones born with the session.
#[must_use]
pub fn ancestors_within(window: Duration) -> Vec<Process> {
    let Some((_, own_start)) = stat(std::process::id()) else {
        return Vec::new();
    };
    let ticks = window.as_secs().saturating_mul(TICKS_PER_SECOND);
    let cutoff = own_start.saturating_sub(ticks);
    ancestors()
        .into_iter()
        .take_while(|p| p.started >= cutoff)
        .collect()
}

/// The session the nearest bonded ancestor belongs to, when exactly one
/// session recorded it. An ancestor recorded for two sessions is shared
/// by both and ends the search: anything above it is shared more widely.
#[must_use]
pub fn session_for<'a>(bonds: &'a [Bond], ancestors: &[Process]) -> Option<&'a str> {
    for process in ancestors {
        let mut found: Option<&str> = None;
        for bond in bonds.iter().filter(|b| b.processes.contains(process)) {
            match found {
                None => found = Some(&bond.id),
                Some(id) if id == bond.id => {}
                Some(_) => return None,
            }
        }
        if found.is_some() {
            return found;
        }
    }
    None
}

/// `(ppid, start ticks)` of `pid` from `/proc/<pid>/stat`.
fn stat(pid: u32) -> Option<(u32, u64)> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat(&text)
}

/// Parse a `/proc/<pid>/stat` line: the fields after the parenthesised
/// command name are state, ppid (field 4), …, start time (field 22).
fn parse_stat(text: &str) -> Option<(u32, u64)> {
    let rest = &text[text.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let ppid = fields.nth(1)?.parse().ok()?;
    // Field 22 is the twentieth after the state, of which two are consumed.
    let started = fields.nth(17)?.parse().ok()?;
    Some((ppid, started))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;

    use super::{Bond, Process, ancestors, parse_stat, session_for};

    type TestResult = Result<(), Box<dyn Error>>;

    fn bond(id: &str, pids: &[(u32, u64)]) -> Bond {
        Bond::new(
            id,
            pids.iter().map(|&(p, s)| Process::new(p, s)).collect(),
            0,
        )
    }

    #[test]
    fn stat_line_yields_ppid_and_start_time_past_a_tricky_name() {
        let line = "42 (a (b) c) S 7 42 42 0 -1 4194560 1 0 0 0 0 0 0 0 20 0 1 0 12345 0 0 \
                    18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
        assert_eq!(parse_stat(line), Some((7, 12345)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn own_ancestors_start_with_the_parent() -> TestResult {
        let status = fs::read_to_string("/proc/self/status")?;
        let ppid: u32 = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .ok_or("no PPid line")?
            .trim()
            .parse()?;
        let chain = ancestors();
        let first = chain.first().ok_or("no ancestors")?;
        assert_eq!(first.pid(), ppid);
        assert!(first.started() > 0);
        Ok(())
    }

    #[test]
    fn nearest_bonded_ancestor_wins() {
        let bonds = [
            bond("outer", &[(10, 1), (2, 1)]),
            bond("inner", &[(20, 5), (10, 1)]),
        ];
        let mine = [
            Process::new(30, 9),
            Process::new(20, 5),
            Process::new(10, 1),
        ];
        assert_eq!(session_for(&bonds, &mine), Some("inner"));
    }

    #[test]
    fn a_reused_pid_does_not_match() {
        let bonds = [bond("s", &[(20, 5)])];
        assert_eq!(session_for(&bonds, &[Process::new(20, 6)]), None);
    }

    #[test]
    fn an_ancestor_shared_by_two_sessions_ends_the_search() {
        let bonds = [bond("a", &[(10, 1)]), bond("b", &[(10, 1), (2, 1)])];
        let mine = [Process::new(10, 1), Process::new(2, 1)];
        assert_eq!(session_for(&bonds, &mine), None);
    }

    #[test]
    fn two_bonds_of_one_session_are_not_ambiguous() {
        let bonds = [bond("s", &[(10, 1)]), bond("s", &[(11, 2), (10, 1)])];
        assert_eq!(session_for(&bonds, &[Process::new(10, 1)]), Some("s"));
    }
}
