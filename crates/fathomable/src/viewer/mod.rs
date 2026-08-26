// @okf-doc: /decisions/0010-viewer-ux.md
//! The single-file Markdown viewer: terminal setup, event loop, live reload.

mod clipboard;
mod keys;
mod ui;
mod view;

use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fathomable_core::Document;
use notify::{RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use view::{Effect, View};

/// How long to wait after a change notification before re-reading, so an
/// editor's write-then-rename lands as one reload.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(40);

/// Run the viewer on `path` until the user quits.
pub fn run(
    path: &Path,
    session_id: &str,
    theme: &fathomable_core::theme::Theme,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(run_async(path, session_id, theme))
}

/// Restores the terminal on drop so a panic or error never leaves raw mode on.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("cannot enable raw mode")?;
        crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
            .context("cannot enter alternate screen")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

async fn run_async(
    path: &Path,
    session_id: &str,
    theme: &fathomable_core::theme::Theme,
) -> anyhow::Result<()> {
    let mut document = Document::load(path)?;
    let display_path = path.display().to_string();

    let (input_tx, mut input_rx) = mpsc::channel::<io::Result<Event>>(64);
    thread::Builder::new()
        .name("input".to_owned())
        .spawn(move || {
            loop {
                let event = crossterm::event::read();
                let failed = event.is_err();
                if input_tx.blocking_send(event).is_err() || failed {
                    break;
                }
            }
        })
        .context("cannot start input thread")?;

    let (reload_tx, mut reload_rx) = mpsc::unbounded_channel::<()>();
    let target_path: PathBuf = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let watch_dir = target_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let target = target_path.clone();
    let mut watcher =
        notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
            Ok(event) if event.paths.iter().any(|p| p == &target) => {
                let _ = reload_tx.send(());
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "file watcher error"),
        })
        .context("cannot create file watcher")?;
    watcher
        .watch(&watch_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("cannot watch {}", watch_dir.display()))?;

    let _guard = TerminalGuard::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("cannot initialise terminal")?;
    let theme = ui::Theme::from_core(theme);
    let status = ui::StatusInfo {
        path: &display_path,
        session: session_id,
    };

    let size = terminal.size().context("cannot read terminal size")?;
    let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
    let mut view = View::new(document.text().to_owned(), 0, ui::text_rows(area));
    let gutter = ui::gutter_width(&view);
    view.resize(
        usize::from(size.width).saturating_sub(gutter),
        ui::text_rows(area),
    );
    tracing::info!(path = %display_path, lines = view.layout().lines().len(), "viewer opened");

    loop {
        terminal
            .draw(|frame| ui::draw(frame, &view, &theme, &status))
            .context("draw failed")?;
        let effect = tokio::select! {
            event = input_rx.recv() => match event {
                Some(Ok(event)) => handle_event(&mut view, &event, &terminal)?,
                Some(Err(error)) => return Err(error).context("reading terminal input"),
                None => Effect::Quit,
            },
            notice = reload_rx.recv() => {
                if notice.is_none() {
                    Effect::None
                } else {
                    tokio::time::sleep(RELOAD_DEBOUNCE).await;
                    while reload_rx.try_recv().is_ok() {}
                    match document.reload() {
                        Ok(true) => {
                            tracing::info!(path = %display_path, "reloaded after change");
                            view.reload(document.text().to_owned());
                        }
                        Ok(false) => {}
                        Err(error) => tracing::warn!(%error, "reload failed; keeping previous text"),
                    }
                    Effect::None
                }
            }
        };
        match effect {
            Effect::None => {}
            Effect::Quit => break,
            Effect::Copy(text) => {
                tracing::debug!(bytes = text.len(), "copied selection via OSC 52");
                clipboard::copy(&text).context("cannot write to clipboard")?;
            }
        }
    }
    tracing::info!("viewer closed");
    Ok(())
}

fn handle_event(
    view: &mut View,
    event: &Event,
    terminal: &Terminal<CrosstermBackend<io::Stdout>>,
) -> anyhow::Result<Effect> {
    let area = terminal.size().context("cannot read terminal size")?;
    let rows = usize::from(area.height.saturating_sub(1));
    let gutter = ui::gutter_width(view);
    Ok(match event {
        Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
            keys::handle_key(view, *key)
        }
        Event::Mouse(mouse) => keys::handle_mouse(view, *mouse, gutter, rows),
        Event::Resize(width, height) => {
            let rows = usize::from(height.saturating_sub(1));
            view.resize(usize::from(*width).saturating_sub(gutter), rows);
            Effect::None
        }
        _ => Effect::None,
    })
}
