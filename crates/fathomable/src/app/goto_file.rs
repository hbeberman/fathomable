//! `gf` (ADR 0052): open the file the reference under the cursor names,
//! in the viewer, at its line.
//!
//! The reference is the rendered link's destination or the bare word
//! under the cursor, as [`fathomable_core::link`] reads it. It resolves
//! against the current document's directory first and the workspace root
//! second, and must be a file inside the workspace. The hop is a far move,
//! so `Alt-Left` returns; a Ctrl-click and the context menu run the same
//! action.

use std::path::{Path, PathBuf};

use fathomable_core::link::{self, Target};

use crate::app::view::View;
use crate::app::{App, Focus};

impl View {
    /// The reference under the cursor: the link's destination on a
    /// rendered link, else the path-like word around the cursor.
    #[must_use]
    pub(crate) fn file_reference(&self) -> Option<String> {
        if let Some(url) = self.link_at_cursor() {
            return Some(url.to_owned());
        }
        let cursor = self.cursor();
        let line = self.layout().lines().get(cursor.row)?;
        let text = line.text();
        let byte = line.byte_at(cursor.col)?;
        link::word_at(&text, byte).map(str::to_owned)
    }

    /// The file the reference under the cursor names, if it names one.
    #[must_use]
    pub(crate) fn file_target(&self) -> Option<Target> {
        self.file_reference()
            .and_then(|reference| link::parse(&reference))
    }
}

impl App {
    /// `gf`: open the file under the cursor at its line; a notice says
    /// why when there is no file to open.
    pub(crate) fn goto_file(&mut self) {
        let Some(reference) = self.view().file_reference() else {
            self.notice("no file reference here");
            return;
        };
        let Some(target) = link::parse(&reference) else {
            self.notice(format!("not a file: {reference}; gx opens it"));
            return;
        };
        let relative = match self.resolve_reference(target.path()) {
            Ok(relative) => relative,
            Err(notice) => {
                self.notice(notice);
                return;
            }
        };
        self.close_popup();
        self.focus = Focus::View;
        self.open(&relative);
        if self.current_path() == relative {
            self.view_mut().goto_source_line(target.line().unwrap_or(1));
        }
    }

    /// Whether the reference under the cursor names a file that is there,
    /// which is when the context menu offers `gf`.
    #[must_use]
    pub(super) fn file_here(&self) -> bool {
        self.view()
            .file_target()
            .is_some_and(|target| self.resolve_reference(target.path()).is_ok())
    }

    /// The root-relative file `path` names, read against the current
    /// document's directory and then the root; `Err` carries the notice
    /// when it is outside the workspace, a directory, or not there.
    fn resolve_reference(&self, path: &Path) -> Result<PathBuf, String> {
        let root = self.workspace.root();
        let candidates: Vec<PathBuf> = if path.is_absolute() {
            vec![path.to_path_buf()]
        } else {
            let here = self
                .current_path()
                .parent()
                .map(|dir| root.join(dir).join(path));
            here.into_iter().chain([root.join(path)]).collect()
        };
        for candidate in &candidates {
            let relative = self.workspace.relative(candidate);
            if relative.is_absolute() {
                // Canonicalised out of the workspace.
                continue;
            }
            let absolute = root.join(&relative);
            if absolute.is_file() {
                return Ok(relative);
            }
            if absolute.is_dir() {
                return Err(format!("{} is a directory", relative.display()));
            }
        }
        if path.is_absolute() {
            Err(format!("{} is outside the workspace", path.display()))
        } else {
            Err(format!("no file {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_testing::TempDir;

    use crate::app::input::bindings::Action;
    use crate::app::testing::AppBuilder;
    use crate::app::{App, Options};

    fn fixture(name: &str) -> anyhow::Result<(TempDir, App)> {
        let dir = TempDir::new(&format!("goto-file-{name}"))?;
        fs::create_dir_all(dir.0.join("docs"))?;
        fs::create_dir_all(dir.0.join("src"))?;
        fs::write(
            dir.0.join("docs/notes.md"),
            "# Notes\n\nSee [the guide](guide.md#L3) and `src/lib.rs:2` (or src/).\n\nAlso <https://example.com/x.md> and nothing.\n",
        )?;
        fs::write(dir.0.join("docs/guide.md"), "# Guide\n\nthird\n")?;
        fs::write(dir.0.join("src/lib.rs"), "fn a() {}\nfn b() {}\n")?;
        let root = dir.0.clone();
        let app = AppBuilder::at(&dir.0)
            .unopened()
            .options(move |_| Options::for_test(root))
            .build()?;
        Ok((dir, app))
    }

    /// Put the cursor on rendered row `row` of the open document at the
    /// column where `text` starts.
    fn cursor_on(app: &mut App, row: usize, text: &str) -> anyhow::Result<()> {
        let view = app.view_mut();
        view.goto_top();
        view.move_down(row);
        view.line_start();
        let col = view.layout().lines()[row]
            .text()
            .find(text)
            .ok_or_else(|| anyhow::anyhow!("{text:?} is not on row {row}"))?;
        for _ in 0..col {
            view.move_right();
        }
        Ok(())
    }

    #[test]
    fn a_link_opens_beside_the_document_at_its_fragment_line() -> anyhow::Result<()> {
        let (_dir, mut app) = fixture("link")?;
        app.open(Path::new("docs/notes.md"));
        cursor_on(&mut app, 2, "guide")?;
        assert_eq!(app.view().file_reference().as_deref(), Some("guide.md#L3"));
        app.act(Action::GotoFile);
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        assert_eq!(app.view().cursor_source_line(), Some(3));
        // The way back is the jumplist.
        app.jump_back();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.view().cursor_source_line(), Some(3));
        Ok(())
    }

    #[test]
    fn a_bare_path_resolves_against_the_root_with_its_line() -> anyhow::Result<()> {
        let (_dir, mut app) = fixture("bare")?;
        app.open(Path::new("docs/notes.md"));
        cursor_on(&mut app, 2, "lib.rs")?;
        assert_eq!(app.view().file_reference().as_deref(), Some("src/lib.rs:2"));
        app.act(Action::GotoFile);
        assert_eq!(app.current_path(), Path::new("src/lib.rs"));
        assert_eq!(app.view().cursor_source_line(), Some(2));
        Ok(())
    }

    #[test]
    fn directories_schemes_and_plain_words_notice_and_stay() -> anyhow::Result<()> {
        let (_dir, mut app) = fixture("notice")?;
        app.open(Path::new("docs/notes.md"));
        cursor_on(&mut app, 2, "src/)")?;
        app.act(Action::GotoFile);
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.message.as_deref(), Some("src is a directory"));

        cursor_on(&mut app, 4, "example")?;
        assert!(app.view().file_target().is_none(), "a scheme is not a file");
        app.act(Action::GotoFile);
        assert_eq!(
            app.message.as_deref(),
            Some("not a file: https://example.com/x.md; gx opens it")
        );

        app.view_mut().line_end();
        app.act(Action::GotoFile);
        assert_eq!(app.message.as_deref(), Some("no file nothing"));
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        Ok(())
    }

    #[test]
    fn a_ctrl_click_opens_the_file_under_the_pointer_and_the_menu_offers_it() -> anyhow::Result<()>
    {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        use crate::app::Popup;
        use crate::app::input::menu::Entry;
        use crate::app::input::mouse::handle_mouse;

        let (_dir, mut app) = fixture("mouse")?;
        app.resize(80, 20);
        app.open(Path::new("docs/notes.md"));
        cursor_on(&mut app, 2, "lib.rs")?;
        let col = app.view().cursor().col;
        let column =
            u16::try_from(app.sidebar_width() + crate::app::draw::gutter_width(app.view()) + col)?;
        let row = u16::try_from(app.text_top() + 2)?;
        app.view_mut().goto_top();
        let at = |kind, modifiers| MouseEvent {
            kind,
            column,
            row,
            modifiers,
        };
        handle_mouse(
            &mut app,
            at(MouseEventKind::Down(MouseButton::Right), KeyModifiers::NONE),
        );
        let Some(Popup::Menu(menu)) = app.popup() else {
            anyhow::bail!("a right-click opens the menu");
        };
        let labels: Vec<&str> = menu.entries().iter().map(Entry::label).collect();
        assert!(labels.contains(&"open in viewer"), "{labels:?}");
        app.close_popup();

        handle_mouse(
            &mut app,
            at(
                MouseEventKind::Down(MouseButton::Left),
                KeyModifiers::CONTROL,
            ),
        );
        assert_eq!(app.current_path(), Path::new("src/lib.rs"));
        assert_eq!(app.view().cursor_source_line(), Some(2));
        app.jump_back();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        Ok(())
    }

    #[test]
    fn an_absolute_path_outside_the_workspace_is_refused() -> anyhow::Result<()> {
        let (_dir, mut app) = fixture("outside")?;
        app.open(Path::new("docs/notes.md"));
        let outside = app.resolve_reference(Path::new("/etc/hostname"));
        assert_eq!(
            outside,
            Err("/etc/hostname is outside the workspace".to_owned())
        );
        let escaped = app.resolve_reference(Path::new("../../../../etc/hostname"));
        assert_eq!(escaped, Err("no file ../../../../etc/hostname".to_owned()));
        Ok(())
    }
}
