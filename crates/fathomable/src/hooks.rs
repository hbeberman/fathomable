// @okf-doc: /decisions/0040-agent-subscriptions-and-hooks.md
//! `fathomable hello` and `fathomable pending`: the harness-hook side of
//! agent subscriptions (ADR 0040).
//!
//! Both read the harness's hook JSON on stdin, resolve the workspace from
//! its `cwd`, and print in the shape the harness named by `--hook`
//! expects. Both exit 0 with no output whenever there is nothing to say:
//! no workspace here, no subscription for the session, a subagent, a
//! continuation the hook itself caused, or nothing pending. Neither ever
//! writes to the thread store; both append to the agent register —
//! `hello` the session bond of ADR 0041, `pending` its deliveries.
//!
//! `hello` delivers too when its `source` is `resume`, so a session that
//! comes back after being stopped has its pending threads in context
//! without waiting for a turn to end. That blob is context rather than a
//! blocked stop, so it is composed as [`Occasion::Resume`] and does not
//! count toward the nag; every other `source` is left to the stop hook.

use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use fathomable_core::XdgDirs;
use fathomable_core::agents::{Blob, Register, Subscriber};
use fathomable_core::annotations::{Scope, Store, Thread};
use fathomable_core::bond;
use fathomable_core::config::{AgentsConfig, Config};
use fathomable_core::session::Marker;
use fathomable_core::workspace::Workspace;
use serde_json::{Value, json};

use crate::app::threads::now;

/// The agent harness whose hook is calling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    /// Claude Code: `SessionStart` and `Stop` in `settings.json`.
    Claude,
    /// Codex CLI: `SessionStart` and `Stop` in `hooks.json`.
    Codex,
    /// Copilot CLI: `sessionStart` and `agentStop` in `.github/hooks`.
    Copilot,
    /// VS Code agent mode: `SessionStart` and `Stop` in `.github/hooks`.
    Vscode,
}

/// Why a blob is being composed, which decides how hard it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occasion {
    /// A `Stop` hook or the viewer's `Space w`: the blob forces a turn,
    /// and a delivered-but-unanswered thread counts toward the nag.
    TurnEnd,
    /// A `SessionStart` resume: the blob is only added to context, so it
    /// does not count a check — the nag cadence of ADR 0040 is measured
    /// in turn-ends, not in resumes.
    Resume,
}

/// What a hook's stdin said, in the fields every harness shares.
#[derive(Debug, Default, PartialEq, Eq)]
struct Input {
    session: Option<String>,
    cwd: Option<PathBuf>,
    /// `SessionStart`'s `source`: `startup`, `resume`, `clear`,
    /// `compact`, or `fork`. Absent on the other events.
    source: Option<String>,
    continuation: bool,
    subagent: bool,
}

impl Input {
    fn parse(harness: Harness, value: &Value) -> Self {
        let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        let session = field("session_id").or_else(|| field("sessionId"));
        let subagent = match harness {
            Harness::Claude | Harness::Codex | Harness::Vscode => value.get("agent_id").is_some(),
            // A Copilot subagent's transcript is its parent's (ADR 0040).
            Harness::Copilot => match (field("transcriptPath"), &session) {
                (Some(path), Some(id)) => {
                    path.is_empty()
                        || Path::new(&path)
                            .parent()
                            .and_then(Path::file_name)
                            .and_then(|n| n.to_str())
                            .is_some_and(|parent| parent != id)
                }
                _ => false,
            },
        };
        Self {
            session,
            cwd: field("cwd").map(PathBuf::from),
            source: field("source"),
            continuation: value
                .get("stop_hook_active")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            subagent,
        }
    }
}

/// Read the hook JSON from stdin when something is piped in.
fn read_input(harness: Harness) -> Input {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return Input::default();
    }
    let mut text = String::new();
    if stdin.lock().read_to_string(&mut text).is_err() || text.trim().is_empty() {
        return Input::default();
    }
    serde_json::from_str::<Value>(&text)
        .map_or_else(|_| Input::default(), |v| Input::parse(harness, &v))
}

/// The known workspace whose root is the longest prefix of `cwd`.
fn workspace_for(dirs: &XdgDirs, cwd: &Path) -> Option<PathBuf> {
    Marker::list(dirs)
        .into_iter()
        .map(|m| m.root().to_path_buf())
        .filter(|root| cwd.starts_with(root))
        .max_by_key(|root| root.as_os_str().len())
}

fn agents_config(dirs: &XdgDirs) -> AgentsConfig {
    Config::load(dirs, None).map_or_else(|_| AgentsConfig::default(), |c| c.agents().clone())
}

/// `fathomable hello`: tell the model its session id and how to subscribe.
pub fn hello(dirs: &XdgDirs, harness: Harness, id: Option<String>) -> ExitCode {
    let input = read_input(harness);
    let Some(id) = id.or(input.session) else {
        return ExitCode::SUCCESS;
    };
    if input.subagent {
        return ExitCode::SUCCESS;
    }
    let cwd = input
        .cwd
        .or_else(|| env::current_dir().ok())
        .unwrap_or_default();
    let Some(root) = workspace_for(dirs, &cwd) else {
        return ExitCode::SUCCESS;
    };
    let config = agents_config(dirs);
    bond_session(dirs, &root, &id, &config);
    let types = config.types.join(", ");
    let mut text = format!(
        "Fathomable is watching this workspace ({}): the user reads your work there and \
         leaves review comments on lines. Your session id is {id}. Before you edit, call \
         the fathomable `follow` tool with id \"{id}\", a type (one of: {types}), and the \
         paths you will edit. Fathomable then hands you unanswered comments when your turn \
         ends; act on them and answer with `thread_reply` (pass id \"{id}\" to it if \
         you did not call `follow` on this connection).",
        root.display()
    );
    // A resume comes back with the connection's memory gone and may have
    // missed comments while the session was stopped, so it is handed them
    // here rather than waiting for a turn to end (ADR 0040 note). The
    // other sources are left alone: `startup` and `fork` have not
    // subscribed yet, and `compact` is mid-task, where consuming a
    // delivery — and firing a watch, which `Register::fire` removes —
    // into context the model may not act on would lose it, with the stop
    // hook then silent because it is recorded as delivered.
    if input.source.as_deref() == Some("resume")
        && let Ok(Some(blob)) = compose(dirs, &root, &id, &config, Occasion::Resume)
    {
        text.push_str("\n\n");
        text.push_str(&blob);
    }
    match harness {
        Harness::Claude | Harness::Codex => println!("{text}"),
        Harness::Copilot => println!("{}", json!({ "additionalContext": text })),
        Harness::Vscode => println!(
            "{}",
            json!({ "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": text } })
        ),
    }
    ExitCode::SUCCESS
}

/// Record the young ancestors of this hook process under `id`, so the
/// MCP server spawned by the same harness can sign as it (ADR 0041).
fn bond_session(dirs: &XdgDirs, root: &Path, id: &str, config: &AgentsConfig) {
    let processes = bond::ancestors_within(bond::BOND_WINDOW);
    if processes.is_empty() {
        return;
    }
    let when = now();
    let outcome = Register::open(dirs.agents_file(root), when, config.expire_after)
        .and_then(|mut register| register.bond(id, processes, when));
    if let Err(error) = outcome {
        tracing::warn!(%error, "cannot record the session bond");
    }
}

/// `fathomable pending`: hand the subscriber what it has not seen.
pub fn pending(
    dirs: &XdgDirs,
    harness: Option<Harness>,
    id: Option<String>,
    prompt: bool,
) -> ExitCode {
    let input = harness.map(read_input).unwrap_or_default();
    let Some(id) = id.or(input.session) else {
        return ExitCode::SUCCESS;
    };
    if input.continuation || input.subagent {
        return ExitCode::SUCCESS;
    }
    let cwd = input
        .cwd
        .or_else(|| env::current_dir().ok())
        .unwrap_or_default();
    let Some(root) = workspace_for(dirs, &cwd) else {
        return ExitCode::SUCCESS;
    };
    let config = agents_config(dirs);
    let text = match compose(dirs, &root, &id, &config, Occasion::TurnEnd) {
        Ok(Some(text)) => text,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fathomable: {error}");
            return ExitCode::SUCCESS;
        }
    };
    if prompt {
        println!("{text}");
        return ExitCode::SUCCESS;
    }
    match harness {
        None | Some(Harness::Claude | Harness::Codex) => {
            eprintln!("{text}");
            ExitCode::from(2)
        }
        Some(Harness::Copilot) => {
            println!("{}", json!({ "decision": "block", "reason": text }));
            ExitCode::SUCCESS
        }
        Some(Harness::Vscode) => {
            println!(
                "{}",
                json!({ "hookSpecificOutput": { "hookEventName": "Stop", "decision": "block", "reason": text } })
            );
            ExitCode::SUCCESS
        }
    }
}

/// The threads of `root` in the current git scope, read without
/// re-anchoring: the hook shows, it does not move.
fn scoped_threads(dirs: &XdgDirs, root: &Path) -> Result<Vec<Thread>, String> {
    let store = Store::open(dirs.threads_file(root)).map_err(|e| e.to_string())?;
    let scope = Workspace::discover(root)
        .ok()
        .and_then(|w| w.reachable(store.commits()))
        .map_or_else(Scope::unscoped, Scope::reachable);
    Ok(store
        .threads()
        .iter()
        .filter(|t| scope.includes(t))
        .cloned()
        .collect())
}

/// The prompt for subscriber `id`, recording what it contains as
/// delivered; `None` when there is nothing to say.
pub(crate) fn compose(
    dirs: &XdgDirs,
    root: &Path,
    id: &str,
    config: &AgentsConfig,
    occasion: Occasion,
) -> Result<Option<String>, String> {
    let when = now();
    let mut register = Register::open(dirs.agents_file(root), when, config.expire_after)
        .map_err(|e| e.to_string())?;
    let Some(subscriber) = register.subscriber(id).cloned() else {
        return Ok(None);
    };
    let threads = scoped_threads(dirs, root)?;
    let mut blob = gather(&mut register, &subscriber, &threads, config, occasion, when)
        .map_err(|e| e.to_string())?;
    if blob.is_empty() {
        register.touch(id, when).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    // Split the overflow off before rendering, so that only what the blob
    // actually shows is recorded as delivered and the rest still comes
    // back from `threads_pending`, as the blob's own tail line says.
    blob.fit(&subscriber, config.max_lines);
    let text = blob.render(&subscriber);
    for thread in blob.shown() {
        register
            .deliver(id, thread, when)
            .map_err(|e| e.to_string())?;
    }
    Ok(Some(text))
}

/// Fired watches, fresh deliveries, and a reminder when one is due.
pub(crate) fn gather<'a>(
    register: &mut Register,
    subscriber: &Subscriber,
    threads: &'a [Thread],
    config: &AgentsConfig,
    occasion: Occasion,
    when: u64,
) -> Result<Blob<'a>, fathomable_core::agents::RegisterError> {
    let fired = register.fire(subscriber.id(), threads, when)?;
    let fired: Vec<_> = fired
        .into_iter()
        .map(|f| {
            let remind: Vec<&Thread> = f
                .watch
                .remind()
                .iter()
                .filter_map(|id| threads.iter().find(|t| t.id() == id))
                .collect();
            (f, remind)
        })
        .collect();
    let already: Vec<&Thread> = fired
        .iter()
        .flat_map(|(f, r)| std::iter::once(f.thread).chain(r.iter().copied()))
        .collect();
    let fresh: Vec<&Thread> = register
        .deliverable(subscriber, threads)
        .into_iter()
        .filter(|t| !already.iter().any(|r| r.id() == t.id()))
        .collect();
    let stale = register.stale(subscriber, threads);
    let due =
        occasion == Occasion::TurnEnd && !stale.is_empty() && fresh.is_empty() && fired.is_empty();
    let reminder = if due && register.check(subscriber.id(), config.nag_after, when)? {
        stale
    } else {
        Vec::new()
    };
    Ok(Blob {
        fired,
        fresh,
        reminder,
        listed: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::XdgDirs;
    use fathomable_core::agents::Register;
    use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Store};
    use fathomable_core::config::AgentsConfig;
    use fathomable_core::session::Marker;
    use serde_json::json;

    use super::{Harness, Input, Occasion, compose, workspace_for};
    use crate::app::threads::now;

    type TestResult = Result<(), Box<dyn Error>>;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-hooks-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws/src"))?;
            fs::create_dir_all(dir.join("state"))?;
            Ok(Self(dir))
        }

        fn dirs(&self) -> XdgDirs {
            let state = self.0.join("state").into_os_string();
            XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A subscribed session is handed a thread once; the next check is
    /// silent until someone else speaks; an unsubscribed one hears nothing.
    #[test]
    fn pending_delivers_each_message_once() -> TestResult {
        let dir = TempDir::new("once")?;
        let dirs = dir.dirs();
        let root = dir.0.join("ws").canonicalize()?;
        Marker::new(root.clone()).write(&dirs)?;
        assert_eq!(workspace_for(&dirs, &root.join("src")), Some(root.clone()));
        assert_eq!(workspace_for(&dirs, Path::new("/nowhere")), None);
        let mut store = Store::open(dirs.threads_file(&root))?;
        let id = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(1, 1), "why?"),
            "one\n",
            5,
        )?;
        let config = AgentsConfig {
            nag_after: 2,
            ..AgentsConfig::default()
        };
        assert_eq!(
            compose(&dirs, &root, "ghost", &config, Occasion::TurnEnd)?,
            None
        );
        let when = now();
        let mut register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
        register.subscribe("s-1", "coder", Some("bot"), None, vec![], when)?;
        let text =
            compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?.ok_or("nothing delivered")?;
        assert!(text.contains("1 review thread needs your reply"), "{text}");
        assert!(text.contains("user: why?"), "{text}");
        assert_eq!(
            compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?,
            None,
            "first check is quiet"
        );
        let nag = compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?.ok_or("no reminder")?;
        assert!(
            nag.starts_with("FATHOMABLE reminder: 1 thread still unanswered: a.md:1"),
            "{nag}"
        );
        assert_eq!(
            compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?,
            None
        );
        let me = Author::agent("bot").subscribed("s-1", "coder");
        store.reply(&id, Reply::new(me, 7, "because"))?;
        assert_eq!(
            compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?,
            None,
            "own reply"
        );
        store.reply(&id, Reply::new(Author::User, 8, "hmm"))?;
        let again = compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?.ok_or("nothing")?;
        assert!(again.contains("user: hmm"), "{again}");
        Ok(())
    }

    /// A blob past `max-lines` lists the rest and tells the model to call
    /// `threads_pending` — so the rest must still be deliverable there.
    #[test]
    fn overflow_threads_survive_for_threads_pending() -> TestResult {
        let dir = TempDir::new("overflow")?;
        let dirs = dir.dirs();
        let root = dir.0.join("ws").canonicalize()?;
        Marker::new(root.clone()).write(&dirs)?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        let text = "one\ntwo\nthree\n";
        let mut ids = Vec::new();
        for n in 0..4 {
            ids.push(store.annotate(
                Draft::new(Path::new("a.md"), LineRange::new(1, 3), format!("why {n}?")),
                text,
                5,
            )?);
        }
        let config = AgentsConfig {
            max_lines: 20,
            ..AgentsConfig::default()
        };
        let when = now();
        let mut register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
        register.subscribe("s-1", "coder", Some("bot"), None, vec![], when)?;
        let blob =
            compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?.ok_or("nothing delivered")?;
        assert!(blob.contains("more; call `threads_pending`"), "{blob}");
        // A thread is shown in full only if it has a `── thread <id>`
        // header; the overflow appears by id alone in the closing list.
        let listed: Vec<_> = ids
            .iter()
            .filter(|id| !blob.contains(&format!("── thread {id} ")))
            .collect();
        assert!(!listed.is_empty(), "nothing overflowed: {blob}");

        // The blob told the model to fetch the rest; they must be there.
        let register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
        let subscriber = register.subscriber("s-1").ok_or("gone")?;
        let threads = super::scoped_threads(&dirs, &root)?;
        let deliverable: Vec<_> = register
            .deliverable(subscriber, &threads)
            .into_iter()
            .map(|t| t.id().clone())
            .collect();
        for id in &listed {
            assert!(
                deliverable.contains(id),
                "{id} was listed as \"more; call threads_pending\" but is already \
                 recorded as delivered, so threads_pending will not return it"
            );
        }
        Ok(())
    }

    /// A resume hands the blob over as context, not as a forced turn, so
    /// it must not spend the nag cadence that ADR 0040 counts in
    /// turn-ends — otherwise resuming repeatedly would nag about threads
    /// no turn ever refused to answer.
    #[test]
    fn a_resume_does_not_count_toward_the_nag() -> TestResult {
        let dir = TempDir::new("resume")?;
        let dirs = dir.dirs();
        let root = dir.0.join("ws").canonicalize()?;
        Marker::new(root.clone()).write(&dirs)?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(1, 1), "why?"),
            "one\n",
            5,
        )?;
        let config = AgentsConfig {
            nag_after: 2,
            ..AgentsConfig::default()
        };
        let when = now();
        let mut register = Register::open(dirs.agents_file(&root), when, config.expire_after)?;
        register.subscribe("s-1", "coder", Some("bot"), None, vec![], when)?;
        assert!(compose(&dirs, &root, "s-1", &config, Occasion::Resume)?.is_some());
        for _ in 0..5 {
            assert_eq!(
                compose(&dirs, &root, "s-1", &config, Occasion::Resume)?,
                None,
                "a resume neither repeats the blob nor nags"
            );
        }
        // The cadence is untouched, so it still takes `nag_after`
        // turn-ends to earn the reminder.
        assert_eq!(
            compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?,
            None
        );
        let nag = compose(&dirs, &root, "s-1", &config, Occasion::TurnEnd)?
            .ok_or("no reminder after two turn-ends")?;
        assert!(nag.starts_with("FATHOMABLE reminder:"), "{nag}");
        Ok(())
    }

    #[test]
    fn subagents_are_told_apart_per_harness() {
        let claude = Input::parse(
            Harness::Claude,
            &json!({"session_id": "s", "cwd": "/w", "agent_id": "a", "stop_hook_active": true}),
        );
        assert!(claude.subagent && claude.continuation);
        assert_eq!(claude.session.as_deref(), Some("s"));
        assert_eq!(claude.source, None, "Stop carries no source");
        let resumed = Input::parse(
            Harness::Claude,
            &json!({"session_id": "s", "cwd": "/w", "source": "resume"}),
        );
        assert_eq!(resumed.source.as_deref(), Some("resume"));
        let main = Input::parse(Harness::Codex, &json!({"session_id": "s", "cwd": "/w"}));
        assert!(!main.subagent && !main.continuation);
        let copilot_main = Input::parse(
            Harness::Copilot,
            &json!({"sessionId": "p", "transcriptPath": "/x/session-state/p/events.jsonl"}),
        );
        assert!(!copilot_main.subagent);
        let copilot_sub = Input::parse(
            Harness::Copilot,
            &json!({"sessionId": "c", "transcriptPath": "/x/session-state/p/events.jsonl"}),
        );
        assert!(copilot_sub.subagent);
        let copilot_old = Input::parse(
            Harness::Copilot,
            &json!({"sessionId": "c", "transcriptPath": ""}),
        );
        assert!(copilot_old.subagent);
        let copilot_none = Input::parse(Harness::Copilot, &json!({"sessionId": "c"}));
        assert!(!copilot_none.subagent);
        assert_eq!(Input::parse(Harness::Vscode, &json!({})), Input::default());
    }
}
