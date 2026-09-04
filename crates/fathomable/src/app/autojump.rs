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

use super::{App, Popup, view};

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
    Thread,
    Diff,
    Selecting,
}

impl App {
    fn place(&self) -> Place {
        let view = self.view();
        let aside = if matches!(self.popup, Some(Popup::Compose(_))) {
            Some(Aside::Compose)
        } else if self.list.is_open() {
            Some(Aside::List)
        } else if self.thread.is_some() {
            Some(Aside::Thread)
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

    /// The change auto-jump would take next: the newest on the agent's
    /// follow list, else the newest.
    fn auto_target(&self) -> Option<&Change> {
        self.queue
            .iter()
            .find(|change| {
                self.followed
                    .iter()
                    .any(|followed| change.path.starts_with(followed))
            })
            .or_else(|| self.queue.newest())
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
        if self.popup.is_some() || self.thread.is_some() || self.list.is_open() {
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
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fathomable_core::annotations::Store;
    use fathomable_core::config::JumpConfig;
    use fathomable_core::session::{Request, Response};
    use fathomable_core::workspace::Workspace;

    use super::RECENT_ACTIVITY;
    use crate::app::input::keys;
    use crate::app::{App, Options};

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

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-autojump-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir)?;
            fs::write(dir.join("README.md"), readme())?;
            fs::write(dir.join("notes.md"), NOTES)?;
            fs::write(dir.join("guide.md"), GUIDE)?;
            Ok(Self(dir))
        }

        /// An app with auto-jump on and no debounce, `README.md` open,
        /// and the reader long still.
        fn app(&self) -> anyhow::Result<App> {
            let workspace = Workspace::discover(&self.0)?;
            let jump = JumpConfig {
                auto: true,
                debounce: std::time::Duration::ZERO,
                ..JumpConfig::default()
            };
            let options = Options {
                jump,
                store: Some(Store::open(self.0.join(".threads.jsonl"))?),
                ..Options::for_test(self.0.clone())
            };
            let mut app = App::new(workspace, 100, 30, options);
            app.open(Path::new("README.md"));
            app.view_mut().rest(RECENT_ACTIVITY);
            Ok(app)
        }

        fn changed(&self, app: &mut App, relative: &str, text: &str) -> std::io::Result<()> {
            let absolute = self.0.join(relative);
            fs::write(&absolute, text)?;
            app.on_changes(vec![absolute]);
            Ok(())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn toasts(app: &App) -> Vec<&str> {
        app.toasts().iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn going_elsewhere_switches_off_but_looking_around_does_not() -> anyhow::Result<()> {
        let dir = TempDir::new("leave")?;
        let mut app = dir.app()?;
        dir.changed(&mut app, "notes.md", "notes\n\nfirst\nmore\n")?;
        dir.changed(&mut app, "guide.md", "guide\n\none\ntwo\n")?;

        // Scrolling and searching in place keep auto-jump on.
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('G'));
        assert!(app.auto_jump(), "looking around is not leaving");

        // Stepping to another file with `]f` is leaving.
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('f'));
        assert_ne!(app.current_path(), Path::new("README.md"));
        assert!(!app.auto_jump());
        assert!(toasts(&app).contains(&"auto-jump off"));
        assert!(!app.queue().is_empty(), "the queue is untouched");

        // Back on, opening the thread list is leaving too.
        app.set_auto_jump(true);
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('A'));
        assert!(!app.auto_jump());

        // And so is starting a selection.
        press(&mut app, KeyCode::Esc);
        app.set_auto_jump(true);
        press(&mut app, KeyCode::Char('V'));
        assert!(!app.auto_jump());
        Ok(())
    }

    #[test]
    fn its_own_jump_does_not_switch_it_off() -> anyhow::Result<()> {
        let dir = TempDir::new("own")?;
        let mut app = dir.app()?;
        dir.changed(&mut app, "notes.md", "notes\n\nfirst\nmore\n")?;
        app.tick();
        assert_eq!(app.current_path(), Path::new("notes.md"));
        assert!(app.auto_jump());
        assert!(app.queue().is_empty());
        Ok(())
    }

    #[test]
    fn the_visible_file_scrolls_only_when_the_hunk_is_off_screen() -> anyhow::Result<()> {
        let dir = TempDir::new("visible")?;
        let mut app = dir.app()?;

        // A hunk on screen settles without moving.
        dir.changed(
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
        dir.changed(&mut app, "README.md", &edited)?;
        app.view_mut().rest(RECENT_ACTIVITY);
        assert!(!app.queue().is_empty());
        app.tick();
        assert!(app.view().scroll() > before, "off-screen hunk scrolls");
        assert!(app.queue().is_empty());
        assert!(app.auto_jump());
        Ok(())
    }

    #[test]
    fn a_burst_lands_on_the_followed_file_first() -> anyhow::Result<()> {
        let dir = TempDir::new("burst")?;
        let mut app = dir.app()?;
        let response = app.handle_request(Request::Follow {
            paths: vec![PathBuf::from("notes.md")],
        });
        assert_eq!(response, Response::Done);
        dir.changed(&mut app, "notes.md", "notes\n\nfirst\nmore\n")?;
        dir.changed(&mut app, "guide.md", "guide\n\none\ntwo\n")?;
        assert_eq!(
            app.queue().newest().map(|c| c.path.as_path()),
            Some(Path::new("guide.md"))
        );
        app.tick();
        assert_eq!(
            app.current_path(),
            Path::new("notes.md"),
            "followed beats newest"
        );
        assert_eq!(app.queue().len(), 1, "the rest of the burst stays queued");
        // A followed directory pulls the same way for the files under it.
        fs::create_dir_all(dir.0.join("deep"))?;
        assert_eq!(
            app.handle_request(Request::Follow {
                paths: vec![PathBuf::from("deep")],
            }),
            Response::Done
        );
        dir.changed(&mut app, "deep/inner.md", "inner\n\none\ntwo\n")?;
        dir.changed(&mut app, "guide.md", "guide\n\none\ntwo\nthree\n")?;
        app.view_mut().rest(RECENT_ACTIVITY);
        app.tick();
        assert_eq!(
            app.current_path(),
            Path::new("deep/inner.md"),
            "a followed directory beats newest"
        );
        Ok(())
    }
}
