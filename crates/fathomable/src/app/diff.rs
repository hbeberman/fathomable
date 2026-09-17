// @okf-doc: /decisions/0060-one-diff-two-sides.md
//! Rendering helpers for the viewer-wide comparison.
//!
//! Endpoint selection lives in [`super::comparison`].  This module keeps the
//! existing layout-facing pair types and routes the input actions to that one
//! selection.

use std::path::Path;

use fathomable_core::diff::Whitespace;

use super::{App, PickerKind};

/// A side name retained by the layout and input code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Side {
    /// The selected comparison base.
    ComparisonBase,
    /// The selected comparison target.
    ComparisonTarget,
}

/// Where a diff layout gets its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Text {
    Owned(String),
}

/// What the diff layout displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DiffBody {
    Diff { base: Text, target: Text },
    Notice(String),
}

/// The displayed pair and its labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffView {
    pub(crate) base: Side,
    pub(crate) target: Side,
    pub(crate) header: String,
    pub(crate) badge: String,
    pub(crate) body: DiffBody,
}

impl App {
    /// Open the unified comparison control.
    pub(crate) fn toggle_head_diff(&mut self) {
        self.show_comparison_diff();
    }

    /// Leave the unified diff display.
    pub(crate) fn leave_diff(&mut self) {
        self.comparison_diff = false;
        for doc in &mut self.docs {
            doc.view.leave_diff();
        }
        self.relayout();
    }

    /// Escape the view's transient state, then leave the diff.
    pub(crate) fn escape_view(&mut self) {
        if !self.view_mut().escape() && self.view().diff_view() {
            self.leave_diff();
        }
    }

    /// Open the comparison base picker.
    pub(crate) fn pick_diff_side(&mut self, target: bool) {
        self.open_picker(if target {
            PickerKind::ComparisonTarget
        } else {
            PickerKind::ComparisonBase
        });
    }

    /// Toggle whitespace handling for all comparison surfaces.
    pub(crate) fn toggle_whitespace(&mut self) {
        self.comparison.toggle_whitespace(&mut self.workspace);
        let compare = self.comparison.compare();
        self.compare = compare;
        for doc in &mut self.docs {
            doc.view.set_compare(compare);
        }
        self.rebuild_comparison_status();
        self.sift_tree();
        self.refresh_all_marks();
        self.persist_comparison();
        if self.comparison_diff {
            self.show_comparison_diff();
        }
        self.push_toast(match compare.whitespace {
            Whitespace::Ignore => "whitespace ignored".to_owned(),
            Whitespace::Exact => "whitespace compared".to_owned(),
        });
    }

    /// The pair label shown above a unified diff.
    pub(crate) fn diff_header(&self) -> Option<String> {
        self.view().diff().map(|diff| {
            if self.comparison.compare().whitespace == Whitespace::Ignore {
                format!("{} · whitespace ignored", diff.header)
            } else {
                diff.header.clone()
            }
        })
    }

    /// Rows reserved for the comparison header.
    pub(crate) fn diff_chrome_rows(&self) -> usize {
        usize::from(self.has_document() && self.view().diff_view())
    }

    /// Apply the selected endpoint from a picker.
    pub(super) fn choose_diff_side_input(&mut self, kind: PickerKind, item: &str, input: &str) {
        let value = if input.trim().is_empty() || item != input {
            item.to_owned()
        } else {
            input.to_owned()
        };
        if matches!(kind, PickerKind::ComparisonBase)
            && let Some((first, last)) = value.split_once("..")
        {
            let first = match self.workspace.resolve_revision(first.trim()) {
                Ok(commit) => commit.id(),
                Err(error) => {
                    self.notice(format!("cannot resolve batch start `{first}`: {error}"));
                    return;
                }
            };
            let last = match self.workspace.resolve_revision(last.trim()) {
                Ok(commit) => commit.id(),
                Err(error) => {
                    self.notice(format!("cannot resolve batch end `{last}`: {error}"));
                    return;
                }
            };
            match self.workspace.contiguous_batch(&first, &last) {
                Ok(batch) => {
                    self.comparison.set_base(
                        batch.before().clone(),
                        &mut self.workspace,
                        self.review_points.as_ref(),
                    );
                    self.comparison.set_target(
                        batch.after().clone(),
                        &mut self.workspace,
                        self.review_points.as_ref(),
                    );
                    self.refresh_comparison();
                }
                Err(error) => self.notice(format!("cannot select commit batch: {error}")),
            }
            return;
        }
        let endpoint = match self.resolve_comparison_endpoint(&value) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.notice(error);
                return;
            }
        };
        match kind {
            PickerKind::ComparisonBase => self.set_comparison_base(endpoint),
            PickerKind::ComparisonTarget => self.set_comparison_target(endpoint),
            _ => {}
        }
    }

    /// Show the selected comparison for the current path.
    pub(crate) fn show_comparison_diff(&mut self) {
        let Some(index) = self.current else {
            self.notice("no file open");
            return;
        };
        self.comparison_diff = true;
        if self.comparison.stale() {
            if self.view().diff().is_some() {
                self.view_mut().mark_diff_stale();
                self.relayout();
                return;
            }
            let Some(body) = self.view().retained_comparison_body() else {
                self.notice("comparison is stale; no retained diff is available for this path");
                return;
            };
            let relative = self.docs[index].relative.clone();
            let change = self
                .comparison()
                .and_then(|comparison| {
                    comparison
                        .changes()
                        .iter()
                        .find(|change| change.path() == relative)
                })
                .cloned();
            let label = self.comparison_path_label(change.as_ref());
            self.view_mut().show_diff(DiffView {
                base: Side::ComparisonBase,
                target: Side::ComparisonTarget,
                header: format!("{} · {}", label.0, label.1),
                badge: "DIFF stale".to_owned(),
                body,
            });
            self.relayout();
            return;
        }
        self.apply_comparison_projection(index);
        let relative = self.docs[index].relative.clone();
        let Some(comparison) = self.comparison() else {
            let error = self
                .comparison
                .error()
                .unwrap_or("comparison is not available")
                .to_owned();
            self.notice(error);
            return;
        };
        let change = comparison
            .changes()
            .iter()
            .find(|change| change.path() == relative);
        let body = self.view().retained_comparison_body().unwrap_or_else(|| {
            let base_text = self.comparison_endpoint_text(comparison.base(), &relative);
            let target_text = self.comparison_endpoint_text(comparison.target(), &relative);
            match (base_text, target_text) {
                (Ok(base), Ok(target)) => DiffBody::Diff {
                    base: Text::Owned(base.unwrap_or_default()),
                    target: Text::Owned(target.unwrap_or_default()),
                },
                (Err(error), _) | (_, Err(error)) => DiffBody::Notice(error),
            }
        });
        let label = self.comparison_path_label(change);
        let header = format!("{} · {}", label.0, label.1);
        let badge = if self.comparison.stale() {
            "DIFF stale".to_owned()
        } else {
            "DIFF comparison".to_owned()
        };
        self.view_mut().show_diff(DiffView {
            base: Side::ComparisonBase,
            target: Side::ComparisonTarget,
            header,
            badge,
            body,
        });
        self.relayout();
    }

    /// Kept for callers that need the active pair after a refresh.
    pub(crate) fn refresh_comparison(&mut self) {
        let branch_changed = self.comparison.head_changed(&self.workspace);
        self.comparison
            .refresh(&mut self.workspace, self.review_points.as_ref());
        self.compare = self.comparison.compare();
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            for doc in &mut self.docs {
                doc.view.mark_diff_stale();
            }
            self.notice(error);
            return;
        }
        self.rebuild_comparison_status();
        for index in 0..self.docs.len() {
            self.apply_comparison_projection(index);
        }
        self.refresh_all_marks();
        self.sift_tree();
        if branch_changed
            && self.comparison.target()
                == &fathomable_core::workspace::ComparisonEndpoint::WorkingTree
        {
            self.notice("working tree changed; pinned comparison base retained");
        }
        if self.comparison_diff {
            self.show_comparison_diff();
        }
    }

    fn comparison_path_label(
        &self,
        change: Option<&fathomable_core::diff::PathChange>,
    ) -> (String, String) {
        let base = self.comparison.base().to_string();
        let target = self.comparison.target().to_string();
        if let Some(change) = change {
            if matches!(change.target(), fathomable_core::diff::PathState::Absent) {
                return (base, format!("{target} (deleted)"));
            }
            if matches!(change.base(), fathomable_core::diff::PathState::Absent) {
                return (format!("{base} (empty)"), target);
            }
            if matches!(
                change.target(),
                fathomable_core::diff::PathState::Missing(_)
            ) {
                return (base, format!("{target} (unavailable)"));
            }
        }
        (base, target)
    }

    /// One selected endpoint's text, with explicit review-point reconstruction.
    pub(crate) fn comparison_endpoint_text(
        &self,
        endpoint: &fathomable_core::workspace::ComparisonEndpoint,
        path: &Path,
    ) -> Result<Option<String>, String> {
        if let fathomable_core::workspace::ComparisonEndpoint::ReviewPoint(id) = endpoint {
            let store = self
                .review_points
                .as_ref()
                .ok_or_else(|| format!("review point {id} is unavailable"))?;
            let point = store
                .get(id)
                .ok_or_else(|| format!("review point {id} is unavailable"))?;
            let bytes = store
                .load_bytes(point, &self.workspace, path)
                .map_err(|error| error.to_string())?;
            return bytes
                .map(|bytes| String::from_utf8(bytes).map_err(|error| error.to_string()))
                .transpose();
        }
        self.workspace
            .endpoint_text(endpoint, path)
            .map_err(|error| error.to_string())
    }

    /// Text used to construct a document for a historical-only path.
    pub(crate) fn comparison_display_text(&self, path: &Path) -> Result<Option<String>, String> {
        let Some(comparison) = self.comparison.current() else {
            return Ok(None);
        };
        let target = self.comparison_endpoint_text(comparison.target(), path)?;
        if target.is_some() {
            return Ok(target);
        }
        self.comparison_endpoint_text(comparison.base(), path)
    }
}
