// @okf-doc: /decisions/0021-polish-pass.md
//! The `:` command registry, app-level command handling, and `:status` overlay.
//!
//! The registry is the source of truth for parsing, fuzzy completion, aliases,
//! and user-visible descriptions. `View::execute` keeps `:quit` and numeric
//! jumps; everything else arrives as
//! [`Effect::Command`](crate::app::view::Effect::Command).

use super::App;
use fathomable_core::config::DiffMode;

/// Behavior attached to one registered command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    About,
    Doctor,
    Help,
    Mcp,
    Quit,
    Status,
}

/// Canonical command metadata used by execution and completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandSpec {
    form: &'static str,
    aliases: &'static [&'static str],
    description: &'static str,
    command: Command,
}

impl CommandSpec {
    const fn new(
        form: &'static str,
        aliases: &'static [&'static str],
        description: &'static str,
        command: Command,
    ) -> Self {
        Self {
            form,
            aliases,
            description,
            command,
        }
    }

    pub(crate) const fn form(self) -> &'static str {
        self.form
    }

    pub(crate) const fn aliases(self) -> &'static [&'static str] {
        self.aliases
    }

    pub(crate) const fn description(self) -> &'static str {
        self.description
    }

    pub(crate) const fn command(self) -> Command {
        self.command
    }
}

/// Direction in which command completion moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionDirection {
    Next,
    Previous,
}

/// The matches frozen when command completion starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandCompletion {
    matches: Vec<CommandSpec>,
    selected: Option<usize>,
}

impl CommandCompletion {
    pub(crate) fn new(query: &str) -> Self {
        Self {
            matches: matching_commands(query),
            selected: None,
        }
    }

    pub(crate) fn candidates(&self) -> &[CommandSpec] {
        &self.matches
    }

    pub(crate) const fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub(crate) fn selected(&self) -> Option<CommandSpec> {
        self.selected
            .and_then(|index| self.matches.get(index).copied())
    }

    pub(crate) fn select(&mut self, direction: CompletionDirection) -> Option<CommandSpec> {
        if self.matches.is_empty() {
            return None;
        }
        let last = self.matches.len() - 1;
        self.selected = Some(match (self.selected, direction) {
            (None, CompletionDirection::Next) => 0,
            (None | Some(0), CompletionDirection::Previous) => last,
            (Some(index), CompletionDirection::Next) => (index + 1) % self.matches.len(),
            (Some(index), CompletionDirection::Previous) => index - 1,
        });
        self.selected()
    }
}

const COMMANDS: [CommandSpec; 6] = [
    CommandSpec::new(
        "about",
        &[],
        "Show Fathomable version, license, and repository information.",
        Command::About,
    ),
    CommandSpec::new(
        "doctor",
        &[],
        "Inspect configuration, storage, workspace, and terminal diagnostics.",
        Command::Doctor,
    ),
    CommandSpec::new(
        "help",
        &[],
        "Open or close the getting-started guide.",
        Command::Help,
    ),
    CommandSpec::new(
        "mcp",
        &[],
        "Show setup steps for connecting an agent over MCP.",
        Command::Mcp,
    ),
    CommandSpec::new(
        "quit",
        &["q", "q!", "quit!"],
        "Quit immediately without confirmation. Aliases: q, q!, quit!.",
        Command::Quit,
    ),
    CommandSpec::new(
        "status",
        &[],
        "Show live viewer, workspace, and storage status.",
        Command::Status,
    ),
];

pub(crate) fn find_command(input: &str) -> Option<CommandSpec> {
    COMMANDS
        .iter()
        .copied()
        .find(|spec| spec.form() == input || spec.aliases().contains(&input))
}

fn matching_commands(query: &str) -> Vec<CommandSpec> {
    if query.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    if query.is_empty() {
        return COMMANDS.to_vec();
    }

    let query = query.to_ascii_lowercase();
    let mut matches: Vec<_> = COMMANDS
        .iter()
        .copied()
        .enumerate()
        .filter_map(|(index, command)| {
            best_rank(command, &query).map(|rank| (rank, index, command))
        })
        .collect();
    matches.sort_by_key(|(rank, index, _)| (*rank, *index));
    matches.into_iter().map(|(_, _, command)| command).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MatchRank {
    prefix_penalty: u8,
    span: usize,
    start: usize,
    candidate_len: usize,
}

fn best_rank(command: CommandSpec, query: &str) -> Option<MatchRank> {
    std::iter::once(command.form())
        .chain(command.aliases().iter().copied())
        .filter_map(|candidate| match_rank(candidate, query))
        .min()
}

fn match_rank(candidate: &str, query: &str) -> Option<MatchRank> {
    let candidate = candidate.to_ascii_lowercase();
    let bytes = candidate.as_bytes();
    let mut from = 0;
    let mut first = None;
    let mut last = 0;
    for wanted in query.bytes() {
        let offset = bytes.get(from..)?.iter().position(|byte| *byte == wanted)?;
        let position = from + offset;
        first.get_or_insert(position);
        last = position;
        from = position + 1;
    }
    let start = first?;
    Some(MatchRank {
        prefix_penalty: u8::from(!candidate.starts_with(query)),
        span: last - start + 1,
        start,
        candidate_len: candidate.len(),
    })
}

impl App {
    /// Run a `:` command the view did not handle itself.
    pub(crate) fn command(&mut self, input: &str) {
        match find_command(input).map(CommandSpec::command) {
            Some(Command::Status) => self.open_status(),
            Some(Command::Help) => self.open_getting_started(),
            Some(Command::Doctor) => self.open_doctor(),
            Some(Command::About) => self.open_about(),
            Some(Command::Mcp) => self.open_mcp_setup(),
            Some(Command::Quit) | None => {
                self.notice(format!("not a command: {input}"));
            }
        }
    }

    /// The opaque ID of this running viewer.
    pub(crate) fn viewer_label(&self) -> String {
        self.viewer_id.clone()
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
        let provenance = if self.diff_mode() == DiffMode::Off {
            format!(
                ", diff mode off, Target only: {}",
                self.comparison_target_label()
            )
        } else {
            format!(", {}", self.comparison_label())
        };
        let deleted = if self.deleted() { ", deleted" } else { "" };
        let document = if self.has_document() {
            format!(
                "{} ({}{provenance}{deleted}) at {line}:{column}",
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
        let mut lines = vec![
            ("document".to_owned(), document),
            ("diff mode".to_owned(), self.diff_mode().to_string()),
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
                "threads".to_owned(),
                self.store.as_ref().map_or_else(
                    || "unavailable; run :doctor".to_owned(),
                    |store| store.path().display().to_string(),
                ),
            ),
            (
                "thread updates".to_owned(),
                self.thread_updates_degraded.as_ref().map_or_else(
                    || "watching exact store".to_owned(),
                    |reason| format!("degraded: {reason}"),
                ),
            ),
            ("watching".to_owned(), self.watch_status.label()),
            ("proposed".to_owned(), self.proposed_total().to_string()),
            (
                "thread count".to_owned(),
                (thread_counts.active + thread_counts.proposed + thread_counts.resolved)
                    .to_string(),
            ),
        ];
        if let Some(reason) = &self.tree_issue {
            lines.push(("file discovery".to_owned(), reason.clone()));
        } else if self
            .tree
            .as_ref()
            .is_some_and(fathomable_core::tree::Tree::discovery_limited)
        {
            lines.push((
                "file discovery".to_owned(),
                "limited by retained-path budget; coverage is incomplete".to_owned(),
            ));
        }
        if let Some(reason) = self.comparison.error() {
            lines.push(("comparison coverage".to_owned(), reason.to_owned()));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use fathomable_core::config::DiffMode;

    use crate::app::testing;
    use crate::app::watch::{LimitReason, WatchStatus};

    use super::{Command, CommandCompletion, CompletionDirection, find_command, matching_commands};

    #[test]
    fn registry_lists_every_canonical_command_and_alias() {
        let commands = matching_commands("");
        assert_eq!(
            commands
                .iter()
                .map(|command| command.form())
                .collect::<Vec<_>>(),
            ["about", "doctor", "help", "mcp", "quit", "status"]
        );
        for removed in ["licenses", "nohlsearch", "noh", "source"] {
            assert_eq!(find_command(removed), None, "{removed}");
        }

        for alias in ["q", "q!", "quit", "quit!"] {
            assert_eq!(
                find_command(alias).map(super::CommandSpec::command),
                Some(Command::Quit),
                "{alias}"
            );
        }

        assert!(
            commands
                .iter()
                .all(|command| !command.description().contains("Args:"))
        );
    }

    #[test]
    fn status_keeps_limited_workspace_coverage_visible() -> anyhow::Result<()> {
        let dir = testing::workspace("watch-status-limited", testing::README)?;
        let mut app = testing::app(&dir)?;
        app.set_watch_status(WatchStatus::Limited {
            watched: 8,
            examined: 21,
            reason: LimitReason::WorkspaceWatches,
        });

        let watching = app
            .status_lines()
            .into_iter()
            .find_map(|(label, value)| (label == "watching").then_some(value))
            .ok_or_else(|| anyhow::anyhow!("watching status row"))?;
        assert_eq!(
            watching,
            "limited: workspace watch limit reached; raise limits.workspace-watches in config (8 watched, 21 entries examined)"
        );
        Ok(())
    }

    #[test]
    fn matching_is_case_insensitive_subsequence_with_prefixes_first() {
        let forms = |query| {
            matching_commands(query)
                .iter()
                .map(|command| command.form())
                .collect::<Vec<_>>()
        };
        assert_eq!(forms("ST"), ["status"]);
        assert_eq!(forms("dcr"), ["doctor"]);
        assert_eq!(forms("q"), ["quit"]);
        assert_eq!(forms("s"), ["status"]);
        assert!(forms("status now").is_empty());
    }

    #[test]
    fn completion_cycles_the_frozen_match_set() {
        let mut completion = CommandCompletion::new("t");
        assert_eq!(completion.selected_index(), None);
        assert_eq!(
            completion
                .select(CompletionDirection::Next)
                .map(super::CommandSpec::form),
            Some("status")
        );
        assert_eq!(
            completion
                .select(CompletionDirection::Next)
                .map(super::CommandSpec::form),
            Some("quit")
        );
        assert_eq!(
            completion
                .select(CompletionDirection::Previous)
                .map(super::CommandSpec::form),
            Some("status")
        );
    }

    #[test]
    fn off_status_is_explicitly_target_only() -> anyhow::Result<()> {
        let dir = testing::workspace("status-diff-off", testing::README)?;
        let mut app = testing::app(&dir)?;
        assert!(
            app.status_lines()
                .iter()
                .all(|(label, _)| label != "changes")
        );
        app.select_diff_mode(DiffMode::Off);
        app.settle_background();

        let rows = app.status_lines();
        let document = rows
            .iter()
            .find(|(label, _)| label == "document")
            .map(|(_, value)| value)
            .ok_or_else(|| anyhow::anyhow!("document status row"))?;
        assert!(document.contains("diff mode off, Target only: WorkingTree"));
        assert!(!document.contains("Compare:"));
        assert_eq!(
            rows.iter()
                .find(|(label, _)| label == "diff mode")
                .map(|(_, value)| value.as_str()),
            Some("off")
        );
        assert!(rows.iter().all(|(label, _)| label != "changes"));
        Ok(())
    }
}
