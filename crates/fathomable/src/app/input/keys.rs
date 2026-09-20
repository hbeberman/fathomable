// @okf-doc: /decisions/0007-key-grammar-and-mouse.md
//! Turn a key press into an [`Action`] through the binding table, and
//! give each action its meaning on the surface that has focus.
//!
//! [`handle_key`] keeps the keys typed so far as the app's prefix; a
//! sequence the table calls a prefix waits, one it calls a miss is
//! dropped (and cancels an armed delete), and a match runs
//! [`App::act`]. Text entry — the draft, the picker, the command
//! line — takes the characters the table leaves alone.

use crossterm::event::KeyEvent;
use fathomable_core::config::DiffMode;
use fathomable_core::editor::{Edit, Motion};
use fathomable_core::tree::{Rule, Tree};

use super::super::{App, Focus, PickerKind, Popup};
use super::bindings::{Action, Chord, Key, Match, Where, lookup};
use super::help;
use crate::app::threads::list::ReviewView;
use crate::app::view::{Effect, Mode};
use crate::app::{commands::CompletionDirection, doctor_view, licenses, mcp_setup, menu_bar};

/// Rows a scroll key or wheel notch moves.
pub(crate) const WHEEL_LINES: isize = 3;

/// Apply a key press to `app`.
pub(crate) fn handle_key(app: &mut App, key: KeyEvent) -> Effect {
    app.sync_text_height();
    let effect = key_event(app, key);
    app.sync_text_height();
    effect
}

/// The surface a key lands on now, or `None` under a popup that any key
/// closes.
#[must_use]
pub(crate) fn place(app: &App) -> Option<Where> {
    if app.title_menu_open() {
        return None;
    }
    match app.popup() {
        Some(
            Popup::Help(_)
            | Popup::Status
            | Popup::Doctor(_)
            | Popup::Licenses(_)
            | Popup::McpSetup(_)
            | Popup::About
            | Popup::Menu(_)
            | Popup::DiffMode(_)
            | Popup::ConfirmQuit
            | Popup::ConfirmBoard { .. }
            | Popup::ReviewPointAction(_)
            | Popup::ReviewPointRename(_)
            | Popup::ConfirmReviewPointDelete { .. },
        ) => None,
        Some(Popup::Compose(_)) => Some(Where::Draft),
        Some(Popup::Picker(_)) => Some(Where::Picker),
        None => Some(
            if matches!(app.view().mode(), Mode::Command | Mode::Search { .. }) {
                Where::Input
            } else {
                match app.focus() {
                    Focus::View => Where::View,
                    Focus::Tree => Where::Tree,
                    Focus::Review => Where::Review,
                    Focus::ThreadsPane => Where::ThreadsPane,
                }
            },
        ),
    }
}

fn key_event(app: &mut App, key: KeyEvent) -> Effect {
    if Chord::from_event(key)
        .is_some_and(|chord| lookup(Where::Any, &[chord]) == Match::Exact(Action::ApplicationMenu))
    {
        return app.act(Action::ApplicationMenu);
    }
    if app.title_menu_open() {
        return menu_bar::key(app, key);
    }
    if !app.panes_fit() {
        if matches!(app.popup(), Some(Popup::ConfirmQuit)) {
            return confirmation_key(app, key);
        }
        if key.code == crossterm::event::KeyCode::Esc {
            app.close_popup();
            app.take_prefix();
            app.cancel_delete();
            if matches!(app.view().mode(), Mode::Command | Mode::Search { .. }) {
                app.view_mut().escape();
            }
            return Effect::None;
        }
        if key.code == crossterm::event::KeyCode::Char('q') && key.modifiers.is_empty() {
            app.request_quit();
            return Effect::None;
        }
        if app.popup().is_some() || matches!(app.view().mode(), Mode::Command | Mode::Search { .. })
        {
            return Effect::None;
        }
    }
    if matches!(app.popup(), Some(Popup::ReviewPointAction(_))) {
        return review_point_action_key(app, key);
    }
    if matches!(app.popup(), Some(Popup::ReviewPointRename(_))) {
        return review_point_rename_key(app, key);
    }
    if matches!(
        app.popup(),
        Some(
            Popup::ConfirmQuit
                | Popup::ConfirmBoard { .. }
                | Popup::ConfirmReviewPointDelete { .. }
        )
    ) {
        return confirmation_key(app, key);
    }
    if matches!(app.popup(), Some(Popup::Doctor(_))) {
        return doctor_view::key(app, key);
    }
    if matches!(app.popup(), Some(Popup::Licenses(_))) {
        return licenses::key(app, key);
    }
    if matches!(app.popup(), Some(Popup::McpSetup(_))) {
        return mcp_setup::key(app, key);
    }
    if matches!(app.popup(), Some(Popup::About)) {
        if key.code == crossterm::event::KeyCode::Esc {
            app.close_popup();
        }
        return Effect::None;
    }
    if matches!(app.popup(), Some(Popup::DiffMode(_))) {
        return app.mode_menu_key(key);
    }
    if matches!(app.popup(), Some(Popup::Help(_))) {
        return help::key(app, key);
    }
    if key.code == crossterm::event::KeyCode::Char('r')
        && key.modifiers == crossterm::event::KeyModifiers::CONTROL
        && matches!(
            app.popup(),
            Some(Popup::Picker(picker))
                if picker.kind() == PickerKind::ComparisonReviewPoints
        )
    {
        app.clear_message();
        app.rename_selected_review_point();
        return Effect::None;
    }
    app.clear_message();
    app.view_mut().clear_message();
    let Some(chord) = Chord::from_event(key) else {
        app.take_prefix();
        app.cancel_delete();
        return Effect::None;
    };
    // The context menu takes its own keys (ADR 0050).
    if matches!(app.popup(), Some(Popup::Menu(_))) {
        return app.menu_key(chord);
    }
    let Some(place) = place(app) else {
        app.close_popup();
        return Effect::None;
    };
    if app.focus() == Focus::View {
        // Reader activity is retained only for in-memory interaction timing.
        app.view_mut().touch();
    }
    typed(app, place, chord)
}

/// `chord` typed after the pending prefix on `place`: an exact binding
/// fires, a prefix waits, a miss is dropped. A click on a which-key
/// entry comes through here too (ADR 0050).
pub(super) fn typed(app: &mut App, place: Where, chord: Chord) -> Effect {
    let mut typed = app.take_prefix();
    typed.push(chord);
    match lookup(place, &typed) {
        Match::Exact(action) => app.act(action),
        Match::Prefix => {
            if !app.panes_fit()
                && !super::bindings::BINDINGS.iter().any(|binding| {
                    available_while_panes_do_not_fit(binding.action)
                        && binding.keys.iter().any(|keys| keys.starts_with(&typed))
                })
            {
                app.cancel_delete();
                return Effect::None;
            }
            // The first `d` of `dd` arms the delete (ADR 0034).
            if typed == [DELETE_PREFIX] {
                app.arm_delete_here();
            }
            app.set_prefix(typed);
            Effect::None
        }
        Match::Miss if typed.len() == 1 => fallback(app, place, chord),
        Match::Miss => {
            // A sequence that led nowhere is swallowed whole.
            app.cancel_delete();
            Effect::None
        }
    }
}

const DELETE_PREFIX: Chord = Chord {
    key: Key::Char('d'),
    ctrl: false,
    alt: false,
    shift: false,
};

/// A single unbound key: text entry takes characters, everything else
/// drops it.
fn fallback(app: &mut App, place: Where, chord: Chord) -> Effect {
    if !app.panes_fit() {
        return Effect::None;
    }
    match (place, chord.key) {
        (Where::Draft, Key::Char(ch)) if !chord.ctrl && !chord.alt => {
            app.compose_insert(ch.encode_utf8(&mut [0; 4]));
        }
        (Where::Picker, Key::Char(ch)) if !chord.ctrl && !chord.alt => app.picker_char(ch),
        (Where::Input, Key::Char(ch)) => app.view_mut().input_char(ch),
        _ => {}
    }
    Effect::None
}

/// Root-relative path of the row under the tree cursor.
pub(super) fn tree_highlight(app: &App) -> Option<std::path::PathBuf> {
    app.tree()
        .and_then(Tree::current)
        .map(|row| row.path().to_path_buf())
}

/// The actions that are far moves (ADR 0049): when one of these moves the
/// reader, the position it left goes on the jumplist. `Confirm` covers
/// the search line, `:N`, and every Enter that opens a file.
fn is_far_move(action: Action) -> bool {
    matches!(
        action,
        Action::Confirm
            | Action::SearchNext
            | Action::SearchPrev
            | Action::Top
            | Action::Bottom
            | Action::ThreadNext
            | Action::ThreadPrev
            | Action::ChangeNext
            | Action::ChangePrev
            | Action::ChangeFileNext
            | Action::ChangeFilePrev
            | Action::OpenThreadNext
            | Action::OpenThreadPrev
            | Action::GotoFile
    )
}

fn unavailable_while_diff_is_off(action: Action) -> bool {
    matches!(
        action,
        Action::ComparisonWhitespace
            | Action::FilesChanged
            | Action::ChangeNext
            | Action::ChangePrev
            | Action::ChangeFileNext
            | Action::ChangeFilePrev
    )
}

pub(in crate::app) fn available_while_panes_do_not_fit(action: Action) -> bool {
    matches!(
        action,
        Action::Escape
            | Action::ConfirmQuit
            | Action::SidebarToggle
            | Action::TreeToggle
            | Action::ThreadsPaneToggle
            | Action::MenuBarToggle
    )
}

pub(in crate::app) fn unavailable_for_directory(action: Action) -> bool {
    matches!(
        action,
        Action::NewThread
            | Action::FileComment
            | Action::Reply
            | Action::ToggleResolved
            | Action::ToggleAutoResolve
            | Action::EditNewestOwn
            | Action::DeleteThread
            | Action::ArchiveThread
            | Action::RestoreThread
            | Action::SourceView
    )
}

impl App {
    /// Run `action` as the focused surface means it, recording the
    /// position a far move leaves. A search moves the cursor as it is
    /// typed, so its origin is kept from `/` to Enter.
    pub(crate) fn act(&mut self, action: Action) -> Effect {
        if action == Action::ApplicationMenu {
            self.open_app_menu();
            return Effect::None;
        }
        if !self.panes_fit() && !available_while_panes_do_not_fit(action) {
            return Effect::None;
        }
        if self.getting_started() && action != Action::Escape {
            self.close_getting_started();
        }
        let input = place(self) == Some(Where::Input);
        let from = match action {
            Action::SearchForward | Action::SearchBackward => {
                self.search_origin = self.jump_origin();
                None
            }
            Action::Confirm if input => self.search_origin.take().or_else(|| self.jump_origin()),
            Action::Escape if input => {
                self.search_origin = None;
                None
            }
            _ => self.jump_origin(),
        };
        let effect = self.act_placed(action);
        if is_far_move(action)
            && let Some(from) = from
            && self.jump_origin().as_ref() != Some(&from)
        {
            self.record_jump(from);
        }
        effect
    }

    fn accepts_action(&mut self, place: Where, action: Action) -> bool {
        if matches!(
            place,
            Where::View | Where::Tree | Where::Review | Where::ThreadsPane
        ) && !self.pane_has_navigation(self.focus())
            && !available_while_panes_do_not_fit(action)
        {
            return false;
        }
        if self.diff_mode() == DiffMode::Off && unavailable_while_diff_is_off(action) {
            self.notice("diff mode is off");
            return false;
        }
        if matches!(place, Where::View | Where::Tree)
            && self.directory_path().is_some()
            && unavailable_for_directory(action)
        {
            self.notice("select a file to use file or thread actions");
            return false;
        }
        true
    }

    /// The Space menu and the command line mean the same thing
    /// everywhere; the rest is looked up per surface.
    #[expect(
        clippy::too_many_lines,
        reason = "the exhaustive action dispatch is intentionally centralized"
    )]
    fn act_placed(&mut self, action: Action) -> Effect {
        let Some(place) = place(self) else {
            return Effect::None;
        };
        if !self.accepts_action(place, action) {
            return Effect::None;
        }
        match action {
            Action::ApplicationMenu => unreachable!("handled before placement"),
            Action::TreeToggle => self.toggle_tree_shown(),
            Action::PickFile => self.open_picker(PickerKind::Files),
            Action::PickAnyFile => self.open_picker(PickerKind::AllFiles),
            Action::PickRecent => self.open_picker(PickerKind::Recent),
            Action::FileView => self.open_file_view(),
            Action::Review => self.open_review(),
            Action::ReviewRecentlyResolved => self.open_review_view(ReviewView::RecentlyResolved),
            Action::ReviewArchived => self.open_review_view(ReviewView::Archived),
            Action::ArchiveResolved => self.archive_resolved_threads(),
            Action::ClearBoard => self.request_clear_board(),
            Action::ArchiveThread => {
                if let Some(id) = self.thread_cursor().thread().cloned() {
                    self.archive_thread(&id);
                } else {
                    self.notice("no thread here");
                }
            }
            Action::RestoreThread => {
                if let Some(id) = self.thread_cursor().thread().cloned() {
                    self.restore_thread(&id);
                } else {
                    self.notice("no thread here");
                }
            }
            Action::SidebarToggle => self.toggle_sidebar(),
            Action::MenuBarToggle => self.toggle_menu_bar(),
            Action::ThreadsPaneToggle => self.toggle_threads_pane_shown(),
            Action::WindowNext => self.window_next(),
            Action::WindowPrev => self.window_previous(),
            Action::WindowFiles => self.window_files(),
            Action::WindowThreads => self.window_threads(),
            Action::Help => self.open_help(),
            Action::JumpBack => self.jump_back(),
            Action::JumpForward => self.jump_forward(),
            Action::ChangeNext => self.hunk_next(),
            Action::ChangePrev => self.hunk_prev(),
            Action::ChangeFileNext => self.changed_file_next(),
            Action::ChangeFilePrev => self.changed_file_prev(),
            Action::OpenThreadNext => self.thread_step_across(1),
            Action::OpenThreadPrev => self.thread_step_across(-1),
            Action::ConfirmQuit => self.request_quit(),
            // The threads, view, and diff submenus (ADR 0049, ADR 0060)
            // mean the same thing everywhere.
            Action::NewThread => self.start_new_comment(),
            Action::FileComment => self.start_file_comment(),
            Action::Reply => self.thread_reply(),
            Action::ToggleResolved => self.thread_toggle_resolved(),
            Action::ToggleAutoResolve => self.thread_toggle_auto_resolve(),
            Action::EditNewestOwn => self.thread_edit_newest_own(),
            Action::StubResolvedToggle => self.toggle_resolved_stubs(),
            Action::DeleteThread => self.thread_delete_here(),
            Action::SourceView => self.toggle_source_view(),
            Action::DiffNormal => self.select_diff_mode(DiffMode::Normal),
            Action::DiffUnified => self.select_diff_mode(DiffMode::Unified),
            Action::DiffOff => self.select_diff_mode(DiffMode::Off),
            Action::ComparisonSave => self.request_review_point(),
            Action::ComparisonManage => self.request_review_point_manage(),
            Action::ReviewFocusStart => self.start_review_focus(),
            Action::ReviewFocusClear => self.clear_review_focus(),
            Action::ComparisonBase => self.pick_diff_side(false),
            Action::ComparisonTarget => self.pick_diff_side(true),
            Action::ComparisonHeadWorkingTree => self.select_head_working_tree(),
            Action::ComparisonHeadParent => self.select_head_parent(),
            Action::ComparisonCommitParent => self.pick_commit_parent(),
            Action::ComparisonWhitespace => self.toggle_whitespace(),
            Action::WorktreeNext => self.worktree_step(1),
            Action::WorktreePrev => self.worktree_step(-1),
            Action::StubsToggle => self.toggle_stubs(),
            Action::FilesChanged => self.files_toggle(Rule::Changed),
            Action::FilesReviews => self.files_toggle(Rule::Reviews),
            Action::FilesUntracked => self.files_toggle(Rule::Untracked),
            Action::FilesIgnored => self.files_toggle(Rule::Ignored),
            Action::FilesFoldAll => self.toggle_all_directories(),
            Action::ThreadsFoldAll => self.threads_pane_fold_all(),
            Action::PaneScope => self.threads_pane_toggle_scope(),
            Action::ReviewResolved => self.review_toggle_resolved(),
            Action::CommandLine => {
                if place == Where::Tree {
                    self.toggle_tree_focus();
                }
                self.view_mut().start_command();
            }
            _ => {
                return match place {
                    Where::View => self.act_view(action),
                    Where::Tree => self.act_tree(action),
                    Where::ThreadsPane => self.act_threads_pane(action),
                    Where::Review => self.act_list(action),
                    Where::Draft => self.act_box(action),
                    Where::Picker => self.act_picker(action),
                    Where::Input => self.act_input(action),
                    Where::Any => Effect::None,
                };
            }
        }
        Effect::None
    }

    fn act_view(&mut self, action: Action) -> Effect {
        if self.directory_path().is_some() {
            if action != Action::Escape {
                self.notice("select a file to use file or thread actions");
            }
            return Effect::None;
        }
        if self.getting_started() {
            if action == Action::Escape {
                self.close_getting_started();
            }
            return Effect::None;
        }
        match action {
            // `Esc` clears, then leaves the diff (ADR 0060).
            Action::Escape => self.escape_view(),
            // Enter folds or unfolds only when the cursor rests on a
            // thread's header or folded stub (ADR 0065).
            Action::Confirm => self.toggle_thread_header(),
            Action::GotoFile => return self.goto_file(),
            // `c` comments; `z` owns thread expansion and folding (ADR 0065).
            Action::Comment => self.start_comment(),
            // `z` folds or expands the thread here, `Z` the whole file
            // (ADR 0065).
            Action::Fold => self.toggle_thread_here(),
            Action::FoldAll => self.toggle_expand_all(),
            // On an expanded thread's rows the text keeps the thread keys
            // (ADR 0049); elsewhere they act on the thread at the cursor.
            Action::EditMessage => self.thread_edit_message(),
            Action::Delete => self.delete_armed_thread(),
            _ => {
                let view = self.view_mut();
                match action {
                    Action::MoveDown => view.move_down(1),
                    Action::MoveUp => view.move_up(1),
                    Action::MoveLeft => view.move_left(),
                    Action::MoveRight => view.move_right(),
                    Action::LineStart => view.line_start(),
                    Action::LineEnd => view.line_end(),
                    Action::GotoLineStart => view.goto_line_start(),
                    Action::GotoLineEnd => view.goto_line_end(),
                    Action::Top => view.goto_top(),
                    Action::Bottom => view.goto_bottom(),
                    Action::HalfPageDown => view.half_page_down(),
                    Action::HalfPageUp => view.half_page_up(),
                    Action::SearchForward => view.start_search(false),
                    Action::SearchBackward => view.start_search(true),
                    Action::SearchNext => view.search_next(false),
                    Action::SearchPrev => view.search_next(true),
                    Action::SelectChars => view.select_chars(),
                    Action::SelectLines => view.select_lines(),
                    Action::ExtendLine => view.extend_line_below(),
                    Action::Yank => return view.yank(),
                    _ => {}
                }
            }
        }
        Effect::None
    }

    fn act_tree(&mut self, action: Action) -> Effect {
        let before = tree_highlight(self);
        if matches!(
            action,
            Action::MoveDown
                | Action::MoveUp
                | Action::MoveLeft
                | Action::MoveRight
                | Action::Confirm
                | Action::Fold
                | Action::FoldAll
                | Action::Top
                | Action::Bottom
        ) {
            self.cancel_tree_target();
        }
        match action {
            Action::MoveDown => self.with_tree(|tree, _| {
                tree.move_down(1);
                None
            }),
            Action::MoveUp => self.with_tree(|tree, _| {
                tree.move_up(1);
                None
            }),
            Action::MoveLeft => self.with_tree(|tree, _| {
                tree.collapse();
                None
            }),
            Action::MoveRight => {
                self.with_tree_result(Tree::expand);
            }
            Action::Confirm => self.with_tree_result(Tree::activate),
            Action::Fold => self.with_tree_result(Tree::toggle_nearest_directory),
            Action::FoldAll => {
                self.toggle_all_directories();
            }
            Action::CopyPath => return self.copy_tree_path(),
            Action::CopyFullPath => return self.copy_tree_full_path(),
            Action::Top => self.with_tree(|tree, _| {
                tree.goto_top();
                None
            }),
            Action::Bottom => self.with_tree(|tree, _| {
                tree.goto_bottom();
                None
            }),
            Action::Escape => self.toggle_tree_focus(),
            _ => {}
        }
        // The highlight is what the main pane shows (ADR 0023): a key that
        // moved it onto a file shows that file. Comparing paths keeps `R`,
        // `I`, and `Esc` from re-showing a highlight the reader has left.
        if tree_highlight(self) != before {
            self.show_highlight();
        }
        Effect::None
    }

    /// Keys in the threads pane (ADR 0027, `d` per ADR 0034, scope and
    /// resolved toggles per ADR 0049, folds per ADR 0066).
    fn act_threads_pane(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.leave_threads_pane(),
            Action::MoveDown => self.threads_pane_move(1),
            Action::MoveUp => self.threads_pane_move(-1),
            Action::Confirm => self.threads_pane_open(),
            Action::PaneScope => self.threads_pane_toggle_scope(),
            Action::ReviewResolved => self.review_toggle_resolved(),
            Action::Fold => self.threads_pane_fold(),
            Action::FoldAll => self.threads_pane_fold_all(),
            _ => return self.act_on_cursor(action),
        }
        Effect::None
    }

    /// Keys in the review list (ADR 0025, ADR 0049, folds per ADR 0066
    /// and ADR 0076).
    fn act_list(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => {}
            Action::MoveDown => self.review_message_step(1),
            Action::MoveUp => self.review_message_step(-1),
            Action::ThreadPrev => self.review_step(-1),
            Action::ThreadNext => self.review_step(1),
            Action::HalfPageDown => self.review_page(1),
            Action::HalfPageUp => self.review_page(-1),
            Action::Top => self.review_goto(false),
            Action::Bottom => self.review_goto(true),
            Action::Confirm => self.thread_open_in_file(),
            Action::Fold => self.review_fold(),
            Action::FoldAll => self.review_fold_all(),
            Action::ReviewResolved => self.review_toggle_resolved(),
            Action::FileOnly => self.review_toggle_file(),
            _ => return self.act_on_cursor(action),
        }
        Effect::None
    }

    /// The keys the thread surfaces keep for themselves: `e` on the
    /// highlighted message and the second `d` (ADR 0046); reply and
    /// resolve are the same from everywhere.
    fn act_on_cursor(&mut self, action: Action) -> Effect {
        match action {
            Action::EditMessage => self.thread_edit_message(),
            Action::Delete => self.delete_armed_thread(),
            _ => {}
        }
        Effect::None
    }

    /// Keys in the draft (ADR 0018): readline-style motion and
    /// kills; the keys that scroll the thread above a reply.
    fn act_box(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.compose_cancel(),
            Action::Confirm => self.compose_submit(),
            Action::SubmitAutoResolve => self.compose_submit_auto_resolve(),
            Action::Newline => self.compose_edit(Edit::Newline),
            Action::EditDraft => return Effect::EditDraft,
            Action::ScrollUp => self.compose_scroll(-WHEEL_LINES),
            Action::ScrollDown => self.compose_scroll(WHEEL_LINES),
            Action::Backspace => self.compose_edit(Edit::DeleteBack),
            Action::DeleteForward => self.compose_edit(Edit::DeleteForward),
            Action::MoveLeft => self.compose_edit(Edit::Move(Motion::Left)),
            Action::MoveRight => self.compose_edit(Edit::Move(Motion::Right)),
            Action::MoveUp => self.compose_edit(Edit::Move(Motion::Up)),
            Action::MoveDown => self.compose_edit(Edit::Move(Motion::Down)),
            Action::LineStart => self.compose_edit(Edit::Move(Motion::LineStart)),
            Action::LineEnd => self.compose_edit(Edit::Move(Motion::LineEnd)),
            Action::WordBack => self.compose_edit(Edit::Move(Motion::WordBack)),
            Action::WordForward => self.compose_edit(Edit::Move(Motion::WordForward)),
            Action::DeleteWordBack => self.compose_edit(Edit::DeleteWordBack),
            Action::DeleteToLineStart => self.compose_edit(Edit::DeleteToLineStart),
            Action::DeleteToLineEnd => self.compose_edit(Edit::DeleteToLineEnd),
            Action::ClearDraft => self.compose_clear(),
            _ => {}
        }
        Effect::None
    }

    fn act_picker(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.picker_escape(),
            Action::Confirm => self.picker_confirm(),
            Action::Backspace => self.picker_backspace(),
            Action::MoveDown => self.picker_move(1),
            Action::MoveUp => self.picker_move(-1),
            _ => {}
        }
        Effect::None
    }

    fn act_input(&mut self, action: Action) -> Effect {
        if action == Action::Confirm
            && self.directory_path().is_some()
            && self
                .view()
                .input()
                .trim()
                .chars()
                .all(|c| c.is_ascii_digit())
            && !self.view().input().trim().is_empty()
        {
            self.view_mut().escape();
            self.notice("select a file to use file or thread actions");
            return Effect::None;
        }
        let view = self.view_mut();
        match action {
            Action::Escape => {
                view.escape();
            }
            Action::Confirm => return view.confirm(),
            Action::CompleteNext => view.complete_command(CompletionDirection::Next),
            Action::CompletePrevious => view.complete_command(CompletionDirection::Previous),
            Action::Backspace => view.input_backspace(),
            _ => {}
        }
        Effect::None
    }
}

fn confirmation_key(app: &mut App, key: KeyEvent) -> Effect {
    match key.code {
        crossterm::event::KeyCode::Char('y') if app.review_point_delete_confirmation_armed() => {
            app.confirm_review_point_delete();
        }

        crossterm::event::KeyCode::Enter => match app.popup() {
            Some(Popup::ConfirmQuit) => {
                app.close_popup();
                return Effect::Quit;
            }
            Some(Popup::ConfirmBoard { .. }) => app.confirm_clear_board(),
            _ => {}
        },
        crossterm::event::KeyCode::Esc => match app.popup() {
            Some(Popup::ConfirmQuit) => app.close_popup(),
            Some(Popup::ConfirmBoard { .. }) => app.cancel_clear_board(),
            Some(Popup::ConfirmReviewPointDelete { .. }) => {
                app.cancel_review_point_delete();
            }
            _ => {}
        },
        _ => {}
    }
    Effect::None
}

fn review_point_action_key(app: &mut App, key: KeyEvent) -> Effect {
    if !key.modifiers.is_empty() {
        return Effect::None;
    }
    match key.code {
        crossterm::event::KeyCode::Char('r') => app.review_point_action_rename(),
        crossterm::event::KeyCode::Char('d') => {
            app.request_review_point_delete_confirmation();
        }
        crossterm::event::KeyCode::Esc => app.cancel_review_point_action(),
        _ => {}
    }
    Effect::None
}

fn review_point_rename_key(app: &mut App, key: KeyEvent) -> Effect {
    use crossterm::event::{KeyCode, KeyModifiers};

    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => app.submit_review_point_rename(),
        (KeyCode::Esc, KeyModifiers::NONE) => app.cancel_review_point_rename(),
        (KeyCode::Backspace, KeyModifiers::NONE) => {
            app.review_point_rename_edit(Edit::DeleteBack);
        }
        (KeyCode::Delete, KeyModifiers::NONE) => {
            app.review_point_rename_edit(Edit::DeleteForward);
        }
        (KeyCode::Left, KeyModifiers::NONE) => {
            app.review_point_rename_motion(Motion::Left);
        }
        (KeyCode::Right, KeyModifiers::NONE) => {
            app.review_point_rename_motion(Motion::Right);
        }
        (KeyCode::Home, KeyModifiers::NONE) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
            app.review_point_rename_motion(Motion::LineStart);
        }
        (KeyCode::End, KeyModifiers::NONE) => {
            app.review_point_rename_motion(Motion::LineEnd);
        }
        (KeyCode::Char(character), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
            app.review_point_rename_insert(character);
        }
        _ => {}
    }
    Effect::None
}

#[cfg(test)]
mod tests;
