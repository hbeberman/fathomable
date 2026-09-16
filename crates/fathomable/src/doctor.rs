// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! Shared diagnostics for `fathomable --doctor` and the in-app Doctor view.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;
use fathomable_core::config::Config;
use fathomable_core::highlight::{self, Highlighter};
use fathomable_core::session::Record;
use fathomable_core::theme::Theme;
use fathomable_core::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Section,
    Info,
    Ok,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) kind: Kind,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    lines: Vec<Line>,
    ok: bool,
}

impl Report {
    pub(crate) fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub(crate) const fn passed(&self) -> bool {
        self.ok
    }

    fn section(&mut self, text: impl Into<String>) {
        self.lines.push(Line {
            kind: Kind::Section,
            text: text.into(),
        });
    }

    fn info(&mut self, text: impl Into<String>) {
        self.lines.push(Line {
            kind: Kind::Info,
            text: text.into(),
        });
    }

    fn check(&mut self, ok: bool, text: impl Into<String>) {
        self.ok &= ok;
        self.lines.push(Line {
            kind: if ok { Kind::Ok } else { Kind::Fail },
            text: text.into(),
        });
    }
}

/// Collect the same report for the CLI and TUI. The TUI supplies its
/// current terminal size and workspace; the CLI probes both.
pub(crate) fn collect(
    dirs: &XdgDirs,
    config_path: Option<&Path>,
    workspace_path: Option<&Path>,
    terminal_size: Option<(usize, usize)>,
) -> Report {
    let mut report = Report {
        lines: Vec::new(),
        ok: true,
    };
    report.info(format!("fathomable {}", env!("CARGO_PKG_VERSION")));
    report.section("directories");
    report.info(format!("config   {}", dirs.config_dir().display()));
    report.info(format!("state    {}", dirs.state_dir().display()));
    report.info(format!("log      {}", dirs.log_dir().display()));
    report.info(format!("themes   {}", dirs.themes_dir().display()));
    report.info(format!("viewers  {}", dirs.viewers_dir().display()));
    match dirs.runtime_dir() {
        Some(runtime) => report.info(format!("runtime  {}", runtime.display())),
        None => report.info("runtime  unset (XDG_RUNTIME_DIR); the viewer socket needs it"),
    }

    report.section("terminal");
    match terminal_size {
        Some((columns, rows)) => report.info(format!("size     {columns}x{rows}")),
        None => match crossterm::terminal::size() {
            Ok((columns, rows)) => report.info(format!("size     {columns}x{rows}")),
            Err(error) => report.info(format!("size     unavailable ({error})")),
        },
    }

    report.section("checks");
    match log_dir_writable(&dirs.log_dir()) {
        Ok(()) => report.check(true, "log directory is writable"),
        Err(error) => report.check(false, format!("log directory is not writable: {error}")),
    }
    let crashes = crash_reports(&dirs.log_dir());
    report.check(
        true,
        if crashes == 0 {
            "no crash reports".to_owned()
        } else {
            format!(
                "{crashes} crash report{} in {} (`.crash`, newest last)",
                if crashes == 1 { "" } else { "s" },
                dirs.log_dir().display()
            )
        },
    );

    let config = match Config::load(dirs, config_path) {
        Ok(config) => {
            report.check(true, "config parsed");
            config
        }
        Err(error) => {
            report.check(false, format!("config: {error}"));
            Config::default()
        }
    };
    report.check(
        true,
        format!(
            "auto-jump {}",
            if config.jump().auto { "on" } else { "off" }
        ),
    );
    let theme_name = config.theme().to_owned();
    match Theme::load(&theme_name, dirs) {
        Ok(theme) => {
            report.check(true, format!("theme `{}` loaded", theme.name()));
            match Highlighter::new(theme.syntect()) {
                Ok(highlighter) if highlighter.is_enabled() => report.check(
                    true,
                    format!("code highlighting with syntect theme `{}`", theme.syntect()),
                ),
                Ok(_) => report.check(true, "code highlighting off (code.syntect unset)"),
                Err(error) => report.check(false, error.to_string()),
            }
            report.check(
                true,
                format!(
                    "bundled syntect themes: {}",
                    highlight::theme_names().join(", ")
                ),
            );
        }
        Err(error) => report.check(false, error.to_string()),
    }

    workspace_checks(&mut report, dirs, &config, workspace_path);
    let records = Record::list(dirs);
    let live = records.iter().filter(|record| record.is_alive()).count();
    report.check(
        true,
        format!(
            "{live} live viewer{} ({} record{} on disk)",
            if live == 1 { "" } else { "s" },
            records.len(),
            if records.len() == 1 { "" } else { "s" }
        ),
    );
    report
}

/// Print diagnostics. Exit status is failure when any check fails.
pub(crate) fn run(dirs: &XdgDirs) -> ExitCode {
    let report = collect(dirs, None, None, None);
    let mut first = true;
    for line in report.lines() {
        match line.kind {
            Kind::Section => {
                if !first {
                    println!();
                }
                println!("{}", line.text);
            }
            Kind::Info => println!("{}", line.text),
            Kind::Ok => println!("  ok    {}", line.text),
            Kind::Fail => println!("  FAIL  {}", line.text),
        }
        first = false;
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the report keeps the ordered workspace checks together"
)]
fn workspace_checks(
    report: &mut Report,
    dirs: &XdgDirs,
    config: &Config,
    workspace_path: Option<&Path>,
) {
    let discovered = workspace_path.map_or_else(
        || {
            std::env::current_dir()
                .map_err(|error| error.to_string())
                .and_then(|cwd| Workspace::discover(&cwd).map_err(|error| error.to_string()))
        },
        |path| Workspace::discover(path).map_err(|error| error.to_string()),
    );
    let Ok(mut workspace) = discovered else {
        let error = discovered
            .err()
            .unwrap_or_else(|| "unknown error".to_owned());
        report.check(false, format!("workspace: {error}"));
        return;
    };
    if workspace.is_git() {
        match workspace.head_text(Path::new(".fathomable-doctor-probe")) {
            Ok(_) => report.check(
                true,
                format!(
                    "git work tree at {} (HEAD readable)",
                    workspace.root().display()
                ),
            ),
            Err(error) => report.check(false, format!("git: {error}")),
        }
        match workspace.status() {
            Ok(status) => report.check(
                true,
                format!(
                    "{} uncommitted path{} ({} staged)",
                    status.len(),
                    if status.len() == 1 { "" } else { "s" },
                    status
                        .entries()
                        .iter()
                        .filter(|entry| entry.is_staged())
                        .count()
                ),
            ),
            Err(error) => report.check(false, format!("git status: {error}")),
        }
    } else {
        report.check(
            true,
            format!(
                "{} is not a git work tree; no HEAD diff base",
                workspace.root().display()
            ),
        );
    }

    let (watch_ok, watch) = watch_budget(&mut workspace, &config.watch().ignore);
    report.check(watch_ok, watch);
    if let Some(line) = worktrees_line(&workspace) {
        report.check(true, line);
    }
    if let Some(socket) = dirs.viewer_socket(workspace.key(), std::process::id()) {
        let bytes = socket.as_os_str().len();
        if fathomable_core::socket_path_fits(&socket) {
            report.check(true, format!("viewer socket path fits ({bytes} bytes)"));
        } else {
            report.check(
                false,
                format!(
                    "viewer socket path is {bytes} bytes; a Unix socket path holds at most {} (shorten XDG_RUNTIME_DIR): {}",
                    fathomable_core::SOCKET_PATH_MAX,
                    socket.display()
                ),
            );
        }
    }

    let dir = dirs.seen_dir(workspace.key());
    let threads =
        fathomable_core::annotations::Store::open(dirs.threads_file(workspace.key())).ok();
    let pinned = threads
        .iter()
        .flat_map(fathomable_core::annotations::Store::open_paths);
    match fathomable_core::seen::Store::open_pinned(&dir, pinned) {
        Ok(seen) => report.check(
            true,
            format!(
                "{} last-seen snapshot{} ({} bytes) in {}",
                seen.len(),
                if seen.len() == 1 { "" } else { "s" },
                seen.blob_bytes(),
                dir.display()
            ),
        ),
        Err(error) => report.check(false, format!("snapshots: {error}")),
    }
    let dir = dirs.checkpoints_dir(workspace.key());
    match fathomable_core::checkpoints::Store::open(&dir) {
        Ok(checkpoints) => report.check(
            true,
            format!(
                "{} checkpoint{} over {} file{} ({} bytes) in {}",
                checkpoints.events(),
                if checkpoints.events() == 1 { "" } else { "s" },
                checkpoints.len(),
                if checkpoints.len() == 1 { "" } else { "s" },
                checkpoints.blob_bytes(),
                dir.display()
            ),
        ),
        Err(error) => report.check(false, format!("checkpoints: {error}")),
    }
}

fn worktrees_line(workspace: &Workspace) -> Option<String> {
    let worktrees = workspace.worktrees();
    if worktrees.len() < 2 {
        return None;
    }
    Some(format!(
        "{} worktrees share the state keyed by {}: {}",
        worktrees.len(),
        workspace.key().display(),
        worktrees
            .iter()
            .map(fathomable_core::worktrees::Worktree::label)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn watch_budget(workspace: &mut Workspace, extra_ignores: &[String]) -> (bool, String) {
    use fathomable_core::follow::Ignore;

    let ignore = Ignore::new(extra_ignores).unwrap_or_default();
    let root = workspace.root().to_path_buf();
    let mut dirs = 0;
    let mut pending = vec![Path::new("").to_path_buf()];
    while let Some(relative) = pending.pop() {
        if !relative.as_os_str().is_empty() && ignore.is_tree_ignored(&relative) {
            continue;
        }
        dirs += 1;
        let Ok(entries) = workspace.list_dir(&relative) else {
            continue;
        };
        pending.extend(
            entries
                .into_iter()
                .filter(|entry| entry.is_dir() && !entry.is_symlink())
                .map(|entry| relative.join(entry.name()))
                .filter(|path| !ignore.is_tree_ignored(path)),
        );
    }
    let summary = format!(
        "{dirs} visible director{} to watch under {} (ignored trees skipped)",
        if dirs == 1 { "y" } else { "ies" },
        root.display()
    );
    let budget = fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()
        .and_then(|text| text.trim().parse::<usize>().ok());
    match budget {
        Some(max) if dirs < max => (true, format!("{summary}; inotify allows {max} per user")),
        Some(max) => (
            false,
            format!(
                "{summary}, but inotify allows {max} per user: raise fs.inotify.max_user_watches, or live updates have partial coverage"
            ),
        ),
        None => (
            true,
            format!("{summary}; the inotify budget is unknown here"),
        ),
    }
}

fn crash_reports(log_dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(log_dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "crash")
        })
        .count()
}

fn log_dir_writable(log_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(log_dir)?;
    let probe = log_dir.join(format!(".doctor-probe-{}", std::process::id()));
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use fathomable_core::XdgDirs;
    use fathomable_testing::TempDir;

    use super::{Kind, collect};

    #[test]
    fn one_report_backs_cli_and_in_app_diagnostics() -> anyhow::Result<()> {
        let dir = TempDir::new("doctor-report")?;
        let root = dir.0.join("workspace");
        fs::create_dir_all(&root)?;
        fs::write(root.join("README.md"), "# test\n")?;
        let xdg = dir.0.join("xdg");
        let dirs = XdgDirs::resolve(|name| Some(xdg.join(name).into_os_string()));
        let report = collect(&dirs, None, Some(&root), Some((90, 28)));
        assert!(
            report
                .lines()
                .iter()
                .any(|line| line.kind == Kind::Info && line.text == "size     90x28")
        );
        assert!(report.lines().iter().any(|line| {
            line.kind == Kind::Ok && line.text.contains("log directory is writable")
        }));
        assert!(
            report
                .lines()
                .iter()
                .any(|line| line.text.contains("is not a git work tree"))
        );
        Ok(())
    }
}
