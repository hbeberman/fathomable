// @okf-doc: /decisions/0031-lazy-follow.md
//! Lazy follow (ADR 0031).
//!
//! Auto-jump is a monitor, not a leash. It moves to a change once the
//! queue has been quiet for `follow.jump-debounce` and the reader has not
//! touched the view for [`RECENT_ACTIVITY`], but the moment the reader
//! goes somewhere else — another file, a thread, the diff view, a
//! selection — it switches itself off. Looking around inside the file it
//! landed on is not leaving. When the change is in the visible file it
//! scrolls only if the hunk is off screen, and in a burst it prefers the
//! newest file the agent said it is editing.

use std::time::Duration;

use fathomable_core::follow::Change;

use crate::app::{App, Popup, view};

/// Reader activity newer than this holds auto-jump back (ADR 0015).
pub(super) const RECENT_ACTIVITY: Duration = Duration::from_secs(3);

/// Where the reader is, coarsely: enough to tell "went elsewhere" from
/// "looked around".
#[derive(Clone, Copy, PartialEq, Eq)]
struct Place {
    doc: Option<usize>,
    aside: Option<Aside>,
}

/// Something the reader is doing besides reading the document.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Aside {
    Compose,
    List,
    Diff,
    Selecting,
}

impl App {
    fn place(&self) -> Place {
        let view = self.view();
        let aside = if matches!(self.popup, Some(Popup::Compose(_))) {
            Some(Aside::Compose)
        } else if self.review_list.is_open() {
            Some(Aside::List)
        } else if view.diff_view() {
            Some(Aside::Diff)
        } else if view.selection().is_some() || view.mode() == view::Mode::Select {
            Some(Aside::Selecting)
        } else {
            None
        };
        Place {
            doc: self.current,
            aside,
        }
    }

    /// Run one reader event and switch auto-jump off if it went elsewhere.
    pub(super) fn with_navigation_watch<T>(&mut self, event: impl FnOnce(&mut Self) -> T) -> T {
        let before = self.place();
        let result = event(self);
        if self.auto && left(before, self.place()) {
            self.auto = false;
            self.push_toast("auto-jump off".to_owned());
            tracing::info!("auto-jump off: reader navigated away");
        }
        result
    }

    /// The change auto-jump would take next: the newest (ADR 0055).
    fn auto_target(&self) -> Option<&Change> {
        self.queue.newest()
    }

    /// Jump when the queue is quiet and the guards allow it.
    pub(super) fn auto_jump_tick(&mut self) {
        let quiet = self
            .last_change
            .is_some_and(|at| at.elapsed() >= self.jump.debounce);
        if !(self.auto && quiet && self.auto_jump_allowed()) {
            return;
        }
        let Some(change) = self.auto_target().cloned() else {
            return;
        };
        if change.path == self.current_path() && self.view().line_on_screen(change.target.line()) {
            // Already looking at it: settle without moving.
            self.queue.remove(&change.path);
            return;
        }
        self.jump_to(&change);
    }

    fn auto_jump_allowed(&self) -> bool {
        if self.popup.is_some() || self.review_list.is_open() {
            return false;
        }
        let Some(index) = self.current else {
            return true;
        };
        let view = &self.docs[index].view;
        view.selection().is_none()
            && view.mode() == view::Mode::Normal
            && !view.diff_view()
            && view.idle() >= RECENT_ACTIVITY
    }

    /// How long until the next auto-jump could go, `None` when none is
    /// pending.
    pub(super) fn auto_jump_in(&self) -> Option<Duration> {
        if !self.auto || self.queue.is_empty() {
            return None;
        }
        if self.popup.is_some() || self.review_list.is_open() {
            return None;
        }
        if let Some(index) = self.current {
            let view = &self.docs[index].view;
            if view.selection().is_some() || view.mode() != view::Mode::Normal || view.diff_view() {
                return None;
            }
        }
        let since = self.last_change.map_or(Duration::ZERO, |at| at.elapsed());
        let wait = self.jump.debounce.saturating_sub(since);
        let activity = self.current.map_or(Duration::ZERO, |i| {
            RECENT_ACTIVITY.saturating_sub(self.docs[i].view.idle())
        });
        Some(wait.max(activity).max(Duration::from_millis(50)))
    }
}

/// Whether moving from `before` to `after` is going elsewhere rather
/// than looking around.
fn left(before: Place, after: Place) -> bool {
    after.doc != before.doc || (after.aside.is_some() && after.aside != before.aside)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crossterm::event::KeyCode;
    use fathomable_core::annotations::Store;
    use fathomable_core::config::JumpConfig;

    use super::RECENT_ACTIVITY;
    use crate::app::{App, Options};
    use fathomable_testing::TempDir;

    use crate::app::testing::{AppBuilder, press_key};

    /// Sixty paragraphs: more rows than the 30-row test terminal shows.
    fn readme() -> String {
        let mut text = String::from("# Readme\n");
        for n in 1..=60 {
            text.push_str("\nline ");
            text.push_str(&n.to_string());
            text.push('\n');
        }
        text
    }
    const NOTES: &str = "notes\n\nfirst\n";
    const GUIDE: &str = "guide\n\none\n";

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = TempDir::new(&format!("autojump-{name}"))?;
        fs::write(dir.0.join("README.md"), readme())?;
        fs::write(dir.0.join("notes.md"), NOTES)?;
        fs::write(dir.0.join("guide.md"), GUIDE)?;
        Ok(dir)
    }

    /// An app with auto-jump on and no debounce, `README.md` open,
    /// and the reader long still.
    fn app(dir: &TempDir) -> anyhow::Result<App> {
        let jump = JumpConfig {
            auto: true,
            debounce: std::time::Duration::ZERO,
            ..JumpConfig::default()
        };
        let store = Store::open(dir.0.join(".threads.jsonl"))?;
        let mut app = AppBuilder::at(&dir.0)
            .options(|o| Options {
                jump,
                store: Some(store),
                ..o
            })
            .build()?;
        app.view_mut().rest(RECENT_ACTIVITY);
        Ok(app)
    }

    fn changed(dir: &TempDir, app: &mut App, relative: &str, text: &str) -> std::io::Result<()> {
        let absolute = dir.0.join(relative);
        fs::write(&absolute, text)?;
        app.on_changes(vec![absolute]);
        Ok(())
    }

    fn toasts(app: &App) -> Vec<&str> {
        app.toasts().iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn going_elsewhere_switches_off_but_looking_around_does_not() -> anyhow::Result<()> {
        let dir = fixture("leave")?;
        let mut app = app(&dir)?;
        changed(&dir, &mut app, "notes.md", "notes\n\nfirst\nmore\n")?;
        changed(&dir, &mut app, "guide.md", "guide\n\none\ntwo\n")?;

        // Scrolling and searching in place keep auto-jump on.
        press_key(&mut app, KeyCode::Char('j'));
        press_key(&mut app, KeyCode::Char('G'));
        assert!(app.auto_jump(), "looking around is not leaving");

        // Stepping to another file with `]f` is leaving.
        press_key(&mut app, KeyCode::Char(']'));
        press_key(&mut app, KeyCode::Char('f'));
        assert_ne!(app.current_path(), Path::new("README.md"));
        assert!(!app.auto_jump());
        assert!(toasts(&app).contains(&"auto-jump off"));
        assert!(!app.queue().is_empty(), "the queue is untouched");

        // Back on, opening the review list is leaving too.
        app.set_auto_jump(true);
        press_key(&mut app, KeyCode::Char(' '));
        press_key(&mut app, KeyCode::Char('r'));
        assert!(!app.auto_jump());

        // And so is starting a selection.
        press_key(&mut app, KeyCode::Esc);
        app.set_auto_jump(true);
        press_key(&mut app, KeyCode::Char('V'));
        assert!(!app.auto_jump());
        Ok(())
    }

    #[test]
    fn its_own_jump_does_not_switch_it_off() -> anyhow::Result<()> {
        let dir = fixture("own")?;
        let mut app = app(&dir)?;
        changed(&dir, &mut app, "notes.md", "notes\n\nfirst\nmore\n")?;
        app.tick();
        assert_eq!(app.current_path(), Path::new("notes.md"));
        assert!(app.auto_jump());
        assert!(app.queue().is_empty());
        Ok(())
    }

    #[test]
    fn a_blocking_popup_suspends_the_auto_jump_timer() -> anyhow::Result<()> {
        let dir = fixture("blocked-timer")?;
        let mut app = app(&dir)?;
        changed(&dir, &mut app, "notes.md", "notes\n\nfirst\nmore\n")?;
        assert!(app.auto_jump_in().is_some());
        app.open_status();
        assert_eq!(app.auto_jump_in(), None);
        Ok(())
    }

    #[test]
    fn the_visible_file_scrolls_only_when_the_hunk_is_off_screen() -> anyhow::Result<()> {
        let dir = fixture("visible")?;
        let mut app = app(&dir)?;

        // A hunk on screen settles without moving.
        changed(
            &dir,
            &mut app,
            "README.md",
            &readme().replace("line 3\n", "line 3!\n"),
        )?;
        app.view_mut().rest(RECENT_ACTIVITY);
        let before = app.view().scroll();
        app.tick();
        assert_eq!(app.view().scroll(), before);
        assert!(app.queue().is_empty(), "on-screen hunk settles");
        assert!(app.auto_jump());

        // A hunk far below scrolls into view.
        let edited = readme()
            .replace("line 3\n", "line 3!\n")
            .replace("line 60\n", "line 60!\n");
        changed(&dir, &mut app, "README.md", &edited)?;
        app.view_mut().rest(RECENT_ACTIVITY);
        assert!(!app.queue().is_empty());
        app.tick();
        assert!(app.view().scroll() > before, "off-screen hunk scrolls");
        assert!(app.queue().is_empty());
        assert!(app.auto_jump());
        Ok(())
    }
}
