// @okf-doc: /decisions/0092-per-call-commit-sources.md
//! Request-local immutable commit selection for MCP review tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fathomable_core::content::is_binary;
use fathomable_core::workspace::{CommitId, Workspace};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::Target;

/// Maximum raw commit bytes captured by one selected call.
pub(super) const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// An optional commit source supplied by an MCP caller.
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(super) enum Source {
    /// Select an exact local commit, or resolve the bound checkout's `HEAD`.
    Commit {
        /// `HEAD` or one full 40-hex SHA-1 object ID.
        #[schemars(regex(pattern = r"^(HEAD|[0-9A-Fa-f]{40})$"))]
        revision: String,
    },
}

/// A syntactically validated commit request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommitRequest {
    Head,
    Id(CommitId),
}

impl Source {
    pub(super) fn request(&self) -> Result<CommitRequest, String> {
        match self {
            Self::Commit { revision } if revision == "HEAD" => Ok(CommitRequest::Head),
            Self::Commit { revision } => {
                CommitId::parse(revision)
                    .map(CommitRequest::Id)
                    .map_err(|_error| {
                        "`source.revision` must be exactly `HEAD` or a full 40-hex commit ID"
                            .to_owned()
                    })
            }
        }
    }
}

/// A verified request-local immutable commit.
#[derive(Debug)]
pub(super) struct SelectedCommit {
    id: CommitId,
    workspace: Workspace,
}

impl SelectedCommit {
    pub(super) fn resolve(target: &Target, request: &CommitRequest) -> Result<Self, String> {
        let workspace = bound_workspace(target)?;
        let id = match request {
            CommitRequest::Head => workspace
                .resolve_revision("HEAD")
                .map_err(|error| format!("cannot resolve selected `HEAD`: {error}"))?
                .id(),
            CommitRequest::Id(id) => {
                workspace
                    .commit(id)
                    .map_err(|error| format!("cannot select commit {id}: {error}"))?;
                id.clone()
            }
        };
        Ok(Self { id, workspace })
    }

    pub(super) fn id(&self) -> &CommitId {
        &self.id
    }

    pub(super) fn into_capture(self) -> CommitCapture {
        CommitCapture {
            id: self.id,
            workspace: self.workspace,
            remaining: MAX_SOURCE_BYTES,
            texts: HashMap::new(),
        }
    }
}

fn bound_workspace(target: &Target) -> Result<Workspace, String> {
    let workspace = Workspace::discover(&target.root)
        .map_err(|error| format!("cannot reopen bound repository: {error}"))?;
    if workspace.root() != target.root || workspace.key() != target.key {
        return Err("the bound checkout no longer identifies the original repository".to_owned());
    }
    Ok(workspace)
}

/// Confirm that the checkout still has the repository identity bound at startup.
pub(super) fn validate_binding(target: &Target) -> Result<(), String> {
    bound_workspace(target).map(drop)
}

/// A selected source whose distinct paths are loaded once under one byte budget.
#[derive(Debug)]
pub(super) struct CommitCapture {
    id: CommitId,
    workspace: Workspace,
    remaining: u64,
    texts: HashMap<PathBuf, String>,
}

impl CommitCapture {
    pub(super) fn text(&mut self, path: &Path) -> Result<&str, String> {
        if !self.texts.contains_key(path) {
            let blob = self
                .workspace
                .commit_blob(&self.id, path, self.remaining.min(MAX_SOURCE_BYTES))
                .map_err(|error| {
                    format!(
                        "cannot load {} from selected commit {}: {error}",
                        path.display(),
                        self.id
                    )
                })?
                .ok_or_else(|| {
                    format!(
                        "{} is absent from selected commit {}",
                        path.display(),
                        self.id
                    )
                })?;
            if is_binary(blob.bytes()) {
                return Err(format!(
                    "{} in selected commit {} is binary, not text",
                    path.display(),
                    self.id
                ));
            }
            let size = blob.size();
            let text = String::from_utf8(blob.into_bytes()).map_err(|_error| {
                format!(
                    "{} in selected commit {} is not valid UTF-8 text",
                    path.display(),
                    self.id
                )
            })?;
            self.remaining = self
                .remaining
                .checked_sub(size)
                .ok_or_else(|| "selected source byte budget was exceeded".to_owned())?;
            self.texts.insert(path.to_path_buf(), text);
        }
        self.texts
            .get(path)
            .map(String::as_str)
            .ok_or_else(|| "captured source cache lost a loaded path".to_owned())
    }
}

/// The legacy board cursor or a cursor pinned to a selected commit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub(super) enum After {
    Commit(CommitAfter),
    Board(BoardAfter),
}

impl After {
    pub(super) fn position(&self) -> &BoardAfter {
        match self {
            Self::Board(position) => position,
            Self::Commit(cursor) => &cursor.position,
        }
    }

    pub(super) fn commit(&self) -> Option<&str> {
        match self {
            Self::Board(_) => None,
            Self::Commit(cursor) => Some(&cursor.resolved_commit),
        }
    }

    pub(super) fn board(updated: u64, id: String) -> Self {
        Self::Board(BoardAfter { updated, id })
    }

    pub(super) fn selected(updated: u64, id: String, commit: &CommitId) -> Self {
        Self::Commit(CommitAfter {
            position: BoardAfter { updated, id },
            resolved_commit: commit.as_str().to_owned(),
        })
    }
}

/// A stable position in `threads` ordering.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BoardAfter {
    /// The prior page's last `updated` timestamp.
    pub(super) updated: u64,
    /// The prior page's last thread id.
    pub(super) id: String,
}

/// A stable position bound to one immutable commit-origin filter.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CommitAfter {
    #[serde(flatten)]
    #[schemars(flatten)]
    position: BoardAfter,
    /// The full commit ID echoed by the preceding selected page.
    #[schemars(regex(pattern = r"^[0-9a-f]{40}$"))]
    resolved_commit: String,
}
