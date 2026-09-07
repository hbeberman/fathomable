// @okf-doc: /decisions/0012-workspace-mode.md
//! The viewer's main loop: the terminal, the tokio loop, the watcher and
//! socket wiring, and the effects a key can ask the loop to perform.
//!
//! [`run`] owns the terminal for the life of the viewer; everything the
//! loop hands the app goes through [`App`] in the parent module, so the
//! app itself is tested without a terminal (ADR 0012).

use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fathomable_core::session::Record;
use fathomable_core::theme::Theme;
use fathomable_core::workspace::Workspace;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

use crate::crash;

use super::view::Effect;
use super::{App, Options, clipboard, input, socket, watch};

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
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = crossterm::execute!(
        io::stdout(),
        SetCursorStyle::DefaultUserShape,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

/// Restores the terminal on drop so a panic or error never leaves raw mode on.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        Self::resume()?;
        Ok(Self)
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
            SetCursorStyle::SteadyBlock
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
/// is not submitted; the socket and watcher wait while the editor runs.
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
    let path = std::env::temp_dir().join(format!("fathomable-comment-{}.md", std::process::id()));
    fs::write(&path, draft).with_context(|| format!("cannot write {}", path.display()))?;
    input.pause().await;
    restore_terminal();
    tracing::info!(%editor, path = %path.display(), "editing the comment draft");
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("fathomable")
        .arg(&path)
        .status();
    TerminalGuard::resume()?;
    input.resume();
    terminal.clear().context("cannot redraw after the editor")?;
    match status {
        Ok(status) if status.success() => {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            app.set_compose_text(text.strip_suffix('\n').unwrap_or(&text));
            app.notice("draft loaded from the editor; Enter submits");
        }
        Ok(status) => app.notice(format!("{editor} exited with {status}; draft kept")),
        Err(error) => app.notice(format!("cannot run {editor}: {error}")),
    }
    let _ = fs::remove_file(&path);
    Ok(())
}

/// Serve the viewer socket, or say why there is none: the message is
/// shown in the viewer, since a socket that silently failed leaves the
/// tools falling back to the store with nothing to tell the user.
fn serve_socket(
    record: &Record,
    app: mpsc::Sender<socket::Envelope>,
) -> Result<socket::Serving, String> {
    let Some(path) = record.socket() else {
        tracing::warn!("XDG_RUNTIME_DIR unset; no viewer socket");
        return Err(
            "no viewer socket: XDG_RUNTIME_DIR is unset; agents use the store only".to_owned(),
        );
    };
    match socket::Listener::bind(path) {
        Ok(listener) => Ok(listener.serve(app)),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "cannot listen on the viewer socket");
            Err(format!(
                "no viewer socket ({error}; {}); agents use the store only",
                path.display()
            ))
        }
    }
}

/// Hold the served socket for the viewer's lifetime, or show the user
/// why there is none.
fn keep_socket(app: &mut App, socket: Result<socket::Serving, String>) -> Option<socket::Serving> {
    socket.map_err(|why| app.notice(why)).ok()
}

async fn run_async(
    workspace: Workspace,
    options: Options,
    theme: &Theme,
    open: Option<&Path>,
) -> anyhow::Result<()> {
    let (mut doc_watcher, mut reload_rx) = watch::Watcher::new()?;
    let (request_tx, mut request_rx) = mpsc::channel::<socket::Envelope>(16);
    let socket = serve_socket(&options.record, request_tx);
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
    start_watching(&mut app, &mut doc_watcher);
    app.start_on(open);
    let _socket = keep_socket(&mut app, socket);

    let mut batch = watch::Batch::default();
    loop {
        rewatch(&mut app, &mut doc_watcher);
        doc_watcher.follow(app.current_abs_path());
        app.settle();
        // What a crash report says the viewer was showing (ADR 0022): the
        // `:status` rows, refreshed here so they describe the frame that
        // dies rather than the one before it.
        crash::observe(app.status_lines());
        terminal
            .draw(|frame| crate::app::draw::draw(frame, &app, &theme))
            .context("draw failed")?;
        let effect = tokio::select! {
            event = input.events.recv() => match event {
                Some(Ok(event)) => {
                    let mut effect = handle_event(&mut app, &event);
                    // Coalesce a burst (wheel flick, key repeat) into one
                    // frame: draining here keeps the redraw from lagging
                    // behind the queue and jumping several notches at once.
                    while matches!(effect, Effect::None)
                        && let Ok(next) = input.events.try_recv()
                    {
                        let next = next.context("reading terminal input")?;
                        effect = handle_event(&mut app, &next);
                    }
                    effect
                }
                Some(Err(error)) => return Err(error).context("reading terminal input"),
                None => Effect::Quit,
            },
            notice = reload_rx.recv() => {
                if let Some(raw) = notice {
                    // One quiet period turns a burst of writes into one
                    // change per file (ADR 0015 `follow.hint-debounce`).
                    // The wait is its own arm below, so input keeps
                    // flowing while the burst settles.
                    batch.push(raw, hint_debounce);
                    while let Some(raw) = reload_rx.try_recv() {
                        batch.push(raw, hint_debounce);
                    }
                }
                Effect::None
            }
            () = batch.settled() => {
                let mut events =
                    batch.take(|path| app.last_seen_fingerprint(path), app.max_file_bytes());
                events.retain(|event| event.path().is_none_or(|path| doc_watcher.is_target(path)));
                app.on_events(events);
                Effect::None
            }
            () = tokio::time::sleep(app.tick_in().unwrap_or(Duration::from_hours(1))) => {
                app.tick();
                Effect::None
            }
            walked = app.next_walk() => {
                app.on_walked(walked);
                Effect::None
            }
            envelope = request_rx.recv() => {
                if let Some(socket::Envelope { request, reply }) = envelope {
                    let response = app.handle_request(request);
                    let _ = reply.send(response);
                }
                Effect::None
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM; quitting");
                Effect::Quit
            }
            _ = sighup.recv() => {
                tracing::info!("SIGHUP; quitting");
                Effect::Quit
            }
        };
        if perform(&mut app, &input, &mut terminal, effect)
            .await?
            .is_break()
        {
            break;
        }
    }
    app.on_quit();
    tracing::info!("app closed");
    Ok(())
}

/// The watches a viewer starts with: the root, the thread store's
/// directory (ADR 0024), and the other worktrees' git paths (ADR 0070).
fn start_watching(app: &mut App, doc_watcher: &mut watch::Watcher) {
    let watching = doc_watcher.watch_root(app.workspace().root());
    app.set_watching_root(watching);
    doc_watcher.watch_store(app.store_path());
    app.take_rewatch();
    doc_watcher.watch_worktrees(app.worktree_watch_paths());
}

/// Move the watcher after the app re-rooted, or the worktree set
/// changed (ADR 0070).
fn rewatch(app: &mut App, doc_watcher: &mut watch::Watcher) {
    let Some(rewatch) = app.take_rewatch() else {
        return;
    };
    if let Some(root) = rewatch.root {
        let watching = doc_watcher.watch_root(&root);
        app.set_watching_root(watching);
    }
    doc_watcher.watch_worktrees(&rewatch.extras);
}

/// Do what a key asked of the loop; `Break` when the viewer is to quit.
async fn perform(
    app: &mut App,
    input: &Input,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    effect: Effect,
) -> anyhow::Result<ControlFlow<()>> {
    match effect {
        Effect::None => {}
        Effect::Quit => return Ok(ControlFlow::Break(())),
        Effect::Copy(text) => {
            tracing::debug!(bytes = text.len(), "copied selection via OSC 52");
            clipboard::copy(&text).context("cannot write to clipboard")?;
        }
        Effect::Open(url) => open_url(app, &url),
        Effect::Command(command) => app.command(&command),
        Effect::EditDraft => edit_draft(app, input, terminal).await?,
    }
    Ok(ControlFlow::Continue(()))
}

/// `gx` (ADR 0050): hand `url` to `xdg-open`, the one process the
/// viewer starts. The child is reaped on a thread of its own so the
/// loop never waits on a browser.
fn open_url(app: &mut App, url: &str) {
    let spawned = Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
            app.notice(format!("opening {url}"));
        }
        Err(error) => app.notice(format!("cannot open link with xdg-open: {error}")),
    }
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
