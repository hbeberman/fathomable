// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! `fathomable --mcp`: a stdio MCP server that drives the viewers of a
//! workspace over their Unix sockets and, when none runs, reads and
//! answers the thread store directly (ADR 0003, ADR 0014, ADR 0024).
//!
//! The server holds two pieces of state: the pinned workspace set by
//! `session_switch`, and the subscriber this connection last registered
//! with `follow` (ADR 0040), which signs its replies and is the default
//! `id` of the subscription tools. Every call otherwise resolves the
//! workspace afresh: an
//! explicit `session` argument, then the pin, then the known workspace
//! whose root is the longest prefix of the current directory. `open` and
//! `follow` reach every live viewer of that workspace or the one named by
//! `viewer`; `annotations_list` and `thread_reply` go through a viewer when
//! one runs and to the store on disk when none does. Client identity is
//! read from the request context on each call, so the server behaves the
//! same under the legacy `initialize` flow and discovery-first startup.

use std::env;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::agents::{Register, WatchWhen};
use fathomable_core::annotations::{Author, LineRange, Reply, Scope, Store, Thread, ThreadId};
use fathomable_core::bond::{self, Process};
use fathomable_core::config::AgentsConfig;
use fathomable_core::seen;
use fathomable_core::session::{Marker, Record, Request, Response};
use fathomable_core::workspace::Workspace;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::app::open_thread::follow_reply_lines;
use crate::app::reanchor::follow_snapshots;
use crate::app::rescope;
use crate::app::threads::now;
use crate::hooks;

/// Run the server on stdin/stdout until the client disconnects.
pub fn run(dirs: &XdgDirs, agents: AgentsConfig) -> anyhow::Result<()> {
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
pub struct Server {
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
struct Session {
    root: PathBuf,
    viewers: Vec<Record>,
}

/// `session_switch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SwitchParams {
    /// A workspace root as shown by `session_list`, or a viewer name or id
    /// (which selects that viewer's workspace).
    session: String,
}

/// `open` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenParams {
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
    session: Option<String>,
    /// Viewer name or id to show the file in; every viewer when omitted.
    #[serde(default)]
    viewer: Option<String>,
}

/// `follow` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FollowParams {
    /// Workspace-relative paths you are working on; replaces the last
    /// list. Empty means the whole workspace.
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
    session: Option<String>,
    /// Viewer name or id to tell; every viewer when omitted.
    #[serde(default)]
    viewer: Option<String>,
}

/// `unfollow` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UnfollowParams {
    /// The session id you subscribed with.
    id: String,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    session: Option<String>,
}

/// `annotations_list` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListParams {
    /// Only threads changed at or after this Unix time in seconds.
    #[serde(default)]
    since: Option<u64>,
    /// Only threads on this workspace-relative path.
    #[serde(default)]
    path: Option<PathBuf>,
    /// At most this many threads, oldest change first; default 50. The
    /// summary says how many more there are and the `since` to pass.
    #[serde(default)]
    limit: Option<usize>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    session: Option<String>,
}

/// `threads_pending` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PendingParams {
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
    session: Option<String>,
}

/// One reply in a `thread_reply` batch.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct ReplyItem {
    /// Thread id from `annotations_list` or `threads_pending`.
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
pub struct ReplyParams {
    /// Thread id from `annotations_list` or `threads_pending`, for a
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
    session: Option<String>,
}

/// `thread_watch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WatchParams {
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
    session: Option<String>,
}

/// `thread_unwatch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UnwatchParams {
    /// The watched thread.
    on: String,
    /// The session id you subscribed with; defaults to this connection's.
    #[serde(default)]
    id: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    session: Option<String>,
}

#[tool_router]
impl Server {
    fn new(dirs: XdgDirs, agents: AgentsConfig) -> Self {
        let cwd = env::current_dir().unwrap_or_default();
        if let Some(session) = bind(&sessions(&dirs), &cwd) {
            tracing::info!(root = %session.root.display(), viewers = session.viewers.len(), "workspace contains the cwd");
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
                       `session_switch`. Returns roots and viewer names, not threads.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    fn session_list(&self) -> CallToolResult {
        let all = sessions(&self.dirs);
        let default = self.resolve(None).ok().map(|s| s.root);
        let sessions: Vec<Value> = all
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
        with_summary(json!({ "sessions": sessions }), summary)
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
    fn session_switch(&self, Parameters(p): Parameters<SwitchParams>) -> CallToolResult {
        match self.resolve(Some(&p.session)) {
            Ok(session) => {
                if let Ok(mut pinned) = self.pinned.lock() {
                    *pinned = Some(session.root.clone());
                }
                text(format!(
                    "bound to {} ({} viewer(s) running)",
                    session.root.display(),
                    session.viewers.len()
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
            .broadcast(p.session.as_deref(), p.viewer.as_deref(), &request)
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
                       stop hook and `threads_pending` then hand you every thread on these \
                       files, or that you posted in, whose newest message is someone else's, \
                       once each. Call it once at the start and again when your files change; \
                       works with no viewer running.",
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
        let session = match self.resolve(p.session.as_deref()) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let mut note = String::new();
        let bonded = match (&p.id, &p.kind) {
            (None, Some(_)) => self
                .register(&session.root, now())
                .ok()
                .and_then(|r| r.session_for(&self.ancestors).map(str::to_owned)),
            _ => None,
        };
        let id = p.id.clone().or_else(|| bonded.clone());
        match (&id, &p.kind) {
            (Some(id), Some(kind)) => {
                if !self.agents.allows(kind) {
                    return failure(format!(
                        "unknown agent type `{kind}`; configured types: {}",
                        self.agents.types.join(", ")
                    ));
                }
                let client = context.client_info().map(|c| c.name);
                let when = now();
                let outcome = self.register(&session.root, when).and_then(|mut register| {
                    register
                        .subscribe(
                            id,
                            kind,
                            p.persona.as_deref(),
                            client.as_deref(),
                            p.paths.clone(),
                            when,
                        )
                        .map_err(|e| e.to_string())
                });
                if let Err(error) = outcome {
                    return failure(error);
                }
                if let Ok(mut current) = self.subscriber.lock() {
                    *current = Some((id.clone(), kind.clone(), p.persona.clone()));
                }
                note = if bonded.is_some() {
                    format!("subscribed {id} (your session, found from the harness) as {kind}; ")
                } else {
                    format!("subscribed {id} as {kind}; ")
                };
            }
            (Some(_), None) => {
                return failure(format!(
                    "`id` needs a `type`; configured types: {}",
                    self.agents.types.join(", ")
                ));
            }
            (None, Some(_)) => {
                return failure(
                    "`type` needs the session `id` the hello hook gave you; this session \
                     could not be told from the harness",
                );
            }
            (None, None) => {}
        }
        let files = p.paths.len();
        if session.viewers.is_empty() && p.viewer.is_none() {
            return text(format!(
                "{note}following {files} file(s); no viewer is running"
            ));
        }
        let request = Request::Follow { paths: p.paths };
        match self
            .broadcast(p.session.as_deref(), p.viewer.as_deref(), &request)
            .await
        {
            Ok(count) => text(format!(
                "{note}following {files} file(s) in {count} viewer(s)"
            )),
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
        let session = match self.resolve(p.session.as_deref()) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let when = now();
        let outcome = self.register(&session.root, when).and_then(|mut register| {
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
                       beyond the annotated snippet.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn annotations_list(&self, Parameters(p): Parameters<ListParams>) -> CallToolResult {
        let session = match self.resolve(p.session.as_deref()) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let request = Request::AnnotationsList {
            since: p.since,
            path: p.path.clone(),
        };
        let outcome = match session.viewers.first() {
            Some(viewer) => call(viewer, &request).await,
            None => headless_list(&self.dirs, &session.root, p.since, p.path.as_deref())
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
                       shown to you yet, plus any watch that fired. Each is returned once; \
                       call it when the hello hook or a stop hook tells you to, or before you \
                       finish. Needs a subscription from `follow` with `id` and `type`. Answer \
                       what it returns with one `thread_reply` call.",
        annotations(destructive_hint = false, open_world_hint = false)
    )]
    fn threads_pending(&self, Parameters(p): Parameters<PendingParams>) -> CallToolResult {
        let session = match self.resolve(p.session.as_deref()) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let when = now();
        let mut register = match self.register(&session.root, when) {
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
        let threads = match headless_store(&self.dirs, &session.root) {
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
        let session = match self.resolve(p.session.as_deref()) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let client = context.client_info().map(|c| c.name);
        let subscription = self.signature(p.id, &session.root);
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
            match self.reply_one(&session, author.clone(), item).await {
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
                       on a discussion elsewhere. Needs a subscription.",
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
        self.with_subscriber(p.session.as_deref(), p.id, |register, id, now| {
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
        self.with_subscriber(p.session.as_deref(), p.id, |register, id, now| {
            register
                .unwatch(id, &on, now)
                .map(|()| format!("no longer watching {}", p.on))
        })
    }
}

/// `annotations_list` threads per call unless `limit` says otherwise.
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
        session: Option<&str>,
        id: Option<String>,
        act: impl FnOnce(
            &mut Register,
            &str,
            u64,
        ) -> Result<String, fathomable_core::agents::RegisterError>,
    ) -> CallToolResult {
        let session = match self.resolve(session) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let when = now();
        let mut register = match self.register(&session.root, when) {
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
        session: &Session,
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
        let outcome = match session.viewers.first() {
            Some(viewer) => call(viewer, &request).await,
            None => headless_reply(
                &self.dirs,
                &session.root,
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

    /// The workspace a call addresses: `session` when given (a root, or a
    /// viewer name or id), else the pin, else the one containing the cwd.
    fn resolve(&self, session: Option<&str>) -> Result<Session, String> {
        let all = sessions(&self.dirs);
        if let Some(key) = session {
            if let Some(found) = all
                .iter()
                .find(|s| s.viewers.iter().any(|v| v.is_called(key)))
            {
                return Ok(found.clone());
            }
            let path = PathBuf::from(key);
            let path = path.canonicalize().unwrap_or(path);
            return all
                .iter()
                .find(|s| s.root == path)
                .cloned()
                .ok_or_else(|| format!("no workspace or viewer called `{key}`; see session_list"));
        }
        if let Some(pinned) = self.pinned()
            && let Some(found) = all.iter().find(|s| s.root == pinned)
        {
            return Ok(found.clone());
        }
        let cwd = env::current_dir().unwrap_or_default();
        bind(&all, &cwd).cloned().ok_or_else(|| {
            "no known workspace contains the current directory; call session_list and session_switch"
                .to_owned()
        })
    }

    /// Send `request` to every viewer of the workspace, or to the one
    /// called `viewer`. Returns how many answered `Done`; the first error
    /// fails the call.
    async fn broadcast(
        &self,
        session: Option<&str>,
        viewer: Option<&str>,
        request: &Request,
    ) -> Result<usize, String> {
        let session = self.resolve(session)?;
        let targets: Vec<&Record> = match viewer {
            Some(key) => {
                let found = session
                    .viewers
                    .iter()
                    .find(|v| v.is_called(key))
                    .ok_or_else(|| {
                        format!(
                            "no viewer called `{key}` on {}; see session_list",
                            session.root.display()
                        )
                    })?;
                vec![found]
            }
            None => session.viewers.iter().collect(),
        };
        if targets.is_empty() {
            return Err(format!(
                "no viewer is running for {}; start Fathomable there",
                session.root.display()
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
fn sessions(dirs: &XdgDirs) -> Vec<Session> {
    let mut all: Vec<Session> = Marker::list(dirs)
        .into_iter()
        .map(|marker| Session {
            root: marker.root().to_path_buf(),
            viewers: Vec::new(),
        })
        .collect();
    for record in Record::live(dirs) {
        match all.iter_mut().find(|s| s.root == record.root()) {
            Some(session) => session.viewers.push(record),
            None => all.push(Session {
                root: record.root().to_path_buf(),
                viewers: vec![record],
            }),
        }
    }
    all
}

/// The workspace whose root is the longest prefix of `cwd`.
fn bind<'a>(sessions: &'a [Session], cwd: &Path) -> Option<&'a Session> {
    sessions
        .iter()
        .filter(|s| cwd.starts_with(&s.root))
        .max_by_key(|s| s.root.as_os_str().len())
}

/// Open the workspace's store with no viewer running: threads edited
/// offline are followed through their snapshots first (ADR 0020), and the
/// scope of the current `HEAD` is computed (ADR 0024).
fn headless_store(dirs: &XdgDirs, root: &Path) -> Result<(Store, Scope), String> {
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
                let moved = rescope::follow_head(&mut store, &workspace, &reachable);
                if moved > 0 {
                    tracing::info!(moved, "threads rescoped headlessly");
                    reachable.extend(workspace.head_commit());
                }
                Scope::reachable(reachable)
            }
            None => Scope::unscoped(),
        },
        Err(error) => {
            tracing::warn!(%error, "cannot open the workspace; threads unscoped");
            Scope::unscoped()
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
        .filter(|t| path.is_none_or(|p| t.path() == p))
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

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fathomable", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Fathomable is the user's read-only viewer, where they leave review comments \
                 on the lines you write. Start with `follow` naming the files you will edit \
                 and a `type` to subscribe, so `threads_pending` and the stop hook hand you \
                 each new comment once; add the session `id` the hello hook gave you when \
                 asked for it, and pass it to `thread_reply` too if you never called \
                 `follow` on this connection. Answer with one `thread_reply` carrying \
                 `replies`. `annotations_list` reads any \
                 thread; `open` shows a file in the viewer. Everything but `open` works with \
                 no viewer running. `thread_watch` wakes you when another thread moves.",
            )
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::XdgDirs;
    use fathomable_core::annotations::{Author, Draft, LineRange, Status, Store, ThreadId};
    use fathomable_core::session::{Id, Record};

    use super::{Session, bind, headless_list, headless_reply};

    fn session(root: &str, viewers: usize) -> Session {
        Session {
            root: PathBuf::from(root),
            viewers: (0..viewers)
                .map(|_| Record::new(Id::mint(), PathBuf::from(root), None))
                .collect(),
        }
    }

    #[test]
    fn binding_picks_the_longest_root_even_without_viewers() {
        let sessions = [
            session("/work", 1),
            session("/work/repo", 0),
            session("/x", 2),
        ];
        let bound = bind(&sessions, Path::new("/work/repo/src"));
        assert_eq!(
            bound.map(|s| s.root.as_path()),
            Some(Path::new("/work/repo"))
        );
        assert!(bind(&sessions, Path::new("/tmp")).is_none());
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir =
                std::env::temp_dir().join(format!("fathomable-mcp-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
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

    /// With no viewer, an agent still reads the store and its reply lands
    /// in the file the next viewer loads.
    #[test]
    fn headless_reads_and_answers_the_store() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("headless")?;
        let dirs = dir.dirs();
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
}
