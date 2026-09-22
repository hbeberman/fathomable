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
                .exact_head_commit()
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

/// Confirm that the checkout still has the identity captured when this target
/// was selected, either at startup or for the current call.
pub(super) fn validate_binding(target: &Target) -> Result<(), String> {
    bound_workspace(target).map(drop)
}

/// A selected source whose distinct paths are loaded once under one byte budget.
#[derive(Debug)]
pub(super) struct CommitCapture {
    id: CommitId,
    workspace: Workspace,
    remaining: u64,
    texts: HashMap<PathBuf, Result<String, String>>,
}

impl CommitCapture {
    pub(super) fn text(&mut self, path: &Path) -> Result<&str, String> {
        if !self.texts.contains_key(path) {
            let text = self.load_text(path);
            self.texts.insert(path.to_path_buf(), text);
        }
        self.texts
            .get(path)
            .ok_or_else(|| "captured source cache lost a loaded path".to_owned())?
            .as_ref()
            .map(String::as_str)
            .map_err(Clone::clone)
    }

    fn load_text(&mut self, path: &Path) -> Result<String, String> {
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
        self.remaining = self
            .remaining
            .checked_sub(blob.size())
            .ok_or_else(|| "selected source byte budget was exceeded".to_owned())?;
        if is_binary(blob.bytes()) {
            return Err(format!(
                "{} in selected commit {} is binary, not text",
                path.display(),
                self.id
            ));
        }
        String::from_utf8(blob.into_bytes()).map_err(|_error| {
            format!(
                "{} in selected commit {} is not valid UTF-8 text",
                path.display(),
                self.id
            )
        })
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{After, CommitRequest, SelectedCommit};
    use crate::mcp::Target;
    use fathomable_testing::{TempDir, git};
    use serde_json::json;

    #[test]
    fn commit_source_cursors_preserve_legacy_shape_and_reject_ambiguity()
    -> Result<(), serde_json::Error> {
        let board = r#"{"updated":1,"id":"1-1-1"}"#;
        let cursor: After = serde_json::from_str(board)?;
        assert!(cursor.commit().is_none());
        assert_eq!(serde_json::to_string(&cursor)?, board);

        let selected = json!({
            "updated": 1,
            "id": "1-1-1",
            "resolved_commit": "1111111111111111111111111111111111111111"
        });
        let cursor: After = serde_json::from_value(selected.clone())?;
        assert_eq!(
            cursor.commit(),
            Some("1111111111111111111111111111111111111111")
        );
        assert_eq!(serde_json::to_value(cursor)?, selected);
        for malformed in [
            r#"{"updated":1,"id":"1-1-1","extra":true}"#,
            r#"{"updated":1,"id":"1-1-1","resolved_commit":null}"#,
            r#"{"updated":1,"id":"1-1-1","resolved_commit":1}"#,
            r#"{"updated":1,"id":"1-1-1","resolved_commit":"c","extra":true}"#,
            r#"{"updated":1,"updated":2,"id":"1-1-1"}"#,
            r#"{"updated":1,"id":"1-1-1","resolved_commit":"c","resolved_commit":"d"}"#,
        ] {
            assert!(
                serde_json::from_str::<After>(malformed).is_err(),
                "{malformed}"
            );
        }
        Ok(())
    }

    #[test]
    fn commit_source_rejected_text_is_cached_and_charged() -> Result<(), Box<dyn std::error::Error>>
    {
        for bytes in [&b"\0bin"[..], &b"\xffbad"[..]] {
            let dir = TempDir::new("commit-source-invalid-budget")?;
            git::init(&dir.0)?;
            git::commit_bytes_and_stage(&dir.0, "bad.md", bytes)?;
            let target = Target::discover(&dir.0)?;
            let mut capture =
                SelectedCommit::resolve(&target, &CommitRequest::Head)?.into_capture();
            capture.remaining = 4;

            let first = capture
                .text(Path::new("bad.md"))
                .err()
                .ok_or("invalid text accepted")?;
            assert_eq!(
                capture.remaining, 0,
                "invalid text still consumes raw bytes"
            );
            assert_eq!(
                capture.text(Path::new("bad.md")),
                Err(first),
                "rejected paths must not be loaded and charged twice"
            );
        }
        let dir = TempDir::new("commit-source-mixed-budget")?;
        git::init(&dir.0)?;
        git::commit_and_stage(
            &dir.0,
            &[("bad.md", "\0bin"), ("ok.md", "text"), ("extra.md", "x")],
        )?;
        let target = Target::discover(&dir.0)?;
        let mut capture = SelectedCommit::resolve(&target, &CommitRequest::Head)?.into_capture();
        capture.remaining = 8;
        assert!(
            capture
                .text(Path::new("bad.md"))
                .is_err_and(|error| error.contains("binary"))
        );
        assert_eq!(capture.text(Path::new("ok.md"))?, "text");
        let over = capture
            .text(Path::new("extra.md"))
            .err()
            .ok_or("budget not exhausted")?;
        assert!(over.contains("0-byte limit"), "{over}");
        Ok(())
    }

    #[test]
    fn commit_source_capture_keeps_its_pin_when_head_moves()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("commit-source-head-pin")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("a.md", "first"), ("b.md", "original")])?;
        let target = Target::discover(&dir.0)?;
        let selected = SelectedCommit::resolve(&target, &CommitRequest::Head)?;
        let original = selected.id().clone();
        let mut capture = selected.into_capture();
        assert_eq!(capture.text(Path::new("a.md"))?, "first");

        git::commit_and_stage(&dir.0, &[("a.md", "second"), ("b.md", "changed")])?;
        assert_eq!(capture.text(Path::new("b.md"))?, "original");
        assert_eq!(capture.id, original);
        assert_ne!(
            SelectedCommit::resolve(&target, &CommitRequest::Head)?.id(),
            &original
        );

        fs::rename(dir.0.join(".git"), dir.0.join("former.git"))?;
        let error = SelectedCommit::resolve(&target, &CommitRequest::Id(original))
            .err()
            .ok_or("changed binding accepted")?;
        assert!(error.contains("original repository"), "{error}");
        Ok(())
    }
}
