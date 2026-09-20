// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Explicit workspace review points.
//!
//! These actions save, rename, and delete workspace-wide
//! [`ReviewPointStore`](fathomable_core::review_points::ReviewPointStore)
//! captures. Deleting the selected Base reconciles it without changing Target.

use super::App;
use fathomable_core::editor::{Buffer, Edit, Motion};
use fathomable_core::review_points::{CaptureResult, ReviewPoint, ReviewPointError};

const DISPLAY_NAME_SCALARS: usize = 64;

/// A bounded, single-line rendering of a name from a legacy capture record.
pub(crate) fn review_point_name(name: Option<&str>) -> String {
    let mut rendered = String::new();
    let mut truncated = false;
    for (index, character) in name.unwrap_or("unnamed").chars().enumerate() {
        if index == DISPLAY_NAME_SCALARS {
            truncated = true;
            break;
        }
        rendered.push(
            if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
                '�'
            } else {
                character
            },
        );
    }
    if truncated {
        rendered.push('…');
    }
    rendered
}

/// Extract the immutable ID carried by either review-point row format.
pub(crate) fn review_point_id_from_row(kind: super::PickerKind, row: &str) -> Option<&str> {
    match kind {
        super::PickerKind::ComparisonReviewPoints => row
            .strip_prefix("review point ")
            .and_then(|rest| rest.split_whitespace().next()),
        super::PickerKind::ReviewPointManage => row.rsplit(' ').next(),
        _ => None,
    }
}

#[cfg(test)]
#[path = "review_points/rename_tests.rs"]
mod rename_tests;

impl App {
    /// Reload point metadata before a point-dependent user action.
    pub(crate) fn reload_review_points(&mut self) -> bool {
        if self.annotation_draft_blocks("browsing review points") {
            return false;
        }

        let Some(store) = self.review_points.as_mut() else {
            self.notice("review points unavailable; see the log");
            return false;
        };
        if let Err(error) = store.reload() {
            self.notice(format!("cannot reload review points: {error}"));
            return false;
        }
        true
    }

    /// Reload and repair a selected review-point Source when it was deleted.
    pub(crate) fn reload_selected_review_point(&mut self) -> Result<Option<String>, String> {
        let Some(id) = self.comparison.review_point_base().map(str::to_owned) else {
            return Ok(None);
        };
        if self.has_new_annotation_draft() {
            return Ok(None);
        }
        let Some(store) = self.review_points.as_mut() else {
            return Ok(None);
        };
        store
            .reload()
            .map_err(|error| format!("cannot reload review points: {error}"))?;
        if store.get(&id).is_some() {
            return Ok(None);
        }
        self.comparison
            .replace_missing_review_point_base(&self.workspace);
        Ok(Some(id))
    }

    /// Ask for an optional name before capturing and selecting a review point.
    pub(crate) fn request_review_point(&mut self) {
        if self.annotation_draft_blocks("saving a review point") {
            return;
        }
        if self.review_points.is_none() {
            self.notice("review points unavailable; see the log");
            return;
        }
        self.open_picker(super::PickerKind::ReviewPointName);
    }

    /// Capture the working tree and select that immutable point as Base.
    pub(crate) fn save_review_point(&mut self, name: Option<&str>) {
        if self.annotation_draft_blocks("saving a review point") {
            return;
        }
        let Some(store) = self.review_points.as_mut() else {
            self.notice("review points unavailable; see the log");
            return;
        };
        let result = store.capture(&mut self.workspace, name);
        self.finish_review_point_capture(result);
    }

    fn finish_review_point_capture(&mut self, result: Result<CaptureResult, ReviewPointError>) {
        match result {
            Ok(result) if result.published() => {
                if let Some(point) = result.point() {
                    let id = point.id().to_owned();
                    let short = id.chars().take(8).collect::<String>();
                    let label = point.name().unwrap_or("Unnamed").to_owned();
                    let exclusions = result.issues().len();
                    let (available, persisted) = self.select_review_point(id);
                    let excluded = if exclusions == 0 {
                        String::new()
                    } else {
                        format!("; {exclusions} excluded")
                    };
                    if available && persisted {
                        self.push_toast(format!(
                            "Saved {label} [{short}]{excluded}; comparison scanning"
                        ));
                    } else if available {
                        self.notice(format!(
                            "Saved {label} [{short}]{excluded}; comparison selected for this viewer, but its preference was not saved"
                        ));
                    } else {
                        self.push_toast(format!(
                            "Saved {label} [{short}]{excluded}; comparison unavailable"
                        ));
                    }
                }
            }
            Ok(result) => {
                let detail = result
                    .issues()
                    .first()
                    .map_or("capture was not published", |issue| issue.detail());
                self.notice(format!("review point not saved: {detail}"));
            }
            Err(ReviewPointError::CommitUncertain { point, detail }) => self.notice(format!(
                "review point {} save outcome is uncertain; reload before retrying: {detail}",
                point.chars().take(8).collect::<String>()
            )),
            Err(error) => self.notice(format!("cannot save review point: {error}")),
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::items_after_test_module,
    reason = "deletion behavior tests separate the save and delete App impl blocks"
)]
mod tests {
    use std::fs;
    use std::io::Write as _;
    use std::path::Path;

    use anyhow::Context as _;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fathomable_core::annotations::{
        Author, ContentIdentity, Draft, FullFileDigest, LineRange, OriginSide, OriginVersion,
        ReviewPointFacts, Store,
    };
    use fathomable_core::clock::now;
    use fathomable_core::config::DiffMode;
    use fathomable_core::review_points::ReviewPointStore;
    use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
    use fathomable_testing::{TempDir, git};

    use crate::app::input::keys;
    use crate::app::testing::AppBuilder;
    use crate::app::{PickerKind, Popup};

    fn fixture(name: &str) -> anyhow::Result<(TempDir, std::path::PathBuf)> {
        let dir = TempDir::new(name)?;
        let root = dir.0.join("repo");
        fs::create_dir(&root)?;
        git::init(&root)?;
        git::commit_and_stage(&root, &[("README.md", "base\n")])?;
        fs::write(root.join("README.md"), "point\n")?;
        Ok((dir, root))
    }

    #[test]
    fn saved_point_is_reported_when_its_comparison_is_unavailable() -> anyhow::Result<()> {
        let (dir, root) = fixture("review-point-save-comparison-failure")?;
        fs::write(root.join("README.md"), "base\n")?;
        let points = dir.0.join("points");
        let mut app = AppBuilder::at(&root)
            .unopened()
            .review_points(&points)
            .build()?;
        let result = app
            .review_points
            .as_mut()
            .context("review-point store")?
            .capture(&mut app.workspace, Some("saved"))?;
        let id = result.point().context("published point")?.id().to_owned();
        let objects = root.join(".git/objects");
        fs::rename(&objects, root.join(".git/objects-away"))?;
        fs::create_dir(&objects)?;

        app.finish_review_point_capture(Ok(result));
        app.settle_background();

        assert!(
            app.review_points
                .as_ref()
                .is_some_and(|store| store.get(&id).is_some())
        );
        assert_eq!(app.comparison.base(), &ComparisonEndpoint::ReviewPoint(id));
        assert!(app.comparison.error().is_some());
        assert!(
            app.toasts()
                .last()
                .is_some_and(|toast| toast.text().contains("Saved saved")
                    && toast.text().contains("comparison scanning"))
        );
        assert!(
            app.message()
                .is_some_and(|message| !message.contains("scanning"))
        );
        Ok(())
    }

    #[test]
    fn deletion_requires_y_and_falls_back_without_losing_thread_provenance() -> anyhow::Result<()> {
        let (dir, root) = fixture("review-point-delete-ui")?;
        let points = dir.0.join("points");
        let mut threads = Store::open(dir.0.join("threads.jsonl"))?;
        let mut workspace = Workspace::discover(&root)?;
        let mut point_store = ReviewPointStore::open(&points)?;
        let point = point_store
            .capture(&mut workspace, Some("before fixes"))?
            .point()
            .context("point")?
            .clone();
        let thread = threads.annotate(
            Draft::new(
                Author::User,
                Path::new("README.md"),
                LineRange::new(1, 1),
                "point finding",
            )
            .at_review_point(
                ReviewPointFacts::new(
                    point.id(),
                    point.head().map(ToString::to_string),
                    Some(ContentIdentity::from_text("point\n")),
                    point.checkout_identity(),
                    FullFileDigest::from_bytes(b"point\n"),
                ),
                OriginSide::Base,
            ),
            "point\n",
            now(),
        )?;
        let mut app = AppBuilder::at(&root)
            .review_points(&points)
            .options(move |mut options| {
                options.store = Some(threads);
                options
            })
            .build()?;
        app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
        app.settle_background();
        assert!(matches!(
            app.thread(&thread)
                .map(fathomable_core::annotations::Thread::origin_version),
            Some(OriginVersion::ReviewPoint { id, .. }) if id == point.id()
        ));

        app.request_review_point_manage();
        assert!(matches!(
            app.popup(),
            Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ReviewPointManage
        ));
        app.picker_confirm();
        keys::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert!(matches!(
            app.popup(),
            Some(Popup::ConfirmReviewPointDelete { id, .. }) if id == point.id()
        ));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.review_points
                .as_ref()
                .is_some_and(|store| store.get(point.id()).is_some()),
            "Enter must not cross the destructive confirmation"
        );
        keys::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert!(
            app.review_points
                .as_ref()
                .is_some_and(|store| store.get(point.id()).is_some()),
            "confirmation must be rendered before it is armed"
        );
        app.arm_review_point_delete_confirmation();
        keys::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        );

        assert!(
            app.review_points
                .as_ref()
                .is_some_and(ReviewPointStore::is_empty)
        );
        assert!(!matches!(
            app.comparison.base(),
            ComparisonEndpoint::ReviewPoint(_)
        ));
        assert!(matches!(
            app.thread(&thread)
                .map(fathomable_core::annotations::Thread::origin_version),
            Some(OriginVersion::ReviewPoint { id, .. }) if id == point.id()
        ));
        Ok(())
    }

    #[test]
    fn external_deletion_preserves_target_on_mode_restore_and_blocks_local_draft_deletion()
    -> anyhow::Result<()> {
        let (dir, root) = fixture("review-point-delete-off")?;
        let points = dir.0.join("points");
        let mut workspace = Workspace::discover(&root)?;
        let mut external = ReviewPointStore::open(&points)?;
        let point = external
            .capture(&mut workspace, Some("outside"))?
            .point()
            .context("point")?
            .clone();
        let target = workspace.head_commit().context("HEAD")?;
        let threads = Store::open(dir.0.join("threads.jsonl"))?;
        let mut app = AppBuilder::at(&root)
            .review_points(&points)
            .options(move |mut options| {
                options.store = Some(threads);
                options
            })
            .build()?;
        app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
        app.settle_background();
        app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
        app.settle_background();
        app.select_diff_mode(DiffMode::Off);
        app.settle_background();
        app.start_new_comment();
        external.delete(point.id())?;
        app.open_picker(PickerKind::ComparisonReviewPoints);
        app.settle_background();
        assert!(matches!(app.popup(), Some(Popup::Compose(_))));
        assert!(
            app.review_points
                .as_ref()
                .is_some_and(|store| store.get(point.id()).is_some()),
            "browsing points must not replace draft provenance"
        );
        app.save_review_point(Some("blocked"));
        app.settle_background();
        assert!(
            app.review_points
                .as_ref()
                .is_some_and(|store| store.get(point.id()).is_some()),
            "capture must not replace draft provenance"
        );
        app.request_review_point_manage();
        assert!(matches!(app.popup(), Some(Popup::Compose(_))));
        assert!(
            app.message()
                .is_some_and(|message| message.contains("submit or cancel"))
        );
        app.compose_cancel();
        app.compose_cancel();

        app.select_diff_mode(DiffMode::Normal);
        app.settle_background();
        assert_eq!(app.diff_mode(), DiffMode::Normal);
        assert_eq!(
            app.comparison.target(),
            &ComparisonEndpoint::Commit(CommitId::parse(&target)?)
        );
        assert!(!matches!(
            app.comparison.base(),
            ComparisonEndpoint::ReviewPoint(_)
        ));
        assert_eq!(app.view().text(), "base\n");
        Ok(())
    }

    #[test]
    fn opening_delete_picker_reloads_points_from_another_store() -> anyhow::Result<()> {
        let (dir, root) = fixture("review-point-delete-reload")?;
        let points = dir.0.join("points");
        let mut app = AppBuilder::at(&root).review_points(&points).build()?;
        let mut workspace = Workspace::discover(&root)?;
        let mut external = ReviewPointStore::open(&points)?;
        external.capture(&mut workspace, Some("external"))?;

        app.request_review_point_manage();
        assert!(matches!(
            app.popup(),
            Some(Popup::Picker(picker))
                if picker.kind() == PickerKind::ReviewPointManage && picker.total() == 1
        ));
        Ok(())
    }

    #[test]
    fn stale_picker_cannot_select_a_point_deleted_elsewhere() -> anyhow::Result<()> {
        let (dir, root) = fixture("review-point-delete-stale-picker")?;
        let points = dir.0.join("points");
        let mut workspace = Workspace::discover(&root)?;
        let mut external = ReviewPointStore::open(&points)?;
        let point = external
            .capture(&mut workspace, Some("external"))?
            .point()
            .context("point")?
            .clone();
        let mut app = AppBuilder::at(&root).review_points(&points).build()?;
        app.open_picker(PickerKind::ComparisonReviewPoints);
        app.settle_background();
        external.delete(point.id())?;

        app.picker_confirm();

        assert_ne!(
            app.comparison.base(),
            &ComparisonEndpoint::ReviewPoint(point.id().to_owned())
        );
        assert!(
            app.message()
                .is_some_and(|message| message.contains("unavailable"))
        );
        Ok(())
    }

    #[test]
    fn corrupt_point_store_does_not_block_off_target_refresh() -> anyhow::Result<()> {
        let (dir, root) = fixture("review-point-delete-off-corrupt")?;
        let points = dir.0.join("points");
        let mut workspace = Workspace::discover(&root)?;
        let mut external = ReviewPointStore::open(&points)?;
        let point = external
            .capture(&mut workspace, Some("point"))?
            .point()
            .context("point")?
            .clone();
        git::commit_and_stage(&root, &[("README.md", "first target\n")])?;
        let first = Workspace::discover(&root)?
            .head_commit()
            .context("first target")?;
        git::commit_and_stage(&root, &[("README.md", "second target\n")])?;
        let second = Workspace::discover(&root)?
            .head_commit()
            .context("second target")?;
        let mut app = AppBuilder::at(&root).review_points(&points).build()?;
        app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
        app.settle_background();
        app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
        app.settle_background();
        app.select_diff_mode(DiffMode::Off);
        app.settle_background();
        fs::OpenOptions::new()
            .append(true)
            .open(points.join("review-points.jsonl"))?
            .write_all(b"{\"event\":\"unknown\"}\n")?;

        app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
        app.settle_background();

        assert_eq!(app.diff_mode(), DiffMode::Off);
        assert_eq!(app.view().text(), "second target\n");
        assert_eq!(
            app.comparison.base(),
            &ComparisonEndpoint::ReviewPoint(point.id().to_owned())
        );
        assert!(
            app.message()
                .is_some_and(|message| message.contains("unexpected event"))
        );
        Ok(())
    }

    #[test]
    fn non_git_fallback_clears_deleted_point_only_content() -> anyhow::Result<()> {
        let dir = TempDir::new("review-point-delete-non-git")?;
        let root = dir.0.join("plain");
        fs::create_dir(&root)?;
        fs::write(root.join("README.md"), "point only\n")?;
        let points = dir.0.join("points");
        let mut workspace = Workspace::discover(&root)?;
        let mut external = ReviewPointStore::open(&points)?;
        let point = external
            .capture(&mut workspace, Some("plain"))?
            .point()
            .context("point")?
            .clone();
        fs::remove_file(root.join("README.md"))?;
        let mut app = AppBuilder::at(&root)
            .review_points(&points)
            .unopened()
            .build()?;
        app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
        app.settle_background();
        app.open(Path::new("README.md"));
        assert_eq!(app.view().text(), "point only\n");
        external.delete(point.id())?;

        app.refresh_comparison();
        app.settle_background();

        assert_eq!(app.comparison.base(), &ComparisonEndpoint::EmptyTree);
        assert_eq!(app.view().text(), "");
        assert!(!app.deleted());
        Ok(())
    }
}

impl App {
    /// Open the review-point manager after validating the shared store.
    pub(crate) fn request_review_point_manage(&mut self) {
        if self.annotation_draft_blocks("managing review points") {
            return;
        }
        if !self.reload_review_points() {
            return;
        }
        if self
            .review_points
            .as_ref()
            .is_none_or(fathomable_core::review_points::ReviewPointStore::is_empty)
        {
            self.notice("no review points to manage");
            return;
        }
        self.open_review_point_picker(
            &super::ReviewPointOrigin {
                kind: super::PickerKind::ReviewPointManage,
                query: String::new(),
            },
            None,
            false,
        );
    }

    /// Rows for the manager, with the full stable ID retained at the end.
    pub(crate) fn review_point_manage_choices(&self) -> Vec<String> {
        self.review_points
            .as_ref()
            .into_iter()
            .flat_map(fathomable_core::review_points::ReviewPointStore::list)
            .map(|point| {
                let label = review_point_name(point.name());
                let short = point.id().chars().take(8).collect::<String>();
                format!(
                    "{short} · {} · {label} · {}",
                    super::draw::format_time(point.created()),
                    point.id()
                )
            })
            .collect()
    }

    fn review_point_origin(
        kind: super::PickerKind,
        query: impl Into<String>,
    ) -> super::ReviewPointOrigin {
        super::ReviewPointOrigin {
            kind,
            query: query.into(),
        }
    }

    fn open_review_point_picker(
        &mut self,
        origin: &super::ReviewPointOrigin,
        selected_id: Option<&str>,
        reload: bool,
    ) -> bool {
        if reload {
            let Some(store) = self.review_points.as_mut() else {
                self.popup = None;
                self.notice("review points unavailable; see the log");
                return false;
            };
            if let Err(error) = store.reload() {
                self.popup = None;
                self.notice(format!("cannot reload review points: {error}"));
                return false;
            }
        }
        let items = match origin.kind {
            super::PickerKind::ComparisonReviewPoints => self.comparison_review_point_choices(),
            super::PickerKind::ReviewPointManage => self.review_point_manage_choices(),
            _ => return false,
        };
        self.open_scoped_picker(origin.kind, items, None);
        let Some(super::Popup::Picker(picker)) = self.popup.as_mut() else {
            return false;
        };
        picker.set_query(&origin.query);
        if let Some(id) = selected_id
            && !picker.select_review_point(id)
            && !origin.query.is_empty()
        {
            picker.set_query("");
            picker.select_review_point(id);
        }
        true
    }

    fn point_from_row(&self, kind: super::PickerKind, row: &str) -> Option<ReviewPoint> {
        let id = review_point_id_from_row(kind, row)?;
        self.review_points
            .as_ref()
            .and_then(|store| store.get(id))
            .cloned()
    }

    /// Open the action card for a manager row.
    pub(crate) fn request_review_point_action(&mut self, row: &str, query: &str) {
        let origin = Self::review_point_origin(super::PickerKind::ReviewPointManage, query);
        let Some(point) = self.point_from_row(origin.kind, row) else {
            self.notice("review point is unavailable; reloading");
            self.open_review_point_picker(&origin, None, true);
            return;
        };
        self.popup = Some(super::Popup::ReviewPointAction(
            super::ReviewPointActionState { point, origin },
        ));
    }

    /// Start renaming the highlighted comparison review point.
    pub(crate) fn rename_selected_review_point(&mut self) {
        let selected = match self.popup.as_ref() {
            Some(super::Popup::Picker(picker))
                if picker.kind() == super::PickerKind::ComparisonReviewPoints =>
            {
                picker
                    .selected_item()
                    .map(|row| (picker.kind(), picker.input().to_owned(), row.to_owned()))
            }
            _ => None,
        };
        let Some((kind, query, row)) = selected else {
            self.notice("choose a review point");
            return;
        };
        let origin = Self::review_point_origin(kind, query);
        let Some(point) = self.point_from_row(kind, &row) else {
            self.notice("review point is unavailable; reloading");
            self.open_review_point_picker(&origin, None, true);
            return;
        };
        self.open_review_point_rename(point, origin, false);
    }

    /// Apply a key edit to the one-line rename buffer.
    pub(crate) fn review_point_rename_edit(&mut self, edit: Edit) {
        if let Some(super::Popup::ReviewPointRename(rename)) = self.popup.as_mut() {
            rename.editor.apply(edit);
        }
    }

    /// Insert ordinary text into the one-line rename buffer.
    pub(crate) fn review_point_rename_insert(&mut self, character: char) {
        if let Some(super::Popup::ReviewPointRename(rename)) = self.popup.as_mut()
            && !character.is_control()
            && !matches!(character, '\u{2028}' | '\u{2029}')
        {
            rename.editor.insert(character.encode_utf8(&mut [0; 4]));
        }
    }

    fn open_review_point_rename(
        &mut self,
        expected: ReviewPoint,
        origin: super::ReviewPointOrigin,
        return_to_action: bool,
    ) {
        let initial = expected.name().unwrap_or_default().to_owned();
        self.popup = Some(super::Popup::ReviewPointRename(
            super::ReviewPointRenameState {
                expected,
                origin,
                editor: Buffer::from_text(initial),
                return_to_action,
            },
        ));
    }

    /// Start rename from the selected point's action card.
    pub(crate) fn review_point_action_rename(&mut self) {
        let Some(super::Popup::ReviewPointAction(action)) = self.popup.take() else {
            return;
        };
        self.open_review_point_rename(action.point, action.origin, true);
    }

    /// Return from the action card to its originating picker.
    pub(crate) fn cancel_review_point_action(&mut self) {
        let Some(super::Popup::ReviewPointAction(action)) = self.popup.take() else {
            return;
        };
        let id = action.point.id().to_owned();
        self.open_review_point_picker(&action.origin, Some(&id), true);
    }

    /// Submit a rename using the point snapshot as the compare-and-swap token.
    pub(crate) fn submit_review_point_rename(&mut self) {
        let Some(super::Popup::ReviewPointRename(rename)) = self.popup.take() else {
            return;
        };
        let id = rename.expected.id().to_owned();
        let result = self
            .review_points
            .as_mut()
            .ok_or_else(|| ReviewPointError::Unavailable { point: id.clone() })
            .and_then(|store| store.rename(&rename.expected, Some(rename.editor.text())));
        let notice = match result {
            Ok(_) => {
                self.push_toast(format!(
                    "Renamed review point {}",
                    id.chars().take(8).collect::<String>()
                ));
                None
            }
            Err(ReviewPointError::StaleRename { .. }) => {
                Some("review point changed elsewhere; reloaded without renaming".to_owned())
            }
            Err(ReviewPointError::DuplicateName) => {
                Some("review-point name is already in use; reloaded without renaming".to_owned())
            }
            Err(ReviewPointError::InvalidName) => Some(
                "review-point name must be one line of at most 128 Unicode scalar values; not renamed"
                    .to_owned(),
            ),
            Err(ReviewPointError::Unavailable { .. }) => {
                Some("review point is unavailable; reloaded without renaming".to_owned())
            }
            Err(ReviewPointError::CommitUncertain { .. }) => Some(
                "review-point rename outcome is uncertain; reloaded before retrying".to_owned(),
            ),
            Err(error) => Some(format!("cannot rename review point: {error}")),
        };
        let reloaded = self.open_review_point_picker(&rename.origin, Some(&id), true);
        if reloaded && let Some(notice) = notice {
            self.notice(notice);
        }
    }

    /// Cancel a rename, returning to the action card or originating picker.
    pub(crate) fn cancel_review_point_rename(&mut self) {
        let Some(super::Popup::ReviewPointRename(rename)) = self.popup.take() else {
            return;
        };
        if rename.return_to_action {
            self.popup = Some(super::Popup::ReviewPointAction(
                super::ReviewPointActionState {
                    point: rename.expected,
                    origin: rename.origin,
                },
            ));
        } else {
            let id = rename.expected.id().to_owned();
            self.open_review_point_picker(&rename.origin, Some(&id), false);
        }
    }

    /// Ask for a separate destructive acknowledgement from the action card.
    pub(crate) fn request_review_point_delete_confirmation(&mut self) {
        let Some(super::Popup::ReviewPointAction(return_to)) = self.popup.take() else {
            return;
        };
        let point = return_to.point();
        self.popup = Some(super::Popup::ConfirmReviewPointDelete {
            id: point.id().to_owned(),
            name: point.name().map(str::to_owned),
            created: point.created(),
            armed: false,
            return_to,
        });
    }

    /// Arm the confirmation only after it has been rendered once.
    pub(crate) fn arm_review_point_delete_confirmation(&mut self) {
        if let Some(super::Popup::ConfirmReviewPointDelete { armed, .. }) = self.popup.as_mut() {
            *armed = true;
        }
    }

    /// Whether destructive confirmation input is accepted.
    pub(crate) fn review_point_delete_confirmation_armed(&self) -> bool {
        matches!(
            self.popup,
            Some(super::Popup::ConfirmReviewPointDelete { armed: true, .. })
        )
    }

    /// Confirm deletion and reconcile a selected Base.
    pub(crate) fn confirm_review_point_delete(&mut self) {
        let Some(super::Popup::ConfirmReviewPointDelete { id, return_to, .. }) = self.popup.take()
        else {
            return;
        };
        let selected = self.comparison.review_point_base() == Some(id.as_str());
        let Some(store) = self.review_points.as_mut() else {
            self.popup = Some(super::Popup::ReviewPointAction(return_to));
            self.notice("review points unavailable; see the log");
            return;
        };
        let result = store.delete(&id);
        let origin = return_to.origin.clone();
        match result {
            Ok(result) if result.point().is_none() => {
                if selected {
                    self.refresh_comparison();
                }
                self.open_review_point_picker(&origin, None, true);
                self.notice("review point was already deleted; list reloaded");
            }
            Ok(result) => {
                let point = result.point().map_or(id.as_str(), ReviewPoint::id);
                let reclaimed = result.reclaimed_blobs();
                let issues = result.issues().len();
                if selected {
                    self.refresh_comparison();
                }
                self.open_review_point_picker(&origin, None, true);
                self.push_toast(format!(
                    "review point {} deleted; reclaimed {reclaimed} blob(s)",
                    point.chars().take(8).collect::<String>()
                ));
                if issues > 0 {
                    self.notice(format!(
                        "review point deleted; {issues} blob cleanup issue(s); see the log"
                    ));
                    for issue in result.issues() {
                        tracing::warn!(
                            blob = issue.blob(),
                            detail = issue.detail(),
                            "review-point cleanup incomplete"
                        );
                    }
                }
            }
            Err(error) => {
                self.popup = Some(super::Popup::ReviewPointAction(return_to));
                self.notice(format!("cannot delete review point: {error}"));
            }
        }
    }

    /// Cancel deletion without changing the store.
    pub(crate) fn cancel_review_point_delete(&mut self) {
        if let Some(super::Popup::ConfirmReviewPointDelete { return_to, .. }) = self.popup.take() {
            self.popup = Some(super::Popup::ReviewPointAction(return_to));
            self.notice("review-point deletion cancelled");
        }
    }

    /// Handle the ordinary editing motions supported by the rename popup.
    pub(crate) fn review_point_rename_motion(&mut self, motion: Motion) {
        self.review_point_rename_edit(Edit::Move(motion));
    }
}
