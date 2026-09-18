// @okf-doc: /decisions/0065-z-folds-and-unfolds.md
//! `z`, `Z`, and Enter on a thread heading fold and unfold threads in
//! the text (ADR 0065).
//!
//! `z` is the one key that only opens and closes: on an expanded
//! thread's rows it folds the thread back to its stub, and on a row a
//! thread covers it expands the thread cursor's thread in place. `Z`
//! does it to the whole file: every stub expands, or, when any thread
//! is expanded, every one folds. Enter toggles only while the text
//! cursor rests on an expanded header or its folded stub; it has no
//! key-bar hint. `c` starts a comment and never changes thread
//! expansion.

use fathomable_core::annotations::ThreadId;

use crate::app::App;

impl App {
    /// Enter: fold an expanded header under the cursor, or unfold its
    /// stub in place. Source and message rows do nothing.
    pub(crate) fn toggle_thread_header(&mut self) {
        let row = self.view().cursor().row;
        let Some((stub, index, _)) = self.stub_on_row(row) else {
            return;
        };
        let Some(id) = stub.thread().cloned() else {
            return;
        };
        if stub.expanded() {
            if index == 0 {
                self.fold_thread(&id);
            }
        } else {
            self.expand_thread(id);
        }
    }

    /// `z`: fold the expanded thread the cursor is on, else the thread
    /// cursor's thread when the cursor line has one: fold it when it is
    /// expanded, expand it when it is a stub.
    pub(crate) fn toggle_thread_here(&mut self) {
        let row = self.view().cursor().row;
        let id = match self.expanded_row_message(row) {
            Some((id, _)) => Some(id),
            None if self.threads_at_cursor().is_empty() => None,
            None => self.thread_cursor().thread().cloned(),
        };
        let Some(id) = id else {
            return;
        };
        if self.is_expanded(&id) {
            self.fold_thread(&id);
        } else {
            self.expand_thread(id);
        }
    }

    /// `Z`: expand every stub in the file, or fold every expanded
    /// thread when any is.
    pub(crate) fn toggle_expand_all(&mut self) {
        let ids: Vec<ThreadId> = self
            .stubs()
            .iter()
            .filter_map(|stub| stub.thread().cloned())
            .collect();
        if ids.iter().any(|id| self.is_expanded(id)) {
            for id in &ids {
                self.fold_thread(id);
            }
        } else {
            for id in ids {
                self.expand_thread(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::app::testing::{self, press, screen, source_app};

    /// `z` on a stub's line expands the thread, `z` on its rows folds
    /// it, and the hints name `z`; `Z` opens every thread in the file
    /// and closes them all again.
    #[test]
    fn z_toggles_one_thread_and_shift_z_the_file() -> anyhow::Result<()> {
        let dir = testing::workspace("fold-z", testing::README)?;
        let mut app = source_app(&dir)?;
        for (line, text) in [(3, "first"), (5, "second")] {
            app.view_mut().goto_source_line(line);
            app.start_new_comment();
            app.compose_insert(text);
            app.compose_submit();
        }
        let ids = app.file_threads();
        for id in &ids {
            app.fold_thread(id);
        }
        assert_eq!(app.current_path(), Path::new("README.md"));

        app.view_mut().goto_source_line(3);
        let first = app
            .thread_cursor()
            .thread()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("the thread cursor"))?;
        // The bar names the key (ADR 0067).
        assert!(screen(&app)?.iter().any(|row| row.contains("expand z")));
        press(&mut app, "z");
        assert!(app.is_expanded(&first), "`z` on the stub's line expands it");
        assert!(screen(&app)?.iter().any(|row| row.contains("fold z")));
        press(&mut app, "z");
        assert!(!app.is_expanded(&first), "`z` on its rows folds it");

        // `z` on a line no thread covers does nothing.
        app.view_mut().goto_source_line(1);
        press(&mut app, "z");
        assert!(ids.iter().all(|id| !app.is_expanded(id)));

        press(&mut app, "Z");
        assert!(
            ids.iter().all(|id| app.is_expanded(id)),
            "`Z` expands every thread"
        );
        app.fold_thread(&first);
        press(&mut app, "Z");
        assert!(
            ids.iter().all(|id| !app.is_expanded(id)),
            "`Z` folds them all while any is expanded"
        );
        press(&mut app, "Z");
        assert!(ids.iter().all(|id| app.is_expanded(id)));
        Ok(())
    }
}
