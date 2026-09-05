// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! `fathomable --mcp`: a stdio MCP server that drives the viewers of a
//! workspace over their Unix sockets and, when none runs, reads and
//! answers the thread store directly (ADR 0003, ADR 0014, ADR 0024).
//!
//! The server holds two pieces of state: the pinned workspace set by
//! `workspace_switch`, and the subscriber this connection last registered
//! with `follow` (ADR 0040), which signs its replies and is the default
//! `id` of the subscription tools. Every call otherwise resolves the
//! workspace afresh: an
//! explicit `workspace` argument, then the pin, then the known workspace
//! whose root is the longest prefix of the current directory. `open` and
//! `follow` reach every live viewer of that workspace or the one named by
//! `viewer`; `threads_list` and `thread_reply` go through a viewer when
//! one runs and to the store on disk when none does. Client identity is
//! read from the request context on each call, so the server behaves the
//! same under the legacy `initialize` flow and discovery-first startup.

use std::env;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::agents::{Register, WatchWhen};
use fathomable_core::annotations::{Author, LineRange, Reach, Reply, Store, Thread, ThreadId};
use fathomable_core::bond::{self, Process};
use fathomable_core::config::AgentsConfig;
use fathomable_core::seen;
use fathomable_core::session::{Marker, Record, Request, Response};
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::{Filter, Workspace};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CacheScope, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ProtocolVersion, ResultType, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::app::threads::open::follow_reply_lines;
use crate::app::threads::reach;
use crate::app::threads::reanchor::follow_snapshots;
use crate::hooks;
use fathomable_core::clock::now;

/// Run the server on stdin/stdout until the client disconnects.
pub(crate) fn run(dirs: &XdgDirs, agents: AgentsConfig) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(async {
        let server = Server::new(dirs.clone(), agents);
        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .context("MCP handshake failed")?;
        let reason = service.waiting().await.context("MCP server task failed")?;
        tracing::info!(?reason, "MCP server stopped");
        Ok(())
    })
}

/// The tool server; see the module docs for what it holds.
pub(crate) struct Server {
    dirs: XdgDirs,
    agents: AgentsConfig,
    pinned: Mutex<Option<PathBuf>>,
    /// The `(id, type, persona)` this connection subscribed with, if it did.
    subscriber: Mutex<Option<(String, String, Option<String>)>>,
    /// This process's ancestors, nearest first, matched against the
    /// session bonds the `hello` hook recorded (ADR 0041).
    ancestors: Vec<Process>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

/// A workspace an agent can address: its root and the viewers showing it.
#[derive(Debug, Clone)]
struct Target {
    root: PathBuf,
    viewers: Vec<Record>,
}

/// `workspace_switch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct SwitchParams {
    /// A workspace root as shown by `workspace_list`, or a viewer name or id
    /// (which selects that viewer's workspace).
    workspace: String,
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
    /// Workspace-relative files you are working on, or directories to
    /// take everything under them, including files not written yet;
    /// replaces the last list. Empty means the whole workspace.
    paths: Vec<PathBuf>,
    /// Your harness session id, as the `hello` hook told you. With
    /// `type`, subscribes this session: the stop hook and
    /// `threads_pending` then hand you what others write on these files.
    /// Optional when Fathomable can tell your session from the harness
    /// that started it; the reply says whether it could.
    #[serde(default)]
    id: Option<String>,
    /// Your agent type, one of the configured ones; fixed for the session.
    #[serde(default, rename = "type")]
    kind: Option<String>,
    /// Name to sign as; the client name is recorded alongside it.
    #[serde(default)]
    persona: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
    /// Viewer name or id to tell; every viewer when omitted.
    #[serde(default)]
    viewer: Option<String>,
}

/// `unfollow` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct UnfollowParams {
    /// The session id you subscribed with.
    id: String,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// `threads_list` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ListParams {
    /// Only threads changed at or after this Unix time in seconds.
    #[serde(default)]
    since: Option<u64>,
    /// Only threads on this workspace-relative file, or under this
    /// directory.
    #[serde(default)]
    path: Option<PathBuf>,
    /// At most this many threads, oldest change first; default 50. The
    /// summary says how many more there are and the `since` to pass.
    #[serde(default)]
    limit: Option<usize>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// `threads_pending` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct PendingParams {
    /// The session id you subscribed with; defaults to this connection's.
    #[serde(default)]
    id: Option<String>,
    /// At most this many newly pending threads; default 20. Threads not
    /// returned are not marked as shown to you.
    #[serde(default)]
    limit: Option<usize>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// One reply in a `thread_reply` batch.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReplyItem {
    /// Thread id from `threads_list` or `threads_pending`.
    thread: String,
    /// Reply text; Markdown.
    body: String,
    /// Also resolve the thread.
    #[serde(default)]
    resolve: bool,
    /// First line the thread's lines are on now, when you rewrote them;
    /// the thread re-anchors there before the reply is added.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line`.
    #[serde(default)]
    end_line: Option<usize>,
}

/// `thread_reply` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ReplyParams {
    /// Thread id from `threads_list` or `threads_pending`, for a
    /// single reply.
    #[serde(default)]
    thread: Option<String>,
    /// Reply text for a single reply; Markdown.
    #[serde(default)]
    body: Option<String>,
    /// Also resolve the thread (single reply).
    #[serde(default)]
    resolve: bool,
    /// First line the thread's lines are on now, when you rewrote them
    /// (single reply); the thread re-anchors there before the reply is
    /// added.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line` (single reply).
    #[serde(default)]
    end_line: Option<usize>,
    /// Several replies in one call, instead of `thread` and `body`.
    /// Answer every pending thread this way in one turn.
    #[serde(default)]
    replies: Vec<ReplyItem>,
    /// Name to sign as; the client name is recorded alongside it.
    #[serde(default)]
    persona: Option<String>,
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
    /// The thread to watch.
    on: String,
    /// `message` (someone else posts on it) or `resolved`.
    when: String,
    /// Thread ids to be reminded of, in full, when the watch fires.
    #[serde(default)]
    remind: Vec<String>,
    /// The session id you subscribed with; defaults to this connection's.
    #[serde(default)]
    id: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// `thread_unwatch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct UnwatchParams {
    /// The watched thread.
    on: String,
    /// The session id you subscribed with; defaults to this connection's.
    #[serde(default)]
    id: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

#[tool_router]
impl Server {
    fn new(dirs: XdgDirs, agents: AgentsConfig) -> Self {
        let cwd = env::current_dir().unwrap_or_default();
        if let Some(target) = bind(&targets(&dirs), &cwd) {
            tracing::info!(root = %target.root.display(), viewers = target.viewers.len(), "workspace contains the cwd");
        } else {
            tracing::warn!(cwd = %cwd.display(), "no known workspace contains the cwd");
        }
        Self {
            dirs,
            agents,
            pinned: Mutex::new(None),
            subscriber: Mutex::new(None),
            ancestors: bond::ancestors(),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List the workspaces Fathomable knows and the viewers running in each; \
                       the one later calls address by default is marked. Use it when a call \
                       says no workspace contains the current directory, then pin one with \
                       `workspace_switch`. Returns roots and viewer names, not threads.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    fn workspace_list(&self) -> CallToolResult {
        let all = targets(&self.dirs);
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
        let summary = if all.is_empty() {
            "no known workspaces; start Fathomable in one".to_owned()
        } else {
            all.iter()
                .map(|s| {
                    let mark = if default.as_deref() == Some(s.root.as_path()) {
                        "*"
                    } else {
                        " "
                    };
                    let mut lines = vec![format!("{mark} {}", s.root.display())];
                    if s.viewers.is_empty() {
                        lines.push("    (no viewers running)".to_owned());
                    }
                    for v in &s.viewers {
                        lines.push(format!("    {}  {}", v.name().unwrap_or("-"), v.id()));
                    }
                    lines.join("\n")
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        with_summary(json!({ "workspaces": workspaces }), summary)
    }

    #[tool(
        description = "Pin a workspace as the default for later calls, by its root path or by \
                       the name or id of a viewer showing it. Needed only when the current \
                       directory is outside every known workspace or inside several. The pin \
                       lasts for this connection.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn workspace_switch(&self, Parameters(p): Parameters<SwitchParams>) -> CallToolResult {
        match self.resolve(Some(&p.workspace)) {
            Ok(target) => {
                if let Ok(mut pinned) = self.pinned.lock() {
                    *pinned = Some(target.root.clone());
                }
                text(format!(
                    "bound to {} ({} viewer(s) running)",
                    target.root.display(),
                    target.viewers.len()
                ))
            }
            Err(error) => failure(error),
        }
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
        description = "Say which files you are working on, replacing the previous list; the \
                       viewer marks them and follows your edits there. With `id` (your session \
                       id from the `hello` hook) and `type`, also subscribe this session: the \
                       hooks then hand you every thread on these files, or that you posted \
                       in, whose newest message is someone else's, once each, as your turns \
                       start and end. Call it once at the start and again when your files \
                       change; works with no viewer running. A path is a file, or a \
                       directory to cover everything under it, including files not written \
                       yet. Fails, following nothing new, when a path is in neither form; \
                       the reply then names same-named paths elsewhere.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn follow(
        &self,
        Parameters(p): Parameters<FollowParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let paths = match check_paths(&target.root, &p.paths) {
            Ok(paths) => paths,
            Err(error) => return failure(error),
        };
        let client = context.client_info().map(|c| c.name);
        let note = match self.subscription(
            &target.root,
            p.id,
            p.kind,
            p.persona,
            client.as_deref(),
            &paths,
        ) {
            Ok(note) => note,
            Err(error) => return failure(error),
        };
        let files = listed(&target.root, &paths);
        if target.viewers.is_empty() && p.viewer.is_none() {
            return text(format!("{note}following {files}; no viewer is running"));
        }
        let request = Request::Follow { paths };
        match self
            .broadcast(p.workspace.as_deref(), p.viewer.as_deref(), &request)
            .await
        {
            Ok(count) => text(format!("{note}following {files} in {count} viewer(s)")),
            Err(error) => failure(error),
        }
    }

    #[tool(
        description = "End this session's subscription: its deliveries and watches are \
                       forgotten and the stop hook falls silent for it. The viewer's follow \
                       markers are unchanged. Idempotent for an unknown id.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn unfollow(&self, Parameters(p): Parameters<UnfollowParams>) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let when = now();
        let outcome = self.register(&target.root, when).and_then(|mut register| {
            if register.subscriber(&p.id).is_none() {
                return Ok(false);
            }
            register
                .unsubscribe(&p.id, when)
                .map(|()| true)
                .map_err(|e| e.to_string())
        });
        match outcome {
            Ok(removed) => {
                if let Ok(mut current) = self.subscriber.lock()
                    && current.as_ref().is_some_and(|(id, ..)| *id == p.id)
                {
                    *current = None;
                }
                text(if removed {
                    format!("unsubscribed {}", p.id)
                } else {
                    format!("{} was not subscribed", p.id)
                })
            }
            Err(error) => failure(error),
        }
    }

    #[tool(
        description = "List annotation threads on the current work: each with its id, file, \
                       line range, status, comment, and replies. Filter with `since` (Unix \
                       seconds, threads changed at or after it) and `path`; at most `limit` \
                       threads come back, oldest change first, and the summary says how to get \
                       the rest. Works with no viewer running. Does not return file content \
                       beyond the annotated snippet. `path` is a file, or a directory to \
                       read every thread under it; it fails when it is neither.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn threads_list(&self, Parameters(p): Parameters<ListParams>) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let path = match check_paths(&target.root, p.path.as_slice()) {
            Ok(paths) => paths.into_iter().next(),
            Err(error) => return failure(error),
        };
        let request = Request::ThreadsList {
            since: p.since,
            path: path.clone(),
        };
        let outcome = match target.viewers.first() {
            Some(viewer) => call(viewer, &request).await,
            None => headless_list(&self.dirs, &target.root, p.since, path.as_deref())
                .map(Response::Threads),
        };
        match outcome {
            Ok(Response::Threads(mut threads)) => {
                threads.sort_by_key(Thread::updated);
                let limit = p.limit.unwrap_or(DEFAULT_LIST_LIMIT).max(1);
                let rest = threads.len().saturating_sub(limit);
                threads.truncate(limit);
                let mut summary = summarize(&threads);
                if rest > 0 {
                    let since = threads.last().map_or(0, Thread::updated);
                    let _ = write!(
                        summary,
                        "\n{rest} more; pass since={since} (threads changed at that second may repeat)"
                    );
                }
                match serde_json::to_value(&threads) {
                    Ok(value) => with_summary(json!({ "threads": value, "more": rest }), summary),
                    Err(error) => failure(error.to_string()),
                }
            }
            other => unexpected(other),
        }
    }

    #[tool(
        description = "The threads waiting on you: open threads on the files you follow, or \
                       that you posted in, whose newest message is not yours and has not been \
                       shown to you yet, plus any watch that fired. Each is returned once. \
                       Do not poll it: the hooks deliver as your turns start and end, so after \
                       a wait just end your turn. Call it only when a hook lists more threads \
                       than it showed, or when no hook is installed. Needs a subscription from \
                       `follow` with `id` and `type`. Answer what it returns with one \
                       `thread_reply` call.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    fn threads_pending(&self, Parameters(p): Parameters<PendingParams>) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let when = now();
        let mut register = match self.register(&target.root, when) {
            Ok(register) => register,
            Err(error) => return failure(error),
        };
        let Some(id) = self.session_id(p.id, &register) else {
            return failure(
                "no subscriber id: pass `id`, or call follow with `id` and `type` first",
            );
        };
        let Some(subscriber) = register.subscriber(&id).cloned() else {
            return failure(format!(
                "session {id} is not subscribed; call follow with `id` and `type`"
            ));
        };
        let threads = match headless_store(&self.dirs, &target.root) {
            Ok((store, scope)) => store
                .threads()
                .iter()
                .filter(|t| scope.includes(t))
                .cloned()
                .collect::<Vec<_>>(),
            Err(error) => return failure(error),
        };
        let mut blob = match hooks::gather(
            &mut register,
            &subscriber,
            &threads,
            &self.agents,
            hooks::Occasion::TurnEnd,
            when,
        ) {
            Ok(blob) => blob,
            Err(error) => return failure(error.to_string()),
        };
        let limit = p.limit.unwrap_or(DEFAULT_PENDING_LIMIT).max(1);
        let over = blob.fresh.len().saturating_sub(limit);
        blob.fresh.truncate(limit);
        // The summary is bounded the same way the hook's blob is, so what
        // it lists by id alone is left undelivered and returned by the
        // next call rather than serialized here and lost.
        blob.fit(&subscriber, self.agents.max_lines);
        let rest = over + blob.listed.len();
        let shown: Vec<&Thread> = blob.shown().collect();
        for thread in &shown {
            if let Err(error) = register.deliver(&id, thread, when) {
                return failure(error.to_string());
            }
        }
        let mut summary = if blob.is_empty() {
            "nothing pending".to_owned()
        } else {
            blob.render(&subscriber)
        };
        // What `fit` held back is already named, by id, at the end of the
        // rendered blob; only the threads dropped by `limit` still need
        // announcing, though `more` counts both.
        if over > 0 {
            let _ = write!(summary, "\n{over} more; call threads_pending again");
        }
        match serde_json::to_value(&shown) {
            Ok(value) => with_summary(
                json!({ "threads": value, "more": rest, "reminder": blob.reminder.iter().map(|t| t.id()).collect::<Vec<_>>() }),
                summary,
            ),
            Err(error) => failure(error.to_string()),
        }
    }

    #[tool(
        description = "Reply to one thread (`thread`, `body`) or to several at once \
                       (`replies`), each optionally resolving its thread. Prefer one call \
                       with `replies` for everything `threads_pending` handed you. If you \
                       rewrote the lines a thread is on, pass `line` and `end_line` so it \
                       follows them. Works with no viewer running; signs with your \
                       subscription when this connection has one or your session is known \
                       from the harness, else pass your session `id`.",
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
        let subscription = self.signature(p.id, &target.root);
        let mut author = Author::Agent {
            name: p
                .persona
                .or_else(|| subscription.as_ref().and_then(|(.., name)| name.clone()))
                .or_else(|| client.clone())
                .unwrap_or_else(|| "agent".to_owned()),
            client,
            id: None,
            kind: None,
        };
        if let Some((id, kind, _)) = &subscription {
            author = author.subscribed(id, kind);
        }
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
            _ => return failure("pass `thread` and `body`, or a non-empty `replies` list"),
        }
        let mut lines = Vec::new();
        for item in items {
            match self.reply_one(&target, author.clone(), item).await {
                Ok(line) => lines.push(line),
                Err(error) => {
                    lines.push(error);
                    return failure(lines.join("\n"));
                }
            }
        }
        text(lines.join("\n"))
    }

    #[tool(
        description = "Ask to be woken when another thread moves: `when` is `message` \
                       (someone else posts on it) or `resolved`. When it fires, the stop hook \
                       or `threads_pending` says so and hands you the `remind` threads again \
                       in full, then the watch is gone. Use it to park a thread that depends \
                       on a discussion elsewhere. Needs a subscription; fails when `on` or a \
                       `remind` thread does not exist.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn thread_watch(&self, Parameters(p): Parameters<WatchParams>) -> CallToolResult {
        let when = match p.when.as_str() {
            "message" => WatchWhen::Message,
            "resolved" => WatchWhen::Resolved,
            other => return failure(format!("`when` is `message` or `resolved`, not `{other}`")),
        };
        let on = match thread_id(&p.on) {
            Ok(id) => id,
            Err(error) => return failure(error),
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

    #[tool(
        description = "Cancel a watch set with `thread_watch` on the given thread. Fails \
                       when there is no such watch.",
        annotations(
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    fn thread_unwatch(&self, Parameters(p): Parameters<UnwatchParams>) -> CallToolResult {
        let on = match thread_id(&p.on) {
            Ok(id) => id,
            Err(error) => return failure(error),
        };
        self.with_subscriber(p.workspace.as_deref(), p.id, |register, id, now| {
            register
                .unwatch(id, &on, now)
                .map(|()| format!("no longer watching {}", p.on))
        })
    }
}

/// `threads_list` threads per call unless `limit` says otherwise.
const DEFAULT_LIST_LIMIT: usize = 50;
/// `threads_pending` fresh threads per call unless `limit` says otherwise.
const DEFAULT_PENDING_LIMIT: usize = 20;

fn thread_id(text: &str) -> Result<ThreadId, String> {
    serde_json::from_value(Value::String(text.to_owned())).map_err(|e| e.to_string())
}

impl Server {
    fn pinned(&self) -> Option<PathBuf> {
        self.pinned.lock().ok().and_then(|p| p.clone())
    }

    fn subscriber_id(&self) -> Option<String> {
        self.subscriber
            .lock()
            .ok()
            .and_then(|s| s.as_ref().map(|(id, ..)| id.clone()))
    }

    /// The subscription side of `follow`: subscribe `id` as `kind`,
    /// refuse a half-given pair, or, with neither, refresh the coverage
    /// of the session this connection already speaks for. Returns the
    /// note that prefixes the reply, empty when nothing was registered.
    fn subscription(
        &self,
        root: &Path,
        id: Option<String>,
        kind: Option<String>,
        persona: Option<String>,
        client: Option<&str>,
        paths: &[PathBuf],
    ) -> Result<String, String> {
        let when = now();
        match (id, kind) {
            (Some(id), Some(kind)) => {
                let mut register = self.register(root, when)?;
                self.subscribe(&mut register, &id, &kind, persona, client, paths, when)?;
                Ok(format!("subscribed {id} as {kind}; "))
            }
            (None, Some(kind)) => {
                let mut register = self.register(root, when)?;
                let Some(id) = register.session_for(&self.ancestors).map(str::to_owned) else {
                    return Err(
                        "`type` needs the session `id` the hello hook gave you; this session \
                         could not be told from the harness"
                            .to_owned(),
                    );
                };
                self.subscribe(&mut register, &id, &kind, persona, client, paths, when)?;
                Ok(format!(
                    "subscribed {id} (your session, found from the harness) as {kind}; "
                ))
            }
            (Some(_), None) => Err(format!(
                "`id` needs a `type`; configured types: {}",
                self.agents.types.join(", ")
            )),
            (None, None) => {
                let Ok(mut register) = self.register(root, when) else {
                    return Ok(String::new());
                };
                let Some(id) = self.session_id(None, &register) else {
                    return Ok(String::new());
                };
                let Some(existing) = register.subscriber(&id) else {
                    return Ok(String::new());
                };
                let kind = existing.kind().to_owned();
                let name = existing.name().map(str::to_owned);
                register
                    .subscribe(&id, &kind, name.as_deref(), client, paths.to_vec(), when)
                    .map_err(|e| e.to_string())?;
                Ok(format!("coverage updated for {id}; "))
            }
        }
    }

    /// Subscribe `id` as `kind` in `register` and remember it as this
    /// connection's signature.
    #[expect(clippy::too_many_arguments, reason = "one call per subscription field")]
    fn subscribe(
        &self,
        register: &mut Register,
        id: &str,
        kind: &str,
        persona: Option<String>,
        client: Option<&str>,
        paths: &[PathBuf],
        when: u64,
    ) -> Result<(), String> {
        if !self.agents.allows(kind) {
            return Err(format!(
                "unknown agent type `{kind}`; configured types: {}",
                self.agents.types.join(", ")
            ));
        }
        register
            .subscribe(id, kind, persona.as_deref(), client, paths.to_vec(), when)
            .map_err(|e| e.to_string())?;
        if let Ok(mut current) = self.subscriber.lock() {
            *current = Some((id.to_owned(), kind.to_owned(), persona));
        }
        Ok(())
    }

    /// The session a call speaks for: the `id` it passed, else the one
    /// this connection subscribed with, else the one bonded to this
    /// process's ancestors (ADR 0041).
    fn session_id(&self, given: Option<String>, register: &Register) -> Option<String> {
        given
            .or_else(|| self.subscriber_id())
            .or_else(|| register.session_for(&self.ancestors).map(str::to_owned))
    }

    /// The `(id, type, persona)` a reply from this connection is signed
    /// with: its own `follow`, else the subscription of `given` or of
    /// the bonded session, when there is one.
    fn signature(
        &self,
        given: Option<String>,
        root: &Path,
    ) -> Option<(String, String, Option<String>)> {
        if let Some(own) = self.subscriber.lock().ok().and_then(|s| s.clone()) {
            return Some(own);
        }
        let register = self.register(root, now()).ok()?;
        let id = self.session_id(given, &register)?;
        let subscriber = register.subscriber(&id)?;
        Some((
            id,
            subscriber.kind().to_owned(),
            subscriber.name().map(str::to_owned),
        ))
    }

    /// The workspace's agent register at `now`.
    fn register(&self, root: &Path, now: u64) -> Result<Register, String> {
        Register::open(self.dirs.agents_file(root), now, self.agents.expire_after)
            .map_err(|e| e.to_string())
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
            return failure(
                "no subscriber id: pass `id`, or call follow with `id` and `type` first",
            );
        };
        match act(&mut register, &id, when) {
            Ok(message) => text(message),
            Err(error) => failure(error.to_string()),
        }
    }

    /// One reply of a `thread_reply` call, through a viewer or the store.
    async fn reply_one(
        &self,
        target: &Target,
        author: Author,
        item: ReplyItem,
    ) -> Result<String, String> {
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
            .map(|()| Response::Done),
        };
        match outcome {
            Ok(Response::Done) => Ok(format!(
                "replied to {}{}",
                item.thread,
                if item.resolve { " and resolved it" } else { "" }
            )),
            Ok(Response::Error(message)) | Err(message) => {
                Err(format!("{}: {message}", item.thread))
            }
            Ok(other) => Err(format!("{}: unexpected reply {other:?}", item.thread)),
        }
    }

    /// The workspace a call addresses: `workspace` when given (a root, or a
    /// viewer name or id), else the pin, else the one containing the cwd.
    fn resolve(&self, workspace: Option<&str>) -> Result<Target, String> {
        let all = targets(&self.dirs);
        if let Some(key) = workspace {
            if let Some(found) = all
                .iter()
                .find(|s| s.viewers.iter().any(|v| v.is_called(key)))
            {
                return Ok(found.clone());
            }
            let path = PathBuf::from(key);
            let path = path.canonicalize().unwrap_or(path);
            return all.iter().find(|s| s.root == path).cloned().ok_or_else(|| {
                format!("no workspace or viewer called `{key}`; see workspace_list")
            });
        }
        if let Some(pinned) = self.pinned()
            && let Some(found) = all.iter().find(|s| s.root == pinned)
        {
            return Ok(found.clone());
        }
        let cwd = env::current_dir().unwrap_or_default();
        bind(&all, &cwd).cloned().ok_or_else(|| {
            "no known workspace contains the current directory; call workspace_list and workspace_switch"
                .to_owned()
        })
    }

    /// Send `request` to every viewer of the workspace, or to the one
    /// called `viewer`. Returns how many answered `Done`; the first error
    /// fails the call.
    async fn broadcast(
        &self,
        workspace: Option<&str>,
        viewer: Option<&str>,
        request: &Request,
    ) -> Result<usize, String> {
        let target = self.resolve(workspace)?;
        let targets: Vec<&Record> = match viewer {
            Some(key) => {
                let found = target
                    .viewers
                    .iter()
                    .find(|v| v.is_called(key))
                    .ok_or_else(|| {
                        format!(
                            "no viewer called `{key}` on {}; see workspace_list",
                            target.root.display()
                        )
                    })?;
                vec![found]
            }
            None => target.viewers.iter().collect(),
        };
        if targets.is_empty() {
            return Err(format!(
                "no viewer is running for {}; start Fathomable there",
                target.root.display()
            ));
        }
        let mut count = 0;
        for viewer in targets {
            match call(viewer, request).await? {
                Response::Done => count += 1,
                Response::Error(message) => return Err(message),
                other => return Err(format!("unexpected reply {other:?}")),
            }
        }
        Ok(count)
    }
}

/// One request-response exchange with a viewer's socket.
async fn call(viewer: &Record, request: &Request) -> Result<Response, String> {
    let socket = viewer
        .socket()
        .ok_or_else(|| format!("viewer {} has no socket", viewer.id()))?;
    exchange(socket, &request.to_line())
        .await
        .map_err(|e| format!("viewer {}: {e}", viewer.id()))
}

async fn exchange(socket: &Path, line: &str) -> std::io::Result<Response> {
    let stream = UnixStream::connect(socket).await?;
    let (reader, mut writer) = stream.into_split();
    writer.write_all(format!("{line}\n").as_bytes()).await?;
    let mut reply = String::new();
    BufReader::new(reader).read_line(&mut reply).await?;
    reply
        .trim_end()
        .parse::<Response>()
        .map_err(std::io::Error::other)
}

/// Every known workspace with its live viewers: marked workspaces first
/// (most recently seen first), then any a live viewer names without a
/// marker.
fn targets(dirs: &XdgDirs) -> Vec<Target> {
    let mut all: Vec<Target> = Marker::list(dirs)
        .into_iter()
        .map(|marker| Target {
            root: marker.root().to_path_buf(),
            viewers: Vec::new(),
        })
        .collect();
    for record in Record::live(dirs) {
        match all.iter_mut().find(|s| s.root == record.root()) {
            Some(target) => target.viewers.push(record),
            None => all.push(Target {
                root: record.root().to_path_buf(),
                viewers: vec![record],
            }),
        }
    }
    all
}

/// The workspace whose root is the longest prefix of `cwd`.
fn bind<'a>(targets: &'a [Target], cwd: &Path) -> Option<&'a Target> {
    targets
        .iter()
        .filter(|s| cwd.starts_with(&s.root))
        .max_by_key(|s| s.root.as_os_str().len())
}

/// Open the workspace's store with no viewer running: threads edited
/// offline are followed through their snapshots first (ADR 0020), and the
/// scope of the current `HEAD` is computed (ADR 0024).
/// The paths a `follow` list names, for its reply; a directory is shown
/// with a trailing separator, since it covers what is under it.
fn listed(root: &Path, paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return "the whole workspace".to_owned();
    }
    let names: Vec<String> = paths
        .iter()
        .map(|p| {
            if root.join(p).is_dir() {
                format!("{}/", p.display())
            } else {
                p.display().to_string()
            }
        })
        .collect();
    format!("{} path(s): {}", paths.len(), names.join(", "))
}

/// Check that every path names a file or directory in the workspace at
/// `root`, naming each that does not, and answer with the paths to
/// store: `.` segments dropped, and a path that names the root itself
/// left out, since an empty list already means the whole workspace.
///
/// A path that exists nowhere is matched by its last component against
/// the workspace, so a wrong directory is answered with the right one.
fn check_paths(root: &Path, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut problems = Vec::new();
    let mut clean = Vec::new();
    let mut known: Option<Vec<PathBuf>> = None;
    for path in paths {
        let inside = path.is_relative()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        let shown = path.display();
        if !inside {
            problems.push(format!("{shown} is not a workspace-relative path"));
            continue;
        }
        let path: PathBuf = path
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        if path.as_os_str().is_empty() {
            continue;
        }
        let full = root.join(&path);
        if full.is_file() || full.is_dir() {
            clean.push(path);
            continue;
        }
        let known = known.get_or_insert_with(|| workspace_paths(root));
        let same: Vec<String> = known
            .iter()
            .filter(|k| k.file_name() == path.file_name())
            .map(|k| k.display().to_string())
            .collect();
        problems.push(if same.is_empty() {
            format!(
                "{shown} is nothing in the workspace; follow its directory to cover a file \
                 you have not written yet"
            )
        } else {
            format!(
                "{shown} is nothing in the workspace; did you mean {}?",
                same.join(", ")
            )
        });
    }
    if problems.is_empty() {
        Ok(clean)
    } else {
        Err(problems.join("\n"))
    }
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
        Err(format!("unknown thread(s) {}", unknown.join(", ")))
    }
}

fn headless_store(dirs: &XdgDirs, root: &Path) -> Result<(Store, Reach), String> {
    let mut store = Store::open(dirs.threads_file(root)).map_err(|e| e.to_string())?;
    let pinned: Vec<PathBuf> = store.open_paths().map(Path::to_path_buf).collect();
    match seen::Store::open_pinned(&dirs.seen_dir(root), pinned.iter().map(PathBuf::as_path)) {
        Ok(seen) => {
            let moved = follow_snapshots(&mut store, &seen, root);
            if moved > 0 {
                tracing::info!(moved, "threads re-anchored headlessly");
            }
        }
        Err(error) => tracing::warn!(%error, "cannot open snapshots; reporting stored ranges"),
    }
    let scope = match Workspace::discover(root) {
        Ok(workspace) => match workspace.reachable(store.commits()) {
            Some(mut reachable) => {
                let moved = reach::follow_head(&mut store, &workspace, &reachable);
                if moved > 0 {
                    tracing::info!(moved, "threads rescoped headlessly");
                    reachable.extend(workspace.head_commit());
                }
                Reach::reachable(reachable)
            }
            None => Reach::everything(),
        },
        Err(error) => {
            tracing::warn!(%error, "cannot open the workspace; threads unscoped");
            Reach::everything()
        }
    };
    Ok((store, scope))
}

fn headless_list(
    dirs: &XdgDirs,
    root: &Path,
    since: Option<u64>,
    path: Option<&Path>,
) -> Result<Vec<Thread>, String> {
    let (store, scope) = headless_store(dirs, root)?;
    Ok(store
        .threads()
        .iter()
        .filter(|t| scope.includes(t))
        .filter(|t| since.is_none_or(|s| t.updated() >= s))
        .filter(|t| path.is_none_or(|p| t.path().starts_with(p)))
        .cloned()
        .collect())
}

fn headless_reply(
    dirs: &XdgDirs,
    root: &Path,
    thread: &ThreadId,
    author: Author,
    body: String,
    resolve: bool,
    lines: Option<LineRange>,
) -> Result<(), String> {
    let mut store = Store::open(dirs.threads_file(root)).map_err(|e| e.to_string())?;
    if store.thread(thread).is_none() {
        return Err(format!("unknown thread {thread}"));
    }
    let when = now();
    if let Some(lines) = lines {
        follow_reply_lines(&mut store, root, thread, lines, when)?;
    }
    let reply = Reply::new(author.clone(), when, body);
    let reply = if resolve {
        reply.proposing_resolution()
    } else {
        reply
    };
    store.reply(thread, reply).map_err(|e| e.to_string())?;
    if resolve {
        store
            .resolve(thread, author, when)
            .map_err(|e| e.to_string())?;
    }
    tracing::info!(%thread, resolve, "agent reply added headlessly");
    Ok(())
}

fn summarize(threads: &[Thread]) -> String {
    if threads.is_empty() {
        return "no threads".to_owned();
    }
    threads
        .iter()
        .map(|t| {
            format!(
                "{}  {}:{}  {:?}  {}",
                t.id(),
                t.path().display(),
                t.range(),
                t.status(),
                t.comment().lines().next().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn text(summary: String) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(summary)])
}

fn with_summary(value: Value, summary: String) -> CallToolResult {
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(summary)];
    result
}

fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

fn unexpected(outcome: Result<Response, String>) -> CallToolResult {
    match outcome {
        Ok(Response::Error(message)) | Err(message) => failure(message),
        Ok(other) => failure(format!("unexpected reply {other:?}")),
    }
}

/// The server instructions every client is handed on connect.
fn instructions() -> String {
    format!(
        "Fathomable is the user's read-only viewer, where they leave review comments \
         on the lines you write. Start with `{follow}` naming the files you will edit \
         and a `{kind}` to subscribe; the hooks then hand you each new comment once, \
         as your turns start and end — never poll for comments, and after a wait \
         just end your turn. Add the session `{id}` the hello hook gave you when \
         asked for it, and pass it to `{reply}` too if you never called \
         `{follow}` on this connection. Answer with one `{reply}` carrying \
         `{replies}`. `{pending}` is for the overflow a hook lists by id, or \
         for a harness without hooks. `{list}` reads any thread; `{open}` \
         shows a file in the viewer. Everything but `{open}` works with no viewer \
         running. `{watch}` wakes you when another thread moves.",
        follow = vocab::FOLLOW.name,
        kind = vocab::TYPE,
        id = vocab::ID,
        reply = vocab::THREAD_REPLY.name,
        replies = vocab::REPLIES,
        pending = vocab::THREADS_PENDING.name,
        list = vocab::THREADS_LIST.name,
        open = vocab::OPEN.name,
        watch = vocab::THREAD_WATCH.name,
    )
}

/// The tools as listed to a client: `follow`'s `type` carries the
/// configured agent types as an `enum`, so a model that never saw the
/// hello hook still knows the valid values (ADR 0043).
fn with_types(mut tools: Vec<Tool>, types: &[String]) -> Vec<Tool> {
    if let Some(follow) = tools.iter_mut().find(|t| t.name == vocab::FOLLOW.name) {
        let schema = Arc::make_mut(&mut follow.input_schema);
        if let Some(kind) = schema
            .get_mut("properties")
            .and_then(Value::as_object_mut)
            .and_then(|p| p.get_mut(vocab::TYPE))
            .and_then(Value::as_object_mut)
        {
            kind.insert("enum".to_owned(), json!(types));
        }
    }
    tools
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fathomable", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let cache_hints = context
            .protocol_version()
            .is_some_and(|v| v >= ProtocolVersion::V_2026_07_28);
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools: with_types(self.tool_router.list_all(), &self.agents.types),
            meta: None,
            next_cursor: None,
            ttl_ms: cache_hints.then_some(0),
            cache_scope: cache_hints.then_some(CacheScope::Public),
        })
    }
}

#[cfg(test)]
mod tests {
    use fathomable_testing::TempDir;

    use crate::app::testing;
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::XdgDirs;
    use fathomable_core::agents::Subscriber;
    use fathomable_core::annotations::{Author, Draft, LineRange, Status, Store, ThreadId};
    use fathomable_core::config::AgentsConfig;
    use fathomable_core::session::{Id, Record};

    use serde_json::Value;

    use super::{
        Server, Target, bind, check_paths, headless_list, headless_reply, instructions,
        known_threads, listed, thread_id, vocab, with_types,
    };

    fn target(root: &str, viewers: usize) -> Target {
        Target {
            root: PathBuf::from(root),
            viewers: (0..viewers)
                .map(|_| Record::new(Id::mint(), PathBuf::from(root), None))
                .collect(),
        }
    }

    #[test]
    fn binding_picks_the_longest_root_even_without_viewers() {
        let sessions = [target("/work", 1), target("/work/repo", 0), target("/x", 2)];
        let bound = bind(&sessions, Path::new("/work/repo/src"));
        assert_eq!(
            bound.map(|s| s.root.as_path()),
            Some(Path::new("/work/repo"))
        );
        assert!(bind(&sessions, Path::new("/tmp")).is_none());
    }

    fn dirs(dir: &TempDir) -> XdgDirs {
        let state = dir.0.join("state").into_os_string();
        XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
    }

    /// With no viewer, an agent still reads the store and its reply lands
    /// in the file the next viewer loads.
    /// A follow list is checked against the workspace: a path in the
    /// wrong directory is refused and answered with the right one, so a
    /// subscription never silently covers nothing. A directory is a
    /// legal entry; `.` segments and the root itself drop out.
    #[test]
    fn paths_are_checked_against_the_workspace() -> std::io::Result<()> {
        let dir = testing::bare("mcp-paths")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(root.join("src/deep"))?;
        fs::write(root.join("src/jokes.rs"), "")?;
        fs::write(root.join("src/deep/jokes.rs"), "")?;
        fs::write(root.join("lib.rs"), "")?;
        assert_eq!(check_paths(&root, &[]), Ok(Vec::new()));
        assert_eq!(
            check_paths(
                &root,
                &[
                    PathBuf::from("./src/jokes.rs"),
                    PathBuf::from("src"),
                    PathBuf::from("lib.rs"),
                    PathBuf::from("."),
                ]
            ),
            Ok(vec![
                PathBuf::from("src/jokes.rs"),
                PathBuf::from("src"),
                PathBuf::from("lib.rs"),
            ])
        );
        assert_eq!(
            check_paths(
                &root,
                &[
                    PathBuf::from("jokes.rs"),
                    PathBuf::from("deep"),
                    PathBuf::from("../lib.rs"),
                    PathBuf::from("/etc/passwd"),
                    PathBuf::from("nope.rs"),
                ]
            ),
            Err([
                "jokes.rs is nothing in the workspace; did you mean src/jokes.rs, \
                 src/deep/jokes.rs?",
                "deep is nothing in the workspace; did you mean src/deep?",
                "../lib.rs is not a workspace-relative path",
                "/etc/passwd is not a workspace-relative path",
                "nope.rs is nothing in the workspace; follow its directory to cover a file \
                 you have not written yet",
            ]
            .join("\n"))
        );
        assert_eq!(listed(&root, &[]), "the whole workspace");
        assert_eq!(
            listed(&root, &[PathBuf::from("src"), PathBuf::from("lib.rs")]),
            "2 path(s): src/, lib.rs"
        );
        Ok(())
    }

    /// A watch on a thread that does not exist would never fire, so the
    /// ids are checked against the store first.
    #[test]
    fn watches_name_only_stored_threads() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-watch")?;
        let root = dir.0.join("ws");
        let dirs = dirs(&dir);
        let id = Store::open(dirs.threads_file(&root))?.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(1, 1), "why?"),
            "one\n",
            5,
        )?;
        let other = thread_id("00000000-0000-4000-8000-000000000000")?;
        assert_eq!(known_threads(&dirs, &root, [&id]), Ok(()));
        assert_eq!(
            known_threads(&dirs, &root, [&id, &other]),
            Err(format!("unknown thread(s) {other}"))
        );
        Ok(())
    }

    #[test]
    fn headless_reads_and_answers_the_store() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-headless")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let id = Store::open(dirs.threads_file(&root))?.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(2, 2), "why?"),
            "one\ntwo\n",
            5,
        )?;
        let threads = headless_list(&dirs, &root, None, None)?;
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id(), &id);
        assert!(headless_list(&dirs, &root, Some(6), None)?.is_empty());
        headless_reply(
            &dirs,
            &root,
            &id,
            Author::agent("bot"),
            "because".to_owned(),
            true,
            None,
        )?;
        let again = headless_list(&dirs, &root, None, Some(Path::new("a.md")))?;
        assert_eq!(again[0].replies().len(), 1);
        assert_eq!(again[0].status(), Status::AutoResolved);
        // A directory filter reads every thread under it, and only those.
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/b.md"), "one\ntwo\n")?;
        Store::open(dirs.threads_file(&root))?.annotate(
            Draft::new(Path::new("src/b.md"), LineRange::new(1, 1), "and this?"),
            "one\ntwo\n",
            7,
        )?;
        let under = headless_list(&dirs, &root, None, Some(Path::new("src")))?;
        assert_eq!(
            under.iter().map(super::Thread::path).collect::<Vec<_>>(),
            [Path::new("src/b.md")]
        );
        assert_eq!(headless_list(&dirs, &root, None, None)?.len(), 2);
        assert!(
            headless_reply(
                &dirs,
                &root,
                &serde_json::from_str::<ThreadId>(r#""1-2-3""#)?,
                Author::agent("bot"),
                "x".to_owned(),
                false,
                None,
            )
            .is_err()
        );
        Ok(())
    }

    /// A subscribed connection that calls `follow` again with only
    /// `paths` moves its coverage to them: the register's subscriber
    /// carries the new paths and a thread on the old path is no longer
    /// covered. A connection that never subscribed is left alone.
    #[test]
    fn a_second_follow_updates_the_subscription_paths() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-refollow")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\n")?;
        fs::write(root.join("b.md"), "one\n")?;
        let on_a = {
            let mut store = Store::open(dirs.threads_file(&root))?;
            let id = store.annotate(
                Draft::new(Path::new("a.md"), LineRange::new(1, 1), "why?"),
                "one\n",
                5,
            )?;
            store.thread(&id).cloned().ok_or("thread not stored")?
        };
        let server = Server::new(dirs.clone(), AgentsConfig::default());
        let subscriber = |server: &Server| -> Result<Subscriber, String> {
            server
                .register(&root, 10)?
                .subscriber("s1")
                .cloned()
                .ok_or_else(|| "s1 is not subscribed".to_owned())
        };

        // Not subscribed: paths alone register nothing.
        let note = server.subscription(&root, None, None, None, None, &[PathBuf::from("a.md")])?;
        assert_eq!(note, "");
        assert_eq!(
            subscriber(&server).map(|s| s.id().to_owned()),
            Err("s1 is not subscribed".to_owned())
        );

        let note = server.subscription(
            &root,
            Some("s1".to_owned()),
            Some("coder".to_owned()),
            Some("bot".to_owned()),
            None,
            &[PathBuf::from("a.md")],
        )?;
        assert_eq!(note, "subscribed s1 as coder; ");
        assert!(subscriber(&server)?.covers(&on_a));

        let note = server.subscription(&root, None, None, None, None, &[PathBuf::from("b.md")])?;
        assert_eq!(note, "coverage updated for s1; ");
        let refreshed = subscriber(&server)?;
        assert_eq!(refreshed.paths(), [PathBuf::from("b.md")]);
        assert_eq!(refreshed.kind(), "coder");
        assert_eq!(refreshed.name(), Some("bot"));
        assert!(!refreshed.covers(&on_a));
        Ok(())
    }

    /// Words the agent-facing text may backtick that are not tools or
    /// parameters: the hook, and the config node the guide names.
    const ALLOWED: [&str; 3] = ["hello", "fathomable", "agents.types"];

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
        let live = Server::tool_router().list_all();
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

    #[test]
    fn follow_schema_lists_the_configured_types() -> Result<(), String> {
        let types = ["coder".to_owned(), "qa".to_owned()];
        let tools = with_types(Server::tool_router().list_all(), &types);
        let follow = tools
            .iter()
            .find(|t| t.name == "follow")
            .ok_or("no follow tool")?;
        assert_eq!(
            follow.input_schema["properties"]["type"]["enum"],
            serde_json::json!(["coder", "qa"])
        );
        assert!(
            tools
                .iter()
                .all(|t| t.name == "follow" || t.input_schema["properties"].get("type").is_none())
        );
        Ok(())
    }

    /// The guide's tool table names every tool, nothing else in its
    /// first column, and only known words in its second.
    #[test]
    fn guide_tool_table_matches_the_vocabulary() -> std::io::Result<()> {
        let guide = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide.md");
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
