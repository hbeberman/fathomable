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
    AgentReplyCommand, Author, AutoResolve, Lifecycle, LineHashes, LineRange, Message, Placement,
    Reply, ResolutionOutcome, Status, Store, Thread, ThreadId,
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
/// `threads` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = exclusive_ids_lookup)]
pub(crate) struct ThreadsParams {
    /// Which discussions to list: `open` (default), `resolved`, or `all`.
    #[serde(default)]
    status: Option<StatusFilter>,
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

/// The status selector accepted by `threads`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum StatusFilter {
    /// Discussions that are still open.
    #[default]
    Open,
    /// Discussions resolved by the user.
    Resolved,
    /// Discussions in either state.
    All,
}

impl<'de> Deserialize<'de> for StatusFilter {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "open" => Ok(Self::Open),
            "resolved" => Ok(Self::Resolved),
            "all" => Ok(Self::All),
            other => Err(D::Error::custom(format!(
                "`status` is `{}`, `{}`, or `{}`, not {other:?}",
                vocab::STATUS_OPEN,
                vocab::WHEN_RESOLVED,
                vocab::STATUS_ALL
            ))),
        }
    }
}

/// One reply in a `thread_reply` batch.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = require_line_for_end_line)]
pub(crate) struct ReplyItem {
    /// Discussion id from `threads`.
    thread: String,
    /// Reply text in Markdown.
    body: String,
    /// This reply completes the work and should resolve when authorized.
    #[serde(default)]
    resolve: bool,
    /// Optional retry key: non-whitespace text, at most 256 UTF-8 bytes.
    #[schemars(length(min = 1, max = 256))]
    #[serde(default)]
    idempotency_key: Option<String>,
    /// First line the discussion's lines occupy now, when they moved.
    #[schemars(range(min = 1))]
    #[serde(default)]
    line: Option<usize>,
    /// Last line of that range; defaults to `line`.
    #[schemars(range(min = 1))]
    #[serde(default)]
    end_line: Option<usize>,
}

/// `thread_reply` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplyParams {
    /// One or more replies. The whole batch is validated before any write.
    #[schemars(length(min = 1))]
    replies: Vec<ReplyItem>,
}

const DEFAULT_LIMIT: usize = 50;

/// Add the cross-field range constraint shared by start and reply items.
pub(super) fn require_line_for_end_line(schema: &mut schemars::Schema) {
    schema.insert(
        "allOf".to_owned(),
        json!([{
            "if": {
                "required": ["end_line"],
                "properties": {
                    "end_line": { "type": "integer" }
                }
            },
            "then": {
                "required": ["line"],
                "properties": {
                    "line": { "type": "integer" }
                }
            }
        }]),
    );
}

/// Add the exclusive non-empty `ids` lookup constraint to the input schema.
fn exclusive_ids_lookup(schema: &mut schemars::Schema) {
    if let Some(Value::Object(properties)) = schema.get_mut("properties")
        && let Some(Value::Object(ids)) = properties.get_mut("ids")
    {
        ids.insert("uniqueItems".to_owned(), true.into());
    }
    schema.insert(
        "allOf".to_owned(),
        json!([{
            "if": {
                "required": ["ids"],
                "properties": {
                    "ids": { "minItems": 1 }
                }
            },
            "then": {
                "properties": {
                    "status": { "type": "null" },
                    "path": { "type": "null" },
                    "since": { "type": "null" },
                    "after": { "type": "null" },
                    "limit": { "type": "null" }
                }
            }
        }]),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Open,
    Resolved,
    All,
}

impl Which {
    fn from_status(status: Option<StatusFilter>) -> Self {
        match status.unwrap_or_default() {
            StatusFilter::Open => Self::Open,
            StatusFilter::Resolved => Self::Resolved,
            StatusFilter::All => Self::All,
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
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct Shown {
    /// Stable discussion identifier.
    id: String,
    /// Repository-relative annotated file.
    path: PathBuf,
    /// The currently projected range, or the last-known range when detached.
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<RangeOutput>,
    /// Whether the current content is anchored, edited, detached, or file-wide.
    placement: PlacementOutput,
    /// The stored range used as the comparison reference for `location`.
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor_range: Option<RangeOutput>,
    /// Whether the projected range is unchanged, moved, detached, or file-wide.
    location: LocationOutput,
    /// Current discussion status.
    status: StatusOutput,
    /// Current active, resolution-proposed, or resolved lifecycle.
    lifecycle: LifecycleOutput,
    created: u64,
    /// Latest persisted thread change.
    modified: u64,
    /// Whether the next agent reply has one-shot resolution permission.
    auto_resolve: bool,
    /// Opening comment and replies in append order.
    messages: Vec<MessageOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reanchored_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<PathBuf>,
}

/// A 1-based inclusive source range in an MCP result.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
pub(super) struct RangeOutput {
    /// First line, inclusive and 1-based.
    #[schemars(range(min = 1))]
    start: usize,
    /// Last line, inclusive and 1-based.
    #[schemars(range(min = 1))]
    end: usize,
}

impl From<LineRange> for RangeOutput {
    fn from(range: LineRange) -> Self {
        Self {
            start: range.start(),
            end: range.end(),
        }
    }
}

/// The normalized author kind in an MCP result.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum AuthorKind {
    /// The human user at the keyboard.
    User,
    /// An attributed agent.
    Agent,
}

/// An MCP author object with a uniform tagged shape.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub(super) struct AuthorOutput {
    /// Whether the author is the human user or an agent.
    kind: AuthorKind,
    /// Stable display name; human authors use `user`.
    name: String,
    /// MCP client implementation, when recorded for an agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<String>,
    /// Harness-qualified agent identity, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

impl From<&Author> for AuthorOutput {
    fn from(author: &Author) -> Self {
        match author {
            Author::User => Self {
                kind: AuthorKind::User,
                name: "user".to_owned(),
                client: None,
                id: None,
            },
            Author::Agent { name, client, id } => Self {
                kind: AuthorKind::Agent,
                name: name.clone(),
                client: client.clone(),
                id: id.clone(),
            },
        }
    }
}

/// One uniformly projected thread message.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub(super) struct MessageOutput {
    author: AuthorOutput,
    body: String,
    created: u64,
    modified: u64,
    resolution_proposed: bool,
}

impl From<Message<'_>> for MessageOutput {
    fn from(message: Message<'_>) -> Self {
        Self {
            author: AuthorOutput::from(message.author()),
            body: message.body().to_owned(),
            created: message.created(),
            modified: message.modified(),
            resolution_proposed: message.resolution_proposed(),
        }
    }
}

/// Current content placement state in an MCP result.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum PlacementOutput {
    /// The stored anchor still matches the current content.
    Anchored,
    /// The current lines are an edited replacement of the stored content.
    Edited,
    /// The stored lines are no longer present.
    Detached,
    /// The discussion covers the file as a whole.
    File,
}

impl PlacementOutput {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Anchored => "anchored",
            Self::Edited => "edited",
            Self::Detached => "detached",
            Self::File => "file",
        }
    }
}

/// Location of the projected range relative to the stored anchor.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum LocationOutput {
    /// The projected range equals the stored reference.
    Unchanged,
    /// The projected range differs from the stored reference.
    Moved,
    /// No valid current placement exists; `range` is last-known only.
    Detached,
    /// The discussion is attached to the file as a whole.
    File,
}

/// The open or resolved status in an MCP result.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum StatusOutput {
    /// Awaiting action.
    Open,
    /// Resolved by the user.
    Resolved,
}

/// The current lifecycle in an MCP result.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum LifecycleOutput {
    /// Unresolved without a current resolution proposal.
    Active,
    /// Unresolved after an agent reported completion without permission.
    ResolutionProposed,
    /// Resolved by the user or an authorized agent reply.
    Resolved,
}

/// The complete result returned by a read.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct ThreadsOutput {
    threads: Vec<Shown>,
    more: usize,
    #[schemars(required)]
    next_after: Option<After>,
}

/// The complete result returned by a write.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct WriteOutput {
    pub(super) threads: Vec<Shown>,
}

/// The durable resolution result of one completed reply.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub(super) struct ResolutionResult {
    outcome: ResolutionOutcomeOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<ResolutionReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    guidance: Option<String>,
}

/// The resolution outcome values returned to an MCP caller.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ResolutionOutcomeOutput {
    /// The item did not ask to resolve.
    NotRequested,
    /// The item asked to resolve but awaits review in Fathomable.
    ResolutionProposed,
    /// The item resolved with one-shot permission.
    Resolved,
}

/// Why a requested resolution remains open.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ResolutionReason {
    /// The Fathomable user must review the proposal in Fathomable.
    PendingFathomableUserReview,
}

impl From<ResolutionOutcome> for ResolutionResult {
    fn from(outcome: ResolutionOutcome) -> Self {
        let pending = outcome == ResolutionOutcome::ResolutionProposed;
        Self {
            outcome: match outcome {
                ResolutionOutcome::NotRequested => ResolutionOutcomeOutput::NotRequested,
                ResolutionOutcome::ResolutionProposed => {
                    ResolutionOutcomeOutput::ResolutionProposed
                }
                ResolutionOutcome::Resolved => ResolutionOutcomeOutput::Resolved,
            },
            reason: pending.then_some(ResolutionReason::PendingFathomableUserReview),
            guidance: pending.then(|| {
                "Do not ask for confirmation in chat and do not retry; the Fathomable user will \
                 review it in Fathomable."
                    .to_owned()
            }),
        }
    }
}

/// One completed reply result in request order.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct ReplyResult {
    thread: Shown,
    resolution: ResolutionResult,
    replayed: bool,
}

/// The complete successful `thread_reply` result.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct ReplyWriteOutput {
    results: Vec<ReplyResult>,
}

impl Shown {
    pub(super) fn new(thread: &Thread, placement: Placement) -> Self {
        Self {
            id: thread.id().to_string(),
            path: thread.path().to_path_buf(),
            range: placement.range().map(RangeOutput::from),
            placement: placement_output(placement),
            anchor_range: thread.range().map(RangeOutput::from),
            location: location_output(thread.range(), placement),
            status: status_output(thread.status()),
            lifecycle: lifecycle_output(thread.lifecycle()),
            created: thread.created(),
            modified: thread.modified(),
            auto_resolve: thread.auto_resolve() == AutoResolve::Enabled,
            messages: thread.messages().map(MessageOutput::from).collect(),
            snippet: (!thread.is_on_file()).then(|| thread.snippet().to_owned()),
            commit: thread.commit().map(str::to_owned),
            reanchored_at: thread.reanchored_at(),
            worktree: None,
        }
    }

    fn in_worktree(mut self, worktree: Option<&Path>) -> Self {
        self.worktree = worktree.map(Path::to_path_buf);
        self
    }
}

const fn placement_output(placement: Placement) -> PlacementOutput {
    match placement {
        Placement::Anchored(_) => PlacementOutput::Anchored,
        Placement::Edited(_) => PlacementOutput::Edited,
        Placement::Detached(_) => PlacementOutput::Detached,
        Placement::File => PlacementOutput::File,
    }
}

fn location_output(anchor: Option<LineRange>, placement: Placement) -> LocationOutput {
    match placement {
        Placement::File => LocationOutput::File,
        Placement::Detached(_) => LocationOutput::Detached,
        Placement::Anchored(range) | Placement::Edited(range) => {
            if anchor == Some(range) {
                LocationOutput::Unchanged
            } else {
                LocationOutput::Moved
            }
        }
    }
}

const fn status_output(status: Status) -> StatusOutput {
    match status {
        Status::Open => StatusOutput::Open,
        Status::Resolved => StatusOutput::Resolved,
    }
}

const fn lifecycle_output(lifecycle: Lifecycle) -> LifecycleOutput {
    match lifecycle {
        Lifecycle::Active => LifecycleOutput::Active,
        Lifecycle::ResolutionProposed => LifecycleOutput::ResolutionProposed,
        Lifecycle::Resolved => LifecycleOutput::Resolved,
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
        output_schema = rmcp::handler::server::tool::schema_for_output::<ThreadsOutput>(),
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
        let shown: Vec<Shown> = selected
            .threads
            .iter()
            .map(|thread| {
                let location = trees.locate(&scope, &self.target.root, &roots, thread);
                Shown::new(thread, location.placement)
                    .in_worktree(location.worktree(&self.target.root))
            })
            .collect();
        CallToolResult::structured(json!(ThreadsOutput {
            threads: shown,
            more: selected.more,
            next_after: selected.next_after,
        }))
    }

    #[tool(
        output_schema = rmcp::handler::server::tool::schema_for_output::<ReplyWriteOutput>(),
        description = "Continue one or more existing review discussions. Pass exactly one \
                       non-empty `replies` array; each item names a thread and body, with optional \
                       current line placement, `resolve` completion intent, and retry \
                       `idempotency_key`. Resolution succeeds only with one-shot permission; \
                       otherwise the successful result directs review to Fathomable. The whole \
                       batch is validated before any reply is written.",
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
        let (author, caller) = match self.signer(&context) {
            Ok(identity) => identity,
            Err(error) => return failure(error),
        };
        let (all, scope) = match self.fetch() {
            Ok(all) => all,
            Err(error) => return failure(error),
        };
        let probe_store = if p.replies.iter().any(|item| item.idempotency_key.is_some()) {
            match Store::open(self.dirs.threads_file(&self.target.key)) {
                Ok(store) => Some(store),
                Err(error) => return failure(error.to_string()),
            }
        } else {
            None
        };
        let roots = self.target.roots();
        let mut trees = Trees::new(&self.dirs, &self.target.key, &self.target.root);
        let mut seen = HashSet::with_capacity(p.replies.len());
        let mut seen_keys = HashSet::with_capacity(p.replies.len());
        let mut validated = Vec::with_capacity(p.replies.len());
        let mut problems = Vec::new();
        for (index, item) in p.replies.into_iter().enumerate() {
            if !seen.insert(item.thread.clone()) {
                problems.push(BatchIssue::new(
                    index,
                    format!("{} appears more than once in `replies`", item.thread),
                ));
                continue;
            }
            if let Some(key) = &item.idempotency_key
                && !seen_keys.insert(key.clone())
            {
                problems.push(BatchIssue::new(
                    index,
                    format!(
                        "{} appears more than once in `replies` by `idempotency_key` {key:?}",
                        item.thread
                    ),
                ));
                continue;
            }
            let id = match thread_id(&item.thread) {
                Ok(id) => id,
                Err(error) => {
                    problems.push(BatchIssue::new(index, format!("{}: {error}", item.thread)));
                    continue;
                }
            };
            let exact_thread = probe_store.as_ref().and_then(|store| store.thread(&id));
            if let Err(error) = exact_thread.map_or_else(
                || structural_reply_without_thread(&item),
                |thread| structural_reply(&item, thread),
            ) {
                problems.push(BatchIssue::new(index, error));
                continue;
            }
            let replay = if let (Some(key), Some(store)) =
                (item.idempotency_key.as_deref(), probe_store.as_ref())
            {
                let reply = reply_probe(author.clone(), &item);
                match store.probe_reply_idempotency_for_caller(
                    &id,
                    &reply,
                    item_lines(&item),
                    &caller,
                    key,
                ) {
                    Ok(replay) => replay.is_some(),
                    Err(error) => {
                        problems.push(BatchIssue::new(index, format!("{}: {error}", item.thread)));
                        continue;
                    }
                }
            } else {
                false
            };
            if replay {
                let root = match exact_thread {
                    Some(thread) => trees.locate(&scope, &self.target.root, &roots, thread).root,
                    None => self.target.root.clone(),
                };
                validated.push(ValidatedReply { index, item, root });
            } else {
                match validate_reply(&item, &all, &scope, &roots, &mut trees, &self.target.root) {
                    Ok(root) => validated.push(ValidatedReply { index, item, root }),
                    Err(error) => problems.push(BatchIssue::new(index, error)),
                }
            }
        }
        if !problems.is_empty() {
            return invalid_batch("replies", &problems);
        }

        let mut answered = Vec::with_capacity(validated.len());
        for position in 0..validated.len() {
            let item = &validated[position];
            match self.reply_one(author.clone(), caller.clone(), item).await {
                Ok(answer) => answered.push(answer),
                Err(error) => {
                    return partial_reply_failure(
                        &answered,
                        item,
                        &validated[position + 1..],
                        error,
                        &self.dirs,
                        &self.target.key,
                        &self.target.root,
                    );
                }
            }
        }
        let mut trees = Trees::new(&self.dirs, &self.target.key, &self.target.root);
        let results = answered
            .iter()
            .map(|answered| {
                let placement = trees.place(&self.target.root, &answered.root, &answered.thread);
                ReplyResult {
                    thread: Shown::new(&answered.thread, placement).in_worktree(
                        (answered.root != self.target.root).then_some(answered.root.as_path()),
                    ),
                    resolution: ResolutionResult::from(answered.resolution),
                    replayed: answered.replayed,
                }
            })
            .collect();
        CallToolResult::structured(json!(ReplyWriteOutput { results }))
    }
}

#[derive(Debug, Clone)]
struct ValidatedReply {
    index: usize,
    item: ReplyItem,
    root: PathBuf,
}

struct Answered {
    index: usize,
    thread: Thread,
    root: PathBuf,
    resolution: ResolutionOutcome,
    replayed: bool,
}

impl Server {
    async fn reply_one(
        &self,
        author: Author,
        caller: String,
        validated: &ValidatedReply,
    ) -> Result<Answered, String> {
        let item = &validated.item;
        let thread = thread_id(&item.thread)?;
        let lines = item_lines(item);
        let request = Request::ThreadReply {
            thread: thread.clone(),
            author: author.clone(),
            caller: caller.clone(),
            body: item.body.clone(),
            resolve: item.resolve,
            lines,
            idempotency_key: item.idempotency_key.clone(),
        };
        let outcome = match self.target.viewer(&self.dirs, &validated.root) {
            Some(viewer) => call(&viewer, &request).await,
            None => headless_reply(
                &self.dirs,
                &self.target,
                &validated.root,
                &thread,
                author,
                &caller,
                item,
                lines,
            )
            .map(|answer| {
                if answer.replayed {
                    Response::thread_reply_replayed(answer.thread, answer.resolution)
                } else {
                    Response::thread_reply_applied(answer.thread, answer.resolution)
                }
            }),
        };
        match outcome {
            Ok(Response::ThreadReply(response)) => {
                let (thread, resolution, replayed) = response.into_parts();
                Ok(Answered {
                    index: validated.index,
                    thread,
                    root: validated.root.clone(),
                    resolution,
                    replayed,
                })
            }
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
    let which = Which::from_status(params.status);
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
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
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

/// Encode the requested resolve intent for the Phase A idempotency probe.
///
/// The actual write uses [`AgentReplyCommand`]; this historical message flag
/// is only the compatibility input through which the probe reconstructs the
/// same request fingerprint.
fn reply_probe(author: Author, item: &ReplyItem) -> Reply {
    let reply = Reply::new(author, now(), item.body.clone());
    if item.resolve {
        reply.proposing_resolution()
    } else {
        reply
    }
}

fn item_lines(item: &ReplyItem) -> Option<LineRange> {
    item.line
        .map(|line| LineRange::new(line, item.end_line.unwrap_or(line)))
}

fn structural_reply_without_thread(item: &ReplyItem) -> Result<Option<LineRange>, String> {
    reply_lines(item)
}

fn structural_reply(item: &ReplyItem, thread: &Thread) -> Result<Option<LineRange>, String> {
    let lines = reply_lines(item)?;
    if thread.is_on_file() && lines.is_some() {
        return Err(format!(
            "{} is a file-wide discussion; omit `line` and `end_line`",
            item.thread
        ));
    }
    Ok(lines)
}

fn reply_lines(item: &ReplyItem) -> Result<Option<LineRange>, String> {
    if item.body.trim().is_empty() {
        return Err(format!("{}: `body` is empty", item.thread));
    }
    if item.line.is_none() && item.end_line.is_some() {
        return Err(format!("{}: pass `line` with `end_line`", item.thread));
    }
    if let Some(line) = item.line {
        if line == 0 || item.end_line == Some(0) {
            return Err(format!("{}: line numbers are 1-based", item.thread));
        }
        if let Some(end_line) = item.end_line
            && end_line < line
        {
            return Err(format!(
                "{}: `end_line` ({end_line}) must be at least `line` ({line})",
                item.thread
            ));
        }
    }
    Ok(item_lines(item))
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
    let lines = structural_reply(item, thread)?;
    if let Some(range) = lines {
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

fn shown_line(thread: &Thread, placement: Placement, verb: &str) -> String {
    let at = match placement.range() {
        Some(range) => format!("{}:{range}", thread.path().display()),
        None => thread.path().display().to_string(),
    };
    format!(
        "{verb} {} at {at} ({}){}",
        thread.id(),
        placement_output(placement).as_str(),
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

#[expect(
    clippy::too_many_arguments,
    reason = "Keep headless and viewer reply inputs aligned at the MCP boundary."
)]
fn headless_reply(
    dirs: &XdgDirs,
    target: &Target,
    root: &Path,
    thread: &ThreadId,
    author: Author,
    caller: &str,
    item: &ReplyItem,
    lines: Option<LineRange>,
) -> Result<HeadlessAnswer, String> {
    let mut store =
        Store::open(dirs.threads_file(&target.key)).map_err(|error| error.to_string())?;
    let when = now();
    let head = Workspace::discover(root)
        .map_err(|error| error.to_string())?
        .head_commit();
    let mut command = AgentReplyCommand::new(author, when, item.body.clone()).at_head(head);
    if item.resolve {
        command = command.resolve();
    }
    if let Some(lines) = lines {
        command = command.relocate(lines);
    }
    if let Some(key) = item.idempotency_key.as_deref() {
        command = command.idempotent(caller, key);
    }
    let outcome = store
        .agent_reply(thread, command, |path| {
            fs::read_to_string(root.join(path)).map_err(|error| {
                fathomable_core::annotations::StoreError::message(format!(
                    "cannot read {}: {error}",
                    path.display()
                ))
            })
        })
        .map_err(|error| error.to_string())?;
    let resolution = *outcome.value();
    let replayed = outcome.replayed();
    tracing::info!(%thread, ?resolution, replayed, "agent reply added headlessly");
    let thread = store
        .thread(thread)
        .cloned()
        .ok_or_else(|| format!("thread {thread} vanished after the reply"))?;
    Ok(HeadlessAnswer {
        thread,
        resolution,
        replayed,
    })
}

struct HeadlessAnswer {
    thread: Thread,
    resolution: ResolutionOutcome,
    replayed: bool,
}

#[derive(Debug, Serialize)]
struct IndexedReplyResult {
    item_index: usize,
    result: ReplyResult,
}

#[derive(Debug, Serialize)]
struct FailedReply {
    item_index: usize,
    item: ReplyItem,
    error: String,
}

#[derive(Debug, Serialize)]
struct UnattemptedReply {
    item_index: usize,
    item: ReplyItem,
}

#[derive(Debug, Serialize)]
struct PartialReplyFailure {
    error_code: &'static str,
    completed: Vec<IndexedReplyResult>,
    failed: FailedReply,
    unattempted: Vec<UnattemptedReply>,
}

fn partial_reply_failure(
    answered: &[Answered],
    failed: &ValidatedReply,
    unattempted: &[ValidatedReply],
    error: String,
    dirs: &XdgDirs,
    key: &Path,
    bound: &Path,
) -> CallToolResult {
    let mut trees = Trees::new(dirs, key, bound);
    let completed = answered
        .iter()
        .map(|answered| {
            let placement = trees.place(bound, &answered.root, &answered.thread);
            IndexedReplyResult {
                item_index: answered.index,
                result: ReplyResult {
                    thread: Shown::new(&answered.thread, placement)
                        .in_worktree((answered.root != bound).then_some(answered.root.as_path())),
                    resolution: ResolutionResult::from(answered.resolution),
                    replayed: answered.replayed,
                },
            }
        })
        .collect();
    structured_error(json!(PartialReplyFailure {
        error_code: "PARTIAL_BATCH",
        completed,
        failed: FailedReply {
            item_index: failed.index,
            item: failed.item.clone(),
            error,
        },
        unattempted: unattempted
            .iter()
            .map(|item| UnattemptedReply {
                item_index: item.index,
                item: item.item.clone(),
            })
            .collect(),
    }))
}

pub(super) fn failure(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

fn structured_error(value: Value) -> CallToolResult {
    let text = value.to_string();
    let mut result = CallToolResult::structured_error(value);
    result.content = vec![ContentBlock::text(text)];
    result
}

#[derive(Debug, Serialize)]
/// One invalid item in a write batch, addressed by its zero-based index.
pub(super) struct BatchIssue {
    item_index: usize,
    message: String,
}

impl BatchIssue {
    /// Record why the item at `index` failed prevalidation.
    pub(super) fn new(index: usize, message: impl Into<String>) -> Self {
        Self {
            item_index: index,
            message: message.into(),
        }
    }
}

/// Return an indexed, machine-readable prevalidation failure.
pub(super) fn invalid_batch(items: &str, issues: &[BatchIssue]) -> CallToolResult {
    let message = issues
        .iter()
        .map(|issue| format!("{items}[{}]: {}", issue.item_index, issue.message))
        .collect::<Vec<_>>()
        .join("\n");
    structured_error(json!({
        "error_code": "INVALID_BATCH",
        "message": message,
        "issues": issues,
    }))
}

/// Instructions supplied to every MCP client.
pub(super) fn instructions() -> String {
    "Fathomable holds review discussions attached to files in this repository. \
     When asked, read the relevant threads and their history. Use thread_start \
     for new findings or questions and thread_reply to continue existing \
     discussions. A resolution_proposed result is successful and awaits the \
     Fathomable user's review in Fathomable; do not ask for confirmation in \
     chat and do not retry it. Reading a thread does not authorize changes."
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
    use serde_json::json;

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
        assert_eq!(value["lifecycle"], "resolved");
        assert_eq!(
            value["messages"][0]["author"],
            json!({"kind": "user", "name": "user"})
        );
        assert_eq!(value["messages"][0]["body"], "Should we rename this?");
        assert_eq!(value["messages"][1]["body"], "I propose `other`.");
        assert_eq!(
            value["messages"][1]["author"],
            json!({"kind": "agent", "name": "Copilot"})
        );
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
        let empty = select_filtered(
            std::slice::from_ref(&dir.0),
            all,
            &ThreadsParams {
                status: None,
                path: None,
                since: None,
                after: None,
                limit: Some(0),
                ids: Vec::new(),
            },
        )?;
        assert!(empty.threads.is_empty());
        assert_eq!(empty.more, 2);
        assert!(empty.next_after.is_none());
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
                    idempotency_key: None,
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
                    idempotency_key: None,
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
                    idempotency_key: None,
                    line: Some(2),
                    end_line: None,
                },
                "past the end",
            ),
            (
                ReplyItem {
                    thread: id.to_string(),
                    body: "x".to_owned(),
                    resolve: false,
                    idempotency_key: None,
                    line: Some(2),
                    end_line: Some(1),
                },
                "must be at least",
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
