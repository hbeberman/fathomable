// @okf-doc: /decisions/0007-key-grammar-and-mouse.md
//! Turn a key press into an [`Action`] through the binding table, and
//! give each action its meaning on the surface that has focus.
//!
//! [`handle_key`] keeps the keys typed so far as the app's prefix; a
//! sequence the table calls a prefix waits, one it calls a miss is
//! dropped (and cancels an armed delete), and a match runs
//! [`App::act`]. Text entry — the comment box, the picker, the command
//! line — takes the characters the table leaves alone.

use crossterm::event::KeyEvent;
use fathomable_core::editor::{Edit, Motion};
use fathomable_core::tree::Tree;

use super::super::view::{Effect, Mode};
use super::super::{App, Focus, PickerKind, Popup};
use super::bindings::{Action, Chord, Key, Match, Where, lookup};

/// Rows a scroll key or wheel notch moves.
pub const WHEEL_LINES: isize = 3;

/// Apply a key press to `app`. Going elsewhere switches auto-jump off
/// (ADR 0031).
pub fn handle_key(app: &mut App, key: KeyEvent) -> Effect {
    app.with_navigation_watch(|app| key_event(app, key))
}

/// The surface a key lands on now, or `None` under a popup that any key
/// closes.
#[must_use]
pub fn place(app: &App) -> Option<Where> {
    match app.popup() {
        Some(Popup::Help | Popup::Status) => None,
        Some(Popup::Compose(_)) => Some(Where::Box),
        Some(Popup::Picker(_)) => Some(Where::Picker),
        None => Some(match app.focus() {
            Focus::View if matches!(app.view().mode(), Mode::Command | Mode::Search { .. }) => {
                Where::Input
            }
            Focus::View => Where::View,
            Focus::Sidebar => Where::Tree,
            Focus::Thread => Where::ThreadPane,
            Focus::Threads => Where::List,
            Focus::FileThreads => Where::FileThreads,
        }),
    }
}

fn key_event(app: &mut App, key: KeyEvent) -> Effect {
    app.clear_message();
    app.view_mut().clear_message();
    let Some(chord) = Chord::from_event(key) else {
        app.take_prefix();
        app.cancel_delete();
        return Effect::None;
    };
    let Some(place) = place(app) else {
        app.close_popup();
        return Effect::None;
    };
    if app.focus() == Focus::View {
        // Reader activity holds auto-jump back and delays "seen" (ADR 0015).
        app.view_mut().touch();
    }
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
        (Where::Box, Key::Char(ch)) if !chord.ctrl && !chord.alt => {
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

impl App {
    /// Run `action` as the focused surface means it. The Space menu and
    /// the command line mean the same thing everywhere; the rest is
    /// looked up per surface.
    pub fn act(&mut self, action: Action) -> Effect {
        let Some(place) = place(self) else {
            return Effect::None;
        };
        match action {
            Action::TreeToggleFocus => self.toggle_sidebar_focus(),
            Action::TreeHide => self.hide_sidebar(),
            Action::PickFile => self.open_picker(PickerKind::Files),
            Action::PickAnyFile => self.open_picker(PickerKind::AllFiles),
            Action::PickRecent => self.open_picker(PickerKind::Recent),
            Action::ThreadAtCursor => self.open_thread_at_cursor(),
            Action::ThreadList => self.open_thread_list(),
            Action::FileThreadsFocus => self.focus_file_threads(),
            Action::JumpNewest => self.jump_newest(),
            Action::AutoJumpToggle => self.toggle_auto_jump(),
            Action::ClearChanges => self.clear_queue(),
            Action::Wake => self.wake(),
            Action::Help => self.open_help(),
            Action::CommandLine => {
                if place == Where::Tree {
                    self.toggle_sidebar_focus();
                }
                self.view_mut().start_command();
            }
            _ => {
                return match place {
                    Where::View => self.act_view(action),
                    Where::Tree => self.act_tree(action),
                    Where::ThreadPane => self.act_thread_pane(action),
                    Where::FileThreads => self.act_file_threads(action),
                    Where::List => self.act_list(action),
                    Where::Box => self.act_box(action),
                    Where::Picker => self.act_picker(action),
                    Where::Input => self.act_input(action),
                    Where::Any => Effect::None,
                };
            }
        }
        Effect::None
    }

    fn act_view(&mut self, action: Action) -> Effect {
        match action {
            // At column 0, `h` steps back into the tree; a selection wraps
            // instead (see `View::move_left`).
            Action::MoveLeft
                if self.view().mode() == Mode::Normal
                    && self.tree().is_some()
                    && self.view().at_line_start() =>
            {
                self.toggle_sidebar_focus();
                return Effect::None;
            }
            // `c` opens the thread on the cursor row, else annotates the
            // selection or the cursor line; `C` always annotates (ADR 0027).
            Action::Comment => self.start_comment(),
            Action::NewThread => self.start_new_comment(),
            Action::ThreadNext => self.thread_step_in_file(1),
            Action::ThreadPrev => self.thread_step_in_file(-1),
            Action::ThreadNextAcross => self.thread_step_across(1),
            Action::ThreadPrevAcross => self.thread_step_across(-1),
            Action::WaitingNext => self.waiting_next(),
            Action::WaitingPrev => self.waiting_prev(),
            Action::HunkNext => self.hunk_next(),
            Action::HunkPrev => self.hunk_prev(),
            Action::DirtyNext => self.dirty_next(),
            Action::DirtyPrev => self.dirty_prev(),
            Action::ChangeNext => self.jump_next(),
            Action::ChangePrev => self.jump_prev(),
            Action::HistoryBack => self.history_back(),
            Action::HistoryForward => self.history_forward(),
            _ => {
                let view = self.view_mut();
                match action {
                    Action::MoveDown => view.move_down(1),
                    Action::MoveUp => view.move_up(1),
                    Action::MoveLeft => view.move_left(),
                    Action::MoveRight => view.move_right(),
                    Action::LineStart => view.line_start(),
                    Action::LineEnd => view.line_end(),
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
                    Action::SourceView => view.toggle_source_view(),
                    Action::DiffHead => view.toggle_diff_view(),
                    Action::DiffSeen => view.toggle_seen_diff_view(),
                    Action::Escape => view.escape(),
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
            Action::Top => self.with_tree(|tree, _| {
                tree.goto_top();
                None
            }),
            Action::Bottom => self.with_tree(|tree, _| {
                tree.goto_bottom();
                None
            }),
            Action::TreeRefresh => self.refresh_tree(),
            Action::TreeIgnored => self.toggle_ignored(),
            Action::Escape => self.toggle_sidebar_focus(),
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

    /// Keys while the thread pane has focus (ADR 0013); the thread and
    /// message keys move the one cursor (ADR 0046).
    fn act_thread_pane(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.close_thread(),
            Action::MoveDown => self.message_step(1),
            Action::MoveUp => self.message_step(-1),
            // `h` on the file's first thread hops to the file-threads pane,
            // as `h` at column 0 hops to the tree.
            Action::ThreadPrev if self.cursor_on_first_in_file() => {
                self.thread_to_file_threads();
            }
            Action::ThreadPrev => self.thread_step_in_file(-1),
            Action::ThreadNext => self.thread_step_in_file(1),
            Action::ThreadPrevAcross => self.thread_step_across(-1),
            Action::ThreadNextAcross => self.thread_step_across(1),
            Action::Top => self.message_first(),
            Action::Bottom => self.message_last(),
            Action::HalfPageDown => self.thread_scroll_half_page(1),
            Action::HalfPageUp => self.thread_scroll_half_page(-1),
            Action::ScrollUp => self.thread_scroll(-WHEEL_LINES),
            Action::ScrollDown => self.thread_scroll(WHEEL_LINES),
            _ => return self.act_on_cursor(action),
        }
        Effect::None
    }

    /// Keys in the file-threads pane (ADR 0027, focus and `d` per ADR 0034).
    fn act_file_threads(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.leave_file_threads(),
            Action::MoveDown => self.file_thread_move(1),
            Action::MoveUp => self.file_thread_move(-1),
            Action::Confirm => self.focus_thread_pane(),
            _ => return self.act_on_cursor(action),
        }
        Effect::None
    }

    /// Keys in the thread list (ADR 0025).
    fn act_list(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.close_thread_list(),
            Action::MoveDown => self.message_step(1),
            Action::MoveUp => self.message_step(-1),
            Action::ThreadPrev => self.thread_list_step(-1),
            Action::ThreadNext => self.thread_list_step(1),
            Action::HalfPageDown => self.thread_list_page(1),
            Action::HalfPageUp => self.thread_list_page(-1),
            Action::Top => self.thread_list_goto(false),
            Action::Bottom => self.thread_list_goto(true),
            Action::Confirm => self.thread_open_in_file(),
            Action::Fold => self.thread_list_fold(),
            Action::FoldResolved => self.thread_list_fold_resolved(),
            Action::FileOnly => self.thread_list_toggle_file(),
            _ => return self.act_on_cursor(action),
        }
        Effect::None
    }

    /// The keys every thread surface shares: they act on the cursor's
    /// thread and message (ADR 0046).
    fn act_on_cursor(&mut self, action: Action) -> Effect {
        match action {
            Action::Reply => self.thread_reply(),
            Action::EditMessage => self.thread_edit_message(),
            Action::ToggleResolved => self.thread_toggle_resolved(),
            Action::Delete => self.delete_armed_thread(),
            _ => {}
        }
        Effect::None
    }

    /// Keys in the comment box (ADR 0018): readline-style motion and
    /// kills; the keys that scroll the thread above a reply.
    fn act_box(&mut self, action: Action) -> Effect {
        match action {
            Action::Escape => self.compose_cancel(),
            Action::Confirm => self.compose_submit(),
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
            Action::Escape => self.close_popup(),
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
            Action::Escape => view.escape(),
            Action::Confirm => return view.confirm(),
            Action::Backspace => view.input_backspace(),
            _ => {}
        }
        Effect::None
    }
}
