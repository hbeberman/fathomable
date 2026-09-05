// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! `fathomable --mcp`: a stdio MCP server that drives the viewers of a
//! workspace over their Unix sockets and, when none runs, reads and
//! answers the thread store directly (ADR 0003, ADR 0014, ADR 0024).
//!
//! This module is the transport: the server and its state, how a call
//! finds its workspace, the socket exchange with a viewer, and the store
//! it falls back to. The tools themselves are in [`tools`] (ADR 0055)
//! and, for `thread_start`, in [`start`] (ADR 0061).
//!
//! The server holds two pieces of state: the pinned workspace set by
//! `workspaces`, and the subscriber this connection last registered
//! with `follow` (ADR 0040), which signs its replies and is the default
//! `id` of the subscription tools. Every call otherwise resolves the
//! workspace afresh: an explicit `workspace` argument, then the pin, then
//! the known workspace whose root is the longest prefix of the current
//! directory. `open` reaches every live viewer of that workspace or the
//! one named by `viewer`; `threads` and `thread_reply` go through a
//! viewer when one runs and to the store on disk when none does. Client
//! identity is read from the request context on each call, so the server
//! behaves the same under the legacy `initialize` flow and discovery-first
//! startup.

mod start;
mod tools;

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::agents::Register;
use fathomable_core::annotations::{Author, Reach, Store};
use fathomable_core::bond::{self, Process};
use fathomable_core::config::AgentsConfig;
use fathomable_core::identity;
use fathomable_core::seen;
use fathomable_core::session::{Marker, Record, Request, Response};
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::Workspace;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CacheScope, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ResultType, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool_handler};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::app::threads::reach;
use crate::app::threads::reanchor::follow_snapshots;
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

/// How a message from this connection is signed, and whether a
/// subscription stands behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signature {
    author: Author,
    subscribed: bool,
}

/// A workspace an agent can address: its root and the viewers showing it.
#[derive(Debug, Clone)]
struct Target {
    root: PathBuf,
    viewers: Vec<Record>,
}

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
            tool_router: Self::router(),
        }
    }

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

    /// Every tool: the six of ADR 0055 and `thread_start` (ADR 0061).
    fn router() -> ToolRouter<Self> {
        Self::tool_router() + Self::tool_router_start()
    }

    /// Who a reply or a comment from this connection is: the name fixed
    /// at `follow`, the subscription's id and type when there is one, and
    /// the harness's name otherwise (ADR 0058).
    fn signer(&self, given: Option<String>, root: &Path, client: Option<String>) -> Signature {
        let subscription = self.signature(given, root);
        let mut author = Author::Agent {
            name: subscription
                .as_ref()
                .and_then(|(.., name)| name.clone())
                .unwrap_or_else(|| identity::agent_name(None, client.as_deref())),
            client,
            id: None,
            kind: None,
        };
        if let Some((id, kind, _)) = &subscription {
            author = author.subscribed(id, kind);
        }
        Signature {
            author,
            subscribed: subscription.is_some(),
        }
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
                format!(
                    "no workspace or viewer called `{key}`; call `{}` to see them",
                    vocab::WORKSPACES.name
                )
            });
        }
        if let Some(pinned) = self.pinned()
            && let Some(found) = all.iter().find(|s| s.root == pinned)
        {
            return Ok(found.clone());
        }
        let cwd = env::current_dir().unwrap_or_default();
        bind(&all, &cwd).cloned().ok_or_else(|| {
            format!(
                "no known workspace contains the current directory; call `{ws}` to see them, \
                 then `{ws}` with `{switch}` to pin one",
                ws = vocab::WORKSPACES.name,
                switch = vocab::SWITCH,
            )
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
                            "no viewer called `{key}` on {}; call `{}` to see them",
                            target.root.display(),
                            vocab::WORKSPACES.name
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

/// The target whose root is the longest prefix of `cwd`, if any.
fn bind<'a>(targets: &'a [Target], cwd: &Path) -> Option<&'a Target> {
    targets
        .iter()
        .filter(|s| cwd.starts_with(&s.root))
        .max_by_key(|s| s.root.as_os_str().len())
}

/// The store of `root` with its threads re-anchored to the files as they
/// are now, and the git reach that says which threads the checkout
/// shows; what the tools read when no viewer runs.
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
            .with_instructions(tools::instructions())
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
    use std::path::{Path, PathBuf};

    use fathomable_core::session::{Id, Record};

    use super::{Server, Target, bind, with_types};

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

    #[test]
    fn follow_schema_lists_the_configured_types() -> Result<(), String> {
        let types = ["coder".to_owned(), "qa".to_owned()];
        let tools = with_types(Server::router().list_all(), &types);
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
}
