// @okf-doc: /decisions/0009-cli-and-diagnostics.md
//! Shared diagnostics for `fathomable --doctor` and the in-app Doctor view.

use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use fathomable_core::XdgDirs;
use fathomable_core::annotations::Store;
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
    Warn,
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

    pub(crate) fn has_warnings(&self) -> bool {
        self.lines.iter().any(|line| line.kind == Kind::Warn)
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

    fn warn(&mut self, text: impl Into<String>) {
        self.lines.push(Line {
            kind: Kind::Warn,
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

    report.section("terminal");
    match terminal_size {
        Some((columns, rows)) => report.info(format!("size     {columns}x{rows}")),
        None => match crossterm::terminal::size() {
            Ok((columns, rows)) => report.info(format!("size     {columns}x{rows}")),
            Err(error) => report.info(format!("size     unavailable ({error})")),
        },
    }

    report.section("checks");
    match dirs.shared_state_ancestors() {
        Ok(shared) if shared.is_empty() => {
            report.check(true, "state directory ancestors are not group-writable");
        }
        Ok(shared) => {
            for path in shared {
                report.warn(format!(
                    "state ancestor is group-writable: {}; remove group write (chmod g-w) or choose a private XDG_STATE_HOME",
                    path.display()
                ));
            }
        }
        Err(error) => report.check(false, format!("state directory ancestors: {error}")),
    }
    match log_dir_writable(dirs) {
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
            Kind::Warn => println!("  WARN  {}", line.text),
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
                "{} is not a git work tree; no HEAD diff source",
                workspace.root().display()
            ),
        );
    }

    let (watch_ok, watch) = watch_budget(&mut workspace, &config.watch().ignore);
    report.check(watch_ok, watch);
    match worktrees_line(&workspace) {
        Ok(Some(line)) => report.check(true, line),
        Ok(None) => {}
        Err(error) => report.check(false, format!("worktrees: {error}")),
    }
    let _threads = thread_store(report, dirs, workspace.key());
    let dir = dirs.review_points_dir(workspace.key());
    match fathomable_core::review_points::ReviewPointStore::open_workspace(dirs, workspace.key()) {
        Ok(points) => report.check(
            true,
            format!(
                "{} review point{} ({} bytes) in {}",
                points.len(),
                if points.len() == 1 { "" } else { "s" },
                points.blob_bytes(),
                dir.display()
            ),
        ),
        Err(error) => report.check(false, format!("review points: {error}")),
    }
}

fn thread_store(report: &mut Report, dirs: &XdgDirs, key: &Path) -> Option<Store> {
    let path = dirs.threads_file(key);
    match Store::open_workspace(dirs, key) {
        Ok(store) => {
            let count = store.threads().len();
            report.check(
                true,
                format!(
                    "{count} thread{} in {}",
                    if count == 1 { "" } else { "s" },
                    path.display()
                ),
            );
            Some(store)
        }
        Err(error) => {
            if let Some(mismatch) = error.format_mismatch() {
                report.check(
                    false,
                    format!(
                        "incompatible thread storage versions: {} on disk, {} expected",
                        mismatch.found(),
                        mismatch.expected()
                    ),
                );
                report.info(format!(
                    "recovery  delete {}, then restart Fathomable",
                    path.display()
                ));
            } else {
                report.check(false, format!("thread store {}: {error}", path.display()));
            }
            None
        }
    }
}

fn worktrees_line(
    workspace: &Workspace,
) -> Result<Option<String>, fathomable_core::workspace::WorkspaceError> {
    let worktrees = workspace.worktrees()?;
    if worktrees.len() < 2 {
        return Ok(None);
    }
    Ok(Some(format!(
        "{} worktrees share the state keyed by {}: {}",
        worktrees.len(),
        workspace.key().display(),
        worktrees
            .iter()
            .map(fathomable_core::worktrees::Worktree::label)
            .collect::<Vec<_>>()
            .join(", ")
    )))
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

fn log_dir_writable(dirs: &XdgDirs) -> io::Result<()> {
    let log_dir = dirs.log_dir();
    dirs.prepare_state_dir(&log_dir)?;
    let probe = log_dir.join(format!(".doctor-probe-{}", std::process::id()));
    fathomable_core::private_state::create_new(&probe)?;
    fs::remove_file(&probe)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use fathomable_core::XdgDirs;
    use fathomable_core::workspace::Workspace;
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

    #[test]
    fn incompatible_thread_store_names_versions_and_recovery_file() -> anyhow::Result<()> {
        let dir = TempDir::new("doctor-thread-store")?;
        let root = dir.0.join("workspace");
        fs::create_dir_all(&root)?;
        fs::write(root.join("README.md"), "# test\n")?;
        let xdg = dir.0.join("xdg");
        let dirs = XdgDirs::resolve(|name| Some(xdg.join(name).into_os_string()));
        let workspace = Workspace::discover(&root)?;
        let thread_file = dirs.threads_file(workspace.key());
        dirs.prepare_state_dir(
            thread_file
                .parent()
                .ok_or_else(|| anyhow::anyhow!("thread store path has no parent"))?,
        )?;
        fathomable_core::private_state::write(&thread_file, "{\"v\":3}\n")?;

        let report = collect(&dirs, None, Some(&root), Some((90, 28)));

        assert!(!report.passed());
        assert!(report.lines().iter().any(|line| {
            line.kind == Kind::Fail
                && line.text == "incompatible thread storage versions: 3 on disk, 8 expected"
        }));
        assert!(report.lines().iter().any(|line| {
            line.kind == Kind::Info
                && line.text
                    == format!(
                        "recovery  delete {}, then restart Fathomable",
                        thread_file.display()
                    )
        }));
        Ok(())
    }

    #[test]
    fn shared_state_warning_does_not_fail_and_clears_after_remediation() -> anyhow::Result<()> {
        let dir = TempDir::new("doctor-shared-state")?;
        fs::set_permissions(&dir.0, fs::Permissions::from_mode(0o755))?;
        let root = dir.0.join("workspace");
        fs::create_dir(&root)?;
        fs::write(root.join("README.md"), "# test\n")?;
        let state_home = dir.0.join("shared-state");
        fs::create_dir(&state_home)?;
        fs::set_permissions(&state_home, fs::Permissions::from_mode(0o775))?;
        let dirs = XdgDirs::resolve(|name| {
            (name == "XDG_STATE_HOME").then(|| state_home.clone().into_os_string())
        });

        let warned = collect(&dirs, None, Some(&root), Some((90, 28)));
        assert!(warned.passed());
        assert!(warned.has_warnings());
        assert!(warned.lines().iter().any(|line| {
            line.kind == Kind::Warn
                && line.text.contains(&state_home.display().to_string())
                && line.text.contains("chmod g-w")
                && line.text.contains("private XDG_STATE_HOME")
        }));

        fs::set_permissions(&state_home, fs::Permissions::from_mode(0o755))?;
        let remediated = collect(&dirs, None, Some(&root), Some((90, 28)));
        assert!(remediated.passed());
        assert!(!remediated.has_warnings());
        assert!(remediated.lines().iter().any(|line| {
            line.kind == Kind::Ok && line.text == "state directory ancestors are not group-writable"
        }));
        Ok(())
    }
}
