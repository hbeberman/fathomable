// @okf-doc: /decisions/0089-store-only-mcp.md
//! Compact, process-local change hints, isolated by repository and chat.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use fathomable_core::XdgDirs;
use fathomable_core::annotations::{
    ActivityCursor, ResolutionRecord, RestoreRecord, Status, Store, Thread, ThreadId,
};
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Target, headless_store};

const UNAVAILABLE: &str = "unavailable: query threads with since (Unix seconds), status:\"all\" and optional path; keep your own timestamp";

#[derive(Debug, Clone, Copy, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(super) enum Kind {
    Existing,
    New,
    Add,
    Edit,
    Resolve,
    Reopen,
    Archive,
    Restore,
    Delete,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(super) enum Hints {
    Threads(Vec<(String, Kind)>),
    Unavailable(&'static str),
}

/// Tool result fields plus optional model-visible change hints.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(super) struct WithChanges<T> {
    #[serde(flatten)]
    result: T,
    /// Coalesced [thread ID, kind] hints, or guidance when tracking is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    changes: Option<Hints>,
}

#[derive(Default)]
pub(super) struct Tracker {
    checkpoints: Mutex<BTreeMap<(PathBuf, String), Snapshot>>,
}

impl Tracker {
    pub(super) fn hints(
        &self,
        dirs: &XdgDirs,
        target: &Target,
        caller: String,
    ) -> Result<Option<Hints>, String> {
        // Serialize observation and advancement, not tool execution or transport.
        let mut checkpoints = self
            .checkpoints
            .lock()
            .map_err(|error| format!("change checkpoint lock failed: {error}"))?;
        let current = Snapshot::from_store(&headless_store(dirs, target)?);
        let key = (target.key.clone(), caller);
        let changed = match checkpoints.get(&key) {
            Some(previous) => current.since(previous)?,
            None => current
                .threads
                .iter()
                .filter(|(_, seen)| seen.status == Status::Open && !seen.archived)
                .map(|(id, _)| (id.to_string(), Kind::Existing))
                .collect(),
        };
        checkpoints.insert(key, current);
        Ok((!changed.is_empty()).then_some(Hints::Threads(changed)))
    }
}

struct Snapshot {
    cursor: ActivityCursor,
    backing_observed: bool,
    threads: BTreeMap<ThreadId, Seen>,
}

impl Snapshot {
    fn from_store(store: &Store) -> Self {
        Self {
            cursor: store.activity_cursor(),
            backing_observed: store.backing_file_observed(),
            threads: store
                .all_threads()
                .map(|thread| (thread.id().clone(), Seen::from(thread)))
                .collect(),
        }
    }

    fn since(&self, previous: &Self) -> Result<Vec<(String, Kind)>, String> {
        if previous.backing_observed && !self.backing_observed {
            return Err("previously observed thread store is missing".to_owned());
        }
        if self.cursor < previous.cursor {
            return Err(
                "thread history regressed; restart MCP to reset change tracking".to_owned(),
            );
        }
        let mut changes: BTreeMap<&ThreadId, Kind> = self
            .threads
            .iter()
            .filter_map(|(id, seen)| {
                let kind = previous
                    .threads
                    .get(id)
                    .map_or(Some(Kind::New), |before| seen.since(before));
                kind.map(|kind| (id, kind))
            })
            .collect();
        for id in previous.threads.keys() {
            if !self.threads.contains_key(id) {
                changes.insert(id, Kind::Delete);
            }
        }
        Ok(changes
            .into_iter()
            .map(|(id, kind)| (id.to_string(), kind))
            .collect())
    }
}

/// Keep revisions and classification facts, never duplicate message bodies.
#[derive(PartialEq, Eq)]
struct Seen {
    // The store excludes landing-only bookkeeping from this revision.
    revision: u64,
    replies: usize,
    status: Status,
    archived: bool,
    resolution: Option<u64>,
    restoration: Option<u64>,
}

impl From<&Thread> for Seen {
    fn from(thread: &Thread) -> Self {
        Self {
            revision: thread.revision(),
            replies: thread.replies().len(),
            status: thread.status(),
            archived: thread.is_archived(),
            resolution: thread.latest_resolution().map(ResolutionRecord::ordinal),
            restoration: thread.latest_restore().map(RestoreRecord::ordinal),
        }
    }
}

impl Seen {
    fn since(&self, before: &Self) -> Option<Kind> {
        if self == before {
            None
        } else if self.archived && !before.archived {
            Some(Kind::Archive)
        } else if !self.archived && (before.archived || self.restoration != before.restoration) {
            Some(Kind::Restore)
        } else if self.status == Status::Resolved
            && (before.status != Status::Resolved || self.resolution != before.resolution)
        {
            Some(Kind::Resolve)
        } else if self.status == Status::Open
            && (before.status == Status::Resolved || self.resolution != before.resolution)
        {
            Some(Kind::Reopen)
        } else if self.replies > before.replies {
            Some(Kind::Add)
        } else {
            Some(Kind::Edit)
        }
    }
}

pub(super) fn attach(result: &mut CallToolResult, hints: Result<Option<Hints>, String>) {
    let hints = match hints {
        Ok(Some(hints)) => hints,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "MCP change hints unavailable; checkpoint unchanged");
            Hints::Unavailable(UNAVAILABLE)
        }
    };
    if let Some(output @ Value::Object(_)) = result.structured_content.as_mut() {
        output["changes"] = json!(hints);
        result.content = vec![ContentBlock::text(output.to_string())];
    } else {
        result
            .content
            .push(ContentBlock::text(json!({"changes": hints}).to_string()));
    }
}
