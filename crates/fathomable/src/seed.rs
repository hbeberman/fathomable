//! `fathomable seed FILE`: write declared threads into a workspace's
//! annotation store through the same code the viewer and MCP use.
//!
//! The command is hidden from `--help`: it exists for
//! `scripts/demo-repo.sh` and for tests. The file is one object:
//!
//! ```json
//! {
//!   "threads": [
//!     {
//!       "key": "lib-open",
//!       "path": "src/lib.rs",
//!       "line": 9,
//!       "end_line": 11,
//!       "comment": "Use split_whitespace.",
//!       "replies": [
//!         { "author": { "name": "Copilot", "id": "copilot:other" },
//!           "body": "Agreed." }
//!       ],
//!       "resolved": false,
//!       "detached": false
//!     }
//!   ],
//! }
//! ```
//!
//! Every thread is the user's comment on `line..=end_line` of the
//! repository-relative `path`; `end_line` defaults to `line`. A reply
//! without an `author` is the user's; with one it is that agent's.
//! `resolved` resolves
//! the thread after its replies. `detached` writes the thread against
//! the file as it never was, so the viewer shows it detached. `key`
//! names the thread in the `threads:` table printed on exit, one
//! `key  id` line per thread in file order.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Store, ThreadId};
use fathomable_core::clock::now;
use fathomable_core::workspace::Workspace;
use serde::Deserialize;

/// The declared contents of an annotation store.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    #[serde(default)]
    threads: Vec<ThreadSeed>,
}

/// One thread: the user's comment, then replies, then its resolution.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadSeed {
    key: String,
    path: PathBuf,
    line: usize,
    #[serde(default)]
    end_line: Option<usize>,
    comment: String,
    #[serde(default)]
    replies: Vec<ReplySeed>,
    #[serde(default)]
    resolved: bool,
    #[serde(default)]
    detached: bool,
}

/// A reply; the user's when it names no author.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplySeed {
    #[serde(default)]
    author: Option<AuthorSeed>,
    body: String,
}

/// An agent author retained in seeded conversation history.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorSeed {
    name: String,
    /// The MCP client that carried the reply.
    #[serde(default)]
    client: Option<String>,
    /// Harness-qualified chat identity, when known.
    #[serde(default)]
    id: Option<String>,
}

/// `fathomable seed`: seed the workspace around `workspace` from `file`
/// and print the `threads:` table.
pub(crate) fn run(dirs: &XdgDirs, workspace: &Path, file: &Path) -> ExitCode {
    match seed(dirs, workspace, file) {
        Ok(made) => {
            println!("threads:");
            for (key, id) in made {
                println!("  {key:16} {id}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fathomable: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Write everything `file` declares and return `(key, id)` per thread in
/// file order.
///
/// # Errors
///
/// The file must parse and every path must be readable under the workspace
/// root with its range inside the file.
fn seed(dirs: &XdgDirs, workspace: &Path, file: &Path) -> anyhow::Result<Vec<(String, ThreadId)>> {
    let text =
        fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let declared: Seed = serde_json::from_str(&text)
        .with_context(|| format!("{}: not a seed file", file.display()))?;
    let workspace = Workspace::discover(workspace)?;
    let root = workspace.root();
    let key = workspace.key();
    let commit = workspace.head_commit();
    let when = now();

    let mut store = Store::open_workspace(dirs, key)?;
    let mut made: Vec<(String, ThreadId)> = Vec::with_capacity(declared.threads.len());
    for (n, thread) in declared.threads.iter().enumerate() {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        // Threads are minutes old and in file order so the viewer's sort
        // sees the declared shape.
        let created = when.saturating_sub(600).saturating_add(n);
        let id = write_thread(&mut store, root, commit.as_deref(), thread, created)?;
        made.push((thread.key.clone(), id));
    }

    Ok(made)
}

/// Write one thread with its replies and resolution, `created` seconds
/// after the epoch, and return its id.
fn write_thread(
    store: &mut Store,
    root: &Path,
    commit: Option<&str>,
    thread: &ThreadSeed,
    created: u64,
) -> anyhow::Result<ThreadId> {
    let file = root.join(&thread.path);
    let text =
        fs::read_to_string(&file).with_context(|| format!("cannot read {}", file.display()))?;
    let text = if thread.detached {
        rewritten(&text)
    } else {
        text
    };
    let range = LineRange::new(thread.line, thread.end_line.unwrap_or(thread.line));
    let draft = Draft::new(Author::User, &thread.path, range, thread.comment.as_str())
        .at_commit(commit.map(str::to_owned));
    let id = store
        .annotate(draft, &text, created)
        .with_context(|| format!("thread `{}`", thread.key))?;
    let mut at = created;
    for reply in &thread.replies {
        at = at.saturating_add(60);
        let author = reply
            .author
            .as_ref()
            .map_or(Author::User, |a| Author::Agent {
                name: a.name.clone(),
                client: a.client.clone(),
                id: a.id.clone(),
            });
        store.reply(&id, Reply::new(author, at, reply.body.as_str()))?;
    }
    if thread.resolved {
        store.resolve(&id, commit, at.saturating_add(60))?;
    }
    Ok(id)
}

/// `text` as it never was: every line changed, so an anchor taken from
/// it matches nothing in the file and no context window places it
/// again (ADR 0038). The line count is kept, so any range that fits the
/// file fits this.
fn rewritten(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for line in text.lines() {
        out.push_str(line);
        out.push_str(" (as it was)\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use fathomable_core::XdgDirs;
    use fathomable_core::annotations::{Status, Store};
    use fathomable_testing::TempDir;

    use super::seed;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dirs(dir: &TempDir) -> XdgDirs {
        let state = dir.0.join("state").into_os_string();
        XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
    }

    /// The seeded store reads back an open conversation, a resolved
    /// conversation, and a detached thread as declared.
    #[test]
    fn seeds_thread_conversations() -> TestResult {
        let dir = TempDir::new("seed")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("README.md"), "# Demo\n\nA paragraph.\n\nMore.\n")?;
        fs::write(root.join("src/lib.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n")?;
        let file = dir.0.join("seed.json");
        fs::write(
            &file,
            r#"{
              "threads": [
                {"key": "lib", "path": "src/lib.rs", "line": 2, "end_line": 3, "comment": "why?",
                 "replies": [{"author": {"name": "Copilot", "id": "copilot:other"}, "body": "proposal"}]},
                {"key": "done", "path": "README.md", "line": 1, "comment": "fine", "resolved": true},
                {"key": "gone", "path": "README.md", "line": 3, "comment": "rewritten", "detached": true}
              ]
            }"#,
        )?;
        let dirs = dirs(&dir);
        let root = root.canonicalize()?;

        let made = seed(&dirs, &root, &file)?;
        assert_eq!(
            made.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            ["lib", "done", "gone"]
        );

        let store = Store::open(dirs.threads_file(&root))?;
        assert_eq!(store.threads().len(), 3);
        let lib = store.thread(&made[0].1).ok_or("lib missing")?;
        assert_eq!(lib.status(), Status::Open);
        assert_eq!(lib.replies().len(), 1);
        assert_eq!(lib.replies()[0].author().id(), Some("copilot:other"));
        assert_eq!(lib.replies()[0].body(), "proposal");
        assert!(
            !lib.locate(&fs::read_to_string(root.join("src/lib.rs"))?)
                .is_detached()
        );
        let done = store.thread(&made[1].1).ok_or("done missing")?;
        assert_eq!(done.status(), Status::Resolved);
        let gone = store.thread(&made[2].1).ok_or("gone missing")?;
        assert!(
            gone.locate(&fs::read_to_string(root.join("README.md"))?)
                .is_detached()
        );
        Ok(())
    }
}
