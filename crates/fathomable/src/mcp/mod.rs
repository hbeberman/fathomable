// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! The repository-bound stdio MCP review server.
//!
//! Startup binds one server to the checkout containing `--mcp DIR`, or
//! the process working directory when `DIR` is omitted. Calls cannot route
//! to another checkout. Reads use the shared non-archived board directly and
//! project placement against the bound checkout; writes use a viewer already
//! on that checkout when one is available, and otherwise write the shared
//! store directly.

mod identity;
mod start;
mod tools;

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Store, Thread};
use fathomable_core::session::{Record, Request, Response};
use fathomable_core::workspace::Workspace;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CacheScope, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ResultType, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool_handler};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use identity::Launch;

/// Run the server on stdin/stdout until the client disconnects.
pub(crate) fn run(dirs: &XdgDirs, root: Option<&Path>) -> anyhow::Result<()> {
    let directory = match root {
        Some(root) => root
            .canonicalize()
            .context("cannot resolve MCP repository directory")?,
        None => env::current_dir().context("cannot read MCP startup directory")?,
    };
    anyhow::ensure!(directory.is_dir(), "MCP repository must be a directory");
    let target = Target::discover(&directory)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(async {
        let server = Server::new(dirs.clone(), target, Launch::from_env());
        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .context("MCP handshake failed")?;
        let reason = service.waiting().await.context("MCP server task failed")?;
        tracing::info!(?reason, "MCP server stopped");
        Ok(())
    })
}

/// The tool server and its immutable checkout binding.
pub(crate) struct Server {
    dirs: XdgDirs,
    target: Target,
    launch: Launch,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("root", &self.target.root)
            .finish_non_exhaustive()
    }
}

/// The checkout one MCP process serves.
#[derive(Debug, Clone)]
struct Target {
    key: PathBuf,
    root: PathBuf,
}

impl Target {
    fn discover(directory: &Path) -> anyhow::Result<Self> {
        let workspace = Workspace::discover(directory).context("cannot discover MCP repository")?;
        let target = Self {
            key: workspace.key().to_path_buf(),
            root: workspace.root().to_path_buf(),
        };
        tracing::info!(
            root = %target.root.display(),
            key = %target.key.display(),
            "MCP bound to checkout"
        );
        Ok(target)
    }

    /// A live viewer already showing `root`, when one exists.
    fn viewer(&self, dirs: &XdgDirs, root: &Path) -> Option<Record> {
        Record::live(dirs)
            .into_iter()
            .find(|viewer| viewer.key() == self.key && viewer.root() == root)
    }
}

impl Server {
    fn new(dirs: XdgDirs, target: Target, launch: Launch) -> Self {
        Self {
            dirs,
            target,
            launch,
            tool_router: Self::router(),
        }
    }

    fn router() -> ToolRouter<Self> {
        Self::tool_router() + Self::tool_router_start()
    }

    /// Resolve the automatic caller identity into a stored annotation author
    /// and its stable harness-qualified scope.
    fn signer(&self, context: &RequestContext<RoleServer>) -> Result<(Author, String), String> {
        let caller = self.launch.require(context)?;
        let scope = caller.id.clone();
        Ok((
            Author::Agent {
                name: caller.harness.name().to_owned(),
                client: Some(caller.client),
                id: Some(caller.id),
            },
            scope,
        ))
    }

    /// Every non-archived thread on the shared discussion board.
    fn fetch(&self) -> Result<Vec<fathomable_core::annotations::Thread>, String> {
        headless_list(&self.dirs, &self.target)
    }

    /// Every stored thread, for an explicit id lookup that must remain
    /// reliable after ordinary checkout/status visibility changes.
    fn fetch_exact(&self) -> Result<Vec<Thread>, String> {
        let store = headless_store(&self.dirs, &self.target)?;
        Ok(store.all_threads().cloned().collect())
    }
}

/// One request-response exchange with a viewer's socket.
async fn call(viewer: &Record, request: &Request) -> Result<Response, String> {
    let socket = viewer
        .socket()
        .ok_or_else(|| format!("viewer {} has no socket", viewer.id()))?;
    exchange(socket, &request.to_line())
        .await
        .map_err(|error| format!("viewer {}: {error}", viewer.id()))
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

/// Open the shared store without persisting thread housekeeping.
fn headless_store(dirs: &XdgDirs, target: &Target) -> Result<Store, String> {
    Store::open(dirs.threads_file(&target.key)).map_err(|error| error.to_string())
}

/// Every non-archived thread on the repository discussion board.
fn headless_list(
    dirs: &XdgDirs,
    target: &Target,
) -> Result<Vec<fathomable_core::annotations::Thread>, String> {
    Ok(headless_store(dirs, target)?.threads().to_vec())
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
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
            ttl_ms: cache_hints.then_some(0),
            cache_scope: cache_hints.then_some(CacheScope::Public),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use fathomable_testing::TempDir;

    use super::Target;

    #[test]
    fn binding_accepts_a_plain_repository_directory() -> anyhow::Result<()> {
        let dir = TempDir::new("mcp-binding")?;
        let root = dir.0.join("repo");
        fs::create_dir_all(root.join("src"))?;
        let target = Target::discover(&root)?;
        assert_eq!(target.root, root.canonicalize()?);
        assert_eq!(target.key, target.root);
        Ok(())
    }
}
