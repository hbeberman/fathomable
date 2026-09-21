// @okf-doc: /decisions/0081-the-menu-bar.md
//! The in-app Doctor overlay over the shared CLI diagnostic report.

use fathomable_core::layout::display_width;

use crate::app::report::{Scroll, wrap};
use crate::doctor::{Kind, Report};

#[derive(Debug)]
pub(crate) struct Doctor {
    report: Report,
    pub(super) scroll: Scroll,
}

impl Doctor {
    pub(crate) const fn new(report: Report) -> Self {
        Self {
            report,
            scroll: Scroll::new(),
        }
    }

    pub(crate) const fn report(&self) -> &Report {
        &self.report
    }

    pub(crate) fn visual_lines(&self, width: usize) -> Vec<(Kind, String)> {
        let mut lines = Vec::new();
        for line in self.report.lines() {
            let prefix = match line.kind {
                Kind::Section | Kind::Info => "",
                Kind::Ok => "ok    ",
                Kind::Warn => "WARN  ",
                Kind::Fail => "FAIL  ",
            };
            let indent = if matches!(line.kind, Kind::Ok | Kind::Warn | Kind::Fail) {
                "  "
            } else {
                ""
            };
            let available = width
                .saturating_sub(display_width(indent) + display_width(prefix))
                .max(1);
            let wrapped = wrap(&line.text, available);
            for (index, text) in wrapped.into_iter().enumerate() {
                let lead = if index == 0 {
                    format!("{indent}{prefix}")
                } else {
                    " ".repeat(display_width(indent) + display_width(prefix))
                };
                lines.push((line.kind, format!("{lead}{text}")));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fathomable_core::XdgDirs;
    use fathomable_testing::TempDir;

    use crate::app::{Popup, testing};

    use super::{Doctor, wrap};
    use crate::app::report::key;

    #[test]
    fn long_unbroken_diagnostic_values_wrap_without_losing_text() {
        let text = "/a/very/long/path/that/has/no/whitespace/config.kdl";
        let lines = wrap(text, 12);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| line.chars().count() <= 12));
        assert_eq!(lines.concat(), text);
    }

    #[test]
    fn warnings_have_a_distinct_tui_prefix() -> anyhow::Result<()> {
        let fixture = TempDir::new("doctor-view-warning")?;
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755))?;
        let workspace = fixture.0.join("workspace");
        fs::create_dir(&workspace)?;
        let state_home = fixture.0.join("shared-state");
        fs::create_dir(&state_home)?;
        fs::set_permissions(&state_home, fs::Permissions::from_mode(0o775))?;
        let dirs = XdgDirs::resolve(|name| {
            (name == "XDG_STATE_HOME").then(|| state_home.clone().into_os_string())
        });
        let report = crate::doctor::collect(&dirs, None, Some(&workspace), Some((100, 30)));
        let lines = Doctor::new(report).visual_lines(120);
        assert!(
            lines
                .iter()
                .any(|(_, line)| line.starts_with("  WARN  state ancestor is group-writable:"))
        );
        Ok(())
    }

    #[test]
    fn rerun_clears_a_remediated_warning() -> anyhow::Result<()> {
        let fixture = testing::workspace("doctor-refresh-warning", testing::README)?;
        let state_home = fixture.0.join("shared-state");
        fs::create_dir(&state_home)?;
        fs::set_permissions(&state_home, fs::Permissions::from_mode(0o775))?;
        let dirs = XdgDirs::resolve(|name| {
            (name == "XDG_STATE_HOME").then(|| state_home.clone().into_os_string())
        });
        let mut app = testing::AppBuilder::new(&fixture)
            .options(move |mut options| {
                options.dirs = dirs;
                options
            })
            .build()?;
        app.open_doctor();
        let Some(Popup::Doctor(doctor)) = app.popup() else {
            return Err(anyhow::anyhow!("doctor popup did not open"));
        };
        assert!(doctor.report().has_warnings());

        fs::set_permissions(&state_home, fs::Permissions::from_mode(0o755))?;
        key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        );
        let Some(Popup::Doctor(doctor)) = app.popup() else {
            return Err(anyhow::anyhow!("doctor popup did not remain open"));
        };
        assert!(!doctor.report().has_warnings());
        Ok(())
    }
}
