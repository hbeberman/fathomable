// @okf-doc: /decisions/0082-three-tool-review-core.md
//! The repository review tools exposed over MCP.
//!
//! `threads` reads discussions without mutating them. `thread_reply`
//! continues one or more discussions, after validating the whole batch.
//! `thread_start` lives in [`super::start`].

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use fathomable_core::XdgDirs;
use fathomable_core::annotations::{
    Author, LineHashes, LineRange, Placement, Reply, Status, Store, Thread, ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::context::map_context;
use fathomable_core::reanchor::{Mapping, map_range};
use fathomable_core::seen;
use fathomable_core::session::{Request, Response};
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::{Filter, Workspace};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Server, Target, call};
use crate::app::threads::open::follow_reply_lines;

/// `threads` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThreadsParams {
    /// Which discussions to list: `open` (default), `resolved`, or `all`.
    #[serde(default)]
    status: Option<String>,
    /// Only discussions on this repository-relative file or below this directory.
    #[serde(default)]
    path: Option<PathBuf>,
    /// Only discussions changed at or after this Unix time in seconds.
    #[serde(default)]
    since: Option<u64>,
    /// Continue strictly after this `(updated, id)` position from a prior page.
    #[serde(default)]
    after: Option<After>,
    /// At most this many discussions, oldest change first; default 50.
    #[serde(default)]
    limit: Option<usize>,
    /// Exact discussion ids to read in the given order, including resolved history.
    /// Do not combine this with filters or pagination.
    #[serde(default)]
    ids: Vec<String>,
}

/// A stable position in `threads` ordering.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct After {
    /// The prior page's last `updated` timestamp.
    updated: u64,
    /// The prior page's last thread id.
    id: String,
}

/// One reply in a `thread_reply` batch.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplyItem {
    /// Discussion id from `threads`.
    thread: String,
    /// Reply text in Markdown.
    body: String,
    /// Propose resolving the discussion. Only the user can close it.
    #[serde(default)]
    resolve: bool,
    /// First line the discussion's lines occupy now, when they moved.
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line`.
    #[serde(default)]
    end_line: Option<usize>,
}

/// `thread_reply` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplyParams {
    /// One or more replies. The whole batch is validated before any write.
    replies: Vec<ReplyItem>,
}

const DEFAULT_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Open,
    Resolved,
    All,
}

impl Which {
    fn parse(text: Option<&str>) -> Result<Self, String> {
        match text {
            None | Some(vocab::STATUS_OPEN) => Ok(Self::Open),
            Some(vocab::WHEN_RESOLVED) => Ok(Self::Resolved),
            Some(vocab::STATUS_ALL) => Ok(Self::All),
            Some(other) => Err(format!(
                "`{}` is `{}`, `{}`, or `{}`, not `{other}`",
                vocab::STATUS,
                vocab::STATUS_OPEN,
                vocab::WHEN_RESOLVED,
                vocab::STATUS_ALL
            )),
        }
    }

    fn admits(self, thread: &Thread) -> bool {
        match self {
            Self::Open => matches!(thread.status(), Status::Open),
            Self::Resolved => matches!(thread.status(), Status::Resolved),
            Self::All => true,
        }
    }
}

/// A complete discussion as an MCP caller reads it.
///
/// Both open and resolved discussions include the original author's identity
/// and every reply, so a proposal cannot be mistaken for an agreement merely
/// because the discussion is closed or appears in history.
#[derive(Debug, Serialize)]
pub(super) struct Shown<'a> {
    id: &'a ThreadId,
    path: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<LineRange>,
    placement: &'static str,
    status: Status,
    created: u64,
    updated: u64,
    author: &'a Author,
    comment: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<&'a str>,
    replies: &'a [Reply],
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edited: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<PathBuf>,
}

impl<'a> Shown<'a> {
    pub(super) fn new(thread: &'a Thread, placement: Placement) -> Self {
        Self {
            id: thread.id(),
            path: thread.path(),
            range: placement.range(),
            placement: placement_word(placement),
            status: thread.status(),
            created: thread.created(),
            updated: thread.updated(),
            author: thread.author(),
            comment: thread.comment(),
            snippet: (!thread.is_on_file()).then(|| thread.snippet()),
            replies: thread.replies(),
            commit: thread.commit(),
            edited: thread.edited(),
            worktree: None,
        }
    }

    fn in_worktree(mut self, worktree: Option<&Path>) -> Self {
        self.worktree = worktree.map(Path::to_path_buf);
        self
    }

    fn line(&self) -> String {
        let at = match self.range {
            Some(range) => format!("{}:{range}", self.path.display()),
            None => self.path.display().to_string(),
        };
        let mut line = format!(
            "{}  {at}  {} {}  {}",
            self.id,
            status_word(self.status),
            self.placement,
            self.comment.lines().next().unwrap_or_default()
        );
        if let Some(worktree) = &self.worktree {
            line.push_str("  in ");
            line.push_str(&worktree.display().to_string());
        }
        line
    }
}

const fn placement_word(placement: Placement) -> &'static str {
    match placement {
        Placement::Anchored(_) => "anchored",
        Placement::Edited(_) => "edited",
        Placement::Detached(_) => "detached",
        Placement::File => "file",
    }
}

const fn status_word(status: Status) -> &'static str {
    match status {
        Status::Open => vocab::STATUS_OPEN,
        Status::Resolved => vocab::WHEN_RESOLVED,
    }
}

/// Repository files hashed once per response for placement.
pub(super) struct Tree {
    root: PathBuf,
    hashes: HashMap<PathBuf, Option<LineHashes>>,
    snapshots: Option<seen::Snapshots>,
}

impl Tree {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            hashes: HashMap::new(),
            snapshots: None,
        }
    }

    fn from_state(dirs: &XdgDirs, key: &Path, root: &Path) -> Self {
        let snapshots = match seen::Snapshots::read(&dirs.seen_dir(key)) {
            Ok(snapshots) => Some(snapshots),
            Err(error) => {
                tracing::warn!(%error, "cannot read snapshots for projected thread placement");
                None
            }
        };
        Self {
            root: root.to_path_buf(),
            hashes: HashMap::new(),
            snapshots,
        }
    }

    pub(super) fn place(&mut self, thread: &Thread) -> Placement {
        let hashes = self
            .hashes
            .entry(thread.path().to_path_buf())
            .or_insert_with(|| {
                fs::read_to_string(self.root.join(thread.path()))
                    .ok()
                    .map(|text| LineHashes::of(&text))
            });
        let placement = match (hashes, thread.range()) {
            (Some(hashes), _) => thread.locate_in(hashes),
            (None, Some(range)) => Placement::Detached(range),
            (None, None) => Placement::File,
        };
        if !placement.is_detached() {
            return placement;
        }
        let Some(from) = thread.range() else {
            return placement;
        };
        let Ok(text) = fs::read_to_string(self.root.join(thread.path())) else {
            return placement;
        };
        let through_snapshot = self
            .snapshots
            .as_ref()
            .and_then(|snapshots| match snapshots.text(thread.path()) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %thread.path().display(),
                        "cannot read snapshot; falling back to thread context"
                    );
                    None
                }
            })
            .and_then(|snapshot| {
                let placement = thread.locate(&snapshot);
                let snapshot_range = placement.range().filter(|_| !placement.is_detached())?;
                Some(map_range(&snapshot, &text, snapshot_range))
            });
        let mapping = match through_snapshot {
            Some(mapping @ (Mapping::Edited(_) | Mapping::Moved(_))) => mapping,
            _ => match thread.context() {
                Some(context) => map_context(context, &text, from),
                None => Mapping::Removed,
            },
        };
        match mapping {
            Mapping::Edited(range) | Mapping::Moved(range) => Placement::Edited(range),
            Mapping::Removed => placement,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Location {
    root: PathBuf,
    placement: Placement,
}

impl Location {
    fn worktree(&self, bound: &Path) -> Option<&Path> {
        (self.root != bound).then_some(self.root.as_path())
    }
}

pub(super) struct Trees<'a> {
    dirs: &'a XdgDirs,
    key: &'a Path,
    here: Tree,
    others: HashMap<PathBuf, Tree>,
}

impl<'a> Trees<'a> {
    pub(super) fn new(dirs: &'a XdgDirs, key: &'a Path, root: &Path) -> Self {
        Self {
            dirs,
            key,
            here: Tree::from_state(dirs, key, root),
            others: HashMap::new(),
        }
    }

    fn place(&mut self, bound: &Path, root: &Path, thread: &Thread) -> Placement {
        if root == bound {
            return self.here.place(thread);
        }
        self.others
            .entry(root.to_path_buf())
            .or_insert_with(|| Tree::from_state(self.dirs, self.key, root))
            .place(thread)
    }

    /// Find a current worktree where the discussion still has a file and
    /// a usable projected placement.
    pub(super) fn project(&mut self, roots: &[PathBuf], thread: &Thread) -> Option<Location> {
        let bound = self.here.root.clone();
        roots.iter().find_map(|root| {
            if !root.join(thread.path()).is_file() {
                return None;
            }
            let placement = self.place(&bound, root, thread);
            (!placement.is_detached()).then(|| Location {
                root: root.clone(),
                placement,
            })
        })
    }

    fn locate(
        &mut self,
        scope: &fathomable_core::reach::Reach,
        bound: &Path,
        roots: &[PathBuf],
        thread: &Thread,
    ) -> Location {
        let root = if scope.here(thread) {
            bound.to_path_buf()
        } else if let Some(root) = scope.elsewhere(thread) {
            root.to_path_buf()
        } else if let Some(location) = self.project(roots, thread) {
            return location;
        } else {
            bound.to_path_buf()
        };
        let placement = self.place(bound, &root, thread);
        Location { root, placement }
    }
}

#[tool_router(vis = "pub(super)")]
impl Server {
    #[tool(
        description = "Read review discussions in this repository checkout. By default returns \
                       all open discussions with their complete conversation, author identities, \
                       file placement, and ranges. Filter with `status`, `path`, and `since`; page \
                       with `limit` and the returned `next_after`; or pass `ids` alone to retrieve \
                       exact discussions and resolved history. Reading never assigns, \
                       acknowledges, or consumes a discussion.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn threads(&self, Parameters(p): Parameters<ThreadsParams>) -> CallToolResult {
        let roots = self.target.roots();
        let (selected, scope) = if p.ids.is_empty() {
            let (all, scope) = match self.fetch() {
                Ok(all) => all,
                Err(error) => return failure(error),
            };
            match select_filtered(&roots, &all, &p) {
                Ok(selected) => (OwnedSelection::from(selected), scope),
                Err(error) => return failure(error),
            }
        } else {
            if p.status.is_some()
                || p.path.is_some()
                || p.since.is_some()
                || p.after.is_some()
                || p.limit.is_some()
            {
                return failure("pass `ids` alone, without status, path, since, after, or limit");
            }
            let (all, scope) = match self.fetch_exact() {
                Ok(all) => all,
                Err(error) => return failure(error),
            };
            match select_exact(&all, &p.ids) {
                Ok(selected) => (OwnedSelection::from(selected), scope),
                Err(error) => return failure(error),
            }
        };

        let mut trees = Trees::new(&self.dirs, &self.target.key, &self.target.root);
        let shown: Vec<Shown<'_>> = selected
            .threads
            .iter()
            .map(|thread| {
                let location = trees.locate(&scope, &self.target.root, &roots, thread);
                Shown::new(thread, location.placement)
                    .in_worktree(location.worktree(&self.target.root))
            })
            .collect();
        let mut lines: Vec<String> = shown.iter().map(Shown::line).collect();
        if lines.is_empty() {
            lines.push("no discussions".to_owned());
        }
        if selected.more > 0 {
            let next = selected
                .next_after
                .as_ref()
                .map(|after| json!(after).to_string())
                .unwrap_or_default();
            lines.push(format!(
                "{} more; pass {}={next}",
                selected.more,
                vocab::AFTER,
            ));
        }
        with_summary(
            json!({
                "threads": shown,
                "more": selected.more,
                "next_after": selected.next_after,
            }),
            lines.join("\n"),
        )
    }

    #[tool(
        description = "Continue one or more existing review discussions. Pass exactly one \
                       non-empty `replies` array; each item names a thread and body, with optional \
                       current line placement and a proposal to resolve. Only the user closes \
                       discussions. The whole batch is validated before any reply is written.",
        annotations(
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn thread_reply(
        &self,
        Parameters(p): Parameters<ReplyParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if p.replies.is_empty() {
            return failure("`replies` must contain at least one reply");
        }
        let author = match self.signer(&context) {
            Ok(author) => author,
            Err(error) => return failure(error),
        };
        let (all, scope) = match self.fetch() {
            Ok(all) => all,
            Err(error) => return failure(error),
        };
        let roots = self.target.roots();
        let mut trees = Trees::new(&self.dirs, &self.target.key, &self.target.root);
        let mut seen = HashSet::with_capacity(p.replies.len());
        let mut validated = Vec::with_capacity(p.replies.len());
        let mut problems = Vec::new();
        for item in p.replies {
            if !seen.insert(item.thread.clone()) {
                problems.push(format!(
                    "{} appears more than once in `replies`",
                    item.thread
                ));
                continue;
            }
            match validate_reply(&item, &all, &scope, &roots, &mut trees, &self.target.root) {
                Ok(root) => validated.push(ValidatedReply { item, root }),
                Err(error) => problems.push(error),
            }
        }
        if !problems.is_empty() {
            return failure(problems.join("\n"));
        }

        let mut answered = Vec::with_capacity(validated.len());
        for item in validated {
            let root = item.root.clone();
            match self.reply_one(author.clone(), item).await {
                Ok(thread) => answered.push(Answered { thread, root }),
                Err(error) => {
                    let mut lines =
                        answer_lines(&answered, &self.dirs, &self.target.key, &self.target.root);
                    lines.push(error);
                    return failure(lines.join("\n"));
                }
            }
        }
        let lines = answer_lines(&answered, &self.dirs, &self.target.key, &self.target.root);
        let mut trees = Trees::new(&self.dirs, &self.target.key, &self.target.root);
        let shown: Vec<Shown<'_>> = answered
            .iter()
            .map(|answered| {
                let placement = trees.place(&self.target.root, &answered.root, &answered.thread);
                Shown::new(&answered.thread, placement).in_worktree(
                    (answered.root != self.target.root).then_some(answered.root.as_path()),
                )
            })
            .collect();
        with_summary(json!({ "threads": shown }), lines.join("\n"))
    }
}

struct ValidatedReply {
    item: ReplyItem,
    root: PathBuf,
}

struct Answered {
    thread: Thread,
    root: PathBuf,
}

impl Server {
    async fn reply_one(&self, author: Author, validated: ValidatedReply) -> Result<Thread, String> {
        let item = validated.item;
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
        let outcome = match self.target.viewer(&self.dirs, &validated.root) {
            Some(viewer) => call(&viewer, &request).await,
            None => headless_reply(
                &self.dirs,
                &self.target,
                &validated.root,
                &thread,
                author,
                &item,
                lines,
            )
            .map(|thread| Response::Threads(vec![thread])),
        };
        match outcome {
            Ok(Response::Threads(mut threads)) if threads.len() == 1 => Ok(threads.remove(0)),
            Ok(Response::Error(message)) | Err(message) => {
                Err(format!("{}: {message}", item.thread))
            }
            Ok(other) => Err(format!("{}: unexpected reply {other:?}", item.thread)),
        }
    }
}

struct Selected<'a> {
    threads: Vec<&'a Thread>,
    more: usize,
    next_after: Option<After>,
}

struct OwnedSelection {
    threads: Vec<Thread>,
    more: usize,
    next_after: Option<After>,
}

impl From<Selected<'_>> for OwnedSelection {
    fn from(selected: Selected<'_>) -> Self {
        Self {
            threads: selected.threads.into_iter().cloned().collect(),
            more: selected.more,
            next_after: selected.next_after,
        }
    }
}

fn select_filtered<'a>(
    roots: &[PathBuf],
    all: &'a [Thread],
    params: &ThreadsParams,
) -> Result<Selected<'a>, String> {
    let which = Which::parse(params.status.as_deref())?;
    let path = check_path_in_roots(roots, params.path.as_deref())?;
    let mut threads: Vec<&Thread> = all
        .iter()
        .filter(|thread| which.admits(thread))
        .filter(|thread| params.since.is_none_or(|since| thread.updated() >= since))
        .filter(|thread| {
            params.after.as_ref().is_none_or(|after| {
                (thread.updated(), thread.id().as_str()) > (after.updated, after.id.as_str())
            })
        })
        .filter(|thread| {
            path.as_deref()
                .is_none_or(|path| thread.path().starts_with(path))
        })
        .collect();
    threads.sort_by(|left, right| (left.updated(), left.id()).cmp(&(right.updated(), right.id())));
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let more = threads.len().saturating_sub(limit);
    threads.truncate(limit);
    let next_after = if more > 0 {
        threads.last().map(|thread| After {
            updated: thread.updated(),
            id: thread.id().to_string(),
        })
    } else {
        None
    };
    Ok(Selected {
        threads,
        more,
        next_after,
    })
}

fn select_exact<'a>(all: &'a [Thread], ids: &[String]) -> Result<Selected<'a>, String> {
    let parsed: Vec<ThreadId> = ids
        .iter()
        .map(|id| thread_id(id))
        .collect::<Result<_, _>>()?;
    let duplicates: Vec<&String> = ids
        .iter()
        .enumerate()
        .filter_map(|(index, id)| ids[..index].contains(id).then_some(id))
        .collect();
    if !duplicates.is_empty() {
        return Err(format!(
            "duplicate thread id(s): {}",
            duplicates
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let missing: Vec<String> = parsed
        .iter()
        .filter(|id| all.iter().all(|thread| thread.id() != *id))
        .map(ToString::to_string)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "no stored thread(s) {}; call `threads` without `ids` to list this checkout",
            missing.join(", ")
        ));
    }
    let threads = parsed
        .iter()
        .filter_map(|id| all.iter().find(|thread| thread.id() == id))
        .collect();
    Ok(Selected {
        threads,
        more: 0,
        next_after: None,
    })
}

fn validate_reply(
    item: &ReplyItem,
    all: &[Thread],
    scope: &fathomable_core::reach::Reach,
    roots: &[PathBuf],
    trees: &mut Trees<'_>,
    bound: &Path,
) -> Result<PathBuf, String> {
    let id = match thread_id(&item.thread) {
        Ok(id) => id,
        Err(error) => return Err(format!("{}: {error}", item.thread)),
    };
    let Some(thread) = all.iter().find(|thread| thread.id() == &id) else {
        return Err(format!(
            "no visible thread {}; call `threads` to see this checkout's ids",
            item.thread
        ));
    };
    let location = trees.locate(scope, bound, roots, thread);
    refusal(item, thread, location.placement, &location.root)?;
    Ok(location.root)
}

/// Why an item cannot be replied to, if it cannot.
fn refusal(
    item: &ReplyItem,
    thread: &Thread,
    placement: Placement,
    root: &Path,
) -> Result<(), String> {
    if thread.status() != Status::Open {
        return Err(format!(
            "{} is resolved; only the user can reopen it",
            item.thread
        ));
    }
    if item.body.trim().is_empty() {
        return Err(format!("{}: `body` is empty", item.thread));
    }
    if item.line.is_none() && item.end_line.is_some() {
        return Err(format!("{}: pass `line` with `end_line`", item.thread));
    }
    if thread.is_on_file() && (item.line.is_some() || item.end_line.is_some()) {
        return Err(format!(
            "{} is a file-wide discussion; omit `line` and `end_line`",
            item.thread
        ));
    }
    if let Some(line) = item.line {
        if line == 0 || item.end_line == Some(0) {
            return Err(format!("{}: line numbers are 1-based", item.thread));
        }
        let range = LineRange::new(line, item.end_line.unwrap_or(line));
        let path = root.join(thread.path());
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                return Err(format!(
                    "{}: cannot place in {}: {error}",
                    item.thread,
                    thread.path().display()
                ));
            }
        };
        let count = text.lines().count();
        if range.end() > count {
            return Err(format!(
                "{}: lines {range} are past the end of {} ({count} line{})",
                item.thread,
                thread.path().display(),
                if count == 1 { "" } else { "s" }
            ));
        }
    } else if matches!(placement, Placement::Detached(_)) {
        return Err(format!(
            "{} is detached from {}; pass `line` and optional `end_line` to place it",
            item.thread,
            thread.path().display()
        ));
    }
    Ok(())
}

pub(super) fn shown_lines(threads: &[Thread], tree: &mut Tree, verb: &str) -> Vec<String> {
    threads
        .iter()
        .map(|thread| {
            let placement = tree.place(thread);
            shown_line(thread, placement, verb)
        })
        .collect()
}

fn answer_lines(answered: &[Answered], dirs: &XdgDirs, key: &Path, bound: &Path) -> Vec<String> {
    let mut trees = Trees::new(dirs, key, bound);
    answered
        .iter()
        .map(|answered| {
            let placement = trees.place(bound, &answered.root, &answered.thread);
            shown_line(&answered.thread, placement, "replied to")
        })
        .collect()
}

fn shown_line(thread: &Thread, placement: Placement, verb: &str) -> String {
    let at = match placement.range() {
        Some(range) => format!("{}:{range}", thread.path().display()),
        None => thread.path().display().to_string(),
    };
    format!(
        "{verb} {} at {at} ({}){}",
        thread.id(),
        placement_word(placement),
        if thread.proposes_resolution() {
            ", proposing to resolve it"
        } else {
            ""
        }
    )
}

fn thread_id(text: &str) -> Result<ThreadId, String> {
    serde_json::from_value(Value::String(text.to_owned())).map_err(|error| error.to_string())
}

/// Validate and normalize an optional repository-relative file or directory.
pub(super) fn check_path(root: &Path, path: Option<&Path>) -> Result<Option<PathBuf>, String> {
    check_path_in_roots(&[root.to_path_buf()], path)
}

fn check_path_in_roots(roots: &[PathBuf], path: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let inside = path.is_relative()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    let shown = path.display();
    if !inside {
        return Err(format!("{shown} is not a repository-relative path"));
    }
    let path: PathBuf = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if path.as_os_str().is_empty() {
        return Ok(None);
    }
    if roots.iter().any(|root| {
        let full = root.join(&path);
        full.is_file() || full.is_dir()
    }) {
        return Ok(Some(path));
    }
    let mut candidates = Vec::new();
    for root in roots {
        candidates.extend(repository_paths(root));
    }
    candidates.sort();
    candidates.dedup();
    let same: Vec<String> = candidates
        .iter()
        .filter(|candidate| candidate.file_name() == path.file_name())
        .map(|candidate| candidate.display().to_string())
        .collect();
    Err(if same.is_empty() {
        format!("{shown} is nothing in the repository")
    } else {
        format!(
            "{shown} is nothing in the repository; did you mean {}?",
            same.join(", ")
        )
    })
}

fn repository_paths(root: &Path) -> Vec<PathBuf> {
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

fn headless_reply(
    dirs: &XdgDirs,
    target: &Target,
    root: &Path,
    thread: &ThreadId,
    author: Author,
    item: &ReplyItem,
    lines: Option<LineRange>,
) -> Result<Thread, String> {
    let mut store =
        Store::open(dirs.threads_file(&target.key)).map_err(|error| error.to_string())?;
    if store.thread(thread).is_none() {
        return Err(format!("thread {thread} disappeared before the reply"));
    }
    let when = now();
    if let Some(lines) = lines {
        follow_reply_lines(&mut store, root, thread, lines, when)?;
    }
    let reply = Reply::new(author, when, item.body.clone());
    let reply = if item.resolve {
        reply.proposing_resolution()
    } else {
        reply
    };
    store
        .reply(thread, reply)
        .map_err(|error| error.to_string())?;
    tracing::info!(%thread, proposes = item.resolve, "agent reply added headlessly");
    store
        .thread(thread)
        .cloned()
        .ok_or_else(|| format!("thread {thread} vanished after the reply"))
}

pub(super) fn with_summary(value: Value, summary: String) -> CallToolResult {
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(summary)];
    result
}

pub(super) fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

/// Instructions supplied to every MCP client.
pub(super) fn instructions() -> String {
    "Fathomable holds review discussions attached to files in this repository. \
     When asked, read the relevant threads and their history. Use thread_start \
     for new findings or questions and thread_reply to continue existing \
     discussions. Treat proposals as proposals; follow the user's stated \
     decisions and your assigned task. Reading a thread does not authorize \
     changes. Only the user closes threads."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Status, Store, ThreadId};
    use fathomable_core::clock::now;
    use fathomable_core::vocabulary::ALL;
    use fathomable_testing::TempDir;

    use crate::mcp::Server;

    use super::{
        ReplyItem, Shown, ThreadsParams, Tree, check_path, refusal, select_exact, select_filtered,
    };

    #[test]
    fn vocabulary_matches_the_live_three_tool_schema() -> Result<(), String> {
        let tools = Server::router().list_all();
        assert_eq!(tools.len(), ALL.len());
        for expected in ALL {
            let tool = tools
                .iter()
                .find(|tool| tool.name == expected.name)
                .ok_or_else(|| format!("missing tool {}", expected.name))?;
            let mut actual: Vec<&str> = tool.input_schema["properties"]
                .as_object()
                .ok_or_else(|| format!("{} has no properties", expected.name))?
                .keys()
                .map(String::as_str)
                .collect();
            actual.sort_unstable();
            let mut expected_params = expected.params.to_vec();
            expected_params.sort_unstable();
            assert_eq!(actual, expected_params, "{}", expected.name);
        }
        Ok(())
    }

    #[test]
    fn path_filters_are_repository_relative_and_name_nearby_matches()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-path")?;
        fs::create_dir_all(dir.0.join("src"))?;
        fs::write(dir.0.join("src/lib.rs"), "one\n")?;
        assert_eq!(
            check_path(&dir.0, Some(Path::new("./src")))?,
            Some(PathBuf::from("src"))
        );
        assert_eq!(
            check_path(&dir.0, Some(Path::new("lib.rs")))
                .err()
                .as_deref(),
            Some("lib.rs is nothing in the repository; did you mean src/lib.rs?")
        );
        Ok(())
    }

    #[test]
    fn resolved_reads_keep_the_complete_conversation() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-history")?;
        fs::write(dir.0.join("a.md"), "one\n")?;
        let path = dir.0.join("threads.jsonl");
        let mut store = Store::open(&path)?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "Should we rename this?",
            ),
            "one\n",
            1,
        )?;
        store.reply(
            &id,
            Reply::new(Author::agent("Copilot"), 2, "I propose `other`."),
        )?;
        store.resolve(&id, None, 3)?;
        let thread = store.thread(&id).ok_or("thread")?;
        let mut tree = Tree::new(&dir.0);
        let shown = Shown::new(thread, tree.place(thread));
        let value = serde_json::to_value(shown)?;
        assert_eq!(value["status"], "resolved");
        assert_eq!(value["author"], "user");
        assert_eq!(value["comment"], "Should we rename this?");
        assert_eq!(value["replies"][0]["body"], "I propose `other`.");
        Ok(())
    }

    #[test]
    fn exact_ids_preserve_order_and_filters_page_open_threads()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-selection")?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let first = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "first",
            ),
            "one\n",
            1,
        )?;
        let second = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "second",
            ),
            "one\n",
            2,
        )?;
        let all = store.threads();
        let exact = select_exact(all, &[second.to_string(), first.to_string()])?;
        assert_eq!(exact.threads[0].id(), &second);
        assert_eq!(exact.threads[1].id(), &first);
        let filtered = select_filtered(
            std::slice::from_ref(&dir.0),
            all,
            &ThreadsParams {
                status: None,
                path: None,
                since: None,
                after: None,
                limit: Some(1),
                ids: Vec::new(),
            },
        )?;
        assert_eq!(filtered.threads[0].id(), &first);
        assert_eq!(filtered.more, 1);
        Ok(())
    }

    #[test]
    fn paging_progresses_when_more_than_a_page_has_one_timestamp()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-page-ties")?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        for index in 0..51 {
            store.annotate(
                Draft::new(
                    Author::User,
                    Path::new("a.md"),
                    LineRange::new(1, 1),
                    format!("thread {index}"),
                ),
                "one\n",
                7,
            )?;
        }
        let first = select_filtered(
            std::slice::from_ref(&dir.0),
            store.threads(),
            &ThreadsParams {
                status: None,
                path: None,
                since: None,
                after: None,
                limit: Some(50),
                ids: Vec::new(),
            },
        )?;
        assert_eq!(first.threads.len(), 50);
        assert_eq!(first.more, 1);
        let after = first.next_after.clone().ok_or("missing continuation")?;
        let first_ids: HashSet<&ThreadId> =
            first.threads.iter().map(|thread| thread.id()).collect();
        let second = select_filtered(
            std::slice::from_ref(&dir.0),
            store.threads(),
            &ThreadsParams {
                status: None,
                path: None,
                since: None,
                after: Some(after),
                limit: Some(50),
                ids: Vec::new(),
            },
        )?;
        assert_eq!(second.threads.len(), 1);
        assert_eq!(second.more, 0);
        assert!(second.next_after.is_none());
        assert!(!first_ids.contains(second.threads[0].id()));
        Ok(())
    }

    #[test]
    fn reply_prevalidation_catches_every_expected_input_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-reply-check")?;
        fs::write(dir.0.join("a.md"), "one\n")?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "why?",
            ),
            "one\n",
            now(),
        )?;
        let all = store.threads();
        let mut tree = Tree::new(&dir.0);
        for (item, needle) in [
            (
                ReplyItem {
                    thread: id.to_string(),
                    body: " ".to_owned(),
                    resolve: false,
                    line: None,
                    end_line: None,
                },
                "body",
            ),
            (
                ReplyItem {
                    thread: id.to_string(),
                    body: "x".to_owned(),
                    resolve: false,
                    line: None,
                    end_line: Some(2),
                },
                "pass `line`",
            ),
            (
                ReplyItem {
                    thread: id.to_string(),
                    body: "x".to_owned(),
                    resolve: false,
                    line: Some(2),
                    end_line: None,
                },
                "past the end",
            ),
        ] {
            let placement = tree.place(&all[0]);
            let error = refusal(&item, &all[0], placement, &dir.0)
                .err()
                .unwrap_or_default();
            assert!(error.contains(needle), "{error}");
        }
        assert_eq!(all[0].status(), Status::Open);
        Ok(())
    }
}
