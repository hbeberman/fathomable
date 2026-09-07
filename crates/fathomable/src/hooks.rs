// @okf-doc: /decisions/0042-turn-start-delivery.md
//! `fathomable hello` and `fathomable pending`: the harness-hook side of
//! agent subscriptions (ADR 0040, 0042).
//!
//! Both read the harness's hook JSON on stdin, resolve the workspace from
//! its `cwd`, and print in the shape the harness named by `--hook`
//! expects. Both exit 0 with no output whenever there is nothing to say:
//! no workspace here, no subscription for the session, a subagent, a
//! continuation the hook itself caused, or nothing pending. Neither ever
//! writes to the thread store; both append to the agent register —
//! `hello` the session bond of ADR 0041, `pending` its deliveries.
//!
//! `pending` runs at both ends of a turn and, optionally, after every
//! tool call. From the stop hook it blocks the stop with the blob as the
//! next prompt. From the prompt-submit hook — told apart by
//! `hook_event_name`, or by the `prompt` field no stop payload carries —
//! and from the post-tool-use hook it adds the blob to context and exits
//! 0, so a comment that lands while the agent waits is there on the wake
//! that ends the wait, and one that lands mid-task is there after the
//! next tool result (ADR 0042). Copilot's `notification` hook, fired
//! when a detached shell finishes, is answered the same way: its context
//! is queued as a message that starts a turn even on an idle agent. Its
//! other notifications (permission prompts) get silence: they fire
//! mid-turn and their context is queued until the turn ends, by which
//! time the threads are answered and the blob would only be stale.
//! `hello` delivers the same way when its
//! `source` is `resume`. Context is composed as [`Occasion::Context`]:
//! deliveries and fired watches are recorded, no check is counted, and
//! no reminder is composed, so the nag stays measured in turn-ends.
//!
//! `--verbose` makes a silent hook explain itself: every lookup on the
//! way — what stdin said, which workspace matched, the subscriber, its
//! bonds and watches, the thread counts — goes to stderr, or to stdout
//! when stderr is the answer (a Claude or Codex block), so the harness's
//! hook log becomes a diagnostic channel without changing what the
//! model sees.

use std::env;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::ValueEnum;
use fathomable_core::XdgDirs;
use fathomable_core::agents::{Blob, Register, Subscriber};
use fathomable_core::annotations::{Reach, Store, Thread};
use fathomable_core::bond;
use fathomable_core::config::{AgentsConfig, Config, UserConfig};
use fathomable_core::session::{Marker, Record};
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::Workspace;
use serde_json::{Value, json};

use fathomable_core::clock::now;

/// The agent harness whose hook is calling.
///
/// Claude Code and Codex are exercised end to end. Copilot's hook JSON
/// was captured from CLI 1.0.82, but the tool name its model sees is
/// unverified; VS Code is unverified entirely (the same hook file is
/// documented as read by both). Treat those two as experimental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Harness {
    /// Claude Code: `SessionStart` and `Stop` in `settings.json`.
    Claude,
    /// Codex CLI: `SessionStart` and `Stop` in `hooks.json`.
    Codex,
    /// Copilot CLI: `sessionStart` and `agentStop` in `.github/hooks`.
    /// Hooks observed; tool naming unverified.
    Copilot,
    /// VS Code agent mode: `SessionStart` and `Stop` in `.github/hooks`.
    /// Unverified against the real harness.
    Vscode,
}

impl Harness {
    /// The name the harness shows the model for a Fathomable tool.
    ///
    /// Claude Code and Codex prefix MCP tools with the server name in
    /// their own ways; Copilot and VS Code have not been verified, so
    /// the bare name is used there rather than a guess (ADR 0043).
    pub(crate) fn tool(self, tool: vocab::Tool) -> String {
        match self {
            Self::Claude => format!("mcp__fathomable__{}", tool.name),
            Self::Codex => format!("fathomable.{}", tool.name),
            Self::Copilot | Self::Vscode => tool.name.to_owned(),
        }
    }

    /// One line of the hello text that only this harness needs.
    pub(crate) const fn extra(self) -> Option<&'static str> {
        match self {
            Self::Copilot => {
                Some("A detached shell of yours finishing also brings any new comments with it.")
            }
            Self::Claude | Self::Codex | Self::Vscode => None,
        }
    }
}

/// The hello text: what Fathomable is, the session's facts as labelled
/// fields, and the calls to make, spelled as `harness` shows them.
pub(crate) fn hello_text(
    harness: Harness,
    root: &Path,
    id: &str,
    types: &[String],
    user: &str,
) -> String {
    let first = types.first().map_or("coder", String::as_str);
    let extra = harness
        .extra()
        .map_or_else(String::new, |line| format!(" {line}"));
    format!(
        "Fathomable is the user's read-only viewer on this workspace. They watch the files \
         you touch and leave review comments anchored to lines; you answer them in place.\n\
         It is already connected to you as the MCP server `fathomable` — everything below \
         is a tool call on it, not a shell command, a file to grep for, or something to \
         look up.\n\
         \n  workspace  {root}\
         \n  session    {id}\
         \n  user       {user}   ← who leaves the comments\
         \n  types      {types}   ← the whole list; do not look elsewhere\n\
         \nSubscribe before you edit; it covers the whole workspace:\
         \n  {follow} {{ {kind}: \"{first}\", {id_key}: \"{id}\" }}\n\
         \nComments then reach you as your turns start and end. Never poll; after a wait, \
         just end your turn.{extra}\
         \nAnswer one thread, or several in one call; `{resolve}: true` says you believe \
         the thread is done, and the user closes it:\
         \n  {reply} {{ {thread}: \"<thread id>\", {body}: \"…\" }}\
         \n  {reply} {{ {replies}: [ {{ {thread}, {body}, {resolve} }}, … ] }}\
         \nStart threads of your own on lines the user should look at; they wait on \
         the user:\
         \n  {start} {{ {comments}: [ {{ {path}, {line}, {end_line}, {body} }}, … ] }}\
         \nBe woken when a thread you are not following moves:\
         \n  {watch} {{ {on}: \"<thread id>\", {when}: \"{message}\" }}",
        root = root.display(),
        types = types.join(", "),
        follow = harness.tool(vocab::FOLLOW),
        kind = vocab::TYPE,
        id_key = vocab::ID,
        reply = harness.tool(vocab::THREAD_REPLY),
        thread = vocab::THREAD,
        body = vocab::BODY,
        resolve = vocab::RESOLVE,
        replies = vocab::REPLIES,
        start = harness.tool(vocab::THREAD_START),
        comments = vocab::COMMENTS,
        path = vocab::PATH,
        line = vocab::LINE,
        end_line = vocab::END_LINE,
        watch = harness.tool(vocab::THREAD_WATCH),
        on = vocab::ON,
        when = vocab::WHEN,
        message = vocab::WHEN_MESSAGE,
    )
}

/// The warning `hello` appends when `root` contains `cwd` only by prefix:
/// it is neither the cwd nor the cwd's git root, so nothing is registered
/// where the agent actually works and comments left there go unseen —
/// the shape of the 2026-08-29 incident behind ADR 0043.
pub(crate) fn prefix_warning(root: &Path, cwd: &Path, git_root: Option<&Path>) -> Option<String> {
    if root == cwd || git_root == Some(root) {
        return None;
    }
    Some(format!(
        "\n\nWARNING: matched by prefix — nothing is registered at your cwd ({}). Comments \
         left there will not reach you. Open Fathomable there, or run `fathomable --register`.",
        cwd.display()
    ))
}

/// Why a blob is being composed, which decides how hard it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Occasion {
    /// A `Stop` hook or the viewer's `Space a w`: the blob forces a turn,
    /// and a delivered-but-unanswered thread counts toward the nag.
    TurnEnd,
    /// A `SessionStart` resume or a prompt-submit hook: the blob is only
    /// added to context, so it does not count a check and never carries
    /// a reminder — the nag cadence of ADR 0040 is measured in turn-ends.
    Context,
}

/// Which hook event is calling, as far as `pending` cares.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// The stop hook, or anything unrecognised: the blob blocks the stop.
    #[default]
    Stop,
    /// The prompt-submit hook: the blob is context, and a non-zero exit
    /// would erase the prompt (ADR 0042).
    Prompt,
    /// The post-tool-use hook: the blob is context after the tool result.
    PostTool,
    /// Copilot's `notification` hook (a detached shell finished): the
    /// blob is queued as a message that starts a turn, idle or not.
    Notification,
    /// Any other Copilot notification — a permission prompt, mostly —
    /// fires mid-turn, and its context is queued until the turn ends,
    /// by which time the agent has answered the threads from the stop
    /// hook or its own reading and would answer them again. Silence.
    Other,
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
    event: Event,
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
        // Copilot names no event on `userPromptSubmitted`; every harness's
        // prompt-submit payload carries `prompt`, and no stop payload does.
        // Its `postToolUse` payload is known by `toolName`, and its
        // `notification` by `notificationType`, of which only a detached
        // shell finishing is worth answering.
        let notification = field("notificationType").or_else(|| field("notification_type"));
        let event = match (field("hook_event_name").as_deref(), notification.as_deref()) {
            (Some("UserPromptSubmit" | "userPromptSubmitted"), _) => Event::Prompt,
            (Some("PostToolUse" | "postToolUse"), _) => Event::PostTool,
            (_, Some("shell_detached_completed"))
            | (Some("Notification" | "notification"), None) => Event::Notification,
            (_, Some(_)) => Event::Other,
            (None, None) if value.get("prompt").is_some_and(Value::is_string) => Event::Prompt,
            (None, None) if value.get("toolName").is_some() => Event::PostTool,
            _ => Event::Stop,
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
            event,
        }
    }
}

/// The `--verbose` account of a hook run: each lookup and what it found.
#[derive(Debug, Default)]
struct Diag {
    on: bool,
    lines: Vec<String>,
    flushed: bool,
}

impl Diag {
    fn new(on: bool, command: &str, harness: Option<Harness>) -> Self {
        let mut diag = Self {
            on,
            lines: Vec::new(),
            flushed: false,
        };
        let harness =
            harness.map_or_else(|| "none".to_owned(), |h| format!("{h:?}").to_lowercase());
        diag.note(format!(
            "fathomable {} {command} --hook {harness}; pid {}",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));
        diag
    }

    fn note(&mut self, line: impl Into<String>) {
        if self.on {
            self.lines.push(line.into());
        }
    }

    /// Print to stderr, once; `to_stdout` when stderr carries the answer.
    fn flush(&mut self, to_stdout: bool) {
        if !self.on || self.flushed {
            return;
        }
        self.flushed = true;
        for line in &self.lines {
            if to_stdout {
                println!("fathomable: {line}");
            } else {
                eprintln!("fathomable: {line}");
            }
        }
    }
}

impl Drop for Diag {
    fn drop(&mut self) {
        self.flush(false);
    }
}

impl Input {
    fn describe(&self, diag: &mut Diag) {
        diag.note(format!(
            "input: event {:?}, session {}, cwd {}, source {}, subagent {}, continuation {}",
            self.event,
            self.session.as_deref().unwrap_or("none"),
            self.cwd
                .as_ref()
                .map_or_else(|| "none".to_owned(), |c| c.display().to_string()),
            self.source.as_deref().unwrap_or("none"),
            self.subagent,
            self.continuation
        ));
    }
}

/// Read the hook JSON from stdin when something is piped in.
fn read_input(harness: Harness, diag: &mut Diag) -> Input {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        diag.note("stdin: a terminal, no hook JSON");
        return Input::default();
    }
    let mut text = String::new();
    if stdin.lock().read_to_string(&mut text).is_err() || text.trim().is_empty() {
        diag.note("stdin: empty or unreadable");
        return Input::default();
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(value) => {
            let keys = value.as_object().map_or_else(
                || "not an object".to_owned(),
                |o| o.keys().cloned().collect::<Vec<_>>().join(", "),
            );
            diag.note(format!("stdin: {} bytes of JSON, keys: {keys}", text.len()));
            Input::parse(harness, &value)
        }
        Err(error) => {
            diag.note(format!("stdin: {} bytes, not JSON: {error}", text.len()));
            Input::default()
        }
    }
}

/// Seconds ago, for a diagnostic line.
fn ago(when: u64, now: u64) -> String {
    format!("{}s ago", now.saturating_sub(when))
}

/// Report the workspaces and viewers that could match `cwd`.
fn describe_workspaces(dirs: &XdgDirs, cwd: &Path, root: Option<&Path>, diag: &mut Diag) {
    if !diag.on {
        return;
    }
    let known = Marker::list(dirs);
    if let Some(root) = root {
        diag.note(format!(
            "workspace: {} (of {} known)",
            root.display(),
            known.len()
        ));
    } else {
        let roots: Vec<_> = known
            .iter()
            .flat_map(|m| m.roots().iter().map(|r| r.display().to_string()))
            .collect();
        diag.note(format!(
            "workspace: none of {} known contains {}: [{}]; run `fathomable --register` there",
            known.len(),
            cwd.display(),
            roots.join(", ")
        ));
    }
    let viewers: Vec<_> = Record::live(dirs)
        .into_iter()
        .filter(|r| root.is_none_or(|root| r.root() == root))
        .map(|r| {
            format!(
                "{}{} pid {} socket {}",
                r.id().as_str(),
                r.name().map_or_else(String::new, |n| format!(" ({n})")),
                r.pid(),
                r.socket()
                    .map_or_else(|| "none".to_owned(), |s| s.display().to_string())
            )
        })
        .collect();
    diag.note(format!(
        "viewers: {} live [{}]",
        viewers.len(),
        viewers.join("; ")
    ));
}

/// Report the config, register, and thread store as they concern `id`.
fn describe_state(dirs: &XdgDirs, bound: &Bound, id: &str, config: &AgentsConfig, diag: &mut Diag) {
    if !diag.on {
        return;
    }
    let root = bound.key.as_path();
    diag.note(format!(
        "config: types [{}], nag-after {}, expire-after {}s, max-lines {}, wake {}",
        config.types.join(", "),
        config.nag_after,
        config.expire_after.as_secs(),
        config.max_lines,
        config.wake.as_deref().unwrap_or("none")
    ));
    let when = now();
    let path = dirs.agents_file(root);
    let register = match Register::open(&path, when, config.expire_after) {
        Ok(register) => register,
        Err(error) => {
            diag.note(format!("register: {} unreadable: {error}", path.display()));
            return;
        }
    };
    let subscribers: Vec<_> = register
        .subscribers()
        .iter()
        .map(|s| {
            format!(
                "{} {} client {} seen {}",
                s.id(),
                s.label(),
                s.client().unwrap_or("?"),
                ago(s.seen(), when)
            )
        })
        .collect();
    diag.note(format!(
        "register: {} (exists {}), {} subscribers [{}]",
        path.display(),
        path.exists(),
        subscribers.len(),
        subscribers.join("; ")
    ));
    let bonds: Vec<_> = register
        .bonds()
        .iter()
        .map(|b| {
            let pids: Vec<_> = b.processes().iter().map(|p| p.pid().to_string()).collect();
            format!(
                "{} pids [{}] {}",
                b.id(),
                pids.join(", "),
                ago(b.created(), when)
            )
        })
        .collect();
    diag.note(format!(
        "bonds (MCP servers signing as a session): {} [{}]",
        bonds.len(),
        bonds.join("; ")
    ));
    let watches = register
        .watches()
        .iter()
        .filter(|w| w.subscriber() == id)
        .count();
    match register.subscriber(id) {
        None => diag.note(format!(
            "subscriber {id}: not subscribed here (no `follow` with this id)"
        )),
        Some(subscriber) => {
            diag.note(format!(
                "subscriber {id}: {}, {watches} watches",
                subscriber.label()
            ));
            match scoped_threads(dirs, bound) {
                Ok(threads) => diag.note(format!(
                    "threads: {} in scope, {} deliverable, {} stale for {id}",
                    threads.len(),
                    register.deliverable(subscriber, &threads).len(),
                    register.stale(subscriber, &threads).len()
                )),
                Err(error) => diag.note(format!("threads: {error}")),
            }
        }
    }
}

/// A known workspace at the worktree a hook's cwd is in (ADR 0070).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bound {
    /// The workspace key: what the store and register are keyed by.
    pub(crate) key: PathBuf,
    /// The worktree the cwd is in.
    pub(crate) root: PathBuf,
    /// Every worktree root the marker names.
    pub(crate) roots: Vec<PathBuf>,
}

impl Bound {
    /// A workspace that is its one worktree: a plain directory.
    #[cfg(test)]
    pub(crate) fn plain(root: PathBuf) -> Self {
        Self {
            key: root.clone(),
            roots: vec![root.clone()],
            root,
        }
    }
}

/// The known workspace at the worktree root that is the longest prefix
/// of `cwd`; failing that, the one whose common dir `cwd`'s repository
/// has, at the worktree `cwd` is in, so a worktree added after the
/// marker was written is found (ADR 0070).
fn workspace_for(dirs: &XdgDirs, cwd: &Path) -> Option<Bound> {
    let markers = Marker::list(dirs);
    let by_prefix = markers
        .iter()
        .flat_map(|m| m.roots().iter().map(move |root| (m, root)))
        .filter(|(_, root)| cwd.starts_with(root))
        .max_by_key(|(_, root)| root.as_os_str().len())
        .map(|(m, root)| Bound {
            key: m.key().to_path_buf(),
            root: root.clone(),
            roots: m.roots().to_vec(),
        });
    if by_prefix.is_some() {
        return by_prefix;
    }
    let workspace = Workspace::discover(cwd).ok()?;
    markers
        .iter()
        .find(|m| m.key() == workspace.key())
        .map(|m| Bound {
            key: m.key().to_path_buf(),
            root: workspace.root().to_path_buf(),
            roots: m.roots().to_vec(),
        })
}

/// The `agents` block and the user's name, or their defaults when the
/// config cannot be read.
fn agents_config(dirs: &XdgDirs) -> (AgentsConfig, String) {
    Config::load(dirs, None).map_or_else(
        |_| (AgentsConfig::default(), UserConfig::default().name),
        |c| (c.agents().clone(), c.user().name.clone()),
    )
}

/// `fathomable hello`: tell the model its session id and how to subscribe.
pub(crate) fn hello(
    dirs: &XdgDirs,
    harness: Harness,
    id: Option<String>,
    verbose: bool,
) -> ExitCode {
    let mut diag = Diag::new(verbose, "hello", Some(harness));
    let input = read_input(harness, &mut diag);
    input.describe(&mut diag);
    let Some(id) = id.or(input.session) else {
        diag.note("silent: no session id on stdin or --id");
        return ExitCode::SUCCESS;
    };
    if input.subagent {
        diag.note("silent: a subagent");
        return ExitCode::SUCCESS;
    }
    let cwd = input
        .cwd
        .or_else(|| env::current_dir().ok())
        .unwrap_or_default();
    let bound = workspace_for(dirs, &cwd);
    describe_workspaces(
        dirs,
        &cwd,
        bound.as_ref().map(|b| b.root.as_path()),
        &mut diag,
    );
    let Some(bound) = bound else {
        diag.note("silent: no known workspace");
        return ExitCode::SUCCESS;
    };
    let root = bound.root.clone();
    let (config, user) = agents_config(dirs);
    bond_session(dirs, &bound.key, &id, &config);
    describe_state(dirs, &bound, &id, &config, &mut diag);
    diag.note("answer: the hello text");
    let mut text = hello_text(harness, &root, &id, &config.types, &user);
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let git_root = Workspace::discover(&cwd)
        .ok()
        .map(|w| w.root().to_path_buf());
    if let Some(warning) = prefix_warning(&root, &canonical, git_root.as_deref()) {
        diag.note(format!(
            "prefix match: {} is neither the cwd nor its git root",
            root.display()
        ));
        text.push_str(&warning);
    }
    // A resume comes back with the connection's memory gone and may have
    // missed comments while the session was stopped, so it is handed them
    // here rather than waiting for a turn to end (ADR 0040 note). The
    // other sources are left alone: `startup` and `fork` have not
    // subscribed yet, and `compact` is mid-task, where consuming a
    // delivery — and firing a watch, which `Register::fire` removes —
    // into context the model may not act on would lose it, with the stop
    // hook then silent because it is recorded as delivered.
    if input.source.as_deref() == Some("resume")
        && let Ok(Some(blob)) = compose(dirs, &bound, &id, &config, &user, Occasion::Context)
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
fn bond_session(dirs: &XdgDirs, key: &Path, id: &str, config: &AgentsConfig) {
    let processes = bond::ancestors_within(bond::BOND_WINDOW);
    if processes.is_empty() {
        return;
    }
    let when = now();
    let outcome = Register::open(dirs.agents_file(key), when, config.expire_after)
        .and_then(|mut register| register.bond(id, processes, when));
    if let Err(error) = outcome {
        tracing::warn!(%error, "cannot record the session bond");
    }
}

/// `fathomable pending`: hand the subscriber what it has not seen.
pub(crate) fn pending(
    dirs: &XdgDirs,
    harness: Option<Harness>,
    id: Option<String>,
    prompt: bool,
    verbose: bool,
) -> ExitCode {
    let mut diag = Diag::new(verbose, "pending", harness);
    let input = harness.map_or_else(Input::default, |h| read_input(h, &mut diag));
    input.describe(&mut diag);
    let Some(id) = id.or(input.session) else {
        diag.note("silent: no session id on stdin or --id");
        return ExitCode::SUCCESS;
    };
    if input.continuation || input.subagent {
        diag.note("silent: a stop-hook continuation or a subagent");
        return ExitCode::SUCCESS;
    }
    if input.event == Event::Other {
        diag.note("silent: a notification that is not a detached shell finishing");
        return ExitCode::SUCCESS;
    }
    let cwd = input
        .cwd
        .or_else(|| env::current_dir().ok())
        .unwrap_or_default();
    let bound = workspace_for(dirs, &cwd);
    describe_workspaces(
        dirs,
        &cwd,
        bound.as_ref().map(|b| b.root.as_path()),
        &mut diag,
    );
    let Some(bound) = bound else {
        diag.note("silent: no known workspace");
        return ExitCode::SUCCESS;
    };
    let (config, user) = agents_config(dirs);
    describe_state(dirs, &bound, &id, &config, &mut diag);
    let occasion = match input.event {
        Event::Stop => Occasion::TurnEnd,
        Event::Prompt | Event::PostTool | Event::Notification | Event::Other => Occasion::Context,
    };
    diag.note(format!("occasion: {occasion:?}"));
    let text = match compose(dirs, &bound, &id, &config, &user, occasion) {
        Ok(Some(text)) => text,
        Ok(None) => {
            diag.note("silent: nothing to deliver");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("fathomable: {error}");
            diag.note("silent: the error above");
            return ExitCode::SUCCESS;
        }
    };
    diag.note(format!("answer: {} lines of threads", text.lines().count()));
    if prompt {
        println!("{text}");
        return ExitCode::SUCCESS;
    }
    if input.event != Event::Stop {
        // Context, not a block: exit 2 on a prompt-submit would erase the
        // prompt, and on a post-tool-use would read as a tool error.
        let event = match input.event {
            Event::Prompt => "UserPromptSubmit",
            Event::PostTool | Event::Stop => "PostToolUse",
            Event::Notification | Event::Other => "Notification",
        };
        match harness {
            None | Some(Harness::Claude) if input.event == Event::Prompt => println!("{text}"),
            Some(Harness::Copilot) => println!("{}", json!({ "additionalContext": text })),
            None | Some(Harness::Claude | Harness::Codex | Harness::Vscode) => println!(
                "{}",
                json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } })
            ),
        }
        return ExitCode::SUCCESS;
    }
    match harness {
        None | Some(Harness::Claude | Harness::Codex) => {
            // stderr is the block reason here, so the account goes to
            // stdout, which these harnesses keep in their hook log.
            diag.flush(true);
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

/// The threads of the workspace in the current git scope, the caller's
/// worktree first and every other worktree after it (ADR 0070), read
/// without re-anchoring: the hook shows, it does not move.
fn scoped_threads(dirs: &XdgDirs, bound: &Bound) -> Result<Vec<Thread>, String> {
    let store = Store::open(dirs.threads_file(&bound.key)).map_err(|e| e.to_string())?;
    let mut scope = Workspace::discover(&bound.root)
        .ok()
        .and_then(|w| w.reachable(store.commits()))
        .map_or_else(Reach::everything, Reach::reachable);
    for other in bound.roots.iter().filter(|r| **r != bound.root) {
        if let Some(reachable) = Workspace::discover(other)
            .ok()
            .and_then(|w| w.reachable(store.commits()))
        {
            scope = scope.with_worktree(other.clone(), reachable);
        }
    }
    Ok(store
        .threads()
        .iter()
        .filter(|t| scope.includes(t))
        .cloned()
        .collect())
}

/// The prompt for subscriber `id`, naming the user `user`, recording
/// what it contains as delivered; `None` when there is nothing to say.
pub(crate) fn compose(
    dirs: &XdgDirs,
    bound: &Bound,
    id: &str,
    config: &AgentsConfig,
    user: &str,
    occasion: Occasion,
) -> Result<Option<String>, String> {
    let when = now();
    let mut register = Register::open(dirs.agents_file(&bound.key), when, config.expire_after)
        .map_err(|e| e.to_string())?;
    let Some(subscriber) = register.subscriber(id).cloned() else {
        return Ok(None);
    };
    let threads = scoped_threads(dirs, bound)?;
    let mut blob = gather(&mut register, &subscriber, &threads, config, occasion, when)
        .map_err(|e| e.to_string())?;
    if blob.is_empty() {
        register.touch(id, when).map_err(|e| e.to_string())?;
        return Ok(None);
    }
    // Split the overflow off before rendering, so that only what the blob
    // actually shows is recorded as delivered and the rest still comes
    // back from `threads`, as the blob's own tail line says.
    blob.fit(&subscriber, config.max_lines);
    let text = blob.render(&subscriber, user);
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
mod tests;
