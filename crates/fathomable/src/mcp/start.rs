// @okf-doc: /decisions/0061-agents-start-threads.md
//! Start one or more review discussions in the bound checkout.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use fathomable_core::XdgDirs;
use fathomable_core::annotations::{
    Author, ContentIdentity, Draft, LineRange, MAX_MESSAGE_BYTES, OriginSide, OriginVersion,
    Provenance, Store, Thread,
};
use fathomable_core::clock::now;
use fathomable_core::workspace::{CommitId, Workspace};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{RoleServer, schemars, tool, tool_router};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

use crate::app::threads::{agent_start_draft, read_checkout_text};

use super::Server;
use super::source::Source;
use super::tools::{
    BatchIssue, SelectedWriteOutput, Shown, StartSuccess, Tree, WriteOutput, check_path, failure,
    invalid_batch, invalid_source, require_line_for_end_line, shown_lines, structured_error,
};

/// One comment in a `thread_start` batch.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_line_for_end_line)]
pub(crate) struct StartItem {
    /// Repository-relative path of the file.
    path: PathBuf,
    /// First line the comment is on, 1-based. Omit it only for a comment
    /// on the file as a whole.
    #[schemars(range(min = 1))]
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line`.
    #[schemars(range(min = 1))]
    #[serde(default)]
    end_line: Option<usize>,
    /// PR-style review comment, at most 1024 UTF-8 bytes for a fresh write.
    ///
    /// Start one independently actionable finding on the narrowest relevant
    /// file or line range. Substantial replacements belong in the worktree.
    body: String,
    /// Optional retry key: non-whitespace text, at most 256 UTF-8 bytes.
    #[schemars(length(min = 1, max = 256))]
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// `thread_start` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartParams {
    /// Select one exact immutable commit origin for every comment.
    ///
    /// Omit this only for `WorkingTree`-origin comments. The checkout's
    /// observed `HEAD` never turns a `WorkingTree` origin into a commit origin.
    #[serde(default)]
    source: Option<Source>,
    /// One or more new comments. The whole batch is validated before any write.
    #[schemars(length(min = 1))]
    comments: Vec<StartItem>,
}

/// A comment the batch check passed: where it goes and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    path: PathBuf,
    /// `None` for a comment on the file as a whole (ADR 0063).
    range: Option<LineRange>,
    body: String,
    idempotency_key: Option<String>,
}

#[tool_router(router = tool_router_start, vis = "pub(super)")]
impl Server {
    #[tool(
        name = "thread_start",
        output_schema = rmcp::handler::server::tool::schema_for_output::<StartSuccess>(),
        description = "Start one or more new review discussions. Pass exactly one non-empty \
                       `comments` array. Start one discussion per independently actionable finding, \
                       placed on the narrowest relevant repository file or optional 1-based line \
                       range. Each fresh Markdown body is at most 1024 UTF-8 bytes; substantial \
                       replacements belong in the worktree. Omit `line` only for a file-level \
                       comment. An optional per-item `idempotency_key` makes a retry, including a \
                       historical larger body, replay the same discussion instead of creating \
                       another one. To review a commit, pass \
                       `source:{kind:\"commit\",revision}`; it captures raw text from `HEAD` or one \
                       full local commit ID under a 64 MiB call budget and returns \
                       `resolved_commit`. Omitting `source` creates a WorkingTree origin; an \
                       observed checkout HEAD does not make that a commit origin. The whole batch \
                       is validated before any discussion is written.",
        annotations(
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    #[expect(
        clippy::too_many_lines,
        clippy::needless_pass_by_value,
        reason = "The MCP macro supplies an owned request context, and the handler keeps batch validation together."
    )]
    fn thread_start(
        &self,
        Parameters(p): Parameters<StartParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        if p.comments.is_empty() {
            return failure("`comments` must contain at least one comment");
        }
        let (author, caller) = match self.signer(&context) {
            Ok(identity) => identity,
            Err(error) => return failure(error),
        };
        if let Some(source) = p.source.as_ref() {
            return self.thread_start_selected(&p.comments, source, &author, &caller);
        }

        // Check the whole batch before writing any of it, so that a retry
        // with the fixed list is a whole retry.
        let mut keys = HashSet::with_capacity(p.comments.len());
        let mut placed = Vec::with_capacity(p.comments.len());
        let mut problems = Vec::new();
        let probe_store = if p.comments.iter().any(|item| item.idempotency_key.is_some()) {
            match Store::open_workspace(&self.dirs, &self.target.key) {
                Ok(store) => Some(store),
                Err(error) => return failure(error.to_string()),
            }
        } else {
            None
        };
        for (index, item) in p.comments.iter().enumerate() {
            if let Some(key) = &item.idempotency_key
                && !keys.insert(key.clone())
            {
                problems.push(BatchIssue::new(
                    index,
                    format!(
                        "{} appears more than once in `comments` by `idempotency_key` {key:?}",
                        item.path.display()
                    ),
                ));
                continue;
            }
            let (path, range) = match structural_start(item) {
                Ok(shape) => shape,
                Err(error) => {
                    problems.push(BatchIssue::new(index, error));
                    continue;
                }
            };
            let replay = if let (Some(key), Some(store)) =
                (item.idempotency_key.as_deref(), probe_store.as_ref())
            {
                let draft = match range {
                    Some(range) => Draft::new(author.clone(), &path, range, item.body.clone()),
                    None => Draft::on_file(author.clone(), &path, item.body.clone()),
                };
                match store.probe_start_idempotency_for_caller(&draft, &caller, key) {
                    Ok(replay) => replay.is_some(),
                    Err(error) => {
                        problems.push(BatchIssue::new(
                            index,
                            format!("{}: {error}", item.path.display()),
                        ));
                        continue;
                    }
                }
            } else {
                false
            };
            if replay {
                placed.push((
                    index,
                    Placed {
                        path,
                        range,
                        body: item.body.clone(),
                        idempotency_key: item.idempotency_key.clone(),
                    },
                ));
            } else {
                match place(&self.target.root, item) {
                    Ok(item) => placed.push((index, item)),
                    Err(error) => problems.push(BatchIssue::new(index, error)),
                }
            }
        }
        if !problems.is_empty() {
            return invalid_batch("comments", &problems);
        }

        let mut tree = Tree::new(&self.target.root);
        let mut lines = Vec::new();
        let mut started = Vec::new();
        for (index, item) in placed {
            match self.start_one(author.clone(), &caller, item) {
                Ok(thread) => started.push(thread),
                Err(error) => {
                    lines.extend(shown_lines(&started, &mut tree, "started"));
                    lines.push(format!("comments[{index}]: {error}"));
                    return failure(lines.join("\n"));
                }
            }
        }
        let mut shown = Vec::with_capacity(started.len());
        for thread in &started {
            let placement = match tree.try_place(thread) {
                Ok(placement) => placement,
                Err(error) => return failure(error),
            };
            shown.push(Shown::new(thread, placement));
        }
        CallToolResult::structured(json!(StartSuccess::Ordinary(WriteOutput {
            checkout: self.target.root.clone(),
            threads: shown,
        })))
    }
}

impl Server {
    #[expect(
        clippy::too_many_lines,
        reason = "Selected starts keep whole-batch validation visibly ahead of persistence."
    )]
    fn thread_start_selected(
        &self,
        comments: &[StartItem],
        source: &Source,
        author: &Author,
        caller: &str,
    ) -> CallToolResult {
        let request = match source.request() {
            Ok(request) => request,
            Err(error) => return invalid_source(error, None),
        };
        let mut structural_keys = HashSet::with_capacity(comments.len());
        let mut structural_problems = Vec::new();
        for (index, item) in comments.iter().enumerate() {
            if let Some(key) = &item.idempotency_key
                && !structural_keys.insert(key)
            {
                structural_problems.push(BatchIssue::new(
                    index,
                    format!("{} repeats idempotency key {key:?}", item.path.display()),
                ));
            }
            if let Err(error) = structural_start(item) {
                structural_problems.push(BatchIssue::new(index, error));
            }
        }
        if !structural_problems.is_empty() {
            return invalid_batch("comments", &structural_problems);
        }
        let (commit, mut capture) = match &request {
            super::source::CommitRequest::Head => {
                match super::source::SelectedCommit::resolve(&self.target, &request) {
                    Ok(selected) => (selected.id().clone(), Some(selected.into_capture())),
                    Err(error) => return invalid_source(error, None),
                }
            }
            super::source::CommitRequest::Id(commit) => {
                if let Err(error) = super::source::validate_binding(&self.target) {
                    return invalid_source(error, Some(commit.as_str()));
                }
                (commit.clone(), None)
            }
        };
        let probe_store = match Store::open_workspace(&self.dirs, &self.target.key) {
            Ok(store) => store,
            Err(error) => return failure(error.to_string()),
        };
        let mut keys = HashSet::with_capacity(comments.len());
        let mut prepared = Vec::with_capacity(comments.len());
        let mut problems = Vec::new();

        for (index, item) in comments.iter().enumerate() {
            if let Some(key) = &item.idempotency_key
                && !keys.insert(key.clone())
            {
                problems.push(BatchIssue::new(
                    index,
                    format!("{} repeats idempotency key {key:?}", item.path.display()),
                ));
                continue;
            }
            let (path, range) = match structural_start(item) {
                Ok(shape) => shape,
                Err(error) => {
                    problems.push(BatchIssue::new(index, error));
                    continue;
                }
            };
            let probe = selected_draft(
                author.clone(),
                &path,
                range,
                item.body.clone(),
                &commit,
                None,
            );
            let replay = match item.idempotency_key.as_deref() {
                Some(key) => match selected_replay_path(&probe_store, &probe, caller, key) {
                    Ok(replay) => replay,
                    Err(error) => {
                        problems.push(BatchIssue::new(
                            index,
                            format!("{}: {error}", path.display()),
                        ));
                        continue;
                    }
                },
                None => None,
            };
            if let Some(projected_path) = replay {
                prepared.push(PreparedStart {
                    index,
                    item: Placed {
                        path,
                        range,
                        body: item.body.clone(),
                        idempotency_key: item.idempotency_key.clone(),
                    },
                    text: None,
                    projected_path,
                    preparation: Preparation::Replay,
                });
                continue;
            }
            if item.body.len() > MAX_MESSAGE_BYTES {
                problems.push(BatchIssue::new(
                    index,
                    format!(
                        "{}: `body` has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
                        path.display(),
                        item.body.len()
                    ),
                ));
                continue;
            }
            prepared.push(PreparedStart {
                index,
                item: Placed {
                    path: path.clone(),
                    range,
                    body: item.body.clone(),
                    idempotency_key: item.idempotency_key.clone(),
                },
                text: None,
                projected_path: path,
                preparation: Preparation::Fresh,
            });
        }
        if !problems.is_empty() {
            return invalid_batch("comments", &problems);
        }

        if prepared
            .iter()
            .any(|item| item.preparation == Preparation::Fresh)
        {
            if capture.is_none() {
                capture = match super::source::SelectedCommit::resolve(&self.target, &request) {
                    Ok(selected) => Some(selected.into_capture()),
                    Err(error) => return invalid_source(error, Some(commit.as_str())),
                };
            }
            let Some(capture) = capture.as_mut() else {
                return failure("selected source capture was not initialized");
            };
            for item in &mut prepared {
                if item.preparation == Preparation::Replay {
                    continue;
                }
                let text = match capture.text(&item.item.path) {
                    Ok(text) => text.to_owned(),
                    Err(error) => {
                        problems.push(BatchIssue::new(item.index, error));
                        continue;
                    }
                };
                let count = text.lines().count();
                if let Some(range) = item.item.range
                    && range.end() > count
                {
                    problems.push(BatchIssue::new(
                        item.index,
                        format!(
                            "lines {range} are past the end of {} ({count} line{})",
                            item.item.path.display(),
                            if count == 1 { "" } else { "s" }
                        ),
                    ));
                    continue;
                }
                item.text = Some(text);
            }
        }
        if !problems.is_empty() {
            return invalid_batch("comments", &problems);
        }

        let mut tree = Tree::new(&self.target.root);
        for item in &prepared {
            if let Err(error) = tree.preflight(&item.projected_path) {
                problems.push(BatchIssue::new(item.index, error));
            }
        }
        if !problems.is_empty() {
            return invalid_batch("comments", &problems);
        }

        let mut started = Vec::with_capacity(prepared.len());
        for (position, item) in prepared.iter().enumerate() {
            match headless_start_selected(
                &self.dirs,
                &self.target.key,
                author.clone(),
                caller,
                item,
                &commit,
            ) {
                Ok(thread) => started.push((item.index, thread)),
                Err(error) => {
                    return selected_partial_failure(
                        &self.target.root,
                        &commit,
                        &started,
                        item.index,
                        error,
                        &prepared[position + 1..],
                    );
                }
            }
        }
        let mut shown = Vec::with_capacity(started.len());
        for (_, thread) in &started {
            let placement = match tree.try_place(thread) {
                Ok(placement) => placement,
                Err(error) => return failure(error),
            };
            shown.push(Shown::new(thread, placement));
        }
        CallToolResult::structured(json!(StartSuccess::Selected(SelectedWriteOutput {
            checkout: self.target.root.clone(),
            resolved_commit: commit.as_str().to_owned(),
            threads: shown,
        })))
    }

    /// Write one validated comment to the shared store.
    fn start_one(&self, author: Author, caller: &str, item: Placed) -> Result<Thread, String> {
        let place = match item.range {
            Some(range) => format!("{}:{}", item.path.display(), range.start()),
            None => item.path.display().to_string(),
        };
        headless_start(
            &self.dirs,
            &self.target.key,
            &self.target.root,
            author,
            caller,
            item,
        )
        .map_err(|message| format!("{place}: {message}"))
    }
}

#[derive(Debug)]
struct PreparedStart {
    index: usize,
    item: Placed,
    text: Option<String>,
    projected_path: PathBuf,
    preparation: Preparation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preparation {
    Replay,
    Fresh,
}

fn selected_replay_path(
    store: &Store,
    draft: &Draft,
    caller: &str,
    key: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(id) = store
        .probe_start_idempotency_for_caller(draft, caller, key)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    // The receipt probe reads a newer snapshot than the caller's store.
    let store = Store::open(store.path()).map_err(|error| error.to_string())?;
    store
        .thread(&id)
        .map(|thread| Some(thread.path().to_path_buf()))
        .ok_or_else(|| format!("thread {id} vanished"))
}

fn selected_draft(
    author: Author,
    path: &Path,
    range: Option<LineRange>,
    body: String,
    commit: &CommitId,
    text: Option<&str>,
) -> Draft {
    let draft = match range {
        Some(range) => Draft::new(author, path, range, body),
        None => Draft::on_file(author, path, body),
    }
    .at_selected_commit(commit.clone());
    match text {
        Some(text) => draft.with_provenance(
            Provenance::new(
                OriginVersion::commit(commit.as_str()),
                OriginSide::Unspecified,
            )
            .with_content(ContentIdentity::from_text(text)),
        ),
        None => draft,
    }
}

fn headless_start_selected(
    dirs: &XdgDirs,
    key: &Path,
    author: Author,
    caller: &str,
    prepared: &PreparedStart,
    commit: &CommitId,
) -> Result<Thread, String> {
    let item = &prepared.item;
    let draft = selected_draft(
        author,
        &item.path,
        item.range,
        item.body.clone(),
        commit,
        prepared.text.as_deref(),
    );
    let mut store = Store::open_workspace(dirs, key).map_err(|error| error.to_string())?;
    let id = match item.idempotency_key.as_deref() {
        Some(retry) => store
            .annotate_idempotent_for_caller(draft, now(), caller, retry, |_| {
                prepared.text.clone().ok_or_else(|| {
                    fathomable_core::annotations::StoreError::message(
                        "matching selected replay unexpectedly requested source text",
                    )
                })
            })
            .map_err(|error| error.to_string())?
            .into_value(),
        None => store
            .annotate(
                draft,
                prepared
                    .text
                    .as_deref()
                    .ok_or_else(|| "fresh selected start has no captured text".to_owned())?,
                now(),
            )
            .map_err(|error| error.to_string())?,
    };
    store
        .thread(&id)
        .cloned()
        .ok_or_else(|| format!("thread {id} vanished after the comment"))
}

#[derive(Debug, Serialize)]
struct CompletedStart {
    item_index: usize,
    thread: String,
}

#[derive(Debug, Serialize)]
struct FailedStart {
    item_index: usize,
    error: String,
}

#[derive(Debug, Serialize)]
struct UnattemptedStart {
    item_index: usize,
}

fn selected_partial_failure(
    root: &Path,
    commit: &CommitId,
    completed: &[(usize, Thread)],
    failed: usize,
    error: String,
    unattempted: &[PreparedStart],
) -> CallToolResult {
    structured_error(json!({
        "error_code": "PARTIAL_BATCH",
        "checkout": root,
        "resolved_commit": commit.as_str(),
        "completed": completed.iter().map(|(item_index, thread)| CompletedStart {
            item_index: *item_index,
            thread: thread.id().to_string(),
        }).collect::<Vec<_>>(),
        "failed": FailedStart { item_index: failed, error },
        "unattempted": unattempted.iter().map(|item| UnattemptedStart {
            item_index: item.index,
        }).collect::<Vec<_>>(),
    }))
}

/// Where `item` goes, or why it cannot go there: the path is not a file
/// in the repository, the file is not text, the range runs past its end,
/// or the body is empty or over the message limit. An item with no line
/// is a comment on the file as a whole (ADR 0063).
fn place(root: &Path, item: &StartItem) -> Result<Placed, String> {
    let shown = item.path.display();
    let path = check_path(root, Some(&item.path))?
        .ok_or_else(|| format!("{shown} is the repository root; pass a file"))?;
    let full = root.join(&path);
    if full.is_dir() {
        return Err(format!("{shown} is a directory; pass a file"));
    }
    let text = read_checkout_text(root, &path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::InvalidData {
            format!("{shown} is not a text file")
        } else {
            format!("cannot read {shown} within the repository: {error}")
        }
    })?;
    let (_, range) = structural_start(item)?;
    if item.body.len() > MAX_MESSAGE_BYTES {
        return Err(match item.line {
            Some(line) => format!(
                "{shown}:{line}: `body` has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
                item.body.len()
            ),
            None => format!(
                "{shown}: `body` has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
                item.body.len()
            ),
        });
    }
    let count = text.lines().count();
    if let Some(range) = range
        && range.end() > count
    {
        return Err(format!(
            "lines {range} are past the end of {shown} ({count} line{})",
            if count == 1 { "" } else { "s" }
        ));
    }
    Ok(Placed {
        path,
        range,
        body: item.body.clone(),
        idempotency_key: item.idempotency_key.clone(),
    })
}

fn structural_start(item: &StartItem) -> Result<(PathBuf, Option<LineRange>), String> {
    let shown = item.path.display();
    let path = lexical_path(&item.path)?;
    if item.line.is_none() && item.end_line.is_some() {
        return Err(format!("{shown}: pass `line` with `end_line`"));
    }
    if item.line == Some(0) || item.end_line == Some(0) {
        return Err(format!("{shown}: line numbers are 1-based"));
    }
    if let (Some(line), Some(end_line)) = (item.line, item.end_line)
        && end_line < line
    {
        return Err(format!(
            "{shown}: `end_line` ({end_line}) must be at least `line` ({line})"
        ));
    }
    if item.body.trim().is_empty() {
        return Err(match item.line {
            Some(line) => format!("{shown}:{line}: `body` is empty"),
            None => format!("{shown}: `body` is empty"),
        });
    }
    let range = item
        .line
        .map(|line| LineRange::new(line, item.end_line.unwrap_or(line)));
    Ok((path, range))
}

fn lexical_path(path: &Path) -> Result<PathBuf, String> {
    let shown = path.display();
    if path.to_string_lossy().contains('\0') {
        return Err(format!("{shown} contains a NUL byte"));
    }
    let inside = path.is_relative()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if !inside {
        return Err(format!("{shown} is not a repository-relative path"));
    }
    let normalized: PathBuf = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect();
    if normalized.as_os_str().is_empty() {
        return Err(format!("{shown} is the repository root; pass a file"));
    }
    Ok(normalized)
}

/// Start the thread in the store, stamped with the repository's `HEAD`,
/// and answer with it.
fn headless_start(
    dirs: &XdgDirs,
    key: &Path,
    root: &Path,
    author: Author,
    caller: &str,
    item: Placed,
) -> Result<Thread, String> {
    let mut store = Store::open_workspace(dirs, key).map_err(|e| e.to_string())?;
    let id = if let Some(idempotency_key) = item.idempotency_key.as_deref() {
        let probe = match item.range {
            Some(range) => Draft::new(author.clone(), &item.path, range, item.body.clone()),
            None => Draft::on_file(author.clone(), &item.path, item.body.clone()),
        };
        if let Some(id) = store
            .probe_start_idempotency_for_caller(&probe, caller, idempotency_key)
            .map_err(|error| error.to_string())?
        {
            id
        } else {
            let mut workspace = Workspace::discover(root).map_err(|error| error.to_string())?;
            let (draft, text) =
                agent_start_draft(&mut workspace, author, &item.path, item.range, item.body)?;
            store
                .annotate_idempotent_for_caller(draft, now(), caller, idempotency_key, |_| {
                    Ok(text.clone())
                })
                .map_err(|e| e.to_string())?
                .into_value()
        }
    } else {
        let mut workspace = Workspace::discover(root).map_err(|error| error.to_string())?;
        let (draft, text) =
            agent_start_draft(&mut workspace, author, &item.path, item.range, item.body)?;
        store
            .annotate(draft, &text, now())
            .map_err(|e| e.to_string())?
    };
    tracing::info!(%id, path = %item.path.display(), "agent thread started");
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
    use fathomable_core::annotations::{Author, Draft, LineRange, MAX_MESSAGE_BYTES, Store};
    use fathomable_core::workspace::CommitId;
    use fathomable_testing::TempDir;

    use super::{Placed, StartItem, headless_start, place, selected_replay_path};
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
            idempotency_key: None,
        }
    }

    #[test]
    fn commit_source_probe_observes_a_concurrent_completed_start()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-selected-concurrent-probe")?;
        let file = dir.0.join("threads.jsonl");
        let before = Store::open(&file)?;
        let commit = CommitId::parse("1111111111111111111111111111111111111111")?;
        let draft = Draft::on_file(Author::agent("reviewer"), Path::new("a.md"), "comment")
            .at_selected_commit(commit);
        Store::open(&file)?.annotate_idempotent_for_caller(
            draft.clone(),
            1,
            "copilot:concurrent",
            "same",
            |_| Ok("text\n".to_owned()),
        )?;
        assert_eq!(
            selected_replay_path(&before, &draft, "copilot:concurrent", "same")?,
            Some(PathBuf::from("a.md"))
        );
        Ok(())
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
                idempotency_key: None,
            })
        );
        // No line is a comment on the file as a whole (ADR 0063).
        assert_eq!(
            place(&root, &item("src/lib.rs", None, None, "split this")),
            Ok(Placed {
                path: PathBuf::from("src/lib.rs"),
                range: None,
                body: "split this".to_owned(),
                idempotency_key: None,
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
                item("src/lib.rs", None, Some(2), "x"),
                "src/lib.rs: pass `line` with `end_line`",
            ),
            (
                item("src/lib.rs", Some(0), None, "x"),
                "src/lib.rs: line numbers are 1-based",
            ),
            (
                item("../lib.rs", Some(1), None, "x"),
                "../lib.rs is not a repository-relative path",
            ),
        ];
        for (item, expected) in refused {
            assert_eq!(place(&root, &item).err().as_deref(), Some(expected));
        }
        let oversized = "é".repeat(MAX_MESSAGE_BYTES / 2 + 1);
        assert_eq!(
            place(&root, &item("src/lib.rs", Some(1), None, &oversized)).err(),
            Some(format!(
                "src/lib.rs:1: `body` has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
                oversized.len()
            ))
        );
        let elsewhere = place(&root, &item("lib.rs", Some(1), None, "x"));
        assert_eq!(
            elsewhere.err().as_deref(),
            Some("lib.rs is nothing in the repository; did you mean src/lib.rs?")
        );
        Ok(())
    }

    #[test]
    fn headless_start_rejects_symlink_swaps_after_validation()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        for replace_directory in [false, true] {
            let dir = testing::bare("mcp-start-symlink-swap")?;
            let dirs = dirs(&dir);
            let root = dir.0.join("ws").canonicalize()?;
            fs::create_dir(root.join("nested"))?;
            fs::write(root.join("nested/a.md"), "inside\n")?;
            let outside = dir.0.join("outside");
            fs::create_dir(&outside)?;
            fs::write(outside.join("a.md"), "synthetic outside-only evidence\n")?;
            let placed = place(&root, &item("nested/a.md", Some(1), None, "review"))?;

            if replace_directory {
                fs::rename(root.join("nested"), root.join("old-nested"))?;
                symlink(&outside, root.join("nested"))?;
            } else {
                fs::remove_file(root.join("nested/a.md"))?;
                symlink(outside.join("a.md"), root.join("nested/a.md"))?;
            }
            let result = headless_start(
                &dirs,
                &root,
                &root,
                Author::agent("reviewer"),
                "test:symlink-swap",
                placed,
            );
            assert!(result.is_err(), "{result:?}");
            assert!(Store::open(dirs.threads_file(&root))?.threads().is_empty());
        }
        Ok(())
    }

    /// Without a viewer the thread is written to the store with its
    /// harness-qualified author identity.
    #[test]
    fn headless_start_writes_the_agents_thread() -> Result<(), Box<dyn std::error::Error>> {
        let dir = testing::bare("mcp-start-headless")?;
        let dirs = dirs(&dir);
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\ntwo\n")?;
        let author = Author::Agent {
            name: "Copilot".to_owned(),
            client: Some("copilot-cli".to_owned()),
            id: Some("copilot:s1".to_owned()),
        };
        let thread = headless_start(
            &dirs,
            &root,
            &root,
            author.clone(),
            "copilot:s1",
            Placed {
                path: PathBuf::from("a.md"),
                range: Some(LineRange::new(2, 2)),
                body: "look here".to_owned(),
                idempotency_key: None,
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
