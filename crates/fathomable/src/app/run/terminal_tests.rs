use std::cell::RefCell;
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use anyhow::{Context, anyhow};

use super::{synchronized_frame, write_terminal_restore};

const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";
const HIDE: &[u8] = b"\x1b[?25l";

#[derive(Clone, Default)]
struct Writer {
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
    fail_write: Option<usize>,
    fail_flush: Option<usize>,
    partial_failure: usize,
}

impl Writer {
    fn failing_write(call: usize, partial: usize) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                fail_write: Some(call),
                partial_failure: partial,
                ..State::default()
            })),
        }
    }

    fn failing_flush(call: usize) -> Self {
        let writer = Self::default();
        writer.state.borrow_mut().fail_flush = Some(call);
        writer
    }

    fn record(&self, bytes: &[u8]) {
        self.state.borrow_mut().bytes.extend_from_slice(bytes);
    }

    fn bytes(&self) -> Vec<u8> {
        self.state.borrow().bytes.clone()
    }

    fn flushes(&self) -> usize {
        self.state.borrow().flushes
    }
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.state.borrow_mut();
        state.writes += 1;
        if state.fail_write == Some(state.writes) {
            let partial = state.partial_failure.min(buf.len());
            state.bytes.extend_from_slice(&buf[..partial]);
            state.fail_write = None;
            return Err(io::Error::other("injected write failure"));
        }
        state.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut state = self.state.borrow_mut();
        state.flushes += 1;
        if state.fail_flush == Some(state.flushes) {
            state.fail_flush = None;
            return Err(io::Error::other("injected flush failure"));
        }
        Ok(())
    }
}

#[test]
fn frame_is_hidden_and_synchronized_without_clearing() -> anyhow::Result<()> {
    let mut writer = Writer::default();
    let frame = writer.clone();
    synchronized_frame(&mut writer, || {
        frame.record(b"cells-show-cursor-move-flush");
        Ok(())
    })?;

    let bytes = writer.bytes();
    assert_eq!(
        bytes,
        [BEGIN, HIDE, b"cells-show-cursor-move-flush".as_slice(), END].concat()
    );
    assert!(!bytes.windows(4).any(|window| window == b"\x1b[2J"));
    Ok(())
}

#[test]
fn draw_failure_still_ends_the_update() -> anyhow::Result<()> {
    let mut writer = Writer::default();
    let error = synchronized_frame::<_, ()>(&mut writer, || Err(anyhow!("draw failed")))
        .err()
        .context("draw should fail")?;

    assert!(error.to_string().contains("draw failed"));
    assert!(writer.bytes().ends_with(END));
    Ok(())
}

#[test]
fn partial_begin_write_failure_still_ends_the_update() -> anyhow::Result<()> {
    let mut writer = Writer::failing_write(1, 3);
    let error = synchronized_frame(&mut writer, || Ok(()))
        .err()
        .context("begin should fail")?;

    assert!(format!("{error:#}").contains("cannot begin synchronized frame"));
    assert!(writer.bytes().starts_with(&BEGIN[..3]));
    assert!(writer.bytes().ends_with(END));
    Ok(())
}

#[test]
fn begin_flush_failure_still_ends_the_update() -> anyhow::Result<()> {
    let mut writer = Writer::failing_flush(1);
    let error = synchronized_frame(&mut writer, || Ok(()))
        .err()
        .context("begin should fail")?;

    assert!(format!("{error:#}").contains("cannot begin synchronized frame"));
    assert!(writer.bytes().ends_with(END));
    Ok(())
}

#[test]
fn end_write_failure_is_reported_and_retried() -> anyhow::Result<()> {
    let mut writer = Writer::failing_write(3, 0);
    let error = synchronized_frame(&mut writer, || Ok(()))
        .err()
        .context("end should fail")?;

    assert!(format!("{error:#}").contains("cannot end synchronized frame"));
    assert!(writer.bytes().ends_with(END));
    Ok(())
}

#[test]
fn end_flush_failure_is_reported_and_retried() -> anyhow::Result<()> {
    let mut writer = Writer::failing_flush(2);
    let error = synchronized_frame(&mut writer, || Ok(()))
        .err()
        .context("end should fail")?;

    assert!(format!("{error:#}").contains("cannot end synchronized frame"));
    assert_eq!(
        writer
            .bytes()
            .windows(END.len())
            .filter(|window| *window == END)
            .count(),
        2
    );
    Ok(())
}

#[test]
fn draw_and_cleanup_failures_are_both_reported() -> anyhow::Result<()> {
    let mut writer = Writer::failing_write(3, 0);
    let error = synchronized_frame::<_, ()>(&mut writer, || Err(anyhow!("draw failed")))
        .err()
        .context("draw and end should fail")?;
    let message = format!("{error:#}");

    assert!(message.contains("draw failed"));
    assert!(message.contains("ending synchronized frame also failed"));
    assert!(message.contains("injected write failure"));
    Ok(())
}

#[test]
#[expect(clippy::panic, reason = "the test verifies cleanup during unwinding")]
fn unwind_ends_the_update() {
    let mut writer = Writer::default();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = synchronized_frame::<_, ()>(&mut writer, || panic!("injected draw panic"));
    }));

    assert!(result.is_err());
    assert!(writer.bytes().ends_with(END));
}

#[test]
fn restoration_ends_sync_and_leaves_a_visible_default_cursor() -> anyhow::Result<()> {
    let mut writer = Writer::default();
    write_terminal_restore(&mut writer, true).context("restore commands")?;

    assert_eq!(
        writer.bytes(),
        [
            END,
            b"\x1b[<1u",
            b"\x1b[?25h",
            b"\x1b[0 q",
            b"\x1b[?2004l",
            b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l",
            b"\x1b[?1049l",
        ]
        .concat()
    );
    Ok(())
}

#[test]
fn restoration_retries_failed_end_and_restores_shell_modes() -> anyhow::Result<()> {
    let mut writer = Writer::failing_write(1, 0);
    let error = write_terminal_restore(&mut writer, true)
        .err()
        .context("first synchronized end should fail")?;

    assert!(error.to_string().contains("ending synchronized update"));
    assert!(writer.bytes().starts_with(END));
    assert_shell_modes_restored(&writer);
    Ok(())
}

#[test]
fn restoration_does_not_retry_failed_keyboard_pop() -> anyhow::Result<()> {
    let mut writer = Writer::failing_write(2, 0);
    let error = write_terminal_restore(&mut writer, true)
        .err()
        .context("keyboard pop should fail")?;

    assert!(error.to_string().contains("restoring keyboard mode"));
    assert_eq!(
        writer
            .bytes()
            .windows(b"\x1b[<1u".len())
            .filter(|window| *window == b"\x1b[<1u")
            .count(),
        0
    );
    assert_shell_modes_restored(&writer);
    Ok(())
}

#[test]
fn restoration_flush_failure_is_reported_after_shell_modes() -> anyhow::Result<()> {
    let mut writer = Writer::failing_flush(1);
    let error = write_terminal_restore(&mut writer, false)
        .err()
        .context("flush should fail")?;

    assert!(error.to_string().contains("flushing terminal restoration"));
    assert_shell_modes_restored(&writer);
    Ok(())
}

#[test]
fn restoration_combines_failures_in_observed_order() -> anyhow::Result<()> {
    let mut writer = Writer::failing_write(1, 0);
    writer.state.borrow_mut().fail_flush = Some(1);
    let error = write_terminal_restore(&mut writer, false)
        .err()
        .context("restore should fail")?;

    assert_eq!(
        error.to_string(),
        "ending synchronized update: injected write failure; \
         flushing terminal restoration: injected flush failure"
    );
    assert_shell_modes_restored(&writer);
    Ok(())
}

fn assert_shell_modes_restored(writer: &Writer) {
    let bytes = writer.bytes();
    let shell_modes = [
        b"\x1b[?25h".as_slice(),
        b"\x1b[0 q",
        b"\x1b[?2004l",
        b"\x1b[?1006l\x1b[?1015l\x1b[?1003l\x1b[?1002l\x1b[?1000l",
        b"\x1b[?1049l",
    ]
    .concat();
    assert!(bytes.ends_with(&shell_modes));
    assert_eq!(writer.flushes(), 1);
}
