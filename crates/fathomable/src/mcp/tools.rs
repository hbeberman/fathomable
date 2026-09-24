// @okf-doc: /decisions/0082-three-tool-review-core.md
//! The repository review tools exposed over MCP.
//!
//! `threads` reads discussions without mutating them. `thread_reply`
//! continues one or more discussions, after validating the whole batch.
//! `thread_start` lives in [`super::start`].

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use fathomable_core::XdgDirs;
use fathomable_core::annotations::{
    AgentReplyCommand, ArchiveRecord, Author, AutoResolve, ComparisonFacts, ContentIdentity,
    IndexFacts, Lifecycle, LineHashes, LineRange, MAX_MESSAGE_BYTES, Message, OriginSide,
    OriginVersion, Placement, PlacementContext, Reply, ResolutionOutcome, ResolutionRecord,
    RestoreRecord, ReviewPointFacts, Status, Store, Thread, ThreadId, WorkingTreeFacts,
};
use fathomable_core::clock::now;
use fathomable_core::context::map_context;
use fathomable_core::vocabulary as vocab;
use fathomable_core::workspace::{Filter, Workspace};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::threads::read_checkout_text;

use super::changes::WithChanges;
use super::source::{After, CommitRequest, Source};
use super::{Server, Target};
/// `threads` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(transform = threads_constraints)]
pub(crate) struct ThreadsParams {
    /// Optional project root override for this call.
    #[serde(default)]
    workspace: Option<PathBuf>,
    /// Optionally filter by exact immutable commit origin.
    ///
    /// `WorkingTree` origins never match, even when their observed `HEAD` is
    /// this commit or landing later records an exact full-file match.
    #[serde(default)]
    source: Option<Source>,
    /// Which non-archived discussions to list: `open` (default), `resolved`, or `all`.
    #[serde(default)]
    status: Option<StatusFilter>,
    /// Only discussions on this repository-relative file or below this directory.
    /// With `source`, matches immutable origin paths rather than current placement.
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
    /// Exact discussion ids to read in the given order, including archived history.
    /// Do not combine this with filters or pagination.
    #[serde(default)]
    ids: Vec<String>,
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
    /// PR-style review reply, at most 1024 UTF-8 bytes for a fresh write.
    ///
    /// State what changed and where, or why no change was made. Include only
    /// a small focused snippet when essential; substantial replacements
    /// belong in the worktree.
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
    /// Optional project root override for this call.
    #[serde(default)]
    workspace: Option<PathBuf>,
    /// One or more replies. The whole batch is validated before any write.
    #[schemars(length(min = 1))]
    replies: Vec<ReplyItem>,
}

const DEFAULT_LIMIT: usize = 50;

/// Add the cross-field range constraint shared by start and reply items.
pub(super) fn require_line_for_end_line(schema: &mut schemars::Schema) {
    schema.insert(
        "allOf".to_owned(),
        json!([
            {
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
            },
            {
                "if": {
                    "not": {
                        "required": ["idempotency_key"],
                        "properties": {
                            "idempotency_key": { "type": "string" }
                        }
                    }
                },
                "then": {
                    "properties": {
                        "body": { "maxLength": MAX_MESSAGE_BYTES }
                    }
                }
            }
        ]),
    );
}

/// Add exclusive IDs and source-qualified pagination constraints to the schema.
fn threads_constraints(schema: &mut schemars::Schema) {
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
                    "limit": { "type": "null" },
                    "source": { "type": "null" }
                }
            }
        }, {
            "if": {
                "required": ["after"],
                "properties": {"after": {"type": "object"}}
            },
            "then": {
                "if": {
                    "required": ["source"],
                    "properties": {"source": {"type": "object"}}
                },
                "then": {
                    "properties": {
                        "source": {
                            "properties": {"revision": {"pattern": "^[0-9A-Fa-f]{40}$"}}
                        },
                        "after": {"required": ["resolved_commit"]}
                    }
                },
                "else": {
                    "properties": {
                        "after": {"not": {"required": ["resolved_commit"]}}
                    }
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
        if thread.is_archived() {
            return false;
        }
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
    /// Immutable source evidence captured when the discussion started.
    origin: OriginOutput,
    /// Current checkout-qualified placement evidence.
    placement_evidence: PlacementEvidenceOutput,
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
    /// Whether this discussion is outside normal board membership.
    archived: bool,
    /// Opening comment and replies in append order.
    messages: Vec<MessageOutput>,
    /// Every successful resolution, oldest first.
    resolution_history: Vec<ResolutionOutput>,
    /// Every archive event, oldest first.
    archive_history: Vec<ArchiveOutput>,
    /// Every restore event, oldest first.
    restore_history: Vec<RestoreOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reanchored_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<PathBuf>,
}

/// Immutable origin and provenance facts in an MCP result.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct OriginOutput {
    path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<RangeOutput>,
    snippet: String,
    truncated: bool,
    version: VersionOutput,
    side: SideOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison: Option<ComparisonOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    working_tree: Option<WorkingTreeOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<IndexOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_point: Option<ReviewPointOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<ContentOutput>,
}

/// Current placement evidence, separate from immutable origin.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct PlacementEvidenceOutput {
    path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    range: Option<RangeOutput>,
    version: VersionOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<u64>,
}

/// A compact content identity.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct ContentOutput {
    hash: String,
    bytes: usize,
}

/// A stable version fact used by origin and lifecycle history.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub(super) enum VersionOutput {
    Commit { id: String },
    WorkingTree { observed_head: Option<String> },
    Index { observed_head: Option<String> },
    ReviewPoint { id: String, base: Option<String> },
    EmptyTree,
    Unknown,
}

/// Which side supplied the immutable source evidence.
#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum SideOutput {
    Base,
    Target,
    Unspecified,
}

/// The human comparison whose lines were shown, when supplied.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct ComparisonOutput {
    base: VersionOutput,
    target: VersionOutput,
}

/// Working-tree provenance facts.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct WorkingTreeOutput {
    observed_head: Option<String>,
    dirty: bool,
    added: bool,
    deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<ContentOutput>,
}

/// Index provenance facts.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct IndexOutput {
    observed_head: Option<String>,
    staged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<ContentOutput>,
}

/// Review-point provenance facts.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct ReviewPointOutput {
    id: String,
    base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<ContentOutput>,
}

/// One immutable resolution event.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct ResolutionOutput {
    created: u64,
    actor: AuthorOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<VersionOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    head: Option<String>,
    ordinal: u64,
}

/// One immutable archive event.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct ArchiveOutput {
    created: u64,
    actor: AuthorOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<VersionOutput>,
    ordinal: u64,
}

/// One immutable restore event.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) struct RestoreOutput {
    created: u64,
    actor: AuthorOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<VersionOutput>,
    ordinal: u64,
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

impl From<&ContentIdentity> for ContentOutput {
    fn from(content: &ContentIdentity) -> Self {
        Self {
            hash: content.hash().to_owned(),
            bytes: content.bytes(),
        }
    }
}

impl From<&OriginVersion> for VersionOutput {
    fn from(version: &OriginVersion) -> Self {
        match version {
            OriginVersion::Commit { id } => Self::Commit { id: id.clone() },
            OriginVersion::WorkingTree { observed_head } => Self::WorkingTree {
                observed_head: observed_head.clone(),
            },
            OriginVersion::Index { observed_head } => Self::Index {
                observed_head: observed_head.clone(),
            },
            OriginVersion::ReviewPoint { id, base } => Self::ReviewPoint {
                id: id.clone(),
                base: base.clone(),
            },
            OriginVersion::EmptyTree => Self::EmptyTree,
            OriginVersion::Unknown => Self::Unknown,
        }
    }
}

impl From<OriginSide> for SideOutput {
    fn from(side: OriginSide) -> Self {
        match side {
            OriginSide::Base => Self::Base,
            OriginSide::Target => Self::Target,
            OriginSide::Unspecified => Self::Unspecified,
        }
    }
}

impl From<&ComparisonFacts> for ComparisonOutput {
    fn from(comparison: &ComparisonFacts) -> Self {
        Self {
            base: VersionOutput::from(comparison.base()),
            target: VersionOutput::from(comparison.target()),
        }
    }
}

impl From<&WorkingTreeFacts> for WorkingTreeOutput {
    fn from(facts: &WorkingTreeFacts) -> Self {
        Self {
            observed_head: facts.observed_head().map(str::to_owned),
            dirty: facts.is_dirty(),
            added: facts.is_added(),
            deleted: facts.is_deleted(),
            content: facts.content().map(ContentOutput::from),
        }
    }
}

impl From<&IndexFacts> for IndexOutput {
    fn from(facts: &IndexFacts) -> Self {
        Self {
            observed_head: facts.observed_head().map(str::to_owned),
            staged: facts.is_staged(),
            content: facts.content().map(ContentOutput::from),
        }
    }
}

impl From<&ReviewPointFacts> for ReviewPointOutput {
    fn from(facts: &ReviewPointFacts) -> Self {
        Self {
            id: facts.id().to_owned(),
            base: facts.base().map(str::to_owned),
            content: facts.content().map(ContentOutput::from),
        }
    }
}

impl From<&ResolutionRecord> for ResolutionOutput {
    fn from(record: &ResolutionRecord) -> Self {
        Self {
            created: record.created(),
            actor: AuthorOutput::from(record.actor()),
            checkout: record.checkout().map(str::to_owned),
            version: record.version().map(VersionOutput::from),
            head: record.head().map(str::to_owned),
            ordinal: record.ordinal(),
        }
    }
}

impl From<&ArchiveRecord> for ArchiveOutput {
    fn from(record: &ArchiveRecord) -> Self {
        Self {
            created: record.created(),
            actor: AuthorOutput::from(record.actor()),
            checkout: record.checkout().map(str::to_owned),
            version: record.version().map(VersionOutput::from),
            ordinal: record.ordinal(),
        }
    }
}

impl From<&RestoreRecord> for RestoreOutput {
    fn from(record: &RestoreRecord) -> Self {
        Self {
            created: record.created(),
            actor: AuthorOutput::from(record.actor()),
            checkout: record.checkout().map(str::to_owned),
            version: record.version().map(VersionOutput::from),
            ordinal: record.ordinal(),
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
    /// The checkout used for this call.
    checkout: PathBuf,
    threads: Vec<Shown>,
    more: usize,
    #[schemars(required)]
    next_after: Option<After>,
}

/// The complete result returned by a commit-selected read.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct SelectedThreadsOutput {
    /// The checkout used for this call.
    checkout: PathBuf,
    /// The canonical full ID selected for immutable-origin filtering.
    #[schemars(regex(pattern = r"^[0-9a-f]{40}$"))]
    resolved_commit: String,
    threads: Vec<Shown>,
    more: usize,
    #[schemars(required)]
    next_after: Option<After>,
}

/// Successful read results preserve the legacy or selected wire shape.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(super) enum ThreadsSuccess {
    Ordinary(ThreadsOutput),
    Selected(SelectedThreadsOutput),
}

/// The complete result returned by a write.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct WriteOutput {
    /// The checkout used for this call.
    pub(super) checkout: PathBuf,
    pub(super) threads: Vec<Shown>,
}

/// The complete result returned by a commit-selected start.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct SelectedWriteOutput {
    /// The checkout used for this call.
    pub(super) checkout: PathBuf,
    /// The canonical full ID used for immutable origin capture.
    #[schemars(regex(pattern = r"^[0-9a-f]{40}$"))]
    pub(super) resolved_commit: String,
    pub(super) threads: Vec<Shown>,
}

/// Successful starts preserve the legacy or selected wire shape.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(super) enum StartSuccess {
    Ordinary(WriteOutput),
    Selected(SelectedWriteOutput),
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
    /// The checkout used for this call.
    checkout: PathBuf,
    results: Vec<ReplyResult>,
}

impl Shown {
    pub(super) fn new(thread: &Thread, placement: Placement) -> Self {
        Self {
            id: thread.id().to_string(),
            path: thread.path().to_path_buf(),
            origin: OriginOutput {
                path: thread.origin().path().to_path_buf(),
                range: thread.origin().range().map(RangeOutput::from),
                snippet: thread.origin().snippet().to_owned(),
                truncated: thread.origin().evidence_truncated(),
                version: VersionOutput::from(thread.origin_version()),
                side: SideOutput::from(thread.origin_side()),
                comparison: thread.comparison().map(ComparisonOutput::from),
                working_tree: thread
                    .provenance()
                    .working_tree()
                    .map(WorkingTreeOutput::from),
                index: thread.provenance().index().map(IndexOutput::from),
                review_point: thread
                    .provenance()
                    .review_point()
                    .map(ReviewPointOutput::from),
                content: thread.content_identity().map(ContentOutput::from),
            },
            placement_evidence: PlacementEvidenceOutput {
                path: thread.placement_evidence().path().to_path_buf(),
                range: thread.placement_evidence().range().map(RangeOutput::from),
                version: VersionOutput::from(thread.placement_evidence().version()),
                checkout: thread.placement_evidence().checkout().map(str::to_owned),
                observed_at: thread.placement_evidence().observed_at(),
            },
            range: placement.range().map(RangeOutput::from),
            placement: placement_output(placement),
            anchor_range: thread.range().map(RangeOutput::from),
            location: location_output(thread.range(), placement),
            status: status_output(thread.status()),
            lifecycle: lifecycle_output(thread.lifecycle()),
            created: thread.created(),
            modified: thread.modified(),
            auto_resolve: thread.auto_resolve() == AutoResolve::Enabled,
            archived: thread.is_archived(),
            messages: thread.messages().map(MessageOutput::from).collect(),
            resolution_history: thread
                .resolution_history()
                .iter()
                .map(ResolutionOutput::from)
                .collect(),
            archive_history: thread
                .archive_history()
                .iter()
                .map(ArchiveOutput::from)
                .collect(),
            restore_history: thread
                .restore_history()
                .iter()
                .map(RestoreOutput::from)
                .collect(),
            snippet: (!thread.is_on_file()).then(|| thread.snippet().to_owned()),
            commit: thread.commit().map(str::to_owned),
            reanchored_at: thread.reanchored_at(),
            worktree: None,
        }
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
    texts: HashMap<PathBuf, Option<String>>,
    hashes: HashMap<PathBuf, Option<LineHashes>>,
}

impl Tree {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            texts: HashMap::new(),
            hashes: HashMap::new(),
        }
    }

    /// Maximum attempts used to capture one checkout consistently.
    const MAX_PROJECTION_ATTEMPTS: usize = 2;

    fn observed_head(&self) -> Option<String> {
        Workspace::discover(&self.root)
            .ok()
            .and_then(|workspace| workspace.head_commit())
    }

    fn capture_text(&self, path: &Path) -> Result<Option<String>, String> {
        for attempt in 0..Self::MAX_PROJECTION_ATTEMPTS {
            let before = self.observed_head();
            let text = match read_checkout_text(&self.root, path) {
                Ok(text) => Some(text),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => None,
                Err(error) => {
                    return Err(format!("cannot read {}: {error}", path.display()));
                }
            };
            let after = self.observed_head();
            if before == after {
                return Ok(text);
            }
            if attempt + 1 == Self::MAX_PROJECTION_ATTEMPTS {
                return Err(format!(
                    "checkout HEAD changed while projecting {}",
                    path.display()
                ));
            }
        }
        unreachable!("projection attempts always return or retry")
    }

    fn ensure_captured(&mut self, path: &Path) -> Result<(), String> {
        if !self.texts.contains_key(path) {
            let text = self.capture_text(path)?;
            let hashes = text.as_deref().map(LineHashes::of);
            self.texts.insert(path.to_path_buf(), text);
            self.hashes.insert(path.to_path_buf(), hashes);
        }
        Ok(())
    }

    pub(super) fn preflight(&mut self, path: &Path) -> Result<(), String> {
        self.ensure_captured(path)
    }

    pub(super) fn try_place(&mut self, thread: &Thread) -> Result<Placement, String> {
        self.ensure_captured(thread.path())?;
        let text = self.texts.get(thread.path()).and_then(Option::as_deref);
        let hashes = self.hashes.get(thread.path()).and_then(Option::as_ref);
        let placement = match (hashes, thread.range()) {
            (Some(hashes), _) => thread.locate_in(hashes),
            (None, Some(range)) => Placement::Detached(range),
            (None, None) => Placement::File,
        };
        if !placement.is_detached() {
            return Ok(placement);
        }
        let Some(from) = thread.range() else {
            return Ok(placement);
        };
        let Some(text) = text else {
            return Ok(placement);
        };
        let mapping = match thread
            .placement_evidence()
            .context()
            .or_else(|| thread.origin().context())
        {
            Some(context) => map_context(context, text, from),
            None => fathomable_core::reanchor::Mapping::Removed,
        };
        match mapping {
            fathomable_core::reanchor::Mapping::Edited(range)
            | fathomable_core::reanchor::Mapping::Moved(range) => Ok(Placement::Edited(range)),
            fathomable_core::reanchor::Mapping::Removed => Ok(placement),
        }
    }

    pub(super) fn place(&mut self, thread: &Thread) -> Placement {
        match self.try_place(thread) {
            Ok(placement) => placement,
            Err(error) => {
                tracing::warn!(%error, path = %thread.path().display(), "cannot project thread placement");
                thread.range().map_or(Placement::File, Placement::Detached)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct Location {
    root: PathBuf,
    placement: Placement,
}

pub(super) struct Trees {
    here: Tree,
}

impl Trees {
    pub(super) fn new(root: &Path) -> Self {
        Self {
            here: Tree::new(root),
        }
    }

    fn place(&mut self, root: &Path, thread: &Thread) -> Result<Placement, String> {
        if root != self.here.root {
            return Err(format!(
                "MCP placement is bound to {}; cannot use {}",
                self.here.root.display(),
                root.display()
            ));
        }
        self.here.try_place(thread)
    }

    fn locate(&mut self, bound: &Path, thread: &Thread) -> Result<Location, String> {
        let placement = self.place(bound, thread)?;
        Ok(Location {
            root: bound.to_path_buf(),
            placement,
        })
    }
}

#[tool_router(vis = "pub(super)")]
impl Server {
    #[tool(
        output_schema = rmcp::handler::server::tool::schema_for_output::<WithChanges<ThreadsSuccess>>(),
        description = "Read review discussions in this repository checkout. By default returns \
                       all non-archived open discussions with their complete conversation, \
                       immutable origin, current placement, and lifecycle history. Filter with \
                       `status`, `path`, and `since`; page with `limit` and the returned \
                       `next_after`; or pass `ids` alone to retrieve exact discussions, including \
                       archived history. Optional `source:{kind:\"commit\",revision}` filters exact \
                       immutable commit origins and returns `resolved_commit`; WorkingTree origins \
                       do not match merely because `observed_head` or landing names that commit. \
                       Selected cursors remain pinned to that full ID. Reading never assigns, \
                       acknowledges, or consumes a discussion.",
        annotations(
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "Keep request-local target selection, filter validation, and result projection together."
    )]
    fn threads(&self, Parameters(p): Parameters<ThreadsParams>) -> CallToolResult {
        let target = match self.target(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        if !p.ids.is_empty() && p.source.is_some() {
            return failure("pass non-empty `ids` without `source`");
        }
        let source_request = match p.source.as_ref().map(Source::request).transpose() {
            Ok(request) => request,
            Err(error) => return invalid_source(error, None),
        };
        if matches!(source_request, Some(CommitRequest::Head)) && p.after.is_some() {
            return invalid_source(
                "`source.revision:\"HEAD\"` cannot continue `after`; use the prior full \
                 `resolved_commit` as the source revision",
                None,
            );
        }
        if source_request.is_none() && p.after.as_ref().and_then(After::commit).is_some() {
            return invalid_source(
                "a commit-qualified `after` requires the same full `source`",
                None,
            );
        }

        let selected_commit = match source_request.as_ref() {
            Some(request) => match super::source::SelectedCommit::resolve(&target, request) {
                Ok(selected) => Some(selected),
                Err(error) => return invalid_source(error, None),
            },
            None => None,
        };
        if let Some(commit) = selected_commit.as_ref() {
            match p.after.as_ref() {
                Some(after) if after.commit().is_none() => {
                    return invalid_source(
                        "a selected read requires a commit-qualified `after`",
                        Some(commit.id().as_str()),
                    );
                }
                Some(after) if after.commit() != Some(commit.id().as_str()) => {
                    return invalid_source(
                        "`after.resolved_commit` must match the selected full commit ID",
                        Some(commit.id().as_str()),
                    );
                }
                _ => {}
            }
        }

        let selected = if p.ids.is_empty() {
            let all = match self.fetch(&target) {
                Ok(all) => all,
                Err(error) => return failure(error),
            };
            let selection = match selected_commit.as_ref() {
                Some(commit) => select_commit_filtered(&all, &p, commit.id()),
                None => select_filtered(std::slice::from_ref(&target.root), &all, &p),
            };
            match selection {
                Ok(selected) => OwnedSelection::from(selected),
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
            let all = match self.fetch_exact(&target) {
                Ok(all) => all,
                Err(error) => return failure(error),
            };
            match select_exact(&all, &p.ids) {
                Ok(selected) => OwnedSelection::from(selected),
                Err(error) => return failure(error),
            }
        };

        let mut trees = Trees::new(&target.root);
        let mut shown = Vec::with_capacity(selected.threads.len());
        for thread in &selected.threads {
            let location = match trees.locate(&target.root, thread) {
                Ok(location) => location,
                Err(error) => return failure(error),
            };
            shown.push(Shown::new(thread, location.placement));
        }
        let output = match selected_commit {
            Some(commit) => ThreadsSuccess::Selected(SelectedThreadsOutput {
                checkout: target.root.clone(),
                resolved_commit: commit.id().as_str().to_owned(),
                threads: shown,
                more: selected.more,
                next_after: selected.next_after,
            }),
            None => ThreadsSuccess::Ordinary(ThreadsOutput {
                checkout: target.root.clone(),
                threads: shown,
                more: selected.more,
                next_after: selected.next_after,
            }),
        };
        CallToolResult::structured(json!(output))
    }

    #[tool(
        output_schema = rmcp::handler::server::tool::schema_for_output::<WithChanges<ReplyWriteOutput>>(),
        description = "Continue one or more existing, non-archived review discussions. Pass exactly one \
                       non-empty `replies` array. Each item gives a concise resolution: what changed \
                       and where, or why no change was made. Include only a small focused snippet \
                       when essential; never paste the complete replacement. Each fresh body is at \
                       most 1024 UTF-8 bytes, with optional current line placement, `resolve` \
                       completion intent, and retry `idempotency_key`. Omit `line` and `end_line` \
                       to reply without relocating the discussion, including when its source is \
                       detached in this checkout. Resolution succeeds only with one-shot permission; \
                       otherwise the successful result directs review to Fathomable. Fresh replies \
                       to archived discussions fail; matching keyed retries, including historical \
                       larger bodies, replay their original outcome. The whole batch is validated \
                       before any reply is written.",
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
    fn thread_reply(
        &self,
        Parameters(p): Parameters<ReplyParams>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let target = match self.target(p.workspace.as_deref()) {
            Ok(target) => target,
            Err(error) => return failure(error),
        };
        if p.replies.is_empty() {
            return failure("`replies` must contain at least one reply");
        }
        let (author, caller) = match self.signer(&context) {
            Ok(identity) => identity,
            Err(error) => return failure(error),
        };
        let all = match self.fetch(&target) {
            Ok(all) => all,
            Err(error) => return failure(error),
        };
        let probe_store = match Store::open_workspace(&self.dirs, &target.key) {
            Ok(store) => store,
            Err(error) => return failure(error.to_string()),
        };
        let mut trees = Trees::new(&target.root);
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
            let exact_thread = probe_store.thread(&id);
            if let Err(error) = exact_thread.map_or_else(
                || structural_reply_without_thread(&item),
                |thread| structural_reply(&item, thread),
            ) {
                problems.push(BatchIssue::new(index, error));
                continue;
            }
            let is_replay = if let Some(key) = item.idempotency_key.as_deref() {
                let reply = reply_probe(author.clone(), &item);
                match probe_store.probe_reply_idempotency_for_caller(
                    &id,
                    &reply,
                    item_lines(&item),
                    &caller,
                    key,
                ) {
                    Ok(receipt) => receipt.is_some(),
                    Err(error) => {
                        problems.push(BatchIssue::new(index, format!("{}: {error}", item.thread)));
                        continue;
                    }
                }
            } else {
                false
            };
            if is_replay {
                validated.push(ValidatedReply {
                    index,
                    item,
                    root: target.root.clone(),
                });
            } else {
                if exact_thread.is_some_and(Thread::is_archived) {
                    problems.push(BatchIssue::new(
                        index,
                        format!("{} is archived; restore it before replying", item.thread),
                    ));
                    continue;
                }
                match validate_reply(&item, &all, &mut trees, &target.root) {
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
            match self.reply_one(&target, author.clone(), &caller, item) {
                Ok(answer) => answered.push(answer),
                Err(error) => {
                    return partial_reply_failure(
                        &answered,
                        item,
                        &validated[position + 1..],
                        error,
                        &target.root,
                    );
                }
            }
        }
        let mut trees = Trees::new(&target.root);
        let mut results = Vec::with_capacity(answered.len());
        for answered in &answered {
            let placement = match trees.place(&answered.root, &answered.thread) {
                Ok(placement) => placement,
                Err(error) => return failure(error),
            };
            results.push(ReplyResult {
                thread: Shown::new(&answered.thread, placement),
                resolution: ResolutionResult::from(answered.resolution),
                replayed: answered.replayed,
            });
        }
        CallToolResult::structured(json!(ReplyWriteOutput {
            checkout: target.root.clone(),
            results,
        }))
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
    fn reply_one(
        &self,
        target: &Target,
        author: Author,
        caller: &str,
        validated: &ValidatedReply,
    ) -> Result<Answered, String> {
        let item = &validated.item;
        let thread = thread_id(&item.thread)?;
        let lines = item_lines(item);
        let answer = headless_reply(
            &self.dirs,
            target,
            &validated.root,
            &thread,
            author,
            caller,
            item,
            lines,
        )
        .map_err(|message| format!("{}: {message}", item.thread))?;
        Ok(Answered {
            index: validated.index,
            thread: answer.thread,
            root: validated.root.clone(),
            resolution: answer.resolution,
            replayed: answer.replayed,
        })
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
    let path = match check_path_in_roots(roots, params.path.as_deref()) {
        Ok(path) => path,
        Err(error) => {
            let historical = params
                .path
                .as_deref()
                .and_then(|path| normalize_repository_path(path).ok().flatten())
                .filter(|path| all.iter().any(|thread| thread.path().starts_with(path)));
            match historical {
                Some(path) => Some(path),
                None => return Err(error),
            }
        }
    };
    let mut threads: Vec<&Thread> = all
        .iter()
        .filter(|thread| which.admits(thread))
        .filter(|thread| params.since.is_none_or(|since| thread.modified() >= since))
        .filter(|thread| {
            params.after.as_ref().is_none_or(|after| {
                let after = after.position();
                (thread.modified(), thread.id().as_str()) > (after.updated, after.id.as_str())
            })
        })
        .filter(|thread| {
            path.as_deref()
                .is_none_or(|path| thread.path().starts_with(path))
        })
        .collect();
    threads
        .sort_by(|left, right| (left.modified(), left.id()).cmp(&(right.modified(), right.id())));
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let more = threads.len().saturating_sub(limit);
    threads.truncate(limit);
    let next_after = if more > 0 {
        threads
            .last()
            .map(|thread| After::board(thread.modified(), thread.id().to_string()))
    } else {
        None
    };
    Ok(Selected {
        threads,
        more,
        next_after,
    })
}

fn select_commit_filtered<'a>(
    all: &'a [Thread],
    params: &ThreadsParams,
    commit: &fathomable_core::workspace::CommitId,
) -> Result<Selected<'a>, String> {
    let which = Which::from_status(params.status);
    let path = normalize_repository_path(params.path.as_deref().unwrap_or_else(|| Path::new("")))?;
    let mut threads: Vec<&Thread> = all
        .iter()
        .filter(|thread| which.admits(thread))
        .filter(|thread| thread.origin_version().commit_id() == Some(commit.as_str()))
        .filter(|thread| params.since.is_none_or(|since| thread.modified() >= since))
        .filter(|thread| {
            params.after.as_ref().is_none_or(|after| {
                let after = after.position();
                (thread.modified(), thread.id().as_str()) > (after.updated, after.id.as_str())
            })
        })
        .filter(|thread| {
            path.as_deref()
                .is_none_or(|path| thread.origin().path().starts_with(path))
        })
        .collect();
    threads
        .sort_by(|left, right| (left.modified(), left.id()).cmp(&(right.modified(), right.id())));
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let more = threads.len().saturating_sub(limit);
    threads.truncate(limit);
    let next_after = if more > 0 {
        threads
            .last()
            .map(|thread| After::selected(thread.modified(), thread.id().to_string(), commit))
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
    trees: &mut Trees,
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
    let location = trees.locate(bound, thread)?;
    refusal(item, thread, &location.root)?;
    Ok(location.root)
}

/// Project the canonical reply request into the idempotency probe shape.
///
/// The actual write uses [`AgentReplyCommand`]; the probe uses this same
/// request fingerprint before mutable thread validation.
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
fn refusal(item: &ReplyItem, thread: &Thread, root: &Path) -> Result<(), String> {
    if item.body.len() > MAX_MESSAGE_BYTES {
        return Err(format!(
            "{}: `body` has {} UTF-8 bytes; maximum is {MAX_MESSAGE_BYTES}",
            item.thread,
            item.body.len()
        ));
    }
    if thread.status() != Status::Open {
        return Err(format!(
            "{} is resolved; only the user can reopen it",
            item.thread
        ));
    }
    let lines = structural_reply(item, thread)?;
    if let Some(range) = lines {
        let text = match read_checkout_text(root, thread.path()) {
            Ok(text) => text,
            Err(error) => {
                return Err(format!(
                    "{}: cannot relocate {} in the bound checkout: {error}; omit `line` and \
                     `end_line` to reply without relocating, or run MCP bound to a worktree where \
                     the path is readable",
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
    let Some(path) = normalize_repository_path(path)? else {
        return Ok(None);
    };
    let shown = path.display();
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

fn normalize_repository_path(path: &Path) -> Result<Option<PathBuf>, String> {
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
    Ok((!normalized.as_os_str().is_empty()).then_some(normalized))
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
    reason = "Keep the validated MCP reply inputs explicit at the store boundary."
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
    let mut store = Store::open_workspace(dirs, &target.key).map_err(|error| error.to_string())?;
    let when = now();
    let head = Workspace::discover(root)
        .map_err(|error| error.to_string())?
        .head_commit();
    let mut command = AgentReplyCommand::new(author, when, item.body.clone())
        .at_head(head.clone())
        .at_checkout(root.display().to_string())
        .place_in(
            PlacementContext::new(OriginVersion::working_tree(head))
                .at_checkout(root.display().to_string()),
        );
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
            read_checkout_text(root, path).map_err(|error| {
                fathomable_core::annotations::StoreError::message(format!(
                    "cannot read {}: {error}",
                    path.display()
                ))
            })
        })
        .map_err(|error| error.to_string())?;
    let resolution = *outcome.value();
    let replayed = outcome.replayed();
    tracing::info!(%thread, ?resolution, replayed, "agent reply added");
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
    checkout: PathBuf,
    completed: Vec<IndexedReplyResult>,
    failed: FailedReply,
    unattempted: Vec<UnattemptedReply>,
}

fn partial_reply_failure(
    answered: &[Answered],
    failed: &ValidatedReply,
    unattempted: &[ValidatedReply],
    error: String,
    bound: &Path,
) -> CallToolResult {
    let mut tree = Tree::new(bound);
    let completed = answered
        .iter()
        .map(|answered| {
            let placement = tree.place(&answered.thread);
            IndexedReplyResult {
                item_index: answered.index,
                result: ReplyResult {
                    thread: Shown::new(&answered.thread, placement),
                    resolution: ResolutionResult::from(answered.resolution),
                    replayed: answered.replayed,
                },
            }
        })
        .collect();
    structured_error(json!(PartialReplyFailure {
        error_code: "PARTIAL_BATCH",
        checkout: bound.to_path_buf(),
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

pub(super) fn structured_error(value: Value) -> CallToolResult {
    let text = value.to_string();
    let mut result = CallToolResult::structured_error(value);
    result.content = vec![ContentBlock::text(text)];
    result
}

pub(super) fn invalid_source(message: impl Into<String>, commit: Option<&str>) -> CallToolResult {
    let mut value = json!({
        "error_code": "INVALID_SOURCE",
        "message": message.into(),
    });
    if let Some(commit) = commit {
        value["resolved_commit"] = commit.into();
    }
    structured_error(value)
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
     Treat them like GitHub pull-request review threads, not chat responses or \
     document-delivery channels. Keep each thread focused on one local, actionable \
     issue. Include only the essential evidence and requested action or outcome. \
     Do not paste whole files, whole sections, long replacement text, comprehensive \
     reviews, plans, or status reports. Put substantial edits and content in the \
     worktree, then refer to the relevant path and lines. Start separate threads \
     for independent issues. Never split or chain messages merely to evade the \
     1024-byte body limit. When asked, read the relevant threads and their history. \
     Use thread_start for new findings or questions and thread_reply to continue \
     existing discussions. A resolution_proposed result is successful and awaits \
     the Fathomable user's review in Fathomable; do not ask for confirmation in \
     chat and do not retry it. Reading a thread does not authorize changes. \
     Tool results may include changes:[[id,kind]] hints; fetch those IDs with threads, \
     except delete means the thread is gone. existing marks startup open threads; \
     new marks a new thread, add a reply, edit another update; resolve/reopen and \
     archive/restore mark lifecycle changes. Hints are coalesced per repository/chat \
     until the next tool result, reset on server restart, and absent when unchanged. \
     If tracking is unavailable, query threads with since (Unix seconds), status:\"all\" \
     and optional path, keeping your own timestamp."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{
        Author, Draft, LineRange, MAX_MESSAGE_BYTES, Reply, ResolutionOutcome, Status, Store,
        ThreadId,
    };
    use fathomable_core::clock::now;
    use fathomable_core::vocabulary::ALL;
    use fathomable_testing::TempDir;
    use serde_json::json;

    use crate::mcp::Server;

    use super::{
        Answered, ReplyItem, Shown, ThreadsParams, Tree, ValidatedReply, check_path,
        partial_reply_failure, refusal, select_exact, select_filtered,
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
    fn instructions_keep_agent_messages_pr_thread_sized() {
        let text = super::instructions();
        for expected in [
            "GitHub pull-request review threads",
            "Do not paste whole files",
            "Put substantial edits and content in the worktree",
            "Never split or chain messages",
        ] {
            assert!(text.contains(expected), "missing {expected:?}: {text}");
        }
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
                workspace: None,
                source: None,
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
                workspace: None,
                source: None,
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
                workspace: None,
                source: None,
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
                workspace: None,
                source: None,
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
            (
                ReplyItem {
                    thread: id.to_string(),
                    body: "é".repeat(MAX_MESSAGE_BYTES / 2 + 1),
                    resolve: false,
                    idempotency_key: None,
                    line: None,
                    end_line: None,
                },
                "maximum is 1024",
            ),
        ] {
            let error = refusal(&item, &all[0], &dir.0).err().unwrap_or_default();
            assert!(error.contains(needle), "{error}");
        }
        assert_eq!(all[0].status(), Status::Open);
        Ok(())
    }

    #[test]
    fn partial_reply_failure_preserves_execution_boundary_and_text_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("mcp-partial-reply")?;
        fs::write(dir.0.join("a.md"), "one\n")?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "first",
            ),
            "one\n",
            1,
        )?;
        let thread = store.thread(&id).ok_or("thread")?.clone();
        let item = |thread: &str, body: &str| ReplyItem {
            thread: thread.to_owned(),
            body: body.to_owned(),
            resolve: false,
            idempotency_key: None,
            line: None,
            end_line: None,
        };
        let answered = [Answered {
            index: 0,
            thread,
            root: dir.0.clone(),
            resolution: ResolutionOutcome::NotRequested,
            replayed: false,
        }];
        let failed = ValidatedReply {
            index: 1,
            item: item("2-2-2", "second"),
            root: dir.0.clone(),
        };
        let unattempted = [ValidatedReply {
            index: 2,
            item: item("3-3-3", "third"),
            root: dir.0.clone(),
        }];
        let result = partial_reply_failure(
            &answered,
            &failed,
            &unattempted,
            "injected direct-store failure".to_owned(),
            &dir.0,
        );
        let value = serde_json::to_value(result)?;
        let structured = &value["structuredContent"];
        assert_eq!(value["isError"], true);
        assert_eq!(structured["error_code"], "PARTIAL_BATCH");
        assert_eq!(structured["completed"][0]["item_index"], 0);
        assert_eq!(
            structured["completed"][0]["result"]["thread"]["id"],
            id.to_string()
        );
        assert_eq!(structured["failed"]["item_index"], 1);
        assert_eq!(structured["failed"]["item"]["thread"], "2-2-2");
        assert_eq!(
            structured["failed"]["error"],
            "injected direct-store failure"
        );
        assert_eq!(structured["unattempted"][0]["item_index"], 2);
        assert_eq!(structured["unattempted"][0]["item"]["thread"], "3-3-3");
        let fallback: serde_json::Value =
            serde_json::from_str(value["content"][0]["text"].as_str().ok_or("fallback")?)?;
        assert_eq!(&fallback, structured);
        Ok(())
    }
}
