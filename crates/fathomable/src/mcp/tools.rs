// @okf-doc: /decisions/0055-six-tools.md
//! Six of the seven tools an agent calls (ADR 0055): `workspaces`,
//! `open`, `follow`, `threads`, `thread_reply`, and `thread_watch`; the
//! seventh, `thread_start`, is [`super::start`] (ADR 0061).
//!
//! Each pair of calls with a natural undo is one tool with a flag, a
//! subscription always covers the whole workspace, and one `threads`
//! tool lists open threads by default, flags and delivers the ones
//! waiting on the caller, and widens to resolved ones on request. What
//! an agent sees of a thread is [`Shown`]: the thread's placement in
//! the working tree and no anchor hashes. `thread_reply` answers with
//! the updated thread and refuses, before writing anything, a reply into
//! a resolved thread or a detached one given no line. Every failure
//! names the call that fixes it.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use fathomable_core::XdgDirs;
use fathomable_core::agents::{Register, Subscriber, WatchWhen};
use fathomable_core::annotations::{
    Author, LineRange, Placement, Reply, Status, Store, Thread, ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::identity;
use fathomable_core::session::{Request, Response};
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::{Filter, Workspace};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Server, Target, call, headless_store};
use crate::app::threads::open::follow_reply_lines;

/// `workspaces` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct WorkspacesParams {
    /// A workspace root as listed, or a viewer name or id (which selects
    /// that viewer's workspace): pin it as the default for later calls
    /// on this connection, then list.
    #[serde(default)]
    switch: Option<String>,
}

/// `open` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct OpenParams {
    /// Workspace-relative file path.
    path: PathBuf,
    /// First source line to show, 1-based.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of the range to bring on screen with `line`; nothing is
    /// selected. Pass it for the lines you mean, not the whole file.
    #[serde(default)]
    end_line: Option<usize>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
    /// Viewer name or id to show the file in; every viewer when omitted.
    #[serde(default)]
    viewer: Option<String>,
}

/// `follow` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct FollowParams {
    /// Your harness session id, as the `hello` hook told you. Optional
    /// when Fathomable can tell your session from the harness that
    /// started it; the reply says whether it could.
    #[serde(default)]
    id: Option<String>,
    /// Your agent type, one of the configured ones; fixed for the session.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Name to sign as, fixed for the session; without one you are named
    /// for your harness (Claude, Copilot, Codex).
    #[serde(default)]
    persona: Option<String>,
    /// End the subscription instead: its deliveries and watches are
    /// forgotten and the hooks fall silent for it.
    #[serde(default)]
    end: bool,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// `threads` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ThreadsParams {
    /// Which threads: `open` (the default), `pending` (open, and the
    /// user has the last word), `resolved`, or `all`.
    #[serde(default)]
    status: Option<String>,
    /// Only threads on this workspace-relative file, or under this
    /// directory.
    #[serde(default)]
    path: Option<PathBuf>,
    /// Only threads changed at or after this Unix time in seconds.
    #[serde(default)]
    since: Option<u64>,
    /// At most this many threads, oldest change first; default 50. The
    /// summary says how many more there are and the `since` to pass.
    #[serde(default)]
    limit: Option<usize>,
    /// Your session id, when this connection did not call `follow` and
    /// Fathomable cannot tell your session from the harness; with a
    /// subscription, the `pending` threads count as shown to you.
    #[serde(default)]
    id: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// One reply in a `thread_reply` batch.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReplyItem {
    /// Thread id from `threads`.
    thread: String,
    /// Reply text; Markdown.
    body: String,
    /// Propose resolving the thread: the reply is badged and the thread
    /// stays open, waiting for the user to close it.
    #[serde(default)]
    resolve: bool,
    /// First line the thread's lines are on now, when you rewrote them;
    /// the thread re-anchors there before the reply is added. A
    /// detached thread needs it.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line`.
    #[serde(default)]
    end_line: Option<usize>,
}

/// `thread_reply` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReplyParams {
    /// Thread id from `threads`, for a single reply.
    #[serde(default)]
    thread: Option<String>,
    /// Reply text for a single reply; Markdown.
    #[serde(default)]
    body: Option<String>,
    /// Propose resolving the thread (single reply): the reply is badged
    /// and the thread stays open, waiting for the user to close it.
    #[serde(default)]
    resolve: bool,
    /// First line the thread's lines are on now, when you rewrote them
    /// (single reply); the thread re-anchors there before the reply is
    /// added. A detached thread needs it.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line` (single reply).
    #[serde(default)]
    end_line: Option<usize>,
    /// Several replies in one call, instead of `thread` and `body`.
    /// Answer every pending thread this way in one turn.
    #[serde(default)]
    replies: Vec<ReplyItem>,
    /// Your session id, to sign the replies with your subscription when
    /// this connection did not call `follow` and Fathomable cannot tell
    /// your session from the harness that started it.
    #[serde(default)]
    id: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// `thread_watch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct WatchParams {
    /// The thread to watch, or whose watch to cancel.
    on: String,
    /// `message` (someone else posts on it) or `resolved`; not needed
    /// with `cancel`.
    #[serde(default)]
    when: Option<String>,
    /// Thread ids to be reminded of, in full, when the watch fires.
    #[serde(default)]
    remind: Vec<String>,
    /// Cancel the watch on `on` instead of setting one.
    #[serde(default)]
    cancel: bool,
    /// The session id you subscribed with; defaults to this connection's.
    #[serde(default)]
    id: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// `threads` per call unless `limit` says otherwise.
const DEFAULT_LIMIT: usize = 50;

/// Which threads `threads` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Open,
    /// Open, and the user has the last word (ADR 0058).
    Pending,
    Resolved,
    All,
}

impl Which {
    fn parse(text: Option<&str>) -> Result<Self, String> {
        match text {
            None => Ok(Self::Open),
            Some(word) if word == vocab::STATUS_OPEN => Ok(Self::Open),
            Some(word) if word == vocab::PENDING => Ok(Self::Pending),
            Some(word) if word == vocab::WHEN_RESOLVED => Ok(Self::Resolved),
            Some(word) if word == vocab::STATUS_ALL => Ok(Self::All),
            Some(other) => Err(format!(
                "`{}` is `{}`, `{}`, `{}`, or `{}`, not `{other}`",
                vocab::STATUS,
                vocab::STATUS_OPEN,
                vocab::PENDING,
                vocab::WHEN_RESOLVED,
                vocab::STATUS_ALL
            )),
        }
    }

    fn admits(self, thread: &Thread) -> bool {
        match self {
            Self::Open => thread.status() == Status::Open,
            Self::Pending => thread.awaits_agent(),
            Self::Resolved => thread.status() == Status::Resolved,
            Self::All => true,
        }
    }
}

/// The agent that has the last word on a thread (ADR 0058).
#[derive(Debug, Serialize)]
struct Answered<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<&'a str>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
    /// Whether that reply proposes resolving the thread.
    proposed: bool,
}

impl<'a> Answered<'a> {
    /// The agent with the last word on `thread`, if one has it.
    fn of(thread: &'a Thread) -> Option<Self> {
        if thread.status() != Status::Open {
            return None;
        }
        match thread.last_act().0 {
            Author::User => None,
            Author::Agent {
                name,
                client,
                id,
                kind,
            } => Some(Self {
                name,
                client: client.as_deref(),
                kind: kind.as_deref(),
                id: id.as_deref(),
                proposed: thread.proposes_resolution(),
            }),
        }
    }

    /// `name (type)`, as the viewer labels the agent.
    fn label(&self) -> String {
        match self.kind {
            Some(kind) => format!("{} ({kind})", self.name),
            None => self.name.to_owned(),
        }
    }
}

/// A thread as an agent sees it (ADR 0055): its placement in the
/// working tree and no anchor hashes. An open thread carries its
/// snippet and replies, and says whose word is last: `pending` when it
/// is the user's, `answered` naming the agent otherwise (ADR 0058). A
/// resolved one is only its head.
#[derive(Debug, Serialize)]
pub(super) struct Shown<'a> {
    id: &'a ThreadId,
    path: &'a Path,
    range: LineRange,
    placement: &'static str,
    status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<u64>,
    updated: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    messages: Option<usize>,
    comment: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replies: Option<&'a [Reply]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edited: Option<u64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    answered: Option<Answered<'a>>,
}

impl<'a> Shown<'a> {
    pub(super) fn new(thread: &'a Thread, placement: Placement) -> Self {
        let open = thread.status() == Status::Open;
        Self {
            id: thread.id(),
            path: thread.path(),
            range: placement.range(),
            placement: placement_word(placement),
            status: thread.status(),
            created: open.then(|| thread.created()),
            updated: thread.updated(),
            messages: (!open).then(|| thread.replies().len() + 1),
            comment: if open {
                thread.comment()
            } else {
                thread.comment().lines().next().unwrap_or_default()
            },
            snippet: open.then(|| thread.snippet()),
            replies: open.then(|| thread.replies()),
            commit: thread.commit(),
            edited: thread.edited(),
            pending: thread.awaits_agent(),
            answered: Answered::of(thread),
        }
    }

    /// The one summary line: `id  path:range  status`, then `pending`,
    /// `answered by NAME (type)`, or `proposed by NAME (type)`, then
    /// `edited` or `detached` when they apply, then the comment's first
    /// line.
    fn line(&self) -> String {
        let mut line = format!(
            "{}  {}:{}  {}",
            self.id,
            self.path.display(),
            self.range,
            status_word(self.status)
        );
        if self.pending {
            line.push(' ');
            line.push_str(vocab::PENDING);
        } else if let Some(answered) = &self.answered {
            let verb = if answered.proposed {
                "proposed"
            } else {
                "answered"
            };
            line.push(' ');
            line.push_str(verb);
            line.push_str(" by ");
            line.push_str(&answered.label());
        }
        if self.placement != "anchored" {
            line.push(' ');
            line.push_str(self.placement);
        }
        line.push_str("  ");
        line.push_str(self.comment.lines().next().unwrap_or_default());
        line
    }
}

const fn placement_word(placement: Placement) -> &'static str {
    match placement {
        Placement::Anchored(_) => "anchored",
        Placement::Edited(_) => "edited",
        Placement::Detached(_) => "detached",
    }
}

const fn status_word(status: Status) -> &'static str {
    match status {
        Status::Open => vocab::STATUS_OPEN,
        Status::Resolved => vocab::WHEN_RESOLVED,
    }
}

/// The files of one workspace as they are now, read once each, so that
/// every thread's placement is computed against the same text.
pub(super) struct Tree<'a> {
    root: &'a Path,
    texts: HashMap<PathBuf, Option<String>>,
}

impl<'a> Tree<'a> {
    pub(super) fn new(root: &'a Path) -> Self {
        Self {
            root,
            texts: HashMap::new(),
        }
    }

    /// Where `thread` sits in its file now; detached at its last known
    /// range when the file cannot be read.
    pub(super) fn place(&mut self, thread: &Thread) -> Placement {
        let text = self
            .texts
            .entry(thread.path().to_path_buf())
            .or_insert_with(|| fs::read_to_string(self.root.join(thread.path())).ok());
        match text {
            Some(text) => thread.locate(text),
            None => Placement::Detached(thread.range()),
        }
    }
}

#[tool_router(vis = "pub(super)")]
impl Server {
    #[tool(
        description = "List the workspaces Fathomable knows and the viewers running in each, \
                       the one later calls address marked; `switch` (a root, or a viewer name \
                       or id) pins one for this connection first. Call it when a call says no \
                       workspace contains the current directory. Returns roots and viewer \
                       names, not threads.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn workspaces(&self, Parameters(p): Parameters<WorkspacesParams>) -> CallToolResult {
        let mut lines = Vec::new();
        if let Some(key) = p.switch.as_deref() {
            match self.resolve(Some(key)) {
                Ok(target) => {
                    if let Ok(mut pinned) = self.pinned.lock() {
                        *pinned = Some(target.root.clone());
                    }
                    lines.push(format!("pinned {}", target.root.display()));
                }
                Err(error) => return failure(error),
            }
        }
        let all = super::targets(&self.dirs);
        let default = self.resolve(None).ok().map(|s| s.root);
        let workspaces: Vec<Value> = all
            .iter()
            .map(|s| {
                let viewers: Vec<Value> = s
                    .viewers
                    .iter()
                    .map(|v| {
                        json!({
                            "id": v.id().as_str(),
                            "name": v.name(),
                            "pid": v.pid(),
                            "started": v.started(),
                        })
                    })
                    .collect();
                json!({
                    "root": s.root,
                    "default": default.as_deref() == Some(s.root.as_path()),
                    "viewers": viewers,
                })
            })
            .collect();
        if all.is_empty() {
            lines.push("no known workspaces; start Fathomable in one".to_owned());
        }
        for s in &all {
            let mark = if default.as_deref() == Some(s.root.as_path()) {
                "*"
            } else {
                " "
            };
            lines.push(format!("{mark} {}", s.root.display()));
            if s.viewers.is_empty() {
                lines.push("    (no viewers running)".to_owned());
            }
            for v in &s.viewers {
                lines.push(format!("    {}  {}", v.name().unwrap_or("-"), v.id()));
            }
        }
        with_summary(json!({ "workspaces": workspaces }), lines.join("\n"))
    }

    #[tool(
        description = "Show a workspace file in the user's viewer(s), optionally scrolled to a \
                       line or line range. The range is brought on screen, not selected; give \
                       one only for the lines you are pointing at, not the whole file. Fails \
                       when no viewer is running; it never reads or returns file content.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn open(&self, Parameters(p): Parameters<OpenParams>) -> CallToolResult {
        let request = Request::Open {
            path: p.path.clone(),
            line: p.line,
            end_line: p.end_line,
        };
        match self
            .broadcast(p.workspace.as_deref(), p.viewer.as_deref(), &request)
            .await
        {
            Ok(count) => text(format!("opened {} in {count} viewer(s)", p.path.display())),
            Err(error) => failure(error),
        }
    }

    #[tool(
        description = "Subscribe this session to the whole workspace with your `type` (and \
                       the session `id` the hello hook gave you, when asked for it): the \
                       hooks then hand you every comment the user has the last word on, \
                       once, as your turns start and end. You sign as your harness's name \
                       unless you give a `persona` here, once. `end: true` ends the \
                       subscription, its deliveries, and its watches instead. Works with no \
                       viewer running.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the tool macro hands the request context over by value"
    )]
    fn follow(
        &self,
        Parameters(p): Parameters<FollowParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        if p.end {
            return match self.end_subscription(&target.root, p.id) {
                Ok(message) => text(message),
                Err(error) => failure(error),
            };
        }
        let client = context.client_info().map(|c| c.name);
        match self.subscription(
            &target.root,
            p.id,
            p.kind,
            p.persona.as_deref(),
            client.as_deref(),
        ) {
            Ok(message) => text(message),
            Err(error) => failure(error),
        }
    }

    #[tool(
        description = "List the threads in the workspace, oldest change first: `status` is \
                       `open` (the default), `pending` (open, and the user has the last \
                       word), `resolved`, or `all`; `path` a file or a directory; `since` \
                       and `limit` page. Each comes with its placement in the working tree \
                       and whose word is last: `pending` needs an answer, `answered by` or \
                       `proposed by` names the agent that gave one and awaits the user. \
                       When you are subscribed, the pending ones count as shown to you. Do \
                       not poll it for comments: the hooks deliver them; call it when a hook \
                       lists more than it showed, when no hook is installed, or to read the \
                       board. Works with no viewer running.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn threads(&self, Parameters(p): Parameters<ThreadsParams>) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let which = match Which::parse(p.status.as_deref()) {
            Ok(which) => which,
            Err(error) => return failure(error),
        };
        let path = match check_path(&target.root, p.path.as_deref()) {
            Ok(path) => path,
            Err(error) => return failure(error),
        };
        let all = match self.fetch(&target).await {
            Ok(all) => all,
            Err(error) => return failure(error),
        };
        let when = now();
        let mut register = match self.register(&target.root, when) {
            Ok(register) => register,
            Err(error) => return failure(error),
        };
        let subscriber: Option<Subscriber> = self
            .session_id(p.id, &register)
            .and_then(|id| register.subscriber(&id).cloned());

        // A watch that fired outranks every filter: it is spent now, and
        // what it reminds of need not be pending, so neither could be
        // fetched again (ADR 0040).
        let mut lines = Vec::new();
        let mut fired = Vec::new();
        let mut first: Vec<&Thread> = Vec::new();
        if let Some(subscriber) = &subscriber {
            let watches = match register.fire(subscriber.id(), &all, when) {
                Ok(watches) => watches,
                Err(error) => return failure(error.to_string()),
            };
            for f in watches {
                let remind: Vec<&Thread> = f
                    .watch
                    .remind()
                    .iter()
                    .filter_map(|id| all.iter().find(|t| t.id() == id))
                    .collect();
                lines.push(format!(
                    "watch fired: {} {}{}",
                    f.thread.id(),
                    f.watch.when(),
                    if remind.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "; reminding {}",
                            remind
                                .iter()
                                .map(|t| t.id().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                ));
                fired.push(json!({
                    "on": f.thread.id(),
                    "when": f.watch.when(),
                    "remind": f.watch.remind(),
                }));
                for thread in std::iter::once(f.thread).chain(remind) {
                    if !first.iter().any(|t| t.id() == thread.id()) {
                        first.push(thread);
                    }
                }
            }
        }

        let mut selected: Vec<&Thread> = all
            .iter()
            .filter(|t| which.admits(t))
            .filter(|t| p.since.is_none_or(|s| t.updated() >= s))
            .filter(|t| path.as_deref().is_none_or(|p| t.path().starts_with(p)))
            .filter(|t| !first.iter().any(|f| f.id() == t.id()))
            .collect();
        selected.sort_by_key(|t| t.updated());
        let limit = p.limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let rest = selected.len().saturating_sub(limit);
        selected.truncate(limit);
        let oldest_left = selected.last().map_or(0, |t| t.updated());
        first.extend(selected);
        let threads = first;

        // Returning a pending thread to a subscriber is its delivery: the
        // hooks will not hand it over again (ADR 0055).
        if let Some(subscriber) = &subscriber {
            for thread in register.deliverable(subscriber, threads.iter().copied()) {
                if let Err(error) = register.deliver(subscriber.id(), thread, when) {
                    return failure(error.to_string());
                }
            }
        }

        let mut tree = Tree::new(&target.root);
        let shown: Vec<Shown<'_>> = threads
            .iter()
            .map(|t| Shown::new(t, tree.place(t)))
            .collect();
        if shown.is_empty() {
            lines.push("no threads".to_owned());
        }
        lines.extend(shown.iter().map(Shown::line));
        if rest > 0 {
            lines.push(format!(
                "{rest} more; pass {}={oldest_left} (threads changed at that second may repeat)",
                vocab::SINCE
            ));
        }
        let mut value = json!({ "threads": shown, "more": rest });
        if !fired.is_empty() {
            value["fired"] = Value::Array(fired);
        }
        with_summary(value, lines.join("\n"))
    }

    #[tool(
        description = "Reply to one thread (`thread`, `body`) or to several at once \
                       (`replies`), and get each back as it now stands, with its placement. \
                       `resolve: true` says you believe the thread is done; it stays open and \
                       the user closes it. If you rewrote the lines a thread is on, pass \
                       `line` and `end_line` so it follows them; a detached thread needs \
                       them. Nothing is written when any item is refused. Works with no \
                       viewer running.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    async fn thread_reply(
        &self,
        Parameters(p): Parameters<ReplyParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let client = context.client_info().map(|c| c.name);
        // The name was fixed at `follow`; without one, the harness names
        // the agent (ADR 0058).
        let signed = self.signer(p.id, &target.root, client);
        let author = signed.author;
        let mut items = p.replies;
        match (p.thread, p.body) {
            (Some(thread), Some(body)) => items.insert(
                0,
                ReplyItem {
                    thread,
                    body,
                    resolve: p.resolve,
                    line: p.line,
                    end_line: p.end_line,
                },
            ),
            (None, None) if !items.is_empty() => {}
            _ => {
                return failure(format!(
                    "pass `{}` and `{}`, or a non-empty `{}` list",
                    vocab::THREAD,
                    vocab::BODY,
                    vocab::REPLIES
                ));
            }
        }

        // Check the whole batch before writing any of it, so that a retry
        // with the fixed list is a whole retry.
        let all = match self.fetch(&target).await {
            Ok(all) => all,
            Err(error) => return failure(error),
        };
        let mut tree = Tree::new(&target.root);
        let problems: Vec<String> = items
            .iter()
            .filter_map(|item| refusal(item, &all, &mut tree))
            .collect();
        if !problems.is_empty() {
            return failure(problems.join("\n"));
        }

        let mut lines = Vec::new();
        let mut answered = Vec::new();
        for item in items {
            match self.reply_one(&target, author.clone(), item).await {
                Ok(thread) => answered.push(thread),
                Err(error) => {
                    lines.extend(shown_lines(&answered, &mut tree, "replied to"));
                    lines.push(error);
                    return failure(lines.join("\n"));
                }
            }
        }
        lines.extend(shown_lines(&answered, &mut tree, "replied to"));
        if !signed.subscribed {
            lines.push(format!(
                "signed as {author} with no subscription; call `{}` with `{}` to be told \
                 about answers",
                vocab::FOLLOW.name,
                vocab::TYPE
            ));
        }
        let shown: Vec<Shown<'_>> = answered
            .iter()
            .map(|t| Shown::new(t, tree.place(t)))
            .collect();
        with_summary(json!({ "threads": shown }), lines.join("\n"))
    }

    #[tool(
        description = "Ask to be woken when thread `on` next gets a `message` from someone \
                       else or is `resolved`: the stop hook or `threads` then says so and \
                       hands you the `remind` threads again in full, and the watch is spent. \
                       `cancel: true` removes the watch instead. Use it to park a thread that \
                       depends on a discussion elsewhere. Needs a subscription; fails when \
                       `on` or a `remind` thread does not exist.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn thread_watch(&self, Parameters(p): Parameters<WatchParams>) -> CallToolResult {
        let on = match thread_id(&p.on) {
            Ok(id) => id,
            Err(error) => return failure(error),
        };
        if p.cancel {
            return self.with_subscriber(p.workspace.as_deref(), p.id, |register, id, now| {
                register
                    .unwatch(id, &on, now)
                    .map(|()| format!("no longer watching {}", p.on))
            });
        }
        let when = match p.when.as_deref() {
            Some(word) if word == vocab::WHEN_MESSAGE => WatchWhen::Message,
            Some(word) if word == vocab::WHEN_RESOLVED => WatchWhen::Resolved,
            Some(other) => {
                return failure(format!(
                    "`{}` is `{}` or `{}`, not `{other}`",
                    vocab::WHEN,
                    vocab::WHEN_MESSAGE,
                    vocab::WHEN_RESOLVED
                ));
            }
            None => {
                return failure(format!(
                    "pass `{}` (`{}` or `{}`), or `{}: true` to remove the watch",
                    vocab::WHEN,
                    vocab::WHEN_MESSAGE,
                    vocab::WHEN_RESOLVED,
                    vocab::CANCEL
                ));
            }
        };
        let remind = match p
            .remind
            .iter()
            .map(|s| thread_id(s))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(ids) => ids,
            Err(error) => return failure(error),
        };
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        if let Err(error) = known_threads(
            &self.dirs,
            &target.root,
            std::iter::once(&on).chain(&remind),
        ) {
            return failure(error);
        }
        self.with_subscriber(p.workspace.as_deref(), p.id, |register, id, now| {
            register
                .watch(id, &on, when, remind.clone(), now)
                .map(|()| format!("watching {} for {when}", p.on))
        })
    }
}

impl Server {
    /// The subscription side of `follow`: subscribe `id` as `kind`,
    /// refuse a half-given pair, or, with neither, say whether this
    /// connection already speaks for a subscribed session. Returns the
    /// reply.
    fn subscription(
        &self,
        root: &Path,
        id: Option<String>,
        kind: Option<String>,
        persona: Option<&str>,
        client: Option<&str>,
    ) -> Result<String, String> {
        let when = now();
        let types = self.agents.types.join(", ");
        // The name is fixed here, once (ADR 0058): the persona, else the
        // harness's name, else the client string.
        let given = persona.is_some();
        let name = identity::agent_name(persona, client);
        match (id, kind) {
            (Some(id), Some(kind)) => {
                let mut register = self.register(root, when)?;
                self.subscribe(&mut register, &id, &kind, &name, client, when)?;
                Ok(format!(
                    "subscribed {id} as {kind}, signing as {name}; {COVERAGE}"
                ))
            }
            (None, Some(kind)) => {
                let mut register = self.register(root, when)?;
                let Some(id) = register.session_for(&self.ancestors).map(str::to_owned) else {
                    return Err(format!(
                        "`{}` needs the session `{}` the hello hook gave you; this session \
                         could not be told from the harness",
                        vocab::TYPE,
                        vocab::ID
                    ));
                };
                self.subscribe(&mut register, &id, &kind, &name, client, when)?;
                Ok(format!(
                    "subscribed {id} (your session, found from the harness) as {kind}, \
                     signing as {name}; {COVERAGE}"
                ))
            }
            (Some(_), None) => Err(format!(
                "`{}` needs a `{}`; configured types: {types}",
                vocab::ID,
                vocab::TYPE
            )),
            (None, None) => {
                let nudge = || {
                    format!(
                        "not subscribed: pass `{}` (one of {types}), and the `{}` the hello \
                         hook gave you if asked for it",
                        vocab::TYPE,
                        vocab::ID
                    )
                };
                let mut register = self.register(root, when)?;
                let Some(id) = self.session_id(None, &register) else {
                    return Err(nudge());
                };
                let Some(existing) = register.subscriber(&id) else {
                    return Err(nudge());
                };
                let kind = existing.kind().to_owned();
                let name = if given {
                    name
                } else {
                    existing.name().map_or(name, str::to_owned)
                };
                self.subscribe(&mut register, &id, &kind, &name, client, when)?;
                Ok(format!(
                    "still subscribed {id} as {kind}, signing as {name}; {COVERAGE}"
                ))
            }
        }
    }

    /// Subscribe `id` as `kind` in `register`, signing as `name`, and
    /// remember it as this connection's signature.
    fn subscribe(
        &self,
        register: &mut Register,
        id: &str,
        kind: &str,
        name: &str,
        client: Option<&str>,
        when: u64,
    ) -> Result<(), String> {
        if !self.agents.allows(kind) {
            return Err(format!(
                "unknown agent type `{kind}`; configured types: {}",
                self.agents.types.join(", ")
            ));
        }
        register
            .subscribe(id, kind, Some(name), client, when)
            .map_err(|e| e.to_string())?;
        if let Ok(mut current) = self.subscriber.lock() {
            *current = Some((id.to_owned(), kind.to_owned(), Some(name.to_owned())));
        }
        Ok(())
    }

    /// `follow` with `end`: forget the session's subscription, its
    /// deliveries, and its watches.
    fn end_subscription(&self, root: &Path, id: Option<String>) -> Result<String, String> {
        let when = now();
        let mut register = self.register(root, when)?;
        let Some(id) = self.session_id(id, &register) else {
            return Err(format!(
                "nothing to end: pass `{}`, or call `{}` with `{}` first",
                vocab::ID,
                vocab::FOLLOW.name,
                vocab::TYPE
            ));
        };
        if register.subscriber(&id).is_none() {
            return Ok(format!("{id} was not subscribed"));
        }
        register.unsubscribe(&id, when).map_err(|e| e.to_string())?;
        if let Ok(mut current) = self.subscriber.lock()
            && current.as_ref().is_some_and(|(own, ..)| *own == id)
        {
            *current = None;
        }
        Ok(format!("unsubscribed {id}"))
    }

    /// Run `act` on the register for the subscriber `id` (or this
    /// connection's), answering with its message.
    fn with_subscriber(
        &self,
        workspace: Option<&str>,
        id: Option<String>,
        act: impl FnOnce(
            &mut Register,
            &str,
            u64,
        ) -> Result<String, fathomable_core::agents::RegisterError>,
    ) -> CallToolResult {
        let target = match self.resolve(workspace) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let when = now();
        let mut register = match self.register(&target.root, when) {
            Ok(register) => register,
            Err(error) => return failure(error),
        };
        let Some(id) = self.session_id(id, &register) else {
            return failure(format!(
                "no subscription: call `{}` with `{}` first, or pass `{}`",
                vocab::FOLLOW.name,
                vocab::TYPE,
                vocab::ID
            ));
        };
        match act(&mut register, &id, when) {
            Ok(message) => text(message),
            Err(error) => failure(error.to_string()),
        }
    }

    /// Every thread the checkout shows, through a viewer or the store.
    async fn fetch(&self, target: &Target) -> Result<Vec<Thread>, String> {
        let request = Request::ThreadsList {
            since: None,
            path: None,
        };
        match target.viewers.first() {
            Some(viewer) => match call(viewer, &request).await? {
                Response::Threads(threads) => Ok(threads),
                Response::Error(message) => Err(message),
                Response::Done => Err("unexpected reply Done".to_owned()),
            },
            None => headless_list(&self.dirs, &target.root),
        }
    }

    /// One reply of a `thread_reply` call, through a viewer or the store;
    /// answers with the thread as it then stands.
    async fn reply_one(
        &self,
        target: &Target,
        author: Author,
        item: ReplyItem,
    ) -> Result<Thread, String> {
        let thread = thread_id(&item.thread)?;
        let lines = item
            .line
            .map(|line| LineRange::new(line, item.end_line.unwrap_or(line)));
        let request = Request::ThreadReply {
            thread: thread.clone(),
            author: author.clone(),
            body: item.body.clone(),
            resolve: item.resolve,
            lines,
        };
        let outcome = match target.viewers.first() {
            Some(viewer) => call(viewer, &request).await,
            None => headless_reply(
                &self.dirs,
                &target.root,
                &thread,
                author,
                item.body,
                item.resolve,
                lines,
            )
            .map(|thread| Response::Threads(vec![thread])),
        };
        match outcome {
            Ok(Response::Threads(mut threads)) if threads.len() == 1 => Ok(threads.remove(0)),
            Ok(Response::Error(message)) | Err(message) => {
                Err(format!("{}: {message}", item.thread))
            }
            Ok(other) => Err(format!("{}: unexpected reply {other:?}", item.thread)),
        }
    }
}

/// What subscribing means, appended to every `follow` reply.
const COVERAGE: &str = "comments anywhere in the workspace reach you as your turns start and end";

/// Why `item` cannot be replied to, if it cannot: the thread is unknown,
/// resolved, or detached with no line to place it at.
fn refusal(item: &ReplyItem, all: &[Thread], tree: &mut Tree<'_>) -> Option<String> {
    let Some(thread) = all.iter().find(|t| t.id().to_string() == item.thread) else {
        return Some(format!(
            "no thread {}; call `{}` to see the ids",
            item.thread,
            vocab::THREADS.name
        ));
    };
    if thread.status() != Status::Open {
        return Some(format!(
            "{} is resolved; the user reopens it; call `{}` with `{}: {}` to read it",
            item.thread,
            vocab::THREADS.name,
            vocab::STATUS,
            vocab::STATUS_ALL
        ));
    }
    if item.line.is_none()
        && let Placement::Detached(range) = tree.place(thread)
    {
        return Some(format!(
            "{} is detached: its lines are gone from {}, last seen at {range}; pass `{}` \
             and `{}` to place it",
            item.thread,
            thread.path().display(),
            vocab::LINE,
            vocab::END_LINE
        ));
    }
    None
}

/// The summary line of each thread in `threads`, prefixed with `verb`.
pub(super) fn shown_lines(threads: &[Thread], tree: &mut Tree<'_>, verb: &str) -> Vec<String> {
    threads
        .iter()
        .map(|thread| {
            let placement = tree.place(thread);
            format!(
                "{verb} {} at {}:{} ({}){}",
                thread.id(),
                thread.path().display(),
                placement.range(),
                placement_word(placement),
                if thread.proposes_resolution() {
                    ", proposing to resolve it"
                } else {
                    ""
                }
            )
        })
        .collect()
}

fn thread_id(text: &str) -> Result<ThreadId, String> {
    serde_json::from_value(Value::String(text.to_owned())).map_err(|e| e.to_string())
}

/// Check that `path`, when given, names a file or directory of the
/// workspace at `root`, and answer with it cleaned: `.` segments dropped,
/// and the root itself as no filter at all.
///
/// A path that exists nowhere is matched by its last component against
/// the workspace, so a wrong directory is answered with the right one.
pub(super) fn check_path(root: &Path, path: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let inside = path.is_relative()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    let shown = path.display();
    if !inside {
        return Err(format!("{shown} is not a workspace-relative path"));
    }
    let path: PathBuf = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    if path.as_os_str().is_empty() {
        return Ok(None);
    }
    let full = root.join(&path);
    if full.is_file() || full.is_dir() {
        return Ok(Some(path));
    }
    let same: Vec<String> = workspace_paths(root)
        .iter()
        .filter(|k| k.file_name() == path.file_name())
        .map(|k| k.display().to_string())
        .collect();
    Err(if same.is_empty() {
        format!("{shown} is nothing in the workspace")
    } else {
        format!(
            "{shown} is nothing in the workspace; did you mean {}?",
            same.join(", ")
        )
    })
}

/// Every visible file under `root` and the directories holding them,
/// relative to `root`; empty when the workspace cannot be read.
fn workspace_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(mut workspace) = Workspace::discover(root) else {
        return Vec::new();
    };
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let below = canonical
        .strip_prefix(workspace.root())
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut paths = Vec::new();
    for file in workspace.walk_files(Filter::Visible) {
        let Ok(file) = PathBuf::from(file)
            .strip_prefix(&below)
            .map(Path::to_path_buf)
        else {
            continue;
        };
        for dir in file.ancestors().skip(1) {
            if dir.as_os_str().is_empty() || paths.contains(&dir.to_path_buf()) {
                break;
            }
            paths.push(dir.to_path_buf());
        }
        paths.push(file);
    }
    paths
}

/// Check that every thread id names a thread in the store.
fn known_threads<'a>(
    dirs: &XdgDirs,
    root: &Path,
    ids: impl IntoIterator<Item = &'a ThreadId>,
) -> Result<(), String> {
    let store = Store::open(dirs.threads_file(root)).map_err(|e| e.to_string())?;
    let unknown: Vec<String> = ids
        .into_iter()
        .filter(|id| store.thread(id).is_none())
        .map(ToString::to_string)
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "no thread(s) {}; call `{}` to see the ids",
            unknown.join(", "),
            vocab::THREADS.name
        ))
    }
}

/// Every thread the checkout shows, from the store.
fn headless_list(dirs: &XdgDirs, root: &Path) -> Result<Vec<Thread>, String> {
    let (store, scope) = headless_store(dirs, root)?;
    Ok(store
        .threads()
        .iter()
        .filter(|t| scope.includes(t))
        .cloned()
        .collect())
}

/// Reply to `thread` in the store and answer with it as it then stands.
fn headless_reply(
    dirs: &XdgDirs,
    root: &Path,
    thread: &ThreadId,
    author: Author,
    body: String,
    resolve: bool,
    lines: Option<LineRange>,
) -> Result<Thread, String> {
    let mut store = Store::open(dirs.threads_file(root)).map_err(|e| e.to_string())?;
    if store.thread(thread).is_none() {
        return Err(format!(
            "no thread {thread}; call `{}` to see the ids",
            vocab::THREADS.name
        ));
    }
    let when = now();
    if let Some(lines) = lines {
        follow_reply_lines(&mut store, root, thread, lines, when)?;
    }
    let reply = Reply::new(author, when, body);
    let reply = if resolve {
        reply.proposing_resolution()
    } else {
        reply
    };
    store.reply(thread, reply).map_err(|e| e.to_string())?;
    tracing::info!(%thread, proposes = resolve, "agent reply added headlessly");
    store
        .thread(thread)
        .cloned()
        .ok_or_else(|| format!("thread {thread} vanished after the reply"))
}

fn text(summary: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(summary)])
}

pub(super) fn with_summary(value: Value, summary: String) -> CallToolResult {
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(summary)];
    result
}

pub(super) fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// The server instructions every client is handed on connect.
pub(super) fn instructions() -> String {
    format!(
        "Fathomable is the user's read-only viewer, where they leave review comments \
         on the lines you write. Call `{follow}` with your `{kind}` (and the session `{id}` \
         the hello hook gave you, when asked for it) to subscribe to the workspace; the \
         hooks then hand you each new comment once, as your turns start and end — never \
         poll for comments, and after a wait just end your turn. Answer with one `{reply}` \
         carrying `{replies}`; it returns each thread as it now stands. `{list}` lists the \
         open threads, marks the ones waiting on you, and takes `{status}` for resolved \
         ones: call it when a hook says more are pending, when no hook is installed, or to \
         read history. `{start}` opens threads of your own on lines the user should look \
         at. `{open}` shows a file in the viewer; everything else works with no viewer \
         running. `{watch}` wakes you when another thread moves; `{workspaces}` lists and \
         pins workspaces.",
        follow = vocab::FOLLOW.name,
        kind = vocab::TYPE,
        id = vocab::ID,
        reply = vocab::THREAD_REPLY.name,
        replies = vocab::REPLIES,
        list = vocab::THREADS.name,
        status = vocab::STATUS,
        open = vocab::OPEN.name,
        start = vocab::THREAD_START.name,
        watch = vocab::THREAD_WATCH.name,
        workspaces = vocab::WORKSPACES.name,
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::XdgDirs;
    use fathomable_core::annotations::{
        Author, Draft, LineRange, MessageTarget, Placement, Reply, Status, Store,
    };
    use fathomable_core::config::AgentsConfig;
    use fathomable_testing::TempDir;
    use serde_json::Value;

    use super::{
        ReplyItem, Server, Shown, Tree, Which, check_path, headless_list, headless_reply,
        instructions, known_threads, refusal, thread_id, vocab,
    };
    use crate::app::testing;

    fn dirs(dir: &TempDir) -> XdgDirs {
        let state = dir.0.join("state").into_os_string();
        XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
    }

    fn item(thread: &str, line: Option<usize>) -> ReplyItem {
        ReplyItem {
            thread: thread.to_owned(),
            body: "ok".to_owned(),
            resolve: false,
            line,
            end_line: None,
        }
    }

    /// The `path` filter is checked against the workspace: a path in the
    /// wrong directory is refused and answered with the right one, so a
    /// filter never silently matches nothing. `.` segments and the root
    /// itself drop out.
    #[test]
    fn the_path_filter_is_checked_against_the_workspace() -> std::io::Result<()> {
        let dir = testing::bare("mcp-paths")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(root.join("src/deep"))?;
        fs::write(root.join("src/jokes.rs"), "")?;
        fs::write(root.join("src/deep/jokes.rs"), "")?;
        fs::write(root.join("lib.rs"), "")?;
        assert_eq!(check_path(&root, None), Ok(None));
        assert_eq!(check_path(&root, Some(Path::new("."))), Ok(None));
        assert_eq!(
            check_path(&root, Some(Path::new("./src/jokes.rs"))),
            Ok(Some(PathBuf::from("src/jokes.rs")))
        );
        assert_eq!(
            check_path(&root, Some(Path::new("src"))),
            Ok(Some(PathBuf::from("src")))
        );
        assert_eq!(
            check_path(&root, Some(Path::new("jokes.rs"))),
            Err(
                "jokes.rs is nothing in the workspace; did you mean src/jokes.rs, \
                 src/deep/jokes.rs?"
                    .to_owned()
            )
        );
        assert_eq!(
            check_path(&root, Some(Path::new("deep"))),
            Err("deep is nothing in the workspace; did you mean src/deep?".to_owned())
        );
        assert_eq!(
            check_path(&root, Some(Path::new("../lib.rs"))),
            Err("../lib.rs is not a workspace-relative path".to_owned())
        );
        assert_eq!(
            check_path(&root, Some(Path::new("/etc/passwd"))),
            Err("/etc/passwd is not a workspace-relative path".to_owned())
        );
        assert_eq!(
            check_path(&root, Some(Path::new("nope.rs"))),
            Err("nope.rs is nothing in the workspace".to_owned())
        );
        Ok(())
    }

    /// A watch on a thread that does not exist would never fire, so the
    /// ids are checked against the store first, and the failure says
    /// where the ids are.
    #[test]
    fn watches_name_only_stored_threads() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-watch")?;
        let root = dir.0.join("ws");
        let dirs = dirs(&dir);
        let id = Store::open(dirs.threads_file(&root))?.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?",
            ),
            "one\n",
            5,
        )?;
        let other = thread_id("00000000-0000-4000-8000-000000000000")?;
        assert_eq!(known_threads(&dirs, &root, [&id]), Ok(()));
        assert_eq!(
            known_threads(&dirs, &root, [&id, &other]),
            Err(format!(
                "no thread(s) {other}; call `threads` to see the ids"
            ))
        );
        assert_eq!(Which::parse(None), Ok(Which::Open));
        assert_eq!(Which::parse(Some("all")), Ok(Which::All));
        assert_eq!(
            Which::parse(Some("closed")),
            Err("`status` is `open`, `pending`, `resolved`, or `all`, not `closed`".to_owned())
        );
        Ok(())
    }

    /// With no viewer, an agent still reads the store and its reply lands
    /// in the file the next viewer loads, answered with the thread as it
    /// then stands.
    #[test]
    fn headless_reads_and_answers_the_store() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-headless")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let id = Store::open(dirs.threads_file(&root))?.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(2, 2),
                "why?",
            ),
            "one\ntwo\n",
            5,
        )?;
        let threads = headless_list(&dirs, &root)?;
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id(), &id);
        let answered = headless_reply(
            &dirs,
            &root,
            &id,
            Author::agent("bot"),
            "because".to_owned(),
            true,
            None,
        )?;
        assert_eq!(answered.replies().len(), 1);
        // The agent proposed; only the user resolves (ADR 0053).
        assert_eq!(answered.status(), Status::Open);
        assert!(answered.proposes_resolution());
        assert_eq!(headless_list(&dirs, &root)?, [answered]);
        let missing = headless_reply(
            &dirs,
            &root,
            &serde_json::from_str::<fathomable_core::annotations::ThreadId>(r#""1-2-3""#)?,
            Author::agent("bot"),
            "x".to_owned(),
            false,
            None,
        );
        assert_eq!(
            missing.map(|t| t.id().clone()),
            Err("no thread 1-2-3; call `threads` to see the ids".to_owned())
        );
        Ok(())
    }

    /// A batch is refused as a whole before anything is written: an
    /// unknown id, a resolved thread, and a detached thread without a
    /// line each name what fixes them; a detached thread with a line
    /// goes through.
    #[test]
    fn refusals_name_the_fix() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-refuse")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        let open = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?",
            ),
            "one\ntwo\n",
            5,
        )?;
        let resolved = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(2, 2),
                "fine",
            ),
            "one\ntwo\n",
            6,
        )?;
        store.resolve(&resolved, 7)?;
        let gone = store.annotate(
            Draft::new(Author::User, Path::new("a.md"), LineRange::new(2, 2), "old"),
            "one\nthree\n",
            8,
        )?;
        let all = store.threads().to_vec();
        let mut tree = Tree::new(&root);
        assert_eq!(
            refusal(&item(&open.to_string(), None), &all, &mut tree),
            None
        );
        assert_eq!(
            refusal(&item("1-2-3", None), &all, &mut tree),
            Some("no thread 1-2-3; call `threads` to see the ids".to_owned())
        );
        assert_eq!(
            refusal(&item(&resolved.to_string(), None), &all, &mut tree),
            Some(format!(
                "{resolved} is resolved; the user reopens it; call `threads` with \
                 `status: all` to read it"
            ))
        );
        assert_eq!(
            refusal(&item(&gone.to_string(), None), &all, &mut tree),
            Some(format!(
                "{gone} is detached: its lines are gone from a.md, last seen at 2; pass \
                 `line` and `end_line` to place it"
            ))
        );
        assert_eq!(
            refusal(&item(&gone.to_string(), Some(2)), &all, &mut tree),
            None
        );
        Ok(())
    }

    /// What an agent sees of a thread: placement and current range, no
    /// anchor hashes, the full body only while the thread is open, and
    /// `pending` while the user has the last word, `answered by` or
    /// `proposed by` once an agent has it (ADR 0058), and neither once
    /// resolved.
    #[test]
    fn shown_threads_carry_placement_and_no_anchor() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-shown")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "zero\none\ntwo\n")?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?\nreally",
            ),
            "one\ntwo\n",
            5,
        )?;
        let thread = store.thread(&id).cloned().ok_or("gone")?;
        let mut tree = Tree::new(&root);
        assert_eq!(
            tree.place(&thread),
            Placement::Anchored(LineRange::new(2, 2))
        );
        let shown = Shown::new(&thread, tree.place(&thread));
        assert_eq!(shown.line(), format!("{id}  a.md:2  open pending  why?"));
        let json = serde_json::to_value(&shown)?;
        assert_eq!(json["placement"], "anchored");
        assert_eq!(json["range"], serde_json::to_value(LineRange::new(2, 2))?);
        assert_eq!(json["pending"], true);
        assert!(json.get("answered").is_none());
        assert_eq!(json["comment"], "why?\nreally");
        assert!(json.get("anchor").is_none());
        assert!(json.get("messages").is_none());
        assert_eq!(json["snippet"], "one");

        // An agent's answer names it; a proposal says so; the user's edit
        // of the comment takes the word back.
        let bot = Author::agent("Claude").subscribed("s-1", "coder");
        store.reply(&id, Reply::new(bot.clone(), 6, "because"))?;
        let thread = store.thread(&id).cloned().ok_or("gone")?;
        let shown = Shown::new(&thread, tree.place(&thread));
        assert_eq!(
            shown.line(),
            format!("{id}  a.md:2  open answered by Claude (coder)  why?")
        );
        let json = serde_json::to_value(&shown)?;
        assert!(json.get("pending").is_none());
        assert_eq!(json["answered"]["name"], "Claude");
        assert_eq!(json["answered"]["type"], "coder");
        assert_eq!(json["answered"]["id"], "s-1");
        assert_eq!(json["answered"]["proposed"], false);
        store.reply(&id, Reply::new(bot, 7, "done").proposing_resolution())?;
        let thread = store.thread(&id).cloned().ok_or("gone")?;
        let shown = Shown::new(&thread, tree.place(&thread));
        assert_eq!(
            shown.line(),
            format!("{id}  a.md:2  open proposed by Claude (coder)  why?")
        );
        assert_eq!(serde_json::to_value(&shown)?["answered"]["proposed"], true);
        store.edit(&id, MessageTarget::Comment, "why?\nreally, why?", 8)?;
        let thread = store.thread(&id).cloned().ok_or("gone")?;
        let shown = Shown::new(&thread, tree.place(&thread));
        assert_eq!(shown.line(), format!("{id}  a.md:2  open pending  why?"));
        assert!(Which::Pending.admits(&thread) && !Which::Resolved.admits(&thread));

        store.resolve(&id, 9)?;
        let thread = store.thread(&id).cloned().ok_or("gone")?;
        let shown = Shown::new(&thread, tree.place(&thread));
        assert_eq!(shown.line(), format!("{id}  a.md:2  resolved  why?"));
        assert!(serde_json::to_value(&shown)?.get("answered").is_none());
        assert!(!Which::Pending.admits(&thread));
        let json = serde_json::to_value(&shown)?;
        assert_eq!(json["comment"], "why?");
        assert_eq!(json["messages"], 3);
        assert!(json.get("pending").is_none());
        assert!(json.get("snippet").is_none());
        assert!(json.get("replies").is_none());

        // Detached: the last known range is the stored one, which no
        // viewer has moved.
        fs::write(root.join("a.md"), "nothing\n")?;
        let mut tree = Tree::new(&root);
        let shown = Shown::new(&thread, tree.place(&thread));
        assert_eq!(shown.placement, "detached");
        assert_eq!(
            shown.line(),
            format!("{id}  a.md:1  resolved detached  why?")
        );
        Ok(())
    }

    /// `follow` subscribes, says so again on a bare call, refuses a bare
    /// call from a session that never subscribed, and ends with `end`.
    #[test]
    fn follow_subscribes_and_ends() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-follow")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        let server = Server::new(dirs.clone(), AgentsConfig::default());
        let bare = server.subscription(&root, None, None, None, None);
        assert!(
            bare.as_deref()
                .is_err_and(|e| e.starts_with("not subscribed: pass `type`")),
            "{bare:?}"
        );
        assert_eq!(
            server.subscription(&root, Some("s1".to_owned()), None, None, None),
            Err("`id` needs a `type`; configured types: coder, reviewer, planner".to_owned())
        );
        let note = server.subscription(
            &root,
            Some("s1".to_owned()),
            Some("coder".to_owned()),
            Some("bot"),
            None,
        )?;
        assert!(
            note.starts_with("subscribed s1 as coder, signing as bot; "),
            "{note}"
        );
        let again = server.subscription(&root, None, None, None, None)?;
        assert!(
            again.starts_with("still subscribed s1 as coder, signing as bot; "),
            "{again}"
        );
        let subscriber = server
            .register(&root, 10)?
            .subscriber("s1")
            .cloned()
            .ok_or("s1 is not subscribed")?;
        assert_eq!(subscriber.name(), Some("bot"));
        assert_eq!(server.end_subscription(&root, None)?, "unsubscribed s1");
        assert!(server.register(&root, 10)?.subscriber("s1").is_none());
        assert_eq!(
            server.end_subscription(&root, None),
            Err("nothing to end: pass `id`, or call `follow` with `type` first".to_owned())
        );
        assert_eq!(
            server.end_subscription(&root, Some("s1".to_owned()))?,
            "s1 was not subscribed"
        );
        Ok(())
    }

    /// Words the agent-facing text may backtick that are not tools or
    /// parameters: the hook, and the config nodes the guide names.
    const ALLOWED: [&str; 4] = ["hello", "fathomable", "agents.types", "user.name"];

    fn assert_known(text: &str, site: &str) {
        for ident in vocab::idents(text) {
            assert!(
                ALLOWED.contains(&ident) || vocab::is_known(ident),
                "{site} names unknown `{ident}`"
            );
        }
    }

    /// The vocabulary is the live schema, both ways: every tool and
    /// top-level parameter it lists exists, and nothing exists it does
    /// not list. This is what makes every other check mean something.
    #[test]
    fn vocabulary_matches_the_tool_schema() -> Result<(), String> {
        let live = Server::router().list_all();
        assert_eq!(live.len(), vocab::ALL.len());
        for tool in vocab::ALL {
            let found = live
                .iter()
                .find(|t| t.name == tool.name)
                .ok_or_else(|| format!("`{}` is not a registered tool", tool.name))?;
            let mut props: Vec<&str> = found
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|p| p.keys().map(String::as_str).collect())
                .unwrap_or_default();
            props.sort_unstable();
            let mut listed = tool.params.to_vec();
            listed.sort_unstable();
            assert_eq!(props, listed, "`{}` parameters", tool.name);
            assert_known(found.description.as_deref().unwrap_or_default(), tool.name);
            for (name, prop) in found
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let doc = prop
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert_known(doc, &format!("{}.{name}", tool.name));
            }
        }
        assert_known(&instructions(), "the server instructions");
        Ok(())
    }

    /// The guide's tool table names every tool, nothing else in its
    /// first column, and only known words in its second.
    #[test]
    fn guide_tool_table_matches_the_vocabulary() -> std::io::Result<()> {
        let guide = fathomable_testing::repo_file("docs/guide.md");
        let text = fs::read_to_string(&guide)?;
        let mut rows = text
            .lines()
            .skip_while(|l| !l.starts_with("| Tool | Use |"))
            .skip(2)
            .take_while(|l| l.starts_with("| `"));
        let mut named = Vec::new();
        for row in &mut rows {
            let mut cells = row.trim_matches('|').splitn(2, " | ");
            let tools = cells.next().unwrap_or_default();
            let usage = cells.next().unwrap_or_default();
            for ident in vocab::idents(tools) {
                assert!(
                    vocab::ALL.iter().any(|t| t.name == ident),
                    "guide table row names `{ident}`, not a tool"
                );
                named.push(ident);
            }
            assert_known(usage, "the guide table");
        }
        for tool in vocab::ALL {
            assert!(
                named.contains(&tool.name),
                "guide table lacks `{}`",
                tool.name
            );
        }
        assert_eq!(named.len(), vocab::ALL.len(), "a tool is listed twice");
        Ok(())
    }
}
