// @okf-doc: /decisions/0022-crash-reports.md
//! The block Fathomable prints when it dies: what it was showing, where its
//! state lives, and why it stopped, in one piece to paste at an agent.
//!
//! A panic inside the alternate screen is invisible — the message lands on a
//! screen the terminal throws away on the way out — so the hook installed
//! here hands the terminal back before it writes anything (ADR 0022).

use std::backtrace::Backtrace;
use std::fmt::Write as _;
use std::fs;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where the report is written, and the log it points the reader at.
static PATHS: Mutex<Option<Paths>> = Mutex::new(None);

/// What the viewer was showing, refreshed each frame so a crash can say so.
static STATE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

#[derive(Debug, Clone)]
struct Paths {
    report: PathBuf,
    log: PathBuf,
}

/// Install the panic hook and name the files the report points at.
pub fn arm(report: PathBuf, log: PathBuf) {
    if let Ok(mut paths) = PATHS.lock() {
        *paths = Some(Paths { report, log });
    }
    // The default hook writes its message to whatever screen is current,
    // which for a TUI is the one about to be discarded; this replaces it
    // rather than chaining, so the report is printed once.
    std::panic::set_hook(Box::new(panicked));
}

/// Record what the viewer is showing, for a report that will probably never
/// be needed. Called once per frame; the rows are the `:status` overlay's.
pub fn observe(rows: Vec<(String, String)>) {
    if let Ok(mut state) = STATE.lock() {
        *state = rows;
    }
}

/// Report an error that ends the run. The terminal is already back by the
/// time this is called, so it only prints.
///
/// An error thrown before the viewer drew anything — a mistyped `--theme`,
/// an unreadable config — is a mistake to correct, not a crash to report, so
/// it stays the one line it always was.
pub fn fatal(error: &anyhow::Error) {
    if STATE.try_lock().is_ok_and(|state| state.is_empty()) {
        eprintln!("fathomable: {error:#}");
        return;
    }
    let mut cause = format!("fathomable {} stopped with an error\n", version());
    for (depth, source) in error.chain().enumerate() {
        let lead = if depth == 0 { "  " } else { "  caused by: " };
        let _ = writeln!(cause, "{lead}{source}");
    }
    emit(&cause, None);
}

fn panicked(info: &PanicHookInfo<'_>) {
    // Hand the terminal back first: anything written to the alternate
    // screen goes with it when the screen is left.
    crate::app::restore_terminal();
    let message = info
        .payload_as_str()
        .unwrap_or("panicked with a payload that is not a string");
    let where_ = info
        .location()
        .map_or_else(|| "an unknown location".to_owned(), ToString::to_string);
    let cause = format!(
        "fathomable {} panicked at {where_}\n  {message}\n",
        version()
    );
    emit(&cause, Some(&Backtrace::force_capture().to_string()));
}

/// Print the report to stderr and leave a copy beside the session log.
fn emit(cause: &str, backtrace: Option<&str>) {
    let paths = PATHS.try_lock().ok().and_then(|paths| paths.clone());
    let report = compose(cause, backtrace, paths.as_ref());
    let written = paths
        .as_ref()
        .filter(|paths| write_report(&paths.report, &report).is_ok())
        .map(|paths| paths.report.display().to_string());
    tracing::error!(cause = cause.trim(), "crashed");
    eprintln!();
    eprintln!("{RULE}");
    eprintln!("{}", report.trim_end());
    eprintln!("{RULE}");
    match written {
        Some(path) => eprintln!("Paste the block above at your agent. A copy is in {path}."),
        None => eprintln!("Paste the block above at your agent."),
    }
}

const RULE: &str = "────────────────────────────────────────────────────────────────────────";

/// The report body: what stopped, what the viewer was showing, and where the
/// rest of the evidence is.
fn compose(cause: &str, backtrace: Option<&str>, paths: Option<&Paths>) -> String {
    let mut out = cause.to_owned();
    // `try_lock`: a report missing the state rows beats one that never
    // prints because the frame that was being observed is the one that died.
    let mut rows: Vec<(String, String)> = STATE
        .try_lock()
        .map(|state| state.clone())
        .unwrap_or_default();
    if let Some(paths) = paths {
        rows.push(("log".to_owned(), paths.log.display().to_string()));
    }
    rows.push(("terminal type".to_owned(), terminal_type()));
    rows.push(("build".to_owned(), build()));
    let width = rows.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    out.push('\n');
    for (key, value) in rows {
        let _ = writeln!(out, "{key:<width$}  {value}");
    }
    if let Some(backtrace) = backtrace {
        let _ = write!(out, "\nbacktrace\n{}", trim_backtrace(backtrace));
    }
    out
}

/// A backstop for a trace deep enough to bury the report, after the
/// plumbing is out of the way.
const FRAMES: usize = 24;

/// The frames the reader wants. Everything on the way in and out of the
/// panic — the hook, the async runtime, the process entry point — reads the
/// same in every report, so it goes; what is left keeps its original number,
/// and the gaps say where the plumbing was.
fn trim_backtrace(backtrace: &str) -> String {
    let mut frames: Vec<Vec<&str>> = Vec::new();
    for line in backtrace.lines() {
        // A frame opens with `  N: name` and continues with more deeply
        // indented `at file:line`.
        if starts_frame(line) {
            frames.push(vec![line]);
        } else if let Some(frame) = frames.last_mut() {
            frame.push(line);
        }
    }
    let mut kept = frames
        .iter()
        .filter(|frame| frame.first().is_some_and(|line| !is_plumbing(line)));
    let total = kept.clone().count();
    let mut out = kept
        .by_ref()
        .take(FRAMES)
        .flat_map(|frame| frame.iter())
        .fold(String::new(), |mut out, line| {
            let _ = writeln!(out, "  {}", line.trim_end());
            out
        });
    if total > FRAMES {
        let _ = writeln!(out, "  ... {} more frames", total - FRAMES);
    }
    out
}

/// Whether a frame's opening line names the runtime getting to the code
/// rather than the code itself.
fn is_plumbing(line: &str) -> bool {
    /// The process entry point, whose frames carry no path at all.
    const ENTRY: [&str; 4] = ["main", "_start", "__libc_start_main", "<unknown>"];
    /// Anything on the way into the hook, or out through the runtime.
    const WITHIN: [&str; 11] = [
        "fathomable::crash",
        "std::panicking",
        "core::panicking",
        "std::panic::",
        "std::sys::backtrace",
        "rust_begin_unwind",
        "std::rt::",
        "std::thread::local",
        "core::ops::function::",
        "core::future::future::",
        "tokio::",
    ];
    let name = line
        .trim_start()
        .split_once(": ")
        .map_or(line, |(_, name)| name);
    ENTRY.contains(&name) || WITHIN.iter().any(|noise| name.contains(noise))
}

/// Whether `line` opens a new backtrace frame: `   12: some::name`.
fn starts_frame(line: &str) -> bool {
    let rest = line.trim_start();
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    digits > 0 && rest[digits..].starts_with(": ")
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The terminal's own idea of what it is, for bugs that only one emulator has.
fn terminal_type() -> String {
    let named = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let mut parts = vec![named("TERM").unwrap_or_else(|| "TERM unset".to_owned())];
    parts.extend(named("TERM_PROGRAM"));
    parts.extend(named("COLORTERM"));
    if named("TMUX").is_some() {
        parts.push("under tmux".to_owned());
    }
    parts.join(", ")
}

/// What the reader needs to reproduce the build, not just the run.
fn build() -> String {
    format!(
        "{} {} {} ({} profile)",
        version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    )
}

fn write_report(path: &Path, report: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, report)
}

#[cfg(test)]
mod tests {
    use super::{FRAMES, compose, observe, trim_backtrace};

    /// A frame and its `at` line, as `Backtrace` formats them.
    fn frame(number: usize, name: &str) -> String {
        format!("{number:>4}: {name}\n             at ./{name}.rs:{number}:1\n")
    }

    #[test]
    fn the_backtrace_keeps_the_code_and_drops_the_plumbing() {
        let trace: String = [
            frame(0, "fathomable::crash::panicked"),
            frame(1, "std::panicking::panic_with_hook"),
            frame(2, "fathomable::app::ui::draw"),
            frame(3, "<tokio::runtime::scheduler::Core>::block_on"),
            frame(4, "fathomable::main"),
            frame(5, "main"),
            frame(6, "__libc_start_main"),
        ]
        .concat();
        let kept = trim_backtrace(&trace);
        assert!(kept.contains("2: fathomable::app::ui::draw"), "{kept}");
        assert!(kept.contains("4: fathomable::main"), "{kept}");
        assert!(!kept.contains("panic_with_hook"), "the hook is not the bug");
        assert!(!kept.contains("block_on"), "the runtime is not the bug");
        assert!(!kept.contains("libc"), "the entry point is not the bug");
        // Numbers are the trace's own, so the gaps show what was dropped.
        assert!(!kept.contains("0: "), "{kept}");
    }

    #[test]
    fn a_very_deep_backtrace_is_capped() {
        let trace: String = (0..FRAMES * 2)
            .map(|n| frame(n, &format!("fathomable::deep{n}")))
            .collect();
        let kept = trim_backtrace(&trace);
        let frames = kept
            .lines()
            .filter(|line| line.contains(": fathomable::deep"))
            .count();
        assert_eq!(frames, FRAMES);
        assert!(
            kept.contains(&format!("... {FRAMES} more frames")),
            "{kept}"
        );
    }

    #[test]
    fn the_report_carries_what_the_viewer_last_showed() {
        observe(vec![
            (
                "document".to_owned(),
                "src/main.rs (source) at 12:3".to_owned(),
            ),
            ("terminal".to_owned(), "20 columns x 24 rows".to_owned()),
        ]);
        let report = compose("fathomable 0.0.0 panicked at a.rs:1:1\n", None, None);
        assert!(report.contains("src/main.rs (source) at 12:3"), "{report}");
        assert!(report.contains("20 columns x 24 rows"), "{report}");
        // The build row is always there, so a report is never bare.
        assert!(report.contains("build"), "{report}");
        assert!(!report.contains("backtrace"), "an error has no frames");
    }
}
