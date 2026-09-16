// @okf-doc: /decisions/0082-three-tool-review-core.md
//! Human-invoked agent wake entry point.

use crate::app::App;

impl App {
    /// Report the intentionally unimplemented handoff.
    pub(crate) fn wake(&mut self) {
        // A future wake may optionally hand every open thread, plus a user
        // instruction, to a chosen chat. The handoff mechanism is undecided.
        self.notice("Wake agent is not yet implemented");
    }
}

#[cfg(test)]
mod tests {
    use crate::app::testing;

    #[test]
    fn wake_reports_the_stub_without_changing_viewer_state() -> anyhow::Result<()> {
        let dir = testing::workspace("wake-stub", testing::README)?;
        let mut app = testing::app(&dir)?;
        let path = app.current_path().to_path_buf();
        let queue = app.queue().clone();

        app.wake();

        assert_eq!(app.message(), Some("Wake agent is not yet implemented"));
        assert_eq!(app.current_path(), path);
        assert_eq!(app.queue(), &queue);
        assert!(app.popup().is_none());
        Ok(())
    }
}
