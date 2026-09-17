// @okf-doc: /decisions/0068-what-the-files-pane-shows.md
//! What the files pane shows (ADR 0068): three session toggles under
//! `Space F`, only changed files (`c`), hide untracked files (`u`), and
//! show ignored files (`g`), working from any pane.
//!
//! The rules themselves are the tree's [`Shown`]; this module is the
//! app's hands on them: the toggle, the live label a which-key or menu
//! entry carries so it says what a press does now, the notice while the
//! pane is hidden, and the words the pane's header names the state by.

use fathomable_core::tree::{Rule, Shown, Tree};

use super::App;
use super::input::bindings::{self, Action, Chord, Where};

impl App {
    /// `Space F c` / `u` / `g`: flip `rule` on the files pane, shown or
    /// hidden, keeping the cursor's file where it is still listed.
    pub(crate) fn files_toggle(&mut self, rule: Rule) {
        if !self.ensure_tree() {
            return;
        }
        let status = self.comparison_status().clone();
        let virtual_paths = self.comparison_virtual_paths();
        let snapshot_paths = self.comparison_snapshot_paths();
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let shown = tree.shown().toggled(rule);
        if let Err(error) = tree.set_shown(&mut self.workspace, &status, shown) {
            self.notice(error.to_string());
            return;
        }
        if let Some(paths) = snapshot_paths {
            tree.set_snapshot_paths(&status, paths);
        } else {
            tree.set_virtual_paths(&status, virtual_paths);
        }
        self.scroll_tree();
        self.refresh_directory_selection();
        if !self.sidebar.tree {
            // The header that names the state is not on screen.
            let words = shown_words(shown);
            let state = if words.is_empty() {
                "all files".to_owned()
            } else {
                words.join(" ")
            };
            self.notice(format!("files pane: {state}"));
        }
    }

    /// A new status landed: the files pane lists by it.
    pub(super) fn sift_tree(&mut self) {
        let status = self.comparison_status().clone();
        let virtual_paths = self.comparison_virtual_paths();
        let snapshot_paths = self.comparison_snapshot_paths();
        if let Some(tree) = self.tree.as_mut() {
            if let Some(paths) = snapshot_paths {
                tree.set_snapshot_paths(&status, paths);
            } else {
                tree.set_virtual_paths(&status, virtual_paths);
            }
            self.scroll_tree();
            self.refresh_directory_selection();
        }
    }

    /// What the files pane lists, `Shown::all()` before the tree exists.
    fn files_shown(&self) -> Shown {
        self.tree.as_ref().map_or_else(Shown::all, Tree::shown)
    }

    /// What pressing a toggle's key does now, when that differs from the
    /// table's label: `all files` while only changed files are listed,
    /// `show untracked` while they are hidden, `hide ignored` while they
    /// are shown.
    pub(crate) fn live_label(&self, action: Action) -> Option<&'static str> {
        let shown = self.files_shown();
        match action {
            Action::FilesChanged if shown.changed_only() => Some("all files"),
            Action::FilesUntracked if !shown.untracked() => Some("show untracked"),
            Action::FilesIgnored if shown.ignored() => Some("hide ignored"),
            _ => None,
        }
    }

    /// The label a toggle's entry reads now: the live one, else the
    /// table's without the submenu word.
    pub(crate) fn toggle_label(&self, action: Action) -> &'static str {
        self.live_label(action).unwrap_or(match action {
            Action::FilesChanged => "only changed",
            Action::FilesUntracked => "hide untracked",
            Action::FilesIgnored => "show ignored",
            _ => "",
        })
    }

    /// The which-key entries for the keys typed so far on `place`, each
    /// toggle relabelled with what pressing it does now.
    pub(crate) fn which_key(&self, place: Where) -> Vec<(Chord, String)> {
        bindings::menu_entries(place, self.prefix(), |action| self.live_label(action))
    }

    /// The words the files pane's header names the active rules by,
    /// each saying what is on screen: `changed`, `tracked`, `ignored`.
    pub(crate) fn files_shown_words(&self) -> Vec<&'static str> {
        shown_words(self.files_shown())
    }
}

/// `changed` while only changed files are listed, `tracked` while
/// untracked files are hidden, `ignored` while ignored files are shown.
fn shown_words(shown: Shown) -> Vec<&'static str> {
    let mut words = Vec::new();
    if shown.changed_only() {
        words.push("changed");
    }
    if !shown.untracked() {
        words.push("tracked");
    }
    if shown.ignored() {
        words.push("ignored");
    }
    words
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Context as _;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use fathomable_testing::{TempDir, git};

    use crate::app::input::bindings::Where;
    use crate::app::input::mouse::handle_mouse;
    use crate::app::testing::{self, AppBuilder, press, screen};
    use crate::app::{App, Focus};

    /// A repository with a clean `src/lib.rs`, a modified `README.md`,
    /// an untracked `notes.txt`, and an ignored `build.log`.
    fn fixture(name: &str) -> anyhow::Result<TempDir> {
        let dir = testing::bare(&format!("files-shown-{name}"))?;
        let root = testing::root(&dir);
        git::init(&root)?;
        fs::create_dir_all(root.join("src"))?;
        let committed = [
            ("README.md", "# Readme\n\nhello\n"),
            ("src/lib.rs", "fn lib() {}\n"),
            (".gitignore", "*.log\n"),
        ];
        // The helper writes the objects and the index; the work tree is
        // the test's to write.
        for (path, text) in committed {
            fs::write(root.join(path), text)?;
        }
        git::commit_and_stage(&root, &committed)?;
        fs::write(root.join("README.md"), "# Readme\n\nhello again\n")?;
        fs::write(root.join("notes.txt"), "todo\n")?;
        fs::write(root.join("build.log"), "noise\n")?;
        Ok(dir)
    }

    fn names(app: &App) -> Vec<String> {
        app.tree()
            .map(|tree| {
                tree.rows()
                    .iter()
                    .map(|row| row.path().display().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The which-key label under the `Space F` prefix for `key`.
    fn label_of(app: &App, key: char) -> anyhow::Result<String> {
        app.which_key(Where::View)
            .into_iter()
            .find(|(chord, _)| chord.to_string() == key.to_string())
            .map(|(_, label)| label)
            .with_context(|| format!("no entry {key}"))
    }

    fn header_row(app: &App) -> anyhow::Result<String> {
        Ok(screen(app)?.into_iter().next().unwrap_or_default())
    }

    #[test]
    fn the_toggles_filter_the_pane_and_say_what_a_press_does_now() -> anyhow::Result<()> {
        let dir = fixture("toggles")?;
        let mut app = AppBuilder::new(&dir).build()?;
        app.show_tree();
        assert_eq!(
            names(&app),
            ["src", ".gitignore", "notes.txt", "README.md"],
            "the ignored log is hidden and the rest listed"
        );
        let header = header_row(&app)?;
        assert!(header.contains("+2 -1"), "{header}");

        // `Space F` says what each press would do now.
        press(&mut app, " F");
        assert_eq!(label_of(&app, 'c')?, "only changed");
        assert_eq!(label_of(&app, 'u')?, "hide untracked");
        assert_eq!(label_of(&app, 'g')?, "show ignored");
        press(&mut app, "c");
        assert_eq!(names(&app), ["notes.txt", "README.md"]);
        let header = header_row(&app)?;
        assert!(header.starts_with(" Files"), "{header}");
        assert!(header.contains("changed +2 -1"), "{header}");
        assert!(!header.contains('·'), "{header}");

        press(&mut app, " F");
        assert_eq!(label_of(&app, 'c')?, "all files");
        press(&mut app, "u");
        assert_eq!(
            names(&app),
            ["README.md"],
            "untracked dropped from the changed list"
        );
        assert!(header_row(&app)?.contains("changed tracked +2 -1"));

        // Ignored files are never changed ones: only changed wins.
        press(&mut app, " Fg");
        assert_eq!(names(&app), ["README.md"]);
        // Filter words drop before the counts as the header narrows.
        let header = header_row(&app)?;
        assert!(header.contains("changed tracked +2 -1"), "{header}");
        assert!(!header.contains("ignored"), "{header}");
        press(&mut app, " F");
        assert_eq!(label_of(&app, 'u')?, "show untracked");
        assert_eq!(label_of(&app, 'g')?, "hide ignored");

        // Back to every file the rules allow: ignored shown, untracked hidden.
        press(&mut app, "c");
        assert_eq!(names(&app), ["src", ".gitignore", "build.log", "README.md"]);
        press(&mut app, " Fu Fg");
        assert_eq!(names(&app), ["src", ".gitignore", "notes.txt", "README.md"]);
        assert!(!header_row(&app)?.contains('·'), "no words with no rule on");
        Ok(())
    }

    #[test]
    fn a_toggle_while_the_pane_is_hidden_names_the_state() -> anyhow::Result<()> {
        let dir = fixture("hidden")?;
        let mut app = AppBuilder::new(&dir).build()?;
        assert_eq!(app.focus(), Focus::View);
        press(&mut app, " Fc");
        assert_eq!(app.message(), Some("files pane: changed"));
        press(&mut app, " Fu");
        assert_eq!(app.message(), Some("files pane: changed tracked"));
        press(&mut app, " Fc Fu");
        assert_eq!(app.message(), Some("files pane: all files"));
        // The rules were applied all along: showing the pane lists by them.
        press(&mut app, " Fc");
        app.show_tree();
        assert_eq!(names(&app), ["notes.txt", "README.md"]);
        Ok(())
    }

    #[test]
    fn live_deletion_restoration_keeps_pinned_comparison_entries() -> anyhow::Result<()> {
        use std::path::Path;

        use crate::app::watch::Event;

        let dir = fixture("deleted")?;
        let root = testing::root(&dir);
        let mut app = AppBuilder::new(&dir).build()?;
        app.show_tree();
        let file = Path::new("src/lib.rs");
        app.with_tree_result(|tree, workspace| tree.reveal(workspace, file).map(|_| None));
        fs::remove_file(root.join(file))?;
        app.on_events(vec![Event::Removed(root.join(file))]);
        assert!(
            names(&app).contains(&"src/lib.rs".to_owned()),
            "rows={:?} comparison={:?}",
            names(&app),
            app.comparison().map(|comparison| comparison
                .changes()
                .iter()
                .map(|change| (change.path().display().to_string(), change.kind()))
                .collect::<Vec<_>>())
        );
        assert_eq!(
            app.tree()
                .and_then(|tree| tree.current())
                .map(fathomable_core::tree::Row::path),
            Some(file)
        );

        // A restored tombstone is already listed, but still needs a disk listing.
        fs::write(root.join(file), "fn lib() {}\n")?;
        app.on_events(vec![Event::Change(root.join(file))]);
        assert!(names(&app).contains(&"src/lib.rs".to_owned()));
        assert!(!app.status().contains(file));
        fs::remove_file(root.join(file))?;
        app.on_events(vec![Event::Removed(root.join(file))]);
        press(&mut app, " Fc Fu");
        assert!(names(&app).contains(&"src/lib.rs".to_owned()));

        git::commit_and_stage(
            &root,
            &[
                ("README.md", "# Readme\n\nhello again\n"),
                (".gitignore", "*.log\n"),
            ],
        )?;
        app.on_events(vec![Event::Change(root.join(".git/index"))]);
        app.settle_status();
        assert!(
            names(&app).contains(&"src/lib.rs".to_owned()),
            "the pinned comparison does not advance with a later commit: {:?}",
            app.comparison().map(|comparison| comparison
                .changes()
                .iter()
                .map(|change| change.path().display().to_string())
                .collect::<Vec<_>>())
        );
        press(&mut app, " Fc Fu");
        assert!(app.comparison().is_some_and(|comparison| {
            comparison
                .changes()
                .iter()
                .any(|change| change.path() == Path::new("src/lib.rs"))
        }));
        Ok(())
    }

    #[test]
    fn the_files_title_opens_the_settings_with_live_labels() -> anyhow::Result<()> {
        let dir = fixture("menu")?;
        let mut app = AppBuilder::new(&dir).build()?;
        app.show_tree();
        let click_title = |app: &mut App| {
            let row = app.pane_top();
            handle_mouse(
                app,
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 2,
                    row: u16::try_from(row).unwrap_or(u16::MAX),
                    modifiers: KeyModifiers::NONE,
                },
            )
        };
        click_title(&mut app);
        let labels = |app: &App| -> Vec<String> {
            app.menu()
                .map(|menu| {
                    menu.entries()
                        .iter()
                        .map(|entry| entry.label().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        let shown = labels(&app);
        assert_eq!(shown, ["only changed", "hide untracked", "show ignored"]);
        app.close_popup();
        press(&mut app, " Fc");
        click_title(&mut app);
        let shown = labels(&app);
        assert_eq!(shown.last().map(String::as_str), Some("show ignored"));
        assert!(shown.iter().any(|label| label == "all files"), "{shown:?}");
        app.close_popup();

        let row = app.pane_top();
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Right),
                column: 2,
                row: u16::try_from(row).unwrap_or(u16::MAX),
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(app.menu().is_none(), "right-click leaves the header inert");
        Ok(())
    }
}
