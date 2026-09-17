// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Explicit workspace review points.
//!
//! The old per-file checkpoint timeline is intentionally gone.  These
//! actions save a workspace-wide
//! [`ReviewPointStore`](fathomable_core::review_points::ReviewPointStore)
//! capture and never change the selected comparison.

use super::App;

impl App {
    /// Ask for an optional name before saving a workspace review point.
    pub(crate) fn request_review_point(&mut self) {
        if self.review_points.is_none() {
            self.notice("review points unavailable; see the log");
            return;
        }
        self.open_picker(super::PickerKind::ReviewPointName);
    }

    /// Save a workspace review point without changing the selected comparison.
    pub(crate) fn save_review_point(&mut self, name: Option<&str>) {
        let Some(store) = self.review_points.as_mut() else {
            self.notice("review points unavailable; see the log");
            return;
        };
        match store.capture(&mut self.workspace, name) {
            Ok(result) if result.published() => {
                if let Some(point) = result.point() {
                    self.push_toast(format!("review point saved: {}", point.id()));
                }
                self.refresh_comparison();
            }
            Ok(result) => {
                let detail = result
                    .issues()
                    .first()
                    .map_or("capture was not published", |issue| issue.detail());
                self.notice(format!("review point not saved: {detail}"));
            }
            Err(error) => self.notice(format!("cannot save review point: {error}")),
        }
    }
}
