// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! `fathomable --mcp`: a stdio MCP server that forwards tool calls to a
//! running session over its Unix socket (ADR 0003, ADR 0014).
//!
//! The server holds one piece of state, the default session, chosen at
//! startup by the longest workspace root containing the current directory
//! and changed by `session_switch`. Every other tool is a pure function of
//! its arguments plus the socket reply, and client identity is read from
//! the request context on each call, so the server behaves the same under
//! the legacy `initialize` flow and discovery-first startup.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Thread, ThreadId};
use fathomable_core::session::{Id, Record, Request, Response};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

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
    default: Mutex<Option<Id>>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").finish_non_exhaustive()
    }
}

/// `session_switch` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SwitchParams {
    /// Session id as shown by `session_list`.
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
    /// Session id; defaults to the bound session.
    #[serde(default)]
    session: Option<String>,
}

/// `follow` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FollowParams {
    /// Workspace-relative paths you are working on; replaces the last list.
    paths: Vec<PathBuf>,
    /// Session id; defaults to the bound session.
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
    /// Session id; defaults to the bound session.
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
    /// Name to sign as; the client name is recorded alongside it.
    #[serde(default)]
    persona: Option<String>,
    /// Session id; defaults to the bound session.
    #[serde(default)]
    session: Option<String>,
}

#[tool_router]
impl Server {
    fn new(dirs: XdgDirs) -> Self {
        let cwd = env::current_dir().unwrap_or_default();
        let default = bind(&Record::list(&dirs), &cwd).map(|r| r.id().clone());
        if let Some(id) = &default {
            tracing::info!(session = %id, "bound to session");
        } else {
            tracing::warn!(cwd = %cwd.display(), "no live session contains the cwd");
        }
        Self {
            dirs,
            default: Mutex::new(default),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List running Fathomable sessions; the default is marked.")]
    fn session_list(&self) -> CallToolResult {
        let default = self.default_id();
        let live: Vec<Record> = Record::list(&self.dirs)
            .into_iter()
            .filter(Record::is_alive)
            .collect();
        let sessions: Vec<Value> = live
            .iter()
            .map(|r| {
                json!({
                    "id": r.id().as_str(),
                    "root": r.root(),
                    "started": r.started(),
                    "default": default.as_ref() == Some(r.id()),
                })
            })
            .collect();
        let summary = if live.is_empty() {
            "no live sessions".to_owned()
        } else {
            live.iter()
                .map(|r| {
                    let mark = if default.as_ref() == Some(r.id()) {
                        "*"
                    } else {
                        " "
                    };
                    format!("{mark} {}  {}", r.id(), r.root().display())
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        with_summary(json!({ "sessions": sessions }), summary)
    }

    #[tool(description = "Make a session the default for later calls.")]
    fn session_switch(&self, Parameters(p): Parameters<SwitchParams>) -> CallToolResult {
        match self.record(Some(&p.session)) {
            Ok(record) => {
                if let Ok(mut default) = self.default.lock() {
                    *default = Some(record.id().clone());
                }
                text(format!(
                    "bound to {} at {}",
                    record.id(),
                    record.root().display()
                ))
            }
            Err(error) => failure(error),
        }
    }

    #[tool(
        description = "Open a workspace file in the viewer, optionally at a line or line range."
    )]
    async fn open(&self, Parameters(p): Parameters<OpenParams>) -> CallToolResult {
        let request = Request::Open {
            path: p.path.clone(),
            line: p.line,
            end_line: p.end_line,
        };
        match self.call(p.session.as_deref(), &request).await {
            Ok(Response::Done) => text(format!("opened {}", p.path.display())),
            other => unexpected(other),
        }
    }

    #[tool(
        description = "Tell the viewer which files you are working on; replaces the previous list."
    )]
    async fn follow(&self, Parameters(p): Parameters<FollowParams>) -> CallToolResult {
        let count = p.paths.len();
        let request = Request::Follow { paths: p.paths };
        match self.call(p.session.as_deref(), &request).await {
            Ok(Response::Done) => text(format!("following {count} file(s)")),
            other => unexpected(other),
        }
    }

    #[tool(
        description = "List annotation threads, optionally changed since a Unix time or on one file."
    )]
    async fn annotations_list(&self, Parameters(p): Parameters<ListParams>) -> CallToolResult {
        let request = Request::AnnotationsList {
            since: p.since,
            path: p.path,
        };
        match self.call(p.session.as_deref(), &request).await {
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

    #[tool(description = "Reply to a thread, optionally resolving it.")]
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
        let request = Request::ThreadReply {
            thread,
            author,
            body: p.body,
            resolve: p.resolve,
        };
        match self.call(p.session.as_deref(), &request).await {
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
    fn default_id(&self) -> Option<Id> {
        self.default.lock().ok().and_then(|d| d.clone())
    }

    /// The live record for `session`, or the default one.
    fn record(&self, session: Option<&str>) -> Result<Record, String> {
        let id = match session {
            Some(text) => text.parse::<Id>().map_err(|e| e.to_string())?,
            None => self
                .default_id()
                .ok_or("no session bound; call session_list and session_switch")?,
        };
        Record::list(&self.dirs)
            .into_iter()
            .find(|r| *r.id() == id)
            .filter(Record::is_alive)
            .ok_or_else(|| format!("session {id} is not running"))
    }

    /// One request-response exchange with the session socket.
    async fn call(&self, session: Option<&str>, request: &Request) -> Result<Response, String> {
        let record = self.record(session)?;
        let socket = record
            .socket()
            .ok_or_else(|| format!("session {} has no socket", record.id()))?;
        exchange(socket, &request.to_line())
            .await
            .map_err(|e| format!("session {}: {e}", record.id()))
    }
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

/// The live session whose root is the longest prefix of `cwd`.
fn bind<'a>(records: &'a [Record], cwd: &Path) -> Option<&'a Record> {
    records
        .iter()
        .filter(|r| r.is_alive() && cwd.starts_with(r.root()))
        .max_by_key(|r| r.root().as_os_str().len())
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
                 `since`) to read the user's comments, and `thread_reply` to answer them.",
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use fathomable_core::session::{Id, Record};

    use super::bind;

    fn record(root: &str) -> Record {
        Record::new(Id::mint(), PathBuf::from(root), None)
    }

    #[test]
    fn binding_picks_the_longest_live_root() {
        let records = [record("/work"), record("/work/repo"), record("/elsewhere")];
        let bound = bind(&records, Path::new("/work/repo/src"));
        assert_eq!(bound.map(Record::root), Some(Path::new("/work/repo")));
        assert!(bind(&records, Path::new("/tmp")).is_none());
    }
}
