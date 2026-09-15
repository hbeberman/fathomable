// @okf-doc: /decisions/0061-agents-start-threads.md
//! `thread_start`: an agent opens a thread on lines of a file (ADR 0061),
//! or on the file as a whole (ADR 0063).
//!
//! One comment, or several in a `comments` batch, each on a line range
//! of a workspace file, or on the file itself when it names no line. The
//! batch is checked before anything is written: a path that is not a
//! file, a range past the end of the file, a file that is not text, or
//! an empty body refuses the whole call, naming every offending item. A comment is signed as a reply is and stamped
//! with `HEAD` as the user's comments are, through the viewer when one
//! shows the workspace and through the store when none does. The result
//! is `thread_reply`'s: the new threads as an agent sees them, and one
//! `started` line each.

use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Draft, LineRange, Store, Thread};
use fathomable_core::clock::now;
use fathomable_core::session::{Request, Response};
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::Workspace;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{RoleServer, schemars, tool, tool_router};
use serde::Deserialize;
use serde_json::json;

use super::tools::{Shown, Tree, check_path, failure, shown_lines, with_summary};
use super::{Server, Target, call};

/// One comment in a `thread_start` batch.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartItem {
    /// Workspace-relative path of the file.
    path: PathBuf,
    /// First line the comment is on, 1-based. Omit it only for a comment
    /// on the file as a whole.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line`.
    #[serde(default)]
    end_line: Option<usize>,
    /// The comment; Markdown.
    body: String,
}

/// `thread_start` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartParams {
    /// Workspace-relative path of the file, for a single comment.
    #[serde(default)]
    path: Option<PathBuf>,
    /// First line the comment is on, 1-based (single comment). Omit it
    /// only for a comment on the file as a whole.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line` (single comment).
    #[serde(default)]
    end_line: Option<usize>,
    /// The comment for a single thread; Markdown.
    #[serde(default)]
    body: Option<String>,
    /// Several comments in one call, instead of `path`, `line`, and
    /// `body`. Tag every place in one turn this way.
    #[serde(default)]
    comments: Vec<StartItem>,
    /// Workspace root, viewer name, or viewer id; defaults to the bound
    /// workspace.
    #[serde(default)]
    workspace: Option<String>,
}

/// A comment the batch check passed: where it goes and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    path: PathBuf,
    /// `None` for a comment on the file as a whole (ADR 0063).
    range: Option<LineRange>,
    body: String,
}

#[tool_router(router = tool_router_start, vis = "pub(super)")]
impl Server {
    #[tool(
        name = "thread_start",
        description = "Start a thread of your own on lines of a file, for the user to read \
                       in the viewer: one with `path`, `line`, `end_line`, and `body`, or \
                       several in `comments`. Omit `line` only for a remark about the file \
                       as a whole; a comment about particular lines names them. Returns \
                       each new thread as it stands. The thread waits on the user and \
                       reaches no agent until they answer; it is not a reply, so pass a \
                       thread id to `thread_reply` instead when you are answering. The \
                       batch is checked first: a path that is not a file, a range past \
                       the end, or an empty body refuses the whole call and nothing is \
                       written.",
        annotations(
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn thread_start(
        &self,
        Parameters(p): Parameters<StartParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let target = match self.resolve(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        let signed = match self.signer(&context, &target.key) {
            Ok(signed) => signed,
            Err(error) => return failure(error),
        };
        let mut items = p.comments;
        match (p.path, p.body) {
            (Some(path), Some(body)) => items.insert(
                0,
                StartItem {
                    path,
                    line: p.line,
                    end_line: p.end_line,
                    body,
                },
            ),
            (None, None) if !items.is_empty() => {}
            _ => {
                return failure(format!(
                    "pass `{}`, `{}`, and `{}`, or a non-empty `{}` list",
                    vocab::PATH,
                    vocab::LINE,
                    vocab::BODY,
                    vocab::COMMENTS
                ));
            }
        }

        // Check the whole batch before writing any of it, so that a retry
        // with the fixed list is a whole retry.
        let (placed, problems): (Vec<_>, Vec<_>) = items
            .iter()
            .map(|item| place(&target.root, item))
            .partition(Result::is_ok);
        if !problems.is_empty() {
            let problems: Vec<String> = problems.into_iter().filter_map(Result::err).collect();
            return failure(problems.join("\n"));
        }

        let mut tree = Tree::new(&target.root);
        let mut lines = Vec::new();
        let mut started = Vec::new();
        for item in placed.into_iter().filter_map(Result::ok) {
            match self.start_one(&target, signed.author.clone(), item).await {
                Ok(thread) => started.push(thread),
                Err(error) => {
                    lines.extend(shown_lines(&started, &mut tree, "started"));
                    lines.push(error);
                    return failure(lines.join("\n"));
                }
            }
        }
        lines.extend(shown_lines(&started, &mut tree, "started"));
        if !signed.subscribed {
            lines.push(format!(
                "signed as {} with no subscription; call `{}` with `{}` to be told \
                 about answers when delivery hooks are installed",
                signed.author,
                vocab::FOLLOW.name,
                vocab::TYPE
            ));
        }
        let shown: Vec<Shown<'_>> = started
            .iter()
            .map(|t| Shown::new(t, tree.place(t)))
            .collect();
        with_summary(json!({ "threads": shown }), lines.join("\n"))
    }
}

impl Server {
    /// One comment of a `thread_start` call, through a viewer or the
    /// store; answers with the new thread.
    async fn start_one(
        &self,
        target: &Target,
        author: Author,
        item: Placed,
    ) -> Result<Thread, String> {
        let place = match item.range {
            Some(range) => format!("{}:{}", item.path.display(), range.start()),
            None => item.path.display().to_string(),
        };
        let request = Request::ThreadStart {
            path: item.path.clone(),
            range: item.range,
            author: author.clone(),
            body: item.body.clone(),
        };
        let outcome = match target.viewer_here() {
            Some(viewer) => call(viewer, &request).await,
            None => headless_start(&self.dirs, &target.key, &target.root, author, item)
                .map(|thread| Response::Threads(vec![thread])),
        };
        match outcome {
            Ok(Response::Threads(mut threads)) if threads.len() == 1 => Ok(threads.remove(0)),
            Ok(Response::Error(message)) | Err(message) => Err(format!("{place}: {message}")),
            Ok(other) => Err(format!("{place}: unexpected reply {other:?}")),
        }
    }
}

/// Where `item` goes, or why it cannot go there: the path is not a file
/// in the workspace, the file is not text, the range runs past its end,
/// or the body is empty. An item with no line is a comment on the file
/// as a whole (ADR 0063).
fn place(root: &Path, item: &StartItem) -> Result<Placed, String> {
    let shown = item.path.display();
    let path = check_path(root, Some(&item.path))?
        .ok_or_else(|| format!("{shown} is the workspace root; pass a file"))?;
    let full = root.join(&path);
    if full.is_dir() {
        return Err(format!("{shown} is a directory; pass a file"));
    }
    let text = fs::read(&full)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| format!("{shown} is not a text file"))?;
    let range = item
        .line
        .map(|line| LineRange::new(line, item.end_line.unwrap_or(line)));
    let count = text.lines().count();
    if let Some(range) = range
        && range.end() > count
    {
        return Err(format!(
            "lines {range} are past the end of {shown} ({count} line{})",
            if count == 1 { "" } else { "s" }
        ));
    }
    if item.body.trim().is_empty() {
        return Err(match item.line {
            Some(line) => format!("{shown}:{line}: `{}` is empty", vocab::BODY),
            None => format!("{shown}: `{}` is empty", vocab::BODY),
        });
    }
    Ok(Placed {
        path,
        range,
        body: item.body.clone(),
    })
}

/// Start the thread in the store, stamped with the workspace's `HEAD`,
/// and answer with it.
fn headless_start(
    dirs: &XdgDirs,
    key: &Path,
    root: &Path,
    author: Author,
    item: Placed,
) -> Result<Thread, String> {
    let text = fs::read_to_string(root.join(&item.path))
        .map_err(|error| format!("cannot read {}: {error}", item.path.display()))?;
    let commit = Workspace::discover(root)
        .ok()
        .and_then(|workspace| workspace.head_commit());
    let mut store = Store::open(dirs.threads_file(key)).map_err(|e| e.to_string())?;
    let draft = match item.range {
        Some(range) => Draft::new(author, &item.path, range, item.body),
        None => Draft::on_file(author, &item.path, item.body),
    }
    .at_commit(commit);
    let id = store
        .annotate(draft, &text, now())
        .map_err(|e| e.to_string())?;
    tracing::info!(%id, path = %item.path.display(), "agent thread started headlessly");
    store
        .thread(&id)
        .cloned()
        .ok_or_else(|| format!("thread {id} vanished after the comment"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::XdgDirs;
    use fathomable_core::annotations::{Author, LineRange, Store};
    use fathomable_testing::TempDir;

    use super::{Placed, StartItem, headless_start, place};
    use crate::app::testing;

    fn dirs(dir: &TempDir) -> XdgDirs {
        let state = dir.0.join("state").into_os_string();
        XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
    }

    fn item(path: &str, line: Option<usize>, end_line: Option<usize>, body: &str) -> StartItem {
        StartItem {
            path: PathBuf::from(path),
            line,
            end_line,
            body: body.to_owned(),
        }
    }

    /// The check names what is wrong with each comment and passes a good
    /// one through with its range and cleaned path.
    #[test]
    fn the_check_names_the_fix() -> std::io::Result<()> {
        let dir = testing::bare("mcp-start-check")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/lib.rs"), "one\ntwo\nthree\n")?;
        fs::write(root.join("logo.png"), [0xff, 0xfe, 0x00, 0x80])?;
        let good = place(&root, &item("./src/lib.rs", Some(2), Some(3), "look"));
        assert_eq!(
            good,
            Ok(Placed {
                path: PathBuf::from("src/lib.rs"),
                range: Some(LineRange::new(2, 3)),
                body: "look".to_owned(),
            })
        );
        // No line is a comment on the file as a whole (ADR 0063).
        assert_eq!(
            place(&root, &item("src/lib.rs", None, None, "split this")),
            Ok(Placed {
                path: PathBuf::from("src/lib.rs"),
                range: None,
                body: "split this".to_owned(),
            })
        );
        let refused = [
            (
                item("src", Some(1), None, "x"),
                "src is a directory; pass a file",
            ),
            (
                item("src/lib.rs", None, None, " "),
                "src/lib.rs: `body` is empty",
            ),
            (
                item("src/lib.rs", Some(3), Some(5), "x"),
                "lines 3-5 are past the end of src/lib.rs (3 lines)",
            ),
            (
                item("logo.png", Some(1), None, "x"),
                "logo.png is not a text file",
            ),
            (
                item("src/lib.rs", Some(1), None, " \n"),
                "src/lib.rs:1: `body` is empty",
            ),
            (
                item("../lib.rs", Some(1), None, "x"),
                "../lib.rs is not a workspace-relative path",
            ),
        ];
        for (item, expected) in refused {
            assert_eq!(place(&root, &item).err().as_deref(), Some(expected));
        }
        let elsewhere = place(&root, &item("lib.rs", Some(1), None, "x"));
        assert_eq!(
            elsewhere.err().as_deref(),
            Some("lib.rs is nothing in the workspace; did you mean src/lib.rs?")
        );
        Ok(())
    }

    /// Without a viewer the thread is written to the store as the agent's
    /// and comes back as it stands: waiting on the user, pending for no
    /// one (ADR 0058, 0061).
    #[test]
    fn headless_start_writes_the_agents_thread() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-start-headless")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let author = Author::agent("bot").subscribed("s1", "coder");
        let thread = headless_start(
            &dirs,
            &root,
            &root,
            author.clone(),
            Placed {
                path: PathBuf::from("a.md"),
                range: Some(LineRange::new(2, 2)),
                body: "look here".to_owned(),
            },
        )?;
        assert_eq!(thread.author(), &author);
        assert_eq!(thread.comment(), "look here");
        assert_eq!(thread.snippet(), "two");
        assert!(thread.awaits_user());
        assert!(!thread.awaits_agent());
        // The store holds it with its author, and reloads it so.
        let store = Store::open(dirs.threads_file(&root))?;
        let stored = store.thread(thread.id()).ok_or("thread not stored")?;
        assert_eq!(stored.author(), &author);
        assert_eq!(stored.path(), Path::new("a.md"));
        Ok(())
    }
}
