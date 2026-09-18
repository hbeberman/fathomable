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
            (Some("status"), None, _) => self.open_status(),
            (Some("help"), None, _) => self.open_getting_started(),
            (Some("doctor"), None, _) => self.open_doctor(),
            (Some("licenses"), None, _) => self.open_licenses(),
            (Some("about"), None, _) => self.open_about(),
            (Some("diff"), None, _) => self.toggle_head_diff(),
            (Some("name"), name, None) => self.set_name(name),
            _ => self.notice(format!("not a command: {command}")),
        }
    }

    /// The human-facing viewer label: its name when set, then the id.
    pub(crate) fn viewer_label(&self) -> String {
        match self.record.name() {
            Some(name) => format!("{name} ({})", self.viewer_id),
            None => format!("unnamed ({}); set one with :name", self.viewer_id),
        }
    }

    pub(crate) fn getting_started(&self) -> bool {
        self.getting_started.is_some()
    }

    pub(crate) fn open_getting_started(&mut self) {
        if self.getting_started.is_some() {
            self.close_getting_started();
            return;
        }
        self.park_draft();
        self.popup = None;
        self.getting_started = Some(self.focus);
        self.focus = super::Focus::View;
        self.relayout();
    }

    pub(crate) fn close_getting_started(&mut self) {
        let Some(focus) = self.getting_started.take() else {
            return;
        };
        self.focus = match focus {
            super::Focus::Tree if !self.sidebar.tree => super::Focus::View,
            super::Focus::ThreadsPane if !self.sidebar.threads => super::Focus::View,
            super::Focus::Review if !self.review_list.is_open() => super::Focus::View,
            other => other,
        };
        self.relayout();
    }

    pub(crate) fn open_doctor(&mut self) {
        self.park_draft();
        self.getting_started = None;
        let report = crate::doctor::collect(
            &self.dirs,
            Some(&self.config_path),
            Some(self.workspace.root()),
            Some(self.size()),
        );
        self.popup = Some(super::Popup::Doctor(super::doctor_view::Doctor::new(
            report,
        )));
    }

    pub(crate) fn open_about(&mut self) {
        self.park_draft();
        self.getting_started = None;
        self.popup = Some(super::Popup::About);
    }

    /// The rows of the `:status` overlay: label, value.
    pub(crate) fn status_lines(&self) -> Vec<(String, String)> {
        let view = self.view();
        let (line, column) = view.source_position();
        let base = format!(", {}", self.comparison_label());
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
        let thread_counts = self.review_counts(false);
        vec![
            ("document".to_owned(), document),
            (
                "terminal".to_owned(),
                format!("{} columns x {} rows", self.width, self.height),
            ),
            ("viewer".to_owned(), self.viewer_label()),
            (
                "log".to_owned(),
                crate::logging::log_path(&self.dirs, self.record.id())
                    .display()
                    .to_string(),
            ),
            (
                "workspace".to_owned(),
                self.workspace.root().display().to_string(),
            ),
            ("worktrees".to_owned(), self.worktrees_row()),
            (
                "socket".to_owned(),
                self.record.socket().map_or_else(
                    || "none (XDG_RUNTIME_DIR unset)".to_owned(),
                    |p| p.display().to_string(),
                ),
            ),
            (
                "threads".to_owned(),
                self.store.as_ref().map_or_else(
                    || "unavailable; run :doctor".to_owned(),
                    |store| store.path().display().to_string(),
                ),
            ),
            (
                "watching".to_owned(),
                if self.watching_root {
                    "visible workspace files".to_owned()
                } else {
                    "partial coverage plus the open file".to_owned()
                },
            ),
            ("changes".to_owned(), self.queue.len().to_string()),
            ("proposed".to_owned(), self.proposed_total().to_string()),
            (
                "thread count".to_owned(),
                (thread_counts.active + thread_counts.proposed + thread_counts.resolved)
                    .to_string(),
            ),
        ]
    }
}
