// @okf-doc: /decisions/0060-one-diff-two-sides.md
//! Rendering helpers for the viewer-wide comparison.
//!
//! Endpoint selection lives in [`super::comparison`].  This module keeps the
//! existing layout-facing pair types and routes the input actions to that one
//! selection.

use std::fs;
use std::io;
use std::path::Path;

use fathomable_core::Document;
use fathomable_core::config::DiffMode;
use fathomable_core::content::Policy;
use fathomable_core::diff::Whitespace;
use fathomable_core::workspace::ComparisonEndpoint;

use super::{App, ComparisonSide, PickerKind};

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

/// The displayed pair, status badge, and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffView {
    pub(crate) base: Side,
    pub(crate) target: Side,
    pub(crate) badge: String,
    pub(crate) body: DiffBody,
}

impl App {
    /// The selected session-wide diff presentation.
    pub(crate) const fn diff_mode(&self) -> DiffMode {
        self.diff_mode
    }

    /// Select one session-wide diff presentation.
    pub(crate) fn select_diff_mode(&mut self, mode: DiffMode) {
        if mode == self.diff_mode {
            if mode == DiffMode::Off && self.comparison.restore_mode.is_some() {
                self.comparison.cancel();
                self.refresh_off_target();
            }
            return;
        }
        if self.annotation_draft_blocks("changing diff mode") {
            return;
        }
        if mode == DiffMode::Off {
            self.comparison.cancel();
            if self.diff_mode != DiffMode::Off {
                self.last_active_diff_mode = self.diff_mode;
            }
            self.suspend_changed_filter();
            self.diff_mode = DiffMode::Off;
            self.refresh_off_target();
            return;
        }

        if self.diff_mode == DiffMode::Off {
            let recovered = match self.reload_selected_review_point() {
                Ok(recovered) => recovered,
                Err(error) => {
                    self.notice(error);
                    return;
                }
            };
            self.comparison
                .refresh(&mut self.workspace, self.review_points.as_ref());
            self.comparison.restore_mode = Some(mode);
            if self.comparison.pending() {
                return;
            }
            if let Some(error) = self.comparison.error().map(str::to_owned) {
                self.notice(error);
                return;
            }
            if recovered.is_some() {
                self.repair_review_point_fallback_projection();
            }
            self.activate_diff_mode(mode);
            if let Some(id) = recovered {
                self.notice(format!(
                    "review point {} was deleted; Source reset to {}",
                    id.chars().take(8).collect::<String>(),
                    self.comparison_menu_pair().0
                ));
            }
        } else {
            self.diff_mode = mode;
            self.last_active_diff_mode = mode;
            match mode {
                DiffMode::Unified => self.show_unified_diff(),
                DiffMode::Normal => {
                    for doc in &mut self.docs {
                        doc.view.clear_diff();
                    }
                    self.relayout();
                }
                DiffMode::Off => unreachable!(),
            }
        }
    }

    /// Escape only the view's transient selection, search, or input state.
    pub(crate) fn escape_view(&mut self) {
        self.view_mut().escape();
    }

    /// Toggle rendered/source display when the selected mode supports it.
    pub(crate) fn toggle_source_view(&mut self) {
        if !self.has_document() {
            self.notice("no file open");
            return;
        }
        if self.diff_mode == DiffMode::Unified {
            self.notice("source view is unavailable in unified diff mode");
            return;
        }
        if !self.view().source_view_available() {
            self.notice("rendered view is unavailable for this file");
            return;
        }
        let changed = self.view_mut().toggle_source_view();
        debug_assert!(changed, "source-view availability changed during dispatch");
    }

    /// Whether the current action can switch rendered/source display.
    pub(crate) fn source_view_available(&self) -> bool {
        self.has_document()
            && self.diff_mode != DiffMode::Unified
            && self.view().source_view_available()
    }

    /// Open the comparison base picker.
    pub(crate) fn pick_diff_side(&mut self, target: bool) {
        self.open_picker(if target {
            PickerKind::ComparisonTarget
        } else {
            PickerKind::ComparisonBase
        });
    }

    /// Open the commit picker that compares one commit with its first parent.
    pub(crate) fn pick_commit_parent(&mut self) {
        if !self.workspace.is_git() {
            self.notice("commit comparison is unavailable outside a Git repository");
            return;
        }
        self.open_picker(PickerKind::ComparisonCommit);
    }

    /// Toggle whitespace handling for all comparison surfaces.
    pub(crate) fn toggle_whitespace(&mut self) {
        if self.diff_mode == DiffMode::Off {
            self.notice("diff mode is off");
            return;
        }
        self.comparison.toggle_whitespace();
        self.refresh_comparison();
        let compare = self.comparison.compare();
        for doc in &mut self.docs {
            doc.view.set_compare(compare);
        }
        if self.diff_mode == DiffMode::Off {
            self.comparison_status = fathomable_core::status::Status::default();
        } else {
            self.rebuild_comparison_status();
            self.restore_changed_filter();
        }
        self.sift_tree();
        self.refresh_all_marks();
        self.persist_comparison();
        if self.diff_mode == DiffMode::Unified {
            self.show_unified_diff();
        }
        self.push_toast(match compare.whitespace {
            Whitespace::Ignore => "whitespace ignored".to_owned(),
            Whitespace::Exact => "whitespace compared".to_owned(),
        });
    }

    /// Apply the selected endpoint from a picker.
    pub(super) fn choose_diff_side_input(&mut self, kind: PickerKind, item: &str, input: &str) {
        let side = match kind {
            PickerKind::ComparisonBase => ComparisonSide::Base,
            PickerKind::ComparisonTarget => ComparisonSide::Target,
            PickerKind::ComparisonCommit => ComparisonSide::CommitParent,
            _ => return,
        };
        match item {
            "Tags..." => {
                self.open_picker(PickerKind::ComparisonTags(side));
                return;
            }
            "Branches..." => {
                self.open_picker(PickerKind::ComparisonBranches(side));
                return;
            }
            "Review points..." if side == ComparisonSide::Base => {
                self.open_picker(PickerKind::ComparisonReviewPoints);
                return;
            }
            "Advanced..." => {
                self.open_picker(PickerKind::ComparisonAdvanced(side));
                return;
            }
            _ => {}
        }
        let value = if input.trim().is_empty() || item != input {
            item.to_owned()
        } else {
            input.to_owned()
        };
        let endpoint = match self.resolve_comparison_endpoint(&value) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.notice(error);
                return;
            }
        };
        let alias = if value.eq_ignore_ascii_case("HEAD") {
            Some(super::comparison::EndpointAlias::Head)
        } else {
            value
                .strip_prefix("tag ")
                .map(str::to_owned)
                .map(super::comparison::EndpointAlias::Tag)
        };
        match kind {
            PickerKind::ComparisonBase => self.set_comparison_base_aliased(endpoint, alias),
            PickerKind::ComparisonTarget => self.set_comparison_target_aliased(endpoint, alias),
            PickerKind::ComparisonCommit => {
                let ComparisonEndpoint::Commit(id) = endpoint else {
                    self.notice("choose a commit");
                    return;
                };
                self.select_commit_parent(&id, alias);
            }
            _ => {}
        }
    }

    /// Show the selected comparison for the current path.
    pub(super) fn show_unified_diff(&mut self) {
        let Some(index) = self.current else {
            return;
        };
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
            self.view_mut().show_diff(DiffView {
                base: Side::ComparisonBase,
                target: Side::ComparisonTarget,
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
        let badge = if self.comparison.stale() {
            "DIFF stale".to_owned()
        } else {
            "DIFF comparison".to_owned()
        };
        self.view_mut().show_diff(DiffView {
            base: Side::ComparisonBase,
            target: Side::ComparisonTarget,
            badge,
            body,
        });
        self.relayout();
    }

    /// Kept for callers that need the active pair after a refresh.
    pub(crate) fn refresh_comparison(&mut self) {
        if self.comparison.defer_refresh_for_annotation() {
            return;
        }
        let recovered = match self.reload_selected_review_point() {
            Ok(recovered) => recovered,
            Err(error) => {
                if self.diff_mode == DiffMode::Off && self.comparison.restore_mode.is_none() {
                    self.refresh_off_target();
                    self.notice(error);
                } else {
                    self.comparison.record_error(error);
                    self.apply_refreshed_comparison(false);
                }
                return;
            }
        };
        if self.diff_mode == DiffMode::Off && self.comparison.restore_mode.is_none() {
            self.refresh_off_target();
            if let Some(id) = recovered {
                self.notice(format!(
                    "review point {} was deleted; Source reset to {}",
                    id.chars().take(8).collect::<String>(),
                    self.comparison_menu_pair().0
                ));
            }
            return;
        }
        let branch_changed = self.comparison.head_changed(&self.workspace);
        self.comparison
            .refresh(&mut self.workspace, self.review_points.as_ref());
        if recovered.is_some() {
            self.repair_review_point_fallback_projection();
        }
        self.apply_refreshed_comparison(branch_changed);
        if let Some(id) = recovered {
            self.notice(format!(
                "review point {} was deleted; Source reset to {}",
                id.chars().take(8).collect::<String>(),
                self.comparison_menu_pair().0
            ));
        }
    }

    pub(super) fn apply_refreshed_comparison(&mut self, branch_changed: bool) {
        if self.comparison.pending() {
            for doc in &mut self.docs {
                doc.view.mark_diff_stale();
            }
            return;
        }
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            for doc in &mut self.docs {
                doc.view.mark_diff_stale();
            }
            self.notice(error);
            return;
        }
        self.rebuild_comparison_status();
        self.restore_changed_filter();
        for index in 0..self.docs.len() {
            self.apply_comparison_projection(index);
        }
        self.refresh_all_marks();
        self.sift_tree();
        if branch_changed
            && self.comparison.target()
                == &fathomable_core::workspace::ComparisonEndpoint::WorkingTree
        {
            self.notice("working tree changed; pinned diff source retained");
        }
        match self.diff_mode {
            DiffMode::Unified => self.show_unified_diff(),
            DiffMode::Normal => {
                for doc in &mut self.docs {
                    doc.view.clear_diff();
                }
                self.relayout();
            }
            DiffMode::Off => unreachable!("Off refreshes Target without a comparison"),
        }
    }

    pub(super) fn activate_diff_mode(&mut self, mode: DiffMode) {
        debug_assert_ne!(mode, DiffMode::Off);
        self.diff_mode = mode;
        self.last_active_diff_mode = mode;
        self.off_target_paths = None;
        self.apply_refreshed_comparison(false);
    }

    /// Refresh Target-only state without evaluating or reading Base.
    fn refresh_off_target(&mut self) {
        self.comparison.refresh_target_alias(&self.workspace);
        self.comparison_status = fathomable_core::status::Status::default();
        let target = self.comparison.target().clone();
        if target != ComparisonEndpoint::WorkingTree {
            self.comparison.refresh_paths(&self.workspace);
            self.off_target_paths = None;
            for index in 0..self.docs.len() {
                self.clear_off_projection(
                    index,
                    Some("Target paths scanning; coverage is incomplete".to_owned()),
                );
            }
            self.sift_tree();
            self.relayout();
            return;
        }
        self.comparison.cancel();
        self.off_target_paths = None;
        self.finish_off_target();
    }

    pub(super) fn finish_off_target(&mut self) {
        let mut first_error = None;
        for index in 0..self.docs.len() {
            if let Err(error) = self.apply_off_projection(index)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.refresh_all_marks();
        self.sift_tree();
        if let Some(error) = first_error {
            self.notice(error);
        }
        self.relayout();
    }

    pub(super) fn clear_off_projection(&mut self, index: usize, notice: Option<String>) {
        let doc = &mut self.docs[index];
        doc.view.clear_diff();
        doc.view.clear_comparison_body();
        doc.view.set_bases(None, None);
        let _ = doc.document.replace_snapshot(Vec::new());
        doc.view.reload(String::new());
        doc.view.set_worktree_missing(false);
        doc.comparison_notice = notice;
        doc.deleted = None;
    }

    pub(super) fn apply_off_projection(&mut self, index: usize) -> Result<(), String> {
        let path = self.docs[index].relative.clone();
        let (document, target_absent) = match self.off_target_document(&path) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.clear_off_projection(index, Some(error.clone()));
                return Err(error);
            }
        };
        let text = document.text().unwrap_or_default().to_owned();
        let doc = &mut self.docs[index];
        doc.view.clear_diff();
        doc.view.clear_comparison_body();
        doc.view.set_bases(None, None);
        doc.view.reload(text);
        doc.view.set_worktree_missing(target_absent);
        doc.document = document;
        doc.comparison_notice = target_absent.then(|| "not present in Target".to_owned());
        doc.deleted = None;
        self.queue_highlight(index);
        Ok(())
    }

    /// Load one Off-mode Target document without reading Base or allocating an
    /// over-limit immutable blob.
    pub(crate) fn off_target_document(
        &mut self,
        relative: &Path,
    ) -> Result<(Document, bool), String> {
        let absolute = self.workspace.root().join(relative);
        let policy = Policy {
            attr: self.workspace.diff_attr(relative),
            max_bytes: self.viewer.max_file_bytes(),
        };
        let target = self.comparison.target().clone();
        if target == ComparisonEndpoint::WorkingTree {
            match fs::symlink_metadata(&absolute) {
                Ok(_) => {
                    return Document::load(absolute, policy)
                        .map(|document| (document, false))
                        .map_err(|error| error.to_string());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok((Document::missing(absolute, policy), true));
                }
                Err(error) => {
                    return Err(format!("cannot inspect {}: {error}", absolute.display()));
                }
            }
        }
        let Some(info) = self
            .workspace
            .endpoint_path_info(&target, relative)
            .map_err(|error| error.to_string())?
        else {
            return Ok((Document::missing(absolute, policy), true));
        };
        let size = info.size().ok_or_else(|| {
            format!(
                "{target:?} does not report a byte size for {}",
                relative.display()
            )
        })?;
        let bytes = if size > policy.max_bytes {
            Vec::new()
        } else {
            self.workspace
                .endpoint_bytes(&target, relative)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{} disappeared from {target:?}", relative.display()))?
        };
        Document::from_snapshot_prefix(absolute, bytes, size, policy)
            .map(|document| (document, false))
            .map_err(|error| error.to_string())
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
