//! `fathomable seed FILE`: write the threads, subscribers, and watches a
//! JSON file declares into a workspace's thread store and agent register
//! through the same code the viewer and `--mcp` use, so a script never
//! hand-writes the JSONL formats (ADR 0009, note of 2026-09-05).
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
//!         { "author": { "name": "rev", "id": "other", "type": "reviewer" },
//!           "body": "Agreed." }
//!       ],
//!       "resolved": false,
//!       "detached": false
//!     }
//!   ],
//!   "subscribers": [ { "id": "demo-1", "type": "coder", "name": "demo" } ],
//!   "watches": [
//!     { "subscriber": "demo-1", "on": "lib-open", "when": "resolved",
//!       "remind": ["lib-open"] }
//!   ]
//! }
//! ```
//!
//! Every thread is the user's comment on `line..=end_line` of the
//! workspace-relative `path`; `end_line` defaults to `line`. A reply
//! without an `author` is the user's; with one it is that agent's,
//! subscribed when `id` and `type` are both given. `resolved` resolves
//! the thread after its replies. `detached` writes the thread against
//! the file as it never was, so the viewer shows it detached. `key`
//! names a thread for `watches` and for the `threads:` table printed on
//! exit, one `key  id` line per thread in file order.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use fathomable_core::XdgDirs;
use fathomable_core::agents::{Register, WatchWhen};
use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Store, ThreadId};
use fathomable_core::clock::now;
use fathomable_core::config::{AgentsConfig, Config};
use fathomable_core::workspace::Workspace;
use serde::Deserialize;

/// The declared contents of a store and register.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    #[serde(default)]
    threads: Vec<ThreadSeed>,
    #[serde(default)]
    subscribers: Vec<SubscriberSeed>,
    #[serde(default)]
    watches: Vec<WatchSeed>,
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

/// An agent author, subscribed when both `id` and `type` are given.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorSeed {
    name: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// A subscriber of the register (ADR 0040).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscriberSeed {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    name: Option<String>,
}

/// A watch by `subscriber` on the thread keyed `on`, reminding the
/// threads keyed `remind`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchSeed {
    subscriber: String,
    on: String,
    when: WatchWhen,
    #[serde(default)]
    remind: Vec<String>,
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
/// The file must parse, every path must be readable under the workspace
/// root with its range inside the file, and every key a watch names must
/// belong to a thread of the same file.
fn seed(dirs: &XdgDirs, workspace: &Path, file: &Path) -> anyhow::Result<Vec<(String, ThreadId)>> {
    let text =
        fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let declared: Seed = serde_json::from_str(&text)
        .with_context(|| format!("{}: not a seed file", file.display()))?;
    let workspace = Workspace::discover(workspace)?;
    let root = workspace.root();
    let commit = workspace.head_commit();
    let when = now();

    let mut store = Store::open(dirs.threads_file(root))?;
    let mut made: Vec<(String, ThreadId)> = Vec::with_capacity(declared.threads.len());
    for (n, thread) in declared.threads.iter().enumerate() {
        let n = u64::try_from(n).unwrap_or(u64::MAX);
        // Threads are minutes old and in file order, so the register's
        // freshness rules and the viewer's sort see the shape intended.
        let created = when.saturating_sub(600).saturating_add(n);
        let id = write_thread(&mut store, root, commit.clone(), thread, created)?;
        made.push((thread.key.clone(), id));
    }

    let agents =
        Config::load(dirs, None).map_or_else(|_| AgentsConfig::default(), |c| c.agents().clone());
    let mut register = Register::open(dirs.agents_file(root), when, agents.expire_after)?;
    for subscriber in &declared.subscribers {
        register.subscribe(
            &subscriber.id,
            &subscriber.kind,
            subscriber.name.as_deref(),
            None,
            when,
        )?;
    }
    for watch in &declared.watches {
        let id_of = |key: &str| -> anyhow::Result<ThreadId> {
            made.iter()
                .find(|(k, _)| k == key)
                .map(|(_, id)| id.clone())
                .with_context(|| format!("watch names an unknown thread key `{key}`"))
        };
        let on = id_of(&watch.on)?;
        let remind = watch
            .remind
            .iter()
            .map(|key| id_of(key))
            .collect::<anyhow::Result<Vec<_>>>()?;
        register.watch(&watch.subscriber, &on, watch.when, remind, when)?;
    }
    Ok(made)
}

/// Write one thread with its replies and resolution, `created` seconds
/// after the epoch, and return its id.
fn write_thread(
    store: &mut Store,
    root: &Path,
    commit: Option<String>,
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
    let draft =
        Draft::new(Author::User, &thread.path, range, thread.comment.as_str()).at_commit(commit);
    let id = store
        .annotate(draft, &text, created)
        .with_context(|| format!("thread `{}`", thread.key))?;
    let mut at = created;
    for reply in &thread.replies {
        at = at.saturating_add(60);
        let author = reply
            .author
            .as_ref()
            .map_or(Author::User, |a| match (&a.id, &a.kind) {
                (Some(id), Some(kind)) => Author::agent(a.name.as_str()).subscribed(id, kind),
                _ => Author::agent(a.name.as_str()),
            });
        store.reply(&id, Reply::new(author, at, reply.body.as_str()))?;
    }
    if thread.resolved {
        store.resolve(&id, at.saturating_add(60))?;
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
    use fathomable_core::agents::{Register, WatchWhen};
    use fathomable_core::annotations::{Status, Store};
    use fathomable_core::config::AgentsConfig;
    use fathomable_testing::TempDir;

    use super::seed;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dirs(dir: &TempDir) -> XdgDirs {
        let state = dir.0.join("state").into_os_string();
        XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
    }

    /// The seeded store and register read back as declared: an open
    /// thread with an agent's reply, a resolved one, a detached one, and
    /// a subscriber watching by key.
    #[test]
    fn seeds_threads_and_the_register() -> TestResult {
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
                 "replies": [{"author": {"name": "rev", "id": "other", "type": "reviewer"}, "body": "agreed"}]},
                {"key": "done", "path": "README.md", "line": 1, "comment": "fine", "resolved": true},
                {"key": "gone", "path": "README.md", "line": 3, "comment": "rewritten", "detached": true}
              ],
              "subscribers": [{"id": "demo-1", "type": "coder", "name": "demo"}],
              "watches": [{"subscriber": "demo-1", "on": "done", "when": "resolved", "remind": ["lib"]}]
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
        assert_eq!(lib.replies()[0].author().id(), Some("other"));
        assert_eq!(lib.replies()[0].author().kind(), Some("reviewer"));
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

        let register = Register::open(
            dirs.agents_file(&root),
            0,
            AgentsConfig::default().expire_after,
        )?;
        let subscriber = register.subscriber("demo-1").ok_or("no subscriber")?;
        assert_eq!(subscriber.kind(), "coder");
        assert_eq!(subscriber.name(), Some("demo"));
        let watch = register.watches().first().ok_or("no watch")?;
        assert_eq!(watch.on(), &made[1].1);
        assert_eq!(watch.when(), WatchWhen::Resolved);
        assert_eq!(watch.remind(), &[made[0].1.clone()]);
        Ok(())
    }

    /// A watch on a key no thread carries is refused before anything is
    /// written to the register.
    #[test]
    fn unknown_watch_key_is_an_error() -> TestResult {
        let dir = TempDir::new("seed-key")?;
        let root = dir.0.join("ws");
        fs::create_dir_all(&root)?;
        let file = dir.0.join("seed.json");
        fs::write(
            &file,
            r#"{"subscribers": [{"id": "a", "type": "coder"}],
                "watches": [{"subscriber": "a", "on": "nope", "when": "message"}]}"#,
        )?;
        let dirs = dirs(&dir);
        let error = seed(&dirs, &root, &file).err().ok_or("seed succeeded")?;
        assert!(error.to_string().contains("nope"), "{error:#}");
        let register = Register::open(
            dirs.agents_file(&root.canonicalize()?),
            0,
            AgentsConfig::default().expire_after,
        )?;
        assert!(register.subscriber("a").is_some());
        assert!(register.watches().is_empty());
        Ok(())
    }
}
