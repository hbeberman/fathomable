//! Scaffolding for the app's tests: a workspace in a temp dir, an
//! [`App`] open on it, and keys typed into it.
//!
//! [`workspace`] lays out `ws/README.md` beside a `state/` directory for
//! the stores; [`AppBuilder`] opens an `App` there with the defaults
//! most tests want, and [`press`] types a string of plain keys.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::annotations::{
    AgentReplyCommand, Author, LineRange, OriginVersion, PlacementContext, ResolutionOutcome,
    Store, ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::workspace::Workspace;
use fathomable_testing::TempDir;

use crate::app::input::{keys, mouse};
use crate::app::{App, Options};

/// The README most tests read: a heading, three words, and a list.
pub(crate) const README: &str = "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n";

/// A temp dir holding an empty `ws/` workspace and a `state/` directory.
pub(crate) fn bare(name: &str) -> io::Result<TempDir> {
    let dir = TempDir::new(name)?;
    fs::create_dir_all(dir.0.join("ws"))?;
    fs::create_dir_all(dir.0.join("state"))?;
    Ok(dir)
}

/// [`bare`] with `ws/README.md` holding `readme`.
pub(crate) fn workspace(name: &str, readme: &str) -> io::Result<TempDir> {
    let dir = bare(name)?;
    fs::write(root(&dir).join("README.md"), readme)?;
    Ok(dir)
}

/// The workspace root inside a [`bare`] or [`workspace`] dir.
pub(crate) fn root(dir: &TempDir) -> PathBuf {
    dir.0.join("ws")
}

/// The thread store's file inside a [`bare`] or [`workspace`] dir.
pub(crate) fn store_path(dir: &TempDir) -> PathBuf {
    dir.0.join("state/threads.jsonl")
}

/// Append an agent reply through a separate store handle, then let the
/// viewer observe it through its ordinary reload boundary.
pub(crate) fn external_agent_reply(
    app: &mut App,
    id: &ThreadId,
    author: Author,
    body: &str,
    resolve: bool,
    lines: Option<LineRange>,
) -> anyhow::Result<ResolutionOutcome> {
    let root = app.workspace.root().to_path_buf();
    let path = app
        .thread(id)
        .map(|thread| app.thread_path(thread).to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("thread {id}"))?;
    let head = app.workspace.head_commit();
    let mut command = AgentReplyCommand::new(author, now(), body)
        .at_head(head.clone())
        .at_checkout(root.display().to_string())
        .place_in(
            PlacementContext::new(OriginVersion::working_tree(head))
                .at_checkout(root.display().to_string()),
        );
    if resolve {
        command = command.resolve();
    }
    if let Some(lines) = lines {
        command = command.relocate(lines);
    }
    let mut writer = Store::open(app.store_path())?;
    let outcome = writer.agent_reply(id, command, |_| {
        crate::app::threads::read_checkout_text(&root, &path).map_err(|error| {
            fathomable_core::annotations::StoreError::message(format!(
                "cannot read {}: {error}",
                path.display()
            ))
        })
    })?;
    let resolution = *outcome.value();
    app.reload_store();
    Ok(resolution)
}

/// An `App` on the [`workspace`] in `dir`, 100 by 30, with a thread
/// store and `README.md` open in the rendered view.
pub(crate) fn app(dir: &TempDir) -> anyhow::Result<App> {
    AppBuilder::new(dir).build()
}

/// [`app`] switched to the source view, where rows are source lines.
pub(crate) fn source_app(dir: &TempDir) -> anyhow::Result<App> {
    AppBuilder::new(dir).source_view().build()
}

/// Type `keys` one plain character at a time.
pub(crate) fn press(app: &mut App, keys: &str) {
    for ch in keys.chars() {
        keys::handle_key(app, key(ch));
    }
}

/// Press one key with no modifiers.
pub(crate) fn press_key(app: &mut App, code: KeyCode) {
    keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

/// Complete every source highlight the app has queued.
pub(crate) fn complete_highlights(app: &mut App) {
    loop {
        let jobs = app.take_highlight_jobs();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            let highlighted = job.complete();
            app.apply_highlight(highlighted);
        }
    }
}

/// The event for a plain character key.
pub(crate) fn key(ch: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
}

/// A left press at `column`, `row` on the screen.
pub(crate) fn click(app: &mut App, column: usize, row: usize) {
    mouse::handle_mouse(
        app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        },
    );
}

/// An `App` under construction for a test. [`AppBuilder::new`] takes the
/// [`workspace`] layout; [`AppBuilder::at`] any directory as the root.
pub(crate) struct AppBuilder {
    root: PathBuf,
    store: Option<PathBuf>,
    review_points: Option<PathBuf>,
    width: usize,
    height: usize,
    open: Option<PathBuf>,
    source_view: bool,
    options: Option<Box<dyn FnOnce(Options) -> Options>>,
}

impl AppBuilder {
    /// On `dir`'s `ws/` with the thread store at [`store_path`].
    pub(crate) fn new(dir: &TempDir) -> Self {
        Self {
            store: Some(store_path(dir)),
            ..Self::at(root(dir))
        }
    }

    /// On `root` with no store, the way a viewer runs before any thread.
    pub(crate) fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            store: None,
            review_points: None,
            width: 100,
            height: 30,
            open: Some(PathBuf::from("README.md")),
            source_view: false,
            options: None,
        }
    }

    /// A terminal `width` columns wide.
    pub(crate) fn width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    /// Start with nothing open.
    pub(crate) fn unopened(mut self) -> Self {
        self.open = None;
        self
    }

    /// Switch the opened file to the source view.
    pub(crate) fn source_view(mut self) -> Self {
        self.source_view = true;
        self
    }

    /// Open explicit workspace review-point storage at `path`.
    pub(crate) fn review_points(mut self, path: impl Into<PathBuf>) -> Self {
        self.review_points = Some(path.into());
        self
    }

    /// Adjust the options after the builder has filled its own.
    pub(crate) fn options(mut self, adjust: impl FnOnce(Options) -> Options + 'static) -> Self {
        self.options = Some(Box::new(adjust));
        self
    }

    /// Discover the workspace and build the `App`.
    pub(crate) fn build(self) -> anyhow::Result<App> {
        let workspace = Workspace::discover(&self.root)?;
        let mut options = Options::for_test(self.root.clone());
        if let Some(path) = &self.store {
            options.store = Some(Store::open(path)?);
        }
        if let Some(path) = &self.review_points {
            options.review_points = Some(fathomable_core::review_points::ReviewPointStore::open(
                path,
            )?);
        }
        if let Some(adjust) = self.options {
            options = adjust(options);
        }
        let mut app = App::new(workspace, self.width, self.height, options);
        app.settle_status();
        if let Some(path) = &self.open {
            app.open(Path::new(path));
            if self.source_view {
                app.view_mut().toggle_source_view();
            }
        }
        Ok(app)
    }
}

/// The app drawn on a 100×30 test terminal with the default dark theme,
/// as the cell buffer, for a test that reads styles.
pub(crate) fn buffer(app: &App) -> anyhow::Result<ratatui::buffer::Buffer> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
    terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
    Ok(terminal.backend().buffer().clone())
}

/// The app drawn on a 100×30 test terminal, one trimmed string per row.
pub(crate) fn screen(app: &App) -> anyhow::Result<Vec<String>> {
    let buffer = buffer(app)?;
    Ok((0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect())
}
