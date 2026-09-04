//! Checkpoints (ADR 0049): `Space v c` marks the current file's content on
//! its timeline, `Space v C` marks every non-ignored file whose content
//! moved since its last checkpoint. Both go through the one store in
//! [`fathomable_core::checkpoints`]; a toast counts what was stored.

use std::fs;
use std::path::PathBuf;

use fathomable_core::checkpoints::Origin;
use fathomable_core::content;
use fathomable_core::workspace::Filter;

use super::App;

impl App {
    /// `Space v c`: checkpoint the current file.
    pub fn checkpoint_file(&mut self) {
        let Some(doc) = self.current.and_then(|i| self.docs.get(i)) else {
            self.notice("no file open to checkpoint");
            return;
        };
        let Some(text) = doc.document.text().map(str::to_owned) else {
            self.notice("only text files are checkpointed");
            return;
        };
        let path = doc.relative.clone();
        self.record_checkpoint(Origin::File, &[(path, text)]);
    }

    /// `Space v C`: checkpoint every non-ignored text file whose content
    /// differs from its latest checkpoint, or has none.
    pub fn checkpoint_workspace(&mut self) {
        if self.checkpoints.is_none() {
            self.notice("checkpoints unavailable; see the log");
            return;
        }
        let max_bytes = self.viewer.max_file_bytes();
        let root = self.workspace.root().to_path_buf();
        let files: Vec<(PathBuf, String)> = self
            .workspace
            .walk_files(Filter::Visible)
            .into_iter()
            .map(PathBuf::from)
            .filter_map(|relative| {
                let bytes = fs::read(root.join(&relative)).ok()?;
                if u64::try_from(bytes.len()).is_ok_and(|len| len > max_bytes)
                    || content::is_binary(&bytes)
                {
                    return None;
                }
                let text = String::from_utf8(bytes).ok()?;
                Some((relative, text))
            })
            .collect();
        self.record_checkpoint(Origin::Workspace, &files);
    }

    fn record_checkpoint(&mut self, origin: Origin, files: &[(PathBuf, String)]) {
        let Some(store) = self.checkpoints.as_mut() else {
            self.notice("checkpoints unavailable; see the log");
            return;
        };
        let pairs = files
            .iter()
            .map(|(path, text)| (path.as_path(), text.as_str()));
        match store.record(origin, pairs) {
            Ok(0) => self.push_toast("checkpoint: nothing changed".to_owned()),
            Ok(1) => self.push_toast("checkpoint: 1 file".to_owned()),
            Ok(stored) => self.push_toast(format!("checkpoint: {stored} files")),
            Err(error) => {
                tracing::warn!(%error, "cannot write a checkpoint");
                self.notice(format!("cannot checkpoint: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fathomable_core::checkpoints::Store;
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::TempDir;

    use crate::app::input::keys;
    use crate::app::{App, Options};

    fn press(app: &mut App, keys: &str) {
        for ch in keys.chars() {
            keys::handle_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
    }

    fn last_toast(app: &App) -> String {
        app.toasts()
            .last()
            .map(|t| t.text().to_owned())
            .unwrap_or_default()
    }

    /// `Space v C` stores every text file once, then only the files whose
    /// content moved; `Space v c` appends to the same timeline.
    #[test]
    fn workspace_checkpoints_skip_unchanged_files() -> anyhow::Result<()> {
        let dir = TempDir::new("app-checkpoints")?;
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        fs::write(dir.0.join("ws/README.md"), "# Readme\n")?;
        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n")?;
        fs::write(dir.0.join("ws/logo.bin"), b"\0\x01\x02")?;
        let store = || Store::open(&dir.0.join("state"));
        let options = Options {
            checkpoints: Some(store()?),
            ..Options::for_test(dir.0.join("ws"))
        };
        let mut app = App::new(Workspace::discover(dir.0.join("ws"))?, 100, 30, options);

        press(&mut app, " vC");
        assert_eq!(
            last_toast(&app),
            "checkpoint: 2 files",
            "binary files are skipped"
        );
        press(&mut app, " vC");
        assert_eq!(last_toast(&app), "checkpoint: nothing changed");

        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n\nmore\n")?;
        press(&mut app, " vC");
        assert_eq!(last_toast(&app), "checkpoint: 1 file");

        app.open(Path::new("README.md"));
        press(&mut app, " vc");
        assert_eq!(
            last_toast(&app),
            "checkpoint: nothing changed",
            "the file checkpoint follows the same rule"
        );
        let readme = dir.0.join("ws/README.md");
        fs::write(&readme, "# Readme\n\nedited\n")?;
        app.on_changes(vec![readme]);
        press(&mut app, " vc");
        assert_eq!(last_toast(&app), "checkpoint: 1 file");

        let store = store()?;
        assert_eq!(store.events(), 3);
        let readme = store.timeline(Path::new("README.md"));
        assert_eq!(readme.len(), 2);
        assert!(readme[0].is_workspace());
        assert!(!readme[1].is_workspace());
        assert_eq!(store.text(&readme[1])?, "# Readme\n\nedited\n");
        assert_eq!(store.timeline(Path::new("docs/guide.md")).len(), 2);
        assert!(store.timeline(Path::new("logo.bin")).is_empty());
        Ok(())
    }
}
