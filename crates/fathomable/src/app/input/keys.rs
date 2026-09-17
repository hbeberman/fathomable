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
use fathomable_core::editor::{Edit, Motion};
use fathomable_core::tree::{Rule, Tree};

use super::super::{App, Focus, PickerKind, Popup};
use super::bindings::{Action, Chord, Key, Match, Where, lookup};
use super::help;
use crate::app::threads::list::ReviewView;
use crate::app::view::{Effect, Mode};
use crate::app::{doctor_view, menu_bar};

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
            | Popup::About
            | Popup::Menu(_)
            | Popup::ConfirmBoard { .. },
        ) => None,
        Some(Popup::Compose(_)) => Some(Where::Draft),
        Some(Popup::Picker(_)) => Some(Where::Picker),
        None => Some(match app.focus() {
            Focus::View if matches!(app.view().mode(), Mode::Command | Mode::Search { .. }) => {
                Where::Input
            }
            Focus::View => Where::View,
            Focus::Tree => Where::Tree,
            Focus::Review => Where::Review,
            Focus::ThreadsPane => Where::ThreadsPane,
        }),
    }
}

fn key_event(app: &mut App, key: KeyEvent) -> Effect {
    if app.title_menu_open() {
        return menu_bar::key(app, key);
    }
    if matches!(app.popup(), Some(Popup::ConfirmBoard { .. })) {
        return board_confirmation_key(app, key);
    }
    if matches!(app.popup(), Some(Popup::Doctor(_))) {
        return doctor_view::key(app, key);
    }
    if matches!(app.popup(), Some(Popup::About)) {
        if key.code == crossterm::event::KeyCode::Esc {
            app.close_popup();
        }
        return Effect::None;
    }
    if matches!(app.popup(), Some(Popup::Help(_))) {
        return help::key(app, key);
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
};

/// A single unbound key: text entry takes characters, everything else
/// drops it.
fn fallback(app: &mut App, place: Where, chord: Chord) -> Effect {
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
            | Action::ThreadNextAcross
            | Action::ThreadPrevAcross
            | Action::HunkNext
            | Action::HunkPrev
            | Action::DirtyNext
            | Action::DirtyPrev
            | Action::ChangeNext
            | Action::ChangePrev
            | Action::GotoFile
    )
}

impl App {
    /// Run `action` as the focused surface means it, recording the
    /// position a far move leaves. A search moves the cursor as it is
    /// typed, so its origin is kept from `/` to Enter.
    pub(crate) fn act(&mut self, action: Action) -> Effect {
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

    /// The Space menu and the command line mean the same thing
    /// everywhere; the rest is looked up per surface.
    fn act_placed(&mut self, action: Action) -> Effect {
        let Some(place) = place(self) else {
            return Effect::None;
        };
        match action {
            Action::TreeToggle => self.toggle_tree_shown(),
            Action::PickFile => self.open_picker(PickerKind::Files),
            Action::PickAnyFile => self.open_picker(PickerKind::AllFiles),
            Action::PickRecent => self.open_picker(PickerKind::Recent),
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
            Action::WindowLeft => self.window_left(),
            Action::WindowDown => self.window_down(),
            Action::WindowUp => self.window_up(),
            Action::WindowRight => self.window_right(),
            Action::WindowNext => self.window_next(),
            Action::WindowFiles => self.window_files(),
            Action::WindowThreads => self.window_threads(),
            Action::JumpNewest => self.jump_newest(),
            Action::Wake => self.wake(),
            Action::Help => self.open_help(),
            Action::JumpBack => self.jump_back(),
            Action::JumpForward => self.jump_forward(),
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
            Action::SourceView => {
                if self.comparison_diff {
                    self.leave_diff();
                }
                self.view_mut().toggle_source_view();
            }
            Action::ComparisonControl => self.open_comparison_control(),
            Action::ComparisonSave => self.request_review_point(),
            Action::ComparisonBase => self.pick_diff_side(false),
            Action::ComparisonTarget => self.pick_diff_side(true),
            Action::ComparisonWhitespace => self.toggle_whitespace(),
            Action::WorktreeNext => self.worktree_step(1),
            Action::WorktreePrev => self.worktree_step(-1),
            Action::StubsToggle => self.toggle_stubs(),
            Action::FilesChanged => self.files_toggle(Rule::Changed),
            Action::FilesUntracked => self.files_toggle(Rule::Untracked),
            Action::FilesIgnored => self.files_toggle(Rule::Ignored),
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
            Action::ThreadNext => self.thread_step_in_file(1),
            Action::ThreadPrev => self.thread_step_in_file(-1),
            Action::ThreadNextAcross => self.thread_step_across(1),
            Action::ThreadPrevAcross => self.thread_step_across(-1),
            Action::HunkNext => self.hunk_next(),
            Action::HunkPrev => self.hunk_prev(),
            Action::DirtyNext => self.dirty_next(),
            Action::DirtyPrev => self.dirty_prev(),
            Action::ChangeNext => self.jump_next(),
            Action::ChangePrev => self.jump_prev(),
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
            Action::Escape => self.close_review(),
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
        let view = self.view_mut();
        match action {
            Action::Escape => {
                view.escape();
            }
            Action::Confirm => return view.confirm(),
            Action::Backspace => view.input_backspace(),
            _ => {}
        }
        Effect::None
    }
}

fn board_confirmation_key(app: &mut App, key: KeyEvent) -> Effect {
    match key.code {
        crossterm::event::KeyCode::Enter | crossterm::event::KeyCode::Char('y') => {
            app.confirm_clear_board();
        }
        crossterm::event::KeyCode::Esc | crossterm::event::KeyCode::Char('n') => {
            app.cancel_clear_board();
        }
        _ => {}
    }
    Effect::None
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fathomable_core::annotations::{AutoResolve, MessageTarget};
    use fathomable_testing::TempDir;

    use crate::app::testing::{self, press, source_app};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::handle_key;
    use crate::app::threads::{ComposeTarget, ThreadState};
    use crate::app::{App, Focus, Popup};

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = testing::workspace(&format!("keys-{name}"), testing::README)?;
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n")?;
        Ok(dir)
    }

    fn annotate(app: &mut App, line: usize, text: &str) {
        app.view_mut().goto_source_line(line);
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    fn alt(app: &mut App, code: KeyCode) {
        handle_key(app, KeyEvent::new(code, KeyModifiers::ALT));
    }

    fn here(app: &App) -> (String, Option<usize>) {
        (
            app.current_path().to_string_lossy().into_owned(),
            app.view().cursor_source_line(),
        )
    }

    fn compose_target(app: &App) -> Option<ComposeTarget> {
        match app.popup() {
            Some(Popup::Compose(compose)) => Some(compose.target().clone()),
            _ => None,
        }
    }

    fn screen(app: &App) -> anyhow::Result<String> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        Ok(text)
    }

    /// Thread actions follow the Phase C key grammar.
    #[test]
    fn space_c_acts_on_the_thread_here_from_any_pane() -> anyhow::Result<()> {
        let dir = fixture("threads")?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, "three");
        annotate(&mut app, 5, "five");
        app.view_mut().goto_source_line(3);
        let id = app.marks()[0].id().clone();

        press(&mut app, " cr");
        assert_eq!(compose_target(&app), Some(ComposeTarget::Reply(id.clone())));
        app.compose_insert("answer");
        app.compose_submit();
        assert_eq!(app.thread(&id).map(|t| t.replies().len()), Some(1));
        // A reply expands the thread in place (ADR 0049); focus stays in
        // the text.
        assert_eq!(app.focus(), Focus::View);

        press(&mut app, " ce");
        assert_eq!(
            compose_target(&app),
            Some(ComposeTarget::Edit {
                thread: id.clone(),
                message: MessageTarget::Reply(0),
            }),
            "the newest own message is the reply"
        );
        let draft = match app.popup() {
            Some(Popup::Compose(compose)) => compose.buffer().text().to_owned(),
            _ => String::new(),
        };
        assert_eq!(draft, "answer", "the draft is seeded with the reply");
        app.compose_cancel();

        // Literal thread keys do not steal input or act from the tree.
        app.show_tree();
        assert_eq!(app.focus(), Focus::Tree);
        press(&mut app, "rR");
        assert_eq!(app.marks()[0].kind(), ThreadState::Active);

        // In the text, `R` toggles permission and `r` resolves/reopens.
        app.toggle_tree_focus();
        press(&mut app, "R");
        assert_eq!(
            app.thread(&id)
                .map(fathomable_core::annotations::Thread::auto_resolve),
            Some(AutoResolve::Enabled)
        );
        press(&mut app, "r");
        assert_eq!(app.marks()[0].kind(), ThreadState::Resolved);
        press(&mut app, "R");
        assert_eq!(
            app.thread(&id)
                .map(fathomable_core::annotations::Thread::auto_resolve),
            Some(AutoResolve::Disabled),
            "R is unavailable on a resolved thread"
        );
        press(&mut app, "r");
        assert_eq!(app.marks()[0].kind(), ThreadState::Active);
        press(&mut app, " cd");
        assert_eq!(app.marks().len(), 1);
        assert_eq!(app.marks()[0].range().map(|r| r.start()), Some(5));
        assert_eq!(app.focus(), Focus::View);

        // `Space c c` starts a new thread on the cursor line.
        app.view_mut().goto_source_line(4);
        press(&mut app, " cc");
        assert!(matches!(
            compose_target(&app),
            Some(ComposeTarget::New(range)) if range.start() == 4
        ));
        Ok(())
    }

    #[test]
    fn bare_t_opens_reviews_from_every_normal_pane() -> anyhow::Result<()> {
        let dir = fixture("reviews-open")?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, "three");

        press(&mut app, "t");
        assert!(app.review_list().is_open());
        app.focus_threads_pane();
        press(&mut app, "t");
        assert!(app.review_list().is_open());
        assert_eq!(app.focus(), Focus::Review);

        app.window_files();
        press(&mut app, "t");
        assert!(app.review_list().is_open());
        press(&mut app, "t");
        assert!(app.review_list().is_open());
        assert_eq!(app.focus(), Focus::Review);

        app.start_new_comment();
        press(&mut app, "tRr");
        assert_eq!(app.compose_draft(), Some("tRr"));
        Ok(())
    }

    /// `Space w h` shows a hidden files pane with its highlight on the
    /// current file and takes the keys there; `Space w j` and `Space w k`
    /// step between the panes; `Space w l` returns; `Space w w` cycles,
    /// skipping a hidden pane; a move with nowhere to go does nothing;
    /// `Space v s` toggles the view from the files pane (ADR 0056); and
    /// `Space w f` and `Space w t` name a pane, showing it first when it
    /// is hidden (ADR 0057).
    #[test]
    fn space_w_moves_between_the_panes() -> anyhow::Result<()> {
        let dir = fixture("sidebar")?;
        let mut app = source_app(&dir)?;
        app.open(Path::new("docs/guide.md"));
        assert!(app.tree().is_none());
        press(&mut app, " wl");
        assert_eq!(app.focus(), Focus::View, "nowhere to go");
        press(&mut app, " wh");
        assert_eq!(app.focus(), Focus::Tree);
        let highlighted = app
            .tree()
            .and_then(|tree| tree.current())
            .map(|row| row.path().to_path_buf());
        assert_eq!(highlighted.as_deref(), Some(Path::new("docs/guide.md")));

        let before = app.view().source_view();
        press(&mut app, " vs");
        assert_ne!(app.view().source_view(), before);

        press(&mut app, " wj");
        assert_eq!(app.focus(), Focus::Tree, "the threads pane is hidden");
        app.show_threads_pane();
        press(&mut app, " wj");
        assert_eq!(app.focus(), Focus::ThreadsPane);
        press(&mut app, " wk");
        assert_eq!(app.focus(), Focus::Tree);
        press(&mut app, " wl");
        assert_eq!(app.focus(), Focus::View);

        press(&mut app, " ww");
        assert_eq!(app.focus(), Focus::Tree);
        press(&mut app, " ww");
        assert_eq!(app.focus(), Focus::ThreadsPane);
        press(&mut app, " ww");
        assert_eq!(app.focus(), Focus::View);
        press(&mut app, " pf");
        assert!(!app.sidebar.tree);
        press(&mut app, " ww");
        assert_eq!(app.focus(), Focus::ThreadsPane, "a hidden pane is skipped");
        press(&mut app, " wl");
        assert_eq!(app.focus(), Focus::View);

        press(&mut app, " wf");
        assert!(app.sidebar.tree, "Space w f shows the hidden files pane");
        assert_eq!(app.focus(), Focus::Tree);
        press(&mut app, " pt");
        assert!(!app.sidebar.threads);
        press(&mut app, " wt");
        assert!(
            app.sidebar.threads,
            "Space w t shows the hidden threads pane"
        );
        assert_eq!(app.focus(), Focus::ThreadsPane);
        press(&mut app, " wt");
        assert_eq!(app.focus(), Focus::ThreadsPane, "already there");
        press(&mut app, " wf");
        assert_eq!(app.focus(), Focus::Tree, "from the threads pane");
        Ok(())
    }

    #[test]
    fn unmatched_second_space_cancels_only_the_prefix() -> anyhow::Result<()> {
        let dir = fixture("cancel-space")?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        app.show_threads_pane();
        app.view_mut().goto_source_line(3);
        app.view_mut().select_chars();
        let cursor = app.view().cursor();
        let selection = app.view().selection();
        for focus in [Focus::View, Focus::Tree, Focus::ThreadsPane, Focus::Review] {
            app.focus = focus;
            press(&mut app, " ");
            assert!(!app.prefix().is_empty());
            press(&mut app, " ");
            assert!(app.prefix().is_empty());
            assert_eq!(app.focus(), focus);
            assert_eq!(app.view().cursor(), cursor);
            assert_eq!(app.view().selection(), selection);
        }
        app.focus = Focus::View;
        app.view_mut().clear_selection();
        app.view_mut().set_bases(None, Some("before\n".to_owned()));
        app.toggle_head_diff();
        press(&mut app, "  ");
        assert!(app.view().diff_view(), "cancel does not act as Escape");
        assert!(app.prefix().is_empty());
        Ok(())
    }

    /// Far moves leave positions behind: `]C` across files, `gg`, and a
    /// search jump; `Alt-Left` walks back through them and `Alt-Right`
    /// forward, a new far move dropping the forward part; `j` and the
    /// tree's paging leave nothing (ADR 0049).
    #[test]
    fn alt_left_and_right_walk_the_positions_far_moves_left() -> anyhow::Result<()> {
        let dir = fixture("jumplist")?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, "readme");
        app.open(Path::new("docs/guide.md"));
        annotate(&mut app, 1, "guide");
        app.open(Path::new("README.md"));
        app.view_mut().goto_source_line(5);

        press(&mut app, "]C");
        assert_eq!(here(&app), ("docs/guide.md".to_owned(), Some(1)));
        alt(&mut app, KeyCode::Left);
        assert_eq!(here(&app), ("README.md".to_owned(), Some(5)));
        alt(&mut app, KeyCode::Right);
        assert_eq!(here(&app), ("docs/guide.md".to_owned(), Some(1)));
        alt(&mut app, KeyCode::Right);
        assert_eq!(app.message(), Some("at newest position"));

        // Back, then a new far move: the forward part is gone.
        alt(&mut app, KeyCode::Left);
        assert_eq!(here(&app), ("README.md".to_owned(), Some(5)));
        press(&mut app, "jj");
        press(&mut app, "gg");
        assert_eq!(here(&app), ("README.md".to_owned(), Some(1)));
        alt(&mut app, KeyCode::Right);
        assert_eq!(app.message(), Some("at newest position"));
        alt(&mut app, KeyCode::Left);
        assert_eq!(
            here(&app),
            ("README.md".to_owned(), Some(7)),
            "gg left line 7"
        );

        // A search jump records where it left; `j` records nothing, so
        // back from the line below the match returns to the search's
        // origin and forward to where back started.
        press(&mut app, "/beta");
        handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(here(&app), ("README.md".to_owned(), Some(4)));
        press(&mut app, "j");
        alt(&mut app, KeyCode::Left);
        assert_eq!(here(&app), ("README.md".to_owned(), Some(7)));
        alt(&mut app, KeyCode::Right);
        assert_eq!(here(&app), ("README.md".to_owned(), Some(5)));
        Ok(())
    }

    /// The which-key box leads with the prefix and the submenu's word,
    /// and re-renders at every level.
    #[test]
    fn the_menu_shows_a_breadcrumb_for_the_prefix() -> anyhow::Result<()> {
        let dir = fixture("menu")?;
        let mut app = source_app(&dir)?;
        press(&mut app, " ");
        let text = screen(&app)?;
        assert!(text.contains(" Space "), "the Space menu names its prefix");
        assert!(text.contains("threads…"), "the submenu entry is named");
        press(&mut app, "c");
        let text = screen(&app)?;
        assert!(text.contains(" Space c · threads "), "{text}");
        assert!(text.contains("new thread"), "{text}");
        press(&mut app, "r");
        assert!(matches!(app.popup(), Some(Popup::Compose(_))) || app.message().is_some());
        Ok(())
    }
}
