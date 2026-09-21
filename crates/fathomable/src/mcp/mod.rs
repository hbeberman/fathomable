// @okf-doc: /decisions/0089-store-only-mcp.md
//! The repository-bound stdio MCP review server.
//!
//! Startup normally binds one server to the checkout containing `--mcp DIR`,
//! or the process working directory when `DIR` is omitted. The explicit
//! `--allow-mutable-mcp-root` mode instead lets each call select a checkout.

mod identity;
mod source;
mod start;
mod tools;

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Context;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Store, Thread};
use fathomable_core::workspace::Workspace;
use identity::Launch;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CacheScope, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ResultType, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt, tool_handler};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootMode {
    Fixed,
    PerCall,
}

/// Run the server on stdin/stdout until the client disconnects.
pub(crate) fn run(dirs: &XdgDirs, root: Option<&Path>, root_mode: RootMode) -> anyhow::Result<()> {
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
        let server = Server::new(dirs.clone(), target, Launch::from_env(), root_mode);
        let service = server
            .serve(rmcp::transport::stdio())
            .await
            .context("MCP handshake failed")?;
        let reason = service.waiting().await.context("MCP server task failed")?;
        tracing::info!(?reason, "MCP server stopped");
        Ok(())
    })
}

/// The tool server and its default checkout.
pub(crate) struct Server {
    dirs: XdgDirs,
    target: Target,
    root_mode: RootMode,
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

/// The repository identity and checkout selected at startup or for one call.
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
}

impl Server {
    fn new(dirs: XdgDirs, target: Target, launch: Launch, root_mode: RootMode) -> Self {
        Self {
            dirs,
            target,
            root_mode,
            launch,
            tool_router: Self::router(),
        }
    }

    fn router() -> ToolRouter<Self> {
        Self::tool_router() + Self::tool_router_start()
    }

    fn target(&self, workspace: Option<&Path>) -> Result<Target, String> {
        match workspace {
            Some(_) if self.root_mode == RootMode::Fixed => Err(
                "`workspace` requires starting Fathomable with `--allow-mutable-mcp-root`"
                    .to_owned(),
            ),
            Some(workspace) => Target::discover(workspace).map_err(|error| error.to_string()),
            None => Ok(self.target.clone()),
        }
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
    fn fetch(&self, target: &Target) -> Result<Vec<fathomable_core::annotations::Thread>, String> {
        headless_list(&self.dirs, target)
    }

    /// Every stored thread, for an explicit id lookup that must remain
    /// reliable after ordinary checkout/status visibility changes.
    fn fetch_exact(&self, target: &Target) -> Result<Vec<Thread>, String> {
        let store = headless_store(&self.dirs, target)?;
        Ok(store.all_threads().cloned().collect())
    }
}

/// Open the shared store without persisting thread housekeeping.
fn headless_store(dirs: &XdgDirs, target: &Target) -> Result<Store, String> {
    Store::open_workspace(dirs, &target.key).map_err(|error| error.to_string())
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
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
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
        let mut tools = self.tool_router.list_all();
        for tool in &mut tools {
            let schema = std::sync::Arc::make_mut(&mut tool.input_schema);
            if let Some(serde_json::Value::Object(properties)) = schema.get_mut("properties") {
                if self.root_mode == RootMode::PerCall {
                    if let Some(serde_json::Value::Object(workspace)) =
                        properties.get_mut("workspace")
                    {
                        workspace.insert("description".to_owned(), serde_json::json!(format!(
                            "Project root to operate on. Supports the main checkout and linked worktrees. Default: {}.",
                            self.target.root.display()
                        )));
                        workspace.insert("default".to_owned(), serde_json::json!(self.target.root));
                    }
                } else {
                    properties.remove("workspace");
                }
            }
        }
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools,
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
