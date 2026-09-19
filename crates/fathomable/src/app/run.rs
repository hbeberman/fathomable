// @okf-doc: /decisions/0012-workspace-mode.md
//! The viewer's main loop: the terminal, the tokio loop, the watcher and
//! watcher wiring, and the effects a key can ask the loop to perform.
//!
//! [`run`] owns the terminal for the life of the viewer; everything the
//! loop hands the app goes through [`App`] in the parent module, so the
//! app itself is tested without a terminal (ADR 0012).

use std::fs;
use std::io::{self, Write};
use std::ops::ControlFlow;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::cursor::{Hide, SetCursorStyle, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use fathomable_core::theme::Theme;
use fathomable_core::workspace::Workspace;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::crash;

use super::view::Effect;
use super::{App, Options, clipboard, highlight, input, watch};

mod draft;
#[cfg(test)]
mod terminal_tests;

/// Run the app until the user quits, showing `open` first when given,
/// else the tree (ADR 0012).
pub(crate) fn run(
    workspace: Workspace,
    options: Options,
    theme: &Theme,
    open: Option<&Path>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(run_async(workspace, options, theme, open))
}

/// Whether the alternate screen is ours, and whether the kitty flags were
/// pushed. These live outside [`TerminalGuard`] so the crash reporter can
/// hand the terminal back from a panic hook, which runs before the guard is
/// dropped and so cannot reach it (ADR 0022).
static TERMINAL_ENTERED: AtomicBool = AtomicBool::new(false);
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// Give the terminal back to the shell (or `$EDITOR`). The first call after
/// each entry does the work; later ones find nothing left to undo, so the
/// panic hook and the guard's `Drop` can both call it.
pub(crate) fn restore_terminal() {
    if !TERMINAL_ENTERED.swap(false, Ordering::SeqCst) {
        return;
    }
    let enhanced = KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst);
    let _ = write_terminal_restore(&mut io::stdout(), enhanced);
    let _ = disable_raw_mode();
}

fn write_terminal_restore(writer: &mut impl Write, keyboard_enhanced: bool) -> io::Result<()> {
    let mut failure = None;
    let end = crossterm::queue!(writer, EndSynchronizedUpdate).map(|()| ());
    let retry_end = end.is_err();
    record_restore_error(&mut failure, "ending synchronized update", end);
    if retry_end {
        record_restore_error(
            &mut failure,
            "retrying synchronized update end",
            crossterm::queue!(writer, EndSynchronizedUpdate).map(|()| ()),
        );
    }
    if keyboard_enhanced {
        record_restore_error(
            &mut failure,
            "restoring keyboard mode",
            crossterm::queue!(writer, PopKeyboardEnhancementFlags).map(|()| ()),
        );
    }
    record_restore_error(
        &mut failure,
        "restoring shell terminal modes",
        crossterm::queue!(
            writer,
            Show,
            SetCursorStyle::DefaultUserShape,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        )
        .map(|()| ()),
    );
    record_restore_error(
        &mut failure,
        "flushing terminal restoration",
        writer.flush(),
    );
    failure.map_or(Ok(()), Err)
}

fn record_restore_error(failure: &mut Option<io::Error>, operation: &str, result: io::Result<()>) {
    let Err(error) = result else {
        return;
    };
    let error = io::Error::new(error.kind(), format!("{operation}: {error}"));
    *failure = Some(match failure.take() {
        Some(first) => {
            let kind = first.kind();
            io::Error::new(kind, format!("{first}; {error}"))
        }
        None => error,
    });
}

/// Restores the terminal on drop so a panic or error never leaves raw mode on.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        match Self::resume() {
            Ok(()) => Ok(Self),
            Err(error) => {
                restore_terminal();
                Err(error)
            }
        }
    }

    /// Raw mode, the alternate screen, mouse capture, bracketed paste, and
    /// the kitty flags: on entry and again after `$EDITOR` gives the
    /// terminal back.
    fn resume() -> anyhow::Result<()> {
        enable_raw_mode().context("cannot enable raw mode")?;
        // Marked entered as soon as raw mode is on, not after the whole
        // sequence: if entering the alternate screen fails partway,
        // `restore_terminal` must still turn raw mode off, and the extra
        // leave sequences are no-ops to a terminal still on the main screen.
        TERMINAL_ENTERED.store(true, Ordering::SeqCst);
        crossterm::execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            SetCursorStyle::SteadyBlock,
            Hide
        )
        .context("cannot enter alternate screen")?;
        // Kitty-protocol disambiguation lets Ctrl-Enter differ from Enter in
        // the draft (ADR 0013); terminals without it still get Alt-Enter.
        let enhanced = matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        ) && crossterm::execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        KEYBOARD_ENHANCED.store(enhanced, Ordering::SeqCst);
        tracing::info!(enhanced, "keyboard enhancement");
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// The input thread's state: reading, asked to pause, or paused while
/// `$EDITOR` owns the terminal (ADR 0018).
const INPUT_READING: u8 = 0;
const INPUT_PAUSE_REQUESTED: u8 = 1;
const INPUT_PAUSED: u8 = 2;

/// The input thread: reads terminal events until it is told to pause.
struct Input {
    events: mpsc::Receiver<io::Result<Event>>,
    state: Arc<AtomicU8>,
}

impl Input {
    /// Stop the thread reading, and wait until it has, so nothing typed
    /// into the editor is swallowed here.
    async fn pause(&self) {
        self.state.store(INPUT_PAUSE_REQUESTED, Ordering::SeqCst);
        while self.state.load(Ordering::SeqCst) != INPUT_PAUSED {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn resume(&self) {
        self.state.store(INPUT_READING, Ordering::SeqCst);
    }
}

fn spawn_input() -> anyhow::Result<Input> {
    let (input_tx, events) = mpsc::channel::<io::Result<Event>>(64);
    let state = Arc::new(AtomicU8::new(INPUT_READING));
    let flag = Arc::clone(&state);
    thread::Builder::new()
        .name("input".to_owned())
        .spawn(move || {
            loop {
                match flag.load(Ordering::SeqCst) {
                    INPUT_PAUSE_REQUESTED => flag.store(INPUT_PAUSED, Ordering::SeqCst),
                    INPUT_PAUSED => {
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    _ => {}
                }
                // Poll rather than block so a pause request is seen within
                // a beat; `poll` consumes nothing.
                match crossterm::event::poll(Duration::from_millis(100)) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) => {
                        let _ = input_tx.blocking_send(Err(error));
                        break;
                    }
                }
                let event = crossterm::event::read();
                let failed = event.is_err();
                if input_tx.blocking_send(event).is_err() || failed {
                    break;
                }
            }
        })
        .context("cannot start input thread")?;
    Ok(Input { events, state })
}

/// `Ctrl-e` in the draft: hand it to `$VISUAL` or `$EDITOR`
/// on a temporary file and load the result back (ADR 0018). The comment
/// is not submitted; the event loop waits while the editor runs.
async fn edit_draft(
    app: &mut App,
    input: &Input,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> anyhow::Result<()> {
    let Some(draft) = app.compose_draft() else {
        return Ok(());
    };
    let editor = ["VISUAL", "EDITOR"].iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    let Some(editor) = editor else {
        app.notice("set $VISUAL or $EDITOR to edit the comment there");
        return Ok(());
    };
    let draft = draft::Draft::new(draft).context("cannot create private editor draft")?;
    let result = edit_file(app, input, terminal, &editor, draft.path()).await;
    match (result, draft.close()) {
        (result, Ok(())) => result,
        (Ok(()), Err(error)) => Err(error).context("cannot clean up private editor draft"),
        (Err(error), Err(cleanup)) => Err(error.context(format!(
            "private editor draft cleanup also failed: {cleanup}"
        ))),
    }
}

async fn edit_file(
    app: &mut App,
    input: &Input,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    editor: &str,
    path: &Path,
) -> anyhow::Result<()> {
    input.pause().await;
    restore_terminal();
    tracing::info!(%editor, path = %path.display(), "editing the comment draft");
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("fathomable")
        .arg(path)
        .status();
    TerminalGuard::resume()?;
    input.resume();
    terminal.clear().context("cannot redraw after the editor")?;
    // The editor blocks this current-thread loop. Reconcile durable writes
    // before returning to the normal watcher turn.
    app.reload_store();
    match status {
        Ok(status) if status.success() => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            app.set_compose_text(text.strip_suffix('\n').unwrap_or(&text));
            app.notice("draft loaded from the editor; Enter submits");
        }
        Ok(status) => app.notice(format!("{editor} exited with {status}; draft kept")),
        Err(error) => app.notice(format!("cannot run {editor}: {error}")),
    }
    Ok(())
}

/// Maximum raw notifications consumed in one loop turn.
///
/// This is large enough to coalesce ordinary editor bursts while ensuring a
/// continuously refilled channel cannot monopolize terminal input or timers.
const RAW_DRAIN_LIMIT: usize = 256;

/// Fixed recovery cadence while watcher coverage or store reconciliation is
/// degraded. Incoming events never move an already scheduled deadline.
const WATCH_RETRY: Duration = Duration::from_secs(1);

#[expect(
    clippy::too_many_lines,
    reason = "the event loop keeps every readiness source and its redraw decision together"
)]
async fn run_async(
    workspace: Workspace,
    options: Options,
    theme: &Theme,
    open: Option<&Path>,
) -> anyhow::Result<()> {
    let (opener_tx, mut opener_rx) = mpsc::channel::<Result<(), String>>(16);
    let mut sigterm = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    let mut sighup = signal(SignalKind::hangup()).context("cannot listen for SIGHUP")?;

    // The guard queries the terminal, so it must run before the input
    // thread starts consuming responses.
    let _guard = TerminalGuard::enter()?;
    let mut input = spawn_input()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("cannot initialise terminal")?;
    let theme = crate::app::draw::Theme::from_core(theme);
    let size = terminal.size().context("cannot read terminal size")?;
    let hint_debounce = options.watch.debounce;
    let mut app = App::new(
        workspace,
        usize::from(size.width),
        usize::from(size.height),
        options,
    );
    app.start_on(open);
    let mut highlights =
        highlight::Worker::new().context("cannot start syntax highlight worker")?;
    let (mut doc_watcher, mut reload_rx) = match install_watcher(&mut app) {
        Ok((watcher, raws, healthy)) => {
            if !healthy {
                app.set_watching_root(false);
            }
            (Some(watcher), Some(raws))
        }
        Err(error) => {
            tracing::warn!(%error, "cannot start file watcher");
            app.set_watching_root(false);
            app.set_thread_watch_coverage(false);
            (None, None)
        }
    };
    let mut watch_retry_at = if watch_health(&app, doc_watcher.as_ref()) {
        None
    } else {
        schedule_retry(None, Instant::now())
    };

    let mut batch = watch::Batch::default();
    let mut redraw = true;
    loop {
        highlights
            .submit(app.take_highlight_jobs())
            .context("syntax highlight worker stopped")?;
        if let Some(watcher) = doc_watcher.as_mut() {
            redraw |= rewatch(&mut app, watcher);
            sync_loaded_watches(&app, watcher);
            redraw |= app.set_watching_root(watcher.coverage_complete());
        }
        if watch_health(&app, doc_watcher.as_ref()) {
            watch_retry_at = None;
        } else if watch_retry_at.is_none() {
            watch_retry_at = schedule_retry(watch_retry_at, Instant::now());
        }
        if redraw {
            draw(&app, &theme, &mut terminal)?;
            app.arm_review_point_delete_confirmation();
            redraw = false;
        }
        let (effect, changed) = tokio::select! {
            event = input.events.recv() => match event {
                Some(Ok(event)) => (handle_events(&mut app, &event, &mut input.events)?, true),
                Some(Err(error)) => return Err(error).context("reading terminal input"),
                None => (Effect::Quit, false),
            },
            notice = next_raw(&mut reload_rx) => {
                let mut changed = false;
                if let Some(raw) = notice
                    && let Some(watcher) = doc_watcher.as_mut()
                {
                    let queued = enqueue_raws(
                        &mut app,
                        watcher,
                        &mut reload_rx,
                        &mut batch,
                        raw,
                        hint_debounce,
                    );
                    changed |= queued.changed;
                    if queued.thread_dirty {
                        let reconciled = reconcile_thread_updates(&mut app, watcher);
                        changed |= reconciled.changed;
                    }
                }
                // Raw notices only fill the debounce batch. They do not
                // redraw a frame whose observable state has not changed.
                (Effect::None, changed)
            }
            () = batch.settled() => {
                let changed = doc_watcher.as_mut().is_some_and(|watcher| {
                    apply_batch(&mut app, watcher, &mut batch)
                });
                (Effect::None, changed)
            }
            () = tokio::time::sleep(app.tick_in().unwrap_or(Duration::from_hours(1))) => {
                app.tick();
                (Effect::None, true)
            }
            walked = app.next_walk() => {
                app.on_walked(walked);
                (Effect::None, true)
            }
            highlighted = highlights.next() => {
                let Some(highlighted) = highlighted else {
                    return Err(anyhow::anyhow!("syntax highlight worker stopped"));
                };
                let changed = app.apply_highlight(highlighted);
                (Effect::None, changed)
            }
            result = opener_rx.recv() => {
                let changed = if let Some(Err(why)) = result {
                    app.notice(why);
                    true
                } else {
                    false
                };
                (Effect::None, changed)
            }
            () = retry_deadline(watch_retry_at) => {
                watch_retry_at = None;
                let changed = match doc_watcher.as_mut() {
                    Some(watcher) => retry_watches(&mut app, watcher),
                    None => match install_watcher(&mut app) {
                        Ok((watcher, raws, _healthy)) => {
                            doc_watcher = Some(watcher);
                            reload_rx = Some(raws);
                            true
                        }
                        Err(error) => {
                            tracing::warn!(%error, "cannot recover file watcher");
                            app.set_watching_root(false);
                            app.set_thread_watch_coverage(false);
                            false
                        }
                    },
                };
                if !watch_health(&app, doc_watcher.as_ref()) {
                    watch_retry_at = schedule_retry(watch_retry_at, Instant::now());
                }
                (Effect::None, changed)
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM; quitting");
                (Effect::Quit, false)
            }
            _ = sighup.recv() => {
                tracing::info!("SIGHUP; quitting");
                (Effect::Quit, false)
            }
        };
        redraw |= changed;
        let flow = perform(&mut app, &input, &mut terminal, &opener_tx, effect).await?;
        if flow.is_break() {
            break;
        }
    }
    tracing::info!("app closed");
    Ok(())
}

/// Draw one observable state and retain it for a possible crash report.
fn draw(
    app: &App,
    theme: &crate::app::draw::Theme,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> anyhow::Result<()> {
    crash::observe(app.status_lines());
    synchronized_frame(&mut io::stdout(), || {
        terminal
            .draw(|frame| crate::app::draw::draw(frame, app, theme))
            .context("draw failed")?;
        Ok(())
    })
}

fn synchronized_frame<W, T>(
    writer: &mut W,
    draw: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T>
where
    W: Write,
{
    let mut update = FrameUpdate::new(writer);
    if let Err(error) = update.begin() {
        let error = anyhow::Error::new(error).context("cannot begin synchronized frame");
        return update.finish(Err(error));
    }
    update.finish(draw())
}

struct FrameUpdate<'a, W: Write> {
    writer: &'a mut W,
    ended: bool,
}

impl<'a, W> FrameUpdate<'a, W>
where
    W: Write,
{
    const fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            ended: false,
        }
    }

    fn begin(&mut self) -> io::Result<()> {
        // Ratatui re-emits the frame's final cursor visibility and position.
        // Hiding first also keeps the cursor out of cell painting on terminals
        // that ignore synchronized updates without suppressing the File cursor.
        crossterm::execute!(self.writer, BeginSynchronizedUpdate, Hide)?;
        Ok(())
    }

    fn end(&mut self) -> io::Result<()> {
        crossterm::execute!(self.writer, EndSynchronizedUpdate)?;
        self.ended = true;
        Ok(())
    }

    fn finish<T>(mut self, result: anyhow::Result<T>) -> anyhow::Result<T> {
        match (result, self.end()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(error),
            (Ok(_), Err(cleanup)) => Err(cleanup).context("cannot end synchronized frame"),
            (Err(error), Err(cleanup)) => {
                Err(error.context(format!("ending synchronized frame also failed: {cleanup}")))
            }
        }
    }
}

impl<W> Drop for FrameUpdate<'_, W>
where
    W: Write,
{
    fn drop(&mut self) {
        if !self.ended {
            let _ = crossterm::execute!(self.writer, EndSynchronizedUpdate);
        }
    }
}

/// Keep narrow coverage for every loaded file, including ignored ones.
fn sync_loaded_watches(app: &App, watcher: &mut watch::Watcher) {
    let loaded = app.loaded_abs_paths();
    watcher.follow(loaded.iter().map(std::path::PathBuf::as_path));
}

async fn next_raw(raws: &mut Option<watch::Raws>) -> Option<watch::Raw> {
    match raws {
        Some(raws) => raws.recv().await,
        None => std::future::pending().await,
    }
}

async fn retry_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => std::future::pending().await,
    }
}

/// Apply one debounced watcher batch and repair structural watch changes.
fn apply_batch(app: &mut App, watcher: &mut watch::Watcher, batch: &mut watch::Batch) -> bool {
    let mut events = batch.take(|path| app.loaded_fingerprint(path), app.max_file_bytes());
    events.retain(|event| event.path().is_none_or(|path| watcher.is_target(path)));
    let sync = watcher.needs_root_sync(&events);
    let mut changed = !events.is_empty();
    app.on_events(events);
    if sync {
        let complete = app.sync_workspace_watches(watcher);
        changed |= app.set_watching_root(complete && watcher.coverage_complete());
    }
    changed
}

/// Queue one ready burst without drawing for each raw notification.
fn enqueue_raws(
    app: &mut App,
    watcher: &mut watch::Watcher,
    incoming: &mut Option<watch::Raws>,
    batch: &mut watch::Batch,
    first: watch::Raw,
    debounce: Duration,
) -> Enqueued {
    let raws = drain_raws(incoming, first);
    let mut changed = false;
    let mut thread_dirty = false;
    let mut synthetic = Vec::new();
    for raw in raws {
        let state_dirty = watcher.thread_state_dirty(&raw);
        thread_dirty |= state_dirty;
        watcher.invalidate_state_watches(&raw);
        let arrived = match &raw {
            watch::Raw::Create(path)
            | watch::Raw::RenameTo(path)
            | watch::Raw::Rename { to: path, .. } => Some(path),
            _ => None,
        };
        if let Some(path) = arrived {
            let (created, complete) = app.watch_created(watcher, path);
            synthetic.extend(created);
            if !complete {
                changed |= app.set_watching_root(false);
            }
        }
        if watcher.accepts(&raw)
            && app.raw_is_relevant(&raw)
            && (!state_dirty || matches!(raw, watch::Raw::Rescan))
        {
            batch.push(raw, debounce);
        }
    }
    for raw in synthetic {
        if watcher.accepts(&raw) && app.raw_is_relevant(&raw) {
            batch.push(raw, debounce);
        }
    }
    Enqueued {
        changed,
        thread_dirty,
    }
}

fn drain_raws(incoming: &mut Option<watch::Raws>, first: watch::Raw) -> Vec<watch::Raw> {
    let mut raws = vec![first];
    for _ in 1..RAW_DRAIN_LIMIT {
        let Some(raw) = incoming.as_mut().and_then(watch::Raws::try_recv) else {
            break;
        };
        raws.push(raw);
    }
    raws
}

fn schedule_retry(current: Option<Instant>, now: Instant) -> Option<Instant> {
    current.or(Some(now + WATCH_RETRY))
}

#[derive(Debug, Clone, Copy, Default)]
struct Enqueued {
    changed: bool,
    thread_dirty: bool,
}

/// The watches a viewer starts with: visible workspace directories, the
/// thread store, and worktree Git paths.
fn start_watching(app: &mut App, doc_watcher: &mut watch::Watcher) -> bool {
    let watching = app.sync_workspace_watches(doc_watcher);
    let state = [app.store_path().to_path_buf()];
    let state_covered = doc_watcher.watch_state(state.iter().map(std::path::PathBuf::as_path));
    app.take_rewatch();
    doc_watcher.watch_worktrees(app.worktree_watch_paths());
    app.set_watching_root(watching && doc_watcher.coverage_complete());
    let reloaded = app.reload_store();
    app.set_thread_watch_coverage(state_covered && reloaded.healthy);
    state_covered && reloaded.healthy && doc_watcher.coverage_complete()
}

fn install_watcher(app: &mut App) -> anyhow::Result<(watch::Watcher, watch::Raws, bool)> {
    let (mut watcher, raws) = watch::Watcher::new()?;
    let healthy = start_watching(app, &mut watcher);
    Ok((watcher, raws, healthy))
}

fn reconcile_thread_updates(app: &mut App, watcher: &mut watch::Watcher) -> super::StoreReload {
    let state = [app.store_path().to_path_buf()];
    let covered = watcher.watch_state(state.iter().map(std::path::PathBuf::as_path));
    let mut reloaded = app.reload_store();
    reloaded.changed |= app.set_thread_watch_coverage(covered && reloaded.healthy);
    reloaded
}

fn retry_watches(app: &mut App, watcher: &mut watch::Watcher) -> bool {
    let root = app.sync_workspace_watches(watcher);
    watcher.watch_worktrees(app.worktree_watch_paths());
    sync_loaded_watches(app, watcher);
    app.set_watching_root(root && watcher.coverage_complete());
    let reloaded = reconcile_thread_updates(app, watcher);
    reloaded.changed
}

fn watch_health(app: &App, watcher: Option<&watch::Watcher>) -> bool {
    app.thread_updates_healthy() && watcher.is_some_and(watch::Watcher::coverage_complete)
}

/// Move the watcher after the app re-rooted, or the worktree set
/// changed (ADR 0070).
fn rewatch(app: &mut App, doc_watcher: &mut watch::Watcher) -> bool {
    let Some(rewatch) = app.take_rewatch() else {
        return false;
    };
    let mut changed = false;
    if rewatch.root.is_some() {
        let watching = app.sync_workspace_watches(doc_watcher);
        changed |= app.set_watching_root(watching && doc_watcher.coverage_complete());
    }
    doc_watcher.watch_worktrees(&rewatch.extras);
    let reloaded = reconcile_thread_updates(app, doc_watcher);
    changed || reloaded.changed
}

/// Do what a key asked of the loop; `Break` when the viewer is to quit.
async fn perform(
    app: &mut App,
    input: &Input,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    opener: &mpsc::Sender<Result<(), String>>,
    effect: Effect,
) -> anyhow::Result<ControlFlow<()>> {
    match effect {
        Effect::None => {}
        Effect::Quit => return Ok(ControlFlow::Break(())),
        Effect::Copy(text) => {
            tracing::debug!(bytes = text.len(), "copied selection via OSC 52");
            clipboard::copy(&text).context("cannot write to clipboard")?;
        }
        Effect::Open(url) => open_url(app, &url, opener),
        Effect::Command(command) => app.command(&command),
        Effect::EditDraft => edit_draft(app, input, terminal).await?,
    }
    Ok(ControlFlow::Continue(()))
}

/// Hand an external URL to `xdg-open` without waiting on the browser.
fn open_url(app: &mut App, url: &str, opener: &mpsc::Sender<Result<(), String>>) {
    let mut command = Command::new("xdg-open");
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match spawn_opener(command, opener.clone()) {
        Ok(()) => app.notice(format!("asking xdg-open to open {url}")),
        Err(error) => app.notice(format!("cannot start link opener: {error}")),
    }
}

fn spawn_opener(mut command: Command, results: mpsc::Sender<Result<(), String>>) -> io::Result<()> {
    // Spawn the worker before the process so a thread-start failure cannot
    // leave an unreaped child. A blocking Tokio task would delay runtime
    // shutdown if xdg-open stays alive for the browser's lifetime.
    thread::Builder::new()
        .name("url-opener".to_owned())
        .spawn(move || {
            let result = command
                .spawn()
                .map_err(|error| opener_spawn_error(&error))
                .and_then(|mut child| opener_status(child.wait()));
            // The receiver closes when the viewer exits.
            let _ = results.blocking_send(result);
        })?;
    Ok(())
}

fn opener_spawn_error(error: &io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "cannot open link: xdg-open not found; install xdg-utils and configure a desktop URL opener"
            .to_owned()
    } else {
        format!("cannot start xdg-open: {error}")
    }
}

fn opener_status(result: io::Result<ExitStatus>) -> Result<(), String> {
    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "xdg-open failed ({status}); check your desktop URL opener"
        )),
        Err(error) => Err(format!("cannot wait for xdg-open: {error}")),
    }
}

fn handle_events(
    app: &mut App,
    event: &Event,
    incoming: &mut mpsc::Receiver<io::Result<Event>>,
) -> anyhow::Result<Effect> {
    let mut effect = handle_event(app, event);
    // Coalesce a burst (wheel flick, key repeat) into one frame: draining
    // here keeps the redraw from lagging behind the queue and jumping
    // several notches at once.
    while matches!(effect, Effect::None)
        && let Ok(next) = incoming.try_recv()
    {
        let next = next.context("reading terminal input")?;
        effect = handle_event(app, &next);
    }
    Ok(effect)
}

fn handle_event(app: &mut App, event: &Event) -> Effect {
    match event {
        Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
            input::keys::handle_key(app, *key)
        }
        Event::Mouse(mouse) => input::mouse::handle_mouse(app, *mouse),
        Event::Paste(text) => {
            app.paste(text);
            Effect::None
        }
        Event::Resize(width, height) => {
            app.resize(usize::from(*width), usize::from(*height));
            Effect::None
        }
        _ => Effect::None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::process::Stdio;

    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use super::{
        Command, Context, Duration, Instant, RAW_DRAIN_LIMIT, drain_raws, handle_events, io, mpsc,
        opener_status, schedule_retry, spawn_opener,
    };
    use crate::app::draw;
    use crate::app::input::bindings::Action;
    use crate::app::watch::{Raw, Raws};
    use crate::app::{Popup, testing, watch};
    use fathomable_core::annotations::{Author, Draft, LineRange, Store};

    async fn opener_result(command: Command) -> anyhow::Result<Result<(), String>> {
        let (sender, mut receiver) = mpsc::channel(1);
        spawn_opener(command, sender)?;
        tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await?
            .context("opener closed without reporting a result")
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn queued_picker_click_cannot_confirm_point_deletion_before_redraw() -> anyhow::Result<()> {
        let dir = testing::workspace("queued-point-delete", testing::README)?;
        let points = dir.0.join("points");
        let mut app = testing::AppBuilder::new(&dir)
            .review_points(&points)
            .build()?;
        app.save_review_point(Some("point"));
        app.request_review_point_delete();
        let picker_cell = match app.popup() {
            Some(Popup::Picker(picker)) => {
                let layout = draw::picker_layout(&app, picker);
                (0..app.pane_rows())
                    .flat_map(|row| (0..app.size().0).map(move |column| (column, row)))
                    .find(|(column, row)| {
                        layout.entry_at(*column, *row, picker.matched()) == Some(0)
                    })
                    .context("picker row")?
            }
            _ => anyhow::bail!("delete picker did not open"),
        };
        let confirmation = draw::review_point_delete_confirmation_layout(&app);
        let confirm_cell = (0..app.pane_rows())
            .flat_map(|row| (0..app.size().0).map(move |column| (column, row)))
            .find(|(column, row)| confirmation.action_at(*column, *row) == Some(Action::Confirm))
            .context("confirmation control")?;
        let click = |(column, row)| {
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: u16::try_from(column).unwrap_or(u16::MAX),
                row: u16::try_from(row).unwrap_or(u16::MAX),
                modifiers: KeyModifiers::NONE,
            })
        };
        let (sender, mut incoming) = mpsc::channel(2);
        sender.try_send(Ok(click(confirm_cell)))?;

        handle_events(&mut app, &click(picker_cell), &mut incoming)?;

        assert!(matches!(
            app.popup(),
            Some(Popup::ConfirmReviewPointDelete { armed: false, .. })
        ));
        assert!(
            app.review_points
                .as_ref()
                .is_some_and(|store| !store.is_empty())
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_opener_reports_installation_advice() -> anyhow::Result<()> {
        let error = opener_result(Command::new("./fathomable-no-such-opener/executable"))
            .await?
            .err()
            .context("missing opener should fail")?;
        assert!(error.contains("xdg-open not found"));
        assert!(error.contains("install xdg-utils"));
        assert!(error.contains("desktop URL opener"));
        Ok(())
    }

    #[tokio::test]
    async fn other_spawn_errors_are_reported() -> anyhow::Result<()> {
        let error = opener_result(Command::new("."))
            .await?
            .err()
            .context("executing a directory should fail")?;
        assert!(error.starts_with("cannot start xdg-open:"));
        assert!(!error.contains("not found"));
        Ok(())
    }

    #[tokio::test]
    async fn successful_opener_reports_completion() -> anyhow::Result<()> {
        assert_eq!(opener_result(shell("exit 0")).await?, Ok(()));
        Ok(())
    }

    #[tokio::test]
    async fn nonzero_opener_reports_failure() -> anyhow::Result<()> {
        let error = opener_result(shell("exit 4"))
            .await?
            .err()
            .context("nonzero exit should fail")?;
        assert!(error.contains("xdg-open failed"));
        assert!(error.contains("exit status: 4"));
        assert!(error.contains("desktop URL opener"));
        Ok(())
    }

    #[test]
    fn wait_failure_is_reported() -> anyhow::Result<()> {
        let error = opener_status(Err(io::Error::other("wait failed")))
            .err()
            .context("wait failure should fail")?;
        assert_eq!(error, "cannot wait for xdg-open: wait failed");
        Ok(())
    }

    #[tokio::test]
    async fn pending_opener_does_not_block_the_event_loop() -> anyhow::Result<()> {
        let (reader, mut writer) = UnixStream::pair()?;
        let mut command = shell("read line; exit 4");
        command.stdin(Stdio::from(std::os::fd::OwnedFd::from(reader)));
        let (sender, mut receiver) = mpsc::channel(1);
        spawn_opener(command, sender)?;

        tokio::select! {
            result = receiver.recv() => anyhow::bail!("opener completed before release: {result:?}"),
            () = tokio::task::yield_now() => {}
        }
        writer.write_all(b"release\n")?;
        let result = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await?
            .context("opener closed without reporting its exit")?;
        let error = result.err().context("nonzero exit should fail")?;
        assert!(error.contains("exit status: 4"));
        Ok(())
    }

    #[test]
    fn raw_drain_is_bounded_under_continuous_refill() -> anyhow::Result<()> {
        let (sender, raws) = Raws::test_channel();
        for index in 0..(RAW_DRAIN_LIMIT * 2) {
            sender
                .send(Raw::Modify(format!("/workspace/{index}").into()))
                .context("receiver unexpectedly closed")?;
        }
        let mut raws = Some(raws);
        let first = raws
            .as_mut()
            .and_then(Raws::try_recv)
            .context("first raw")?;
        let drained = drain_raws(&mut raws, first);
        assert_eq!(drained.len(), RAW_DRAIN_LIMIT);
        assert!(
            raws.as_mut().and_then(Raws::try_recv).is_some(),
            "a later loop turn retains work"
        );
        Ok(())
    }

    #[test]
    fn incoming_events_do_not_postpone_existing_retry_deadline() -> anyhow::Result<()> {
        let now = Instant::now();
        let first = schedule_retry(None, now).context("deadline")?;
        let later = schedule_retry(Some(first), now + Duration::from_secs(30));
        assert_eq!(later, Some(first));
        Ok(())
    }

    #[test]
    fn thread_dirtiness_bypasses_continuously_reset_general_debounce() -> anyhow::Result<()> {
        let dir = testing::workspace("run-thread-priority", testing::README)?;
        let mut app = testing::app(&dir)?;
        let (mut watcher, _) = watch::Watcher::new()?;
        app.sync_workspace_watches(&mut watcher);
        let state = testing::store_path(&dir);
        assert!(watcher.watch_state([state.as_path()]));

        let (sender, raws) = Raws::test_channel();
        for _ in 0..(RAW_DRAIN_LIMIT * 2) {
            sender
                .send(Raw::Modify(testing::root(&dir).join("README.md")))
                .context("receiver unexpectedly closed")?;
        }
        let mut raws = Some(raws);
        let mut batch = watch::Batch::default();
        let queued = super::enqueue_raws(
            &mut app,
            &mut watcher,
            &mut raws,
            &mut batch,
            Raw::Modify(state),
            Duration::from_secs(30),
        );
        assert!(
            queued.thread_dirty,
            "the exact store is ready in this loop turn"
        );
        assert!(
            !batch.take(|_| None, u64::MAX).is_empty(),
            "the mixed workspace side remains queued"
        );
        assert!(
            raws.as_mut().and_then(Raws::try_recv).is_some(),
            "continuous refill remains bounded"
        );
        Ok(())
    }

    #[test]
    fn post_install_reconciliation_closes_store_open_watch_race() -> anyhow::Result<()> {
        let dir = testing::workspace("run-watch-install-race", testing::README)?;
        let mut app = testing::app(&dir)?;
        let id = Store::open(testing::store_path(&dir))?.annotate(
            Draft::new(
                Author::agent("reviewer"),
                Path::new("README.md"),
                LineRange::new(3, 3),
                "arrived before watch installation",
            ),
            testing::README,
            1,
        )?;

        let (mut watcher, _) = watch::Watcher::new()?;
        assert!(super::start_watching(&mut app, &mut watcher));
        assert!(app.thread(&id).is_some());
        assert_eq!(app.toasts().len(), 1);
        Ok(())
    }
}
