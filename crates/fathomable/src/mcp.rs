// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! `fathomable --mcp`: a stdio MCP server that drives the viewers of a
//! workspace over their Unix sockets and, when none runs, reads and
//! answers the thread store directly (ADR 0003, ADR 0014, ADR 0024).
//!
//! The server holds one piece of state, the pinned workspace set by
//! `session_switch`. Every call otherwise resolves the workspace afresh: an
//! explicit `session` argument, then the pin, then the known workspace
//! whose root is the longest prefix of the current directory. `open` and
//! `follow` reach every live viewer of that workspace or the one named by
//! `viewer`; `annotations_list` and `thread_reply` go through a viewer when
//! one runs and to the store on disk when none does. Client identity is
//! read from the request context on each call, so the server behaves the
//! same under the legacy `initialize` flow and discovery-first startup.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, LineRange, Reply, Scope, Store, Thread, ThreadId};
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

/// Run the server on stdin/stdout until the client disconnects.
pub fn run(dirs: &XdgDirs) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(async {
        let server = Server::new(dirs.clone());
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
    pinned: Mutex<Option<PathBuf>>,
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
    /// Last line, when a range should be selected.
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
    /// Workspace-relative paths you are working on; replaces the last list.
    paths: Vec<PathBuf>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    session: Option<String>,
    /// Viewer name or id to tell; every viewer when omitted.
    #[serde(default)]
    viewer: Option<String>,
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
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    session: Option<String>,
}

/// `thread_reply` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplyParams {
    /// Thread id from `annotations_list`.
    thread: String,
    /// Reply text.
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
    /// Name to sign as; the client name is recorded alongside it.
    #[serde(default)]
    persona: Option<String>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    session: Option<String>,
}

#[tool_router]
impl Server {
    fn new(dirs: XdgDirs) -> Self {
        let cwd = env::current_dir().unwrap_or_default();
        if let Some(session) = bind(&sessions(&dirs), &cwd) {
            tracing::info!(root = %session.root.display(), viewers = session.viewers.len(), "workspace contains the cwd");
        } else {
            tracing::warn!(cwd = %cwd.display(), "no known workspace contains the cwd");
        }
        Self {
            dirs,
            pinned: Mutex::new(None),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List known workspaces and their running viewers; the default workspace is marked."
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

    #[tool(description = "Make a workspace the default for later calls.")]
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
        description = "Open a workspace file in the viewer(s), optionally at a line or line range."
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
        description = "Tell the viewer(s) which files you are working on; replaces the previous list."
    )]
    async fn follow(&self, Parameters(p): Parameters<FollowParams>) -> CallToolResult {
        let files = p.paths.len();
        let request = Request::Follow { paths: p.paths };
        match self
            .broadcast(p.session.as_deref(), p.viewer.as_deref(), &request)
            .await
        {
            Ok(count) => text(format!("following {files} file(s) in {count} viewer(s)")),
            Err(error) => failure(error),
        }
    }

    #[tool(
        description = "List annotation threads on the current work, optionally changed since a Unix time or on one file."
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
            Ok(Response::Threads(threads)) => {
                let summary = summarize(&threads);
                match serde_json::to_value(&threads) {
                    Ok(value) => with_summary(json!({ "threads": value }), summary),
                    Err(error) => failure(error.to_string()),
                }
            }
            other => unexpected(other),
        }
    }

    #[tool(
        description = "Reply to a thread, optionally resolving it. If you rewrote the lines \
                       the thread is on, pass `line` (and `end_line`) so it follows them."
    )]
    async fn thread_reply(
        &self,
        Parameters(p): Parameters<ReplyParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let client = context.client_info().map(|c| c.name);
        let author = Author::Agent {
            name: p
                .persona
                .or_else(|| client.clone())
                .unwrap_or_else(|| "agent".to_owned()),
            client,
        };
        let thread: ThreadId = match serde_json::from_value(Value::String(p.thread.clone())) {
            Ok(id) => id,
            Err(error) => return failure(error.to_string()),
        };
        let session = match self.resolve(p.session.as_deref()) {
            Ok(session) => session,
            Err(error) => return failure(error),
        };
        let lines = p
            .line
            .map(|line| LineRange::new(line, p.end_line.unwrap_or(line)));
        let request = Request::ThreadReply {
            thread: thread.clone(),
            author: author.clone(),
            body: p.body.clone(),
            resolve: p.resolve,
            lines,
        };
        let outcome = match session.viewers.first() {
            Some(viewer) => call(viewer, &request).await,
            None => headless_reply(
                &self.dirs,
                &session.root,
                &thread,
                author,
                p.body,
                p.resolve,
                lines,
            )
            .map(|()| Response::Done),
        };
        match outcome {
            Ok(Response::Done) => text(format!(
                "replied to {}{}",
                p.thread,
                if p.resolve { " and resolved it" } else { "" }
            )),
            other => unexpected(other),
        }
    }
}

impl Server {
    fn pinned(&self) -> Option<PathBuf> {
        self.pinned.lock().ok().and_then(|p| p.clone())
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
                "Fathomable is the user's read-only viewer. Use `open` to show a file, \
                 `follow` to say which files you are editing, `annotations_list` (with \
                 `since`) to read the user's comments, and `thread_reply` to answer them. \
                 Several viewers may show one workspace; `open` and `follow` reach all of \
                 them unless you name a `viewer`. Reading and answering comments works \
                 with no viewer running.",
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
