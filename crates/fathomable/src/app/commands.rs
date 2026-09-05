// @okf-doc: /decisions/0021-polish-pass.md
//! The `:` commands the view hands up to the app, and the `:status` overlay.
//!
//! `View::execute` keeps the commands that only touch the pane (`:q`,
//! `:noh`, `:source`, `:N`); everything else arrives here as
//! [`Effect::Command`](crate::app::view::Effect::Command).

use super::App;

impl App {
    /// Run a `:` command the view did not handle itself.
    pub(crate) fn command(&mut self, command: &str) {
        let mut words = command.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("auto"), None, _) => self.toggle_auto_jump(),
            (Some("auto"), Some("on"), None) => self.set_auto_jump(true),
            (Some("auto"), Some("off"), None) => self.set_auto_jump(false),
            (Some("status"), None, _) => self.open_status(),
            (Some("diff"), None, _) => self.toggle_head_diff(),
            (Some("diff"), Some("seen"), None) => self.toggle_seen_diff(),
            (Some("name"), name, None) => self.set_name(name),
            _ => self.notice(format!("not a command: {command}")),
        }
    }

    /// The viewer as agents see it: its name when set, then the id.
    pub(crate) fn viewer_label(&self) -> String {
        match self.record.name() {
            Some(name) => format!("{name} ({})", self.viewer_id),
            None => format!("unnamed ({}); set one with :name", self.viewer_id),
        }
    }

    /// The `subscribers` row of `:status` (ADR 0040).
    fn subscriber_row(&self) -> String {
        match self.subscribers().as_slice() {
            [] => "none".to_owned(),
            all => all
                .iter()
                .map(|s| format!("{} {}", s.label(), s.id()))
                .collect::<Vec<_>>()
                .join("; "),
        }
    }

    /// The rows of the `:status` overlay: label, value.
    pub(crate) fn status_lines(&self) -> Vec<(String, String)> {
        let unavailable = || "unavailable (see the log)".to_owned();
        let view = self.view();
        let (line, column) = view.source_position();
        let base = view
            .diff()
            .map(|d| format!(", diff {}", d.header))
            .unwrap_or_default();
        let deleted = if self.deleted() { ", deleted" } else { "" };
        let document = if self.has_document() {
            format!(
                "{} ({}{base}{deleted}) at {line}:{column}",
                self.current_path().display(),
                if view.source_view() {
                    "source"
                } else {
                    "rendered"
                },
            )
        } else {
            "none (the welcome screen)".to_owned()
        };
        let subscribers = self.subscriber_row();
        vec![
            ("document".to_owned(), document),
            (
                "terminal".to_owned(),
                format!("{} columns x {} rows", self.width, self.height),
            ),
            ("viewer".to_owned(), self.viewer_label()),
            (
                "workspace".to_owned(),
                self.workspace.root().display().to_string(),
            ),
            (
                "socket".to_owned(),
                self.record.socket().map_or_else(
                    || "none (XDG_RUNTIME_DIR unset)".to_owned(),
                    |p| p.display().to_string(),
                ),
            ),
            (
                "threads".to_owned(),
                self.store
                    .as_ref()
                    .map_or_else(unavailable, |s| s.path().display().to_string()),
            ),
            (
                "snapshots".to_owned(),
                self.seen
                    .as_ref()
                    .map_or_else(unavailable, |s| s.dir().display().to_string()),
            ),
            (
                "watching".to_owned(),
                if self.watching_root {
                    "the whole workspace".to_owned()
                } else {
                    "the open file's directory only".to_owned()
                },
            ),
            (
                "auto-jump".to_owned(),
                if self.auto { "on" } else { "off" }.to_owned(),
            ),
            ("changes".to_owned(), self.queue.len().to_string()),
            ("proposed".to_owned(), self.proposed_total().to_string()),
            ("waiting".to_owned(), self.waiting_total().to_string()),
            ("subscribers".to_owned(), subscribers),
        ]
    }
}
