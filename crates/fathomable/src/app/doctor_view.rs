// @okf-doc: /decisions/0081-the-menu-bar.md
//! The in-app Doctor overlay over the shared CLI diagnostic report.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::layout::display_width;

use crate::app::view::Effect;
use crate::app::{App, Popup};
use crate::doctor::{Kind, Report};

#[derive(Debug)]
pub(crate) struct Doctor {
    report: Report,
    scroll: usize,
}

impl Doctor {
    pub(crate) const fn new(report: Report) -> Self {
        Self { report, scroll: 0 }
    }

    pub(crate) const fn report(&self) -> &Report {
        &self.report
    }

    pub(crate) const fn scroll(&self) -> usize {
        self.scroll
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

    fn scroll_by(&mut self, delta: isize, width: usize, height: usize) {
        let total = self.visual_lines(width).len();
        let max = total.saturating_sub(height);
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }

    fn clamp(&mut self, width: usize, height: usize) {
        self.scroll_by(0, width, height);
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if display_width(text) <= width {
        return vec![text.to_owned()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let joined = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if display_width(&joined) <= width {
            current = joined;
        } else {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut chunks = hard_wrap(word, width);
            current = chunks.pop().unwrap_or_default();
            lines.extend(chunks);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cells = display_width(&ch.to_string());
        if used + cells > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(ch);
        used += cells;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn state_mut(app: &mut App) -> Option<&mut Doctor> {
    match app.popup.as_mut() {
        Some(Popup::Doctor(doctor)) => Some(doctor),
        _ => None,
    }
}

pub(crate) fn key(app: &mut App, event: KeyEvent) -> Effect {
    let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    let (width, height) = content_size(app);
    match event.code {
        KeyCode::Esc => app.close_popup(),
        KeyCode::Char('r') if plain => app.open_doctor(),
        KeyCode::Down | KeyCode::Char('j') if plain => {
            if let Some(doctor) = state_mut(app) {
                doctor.scroll_by(1, width, height);
            }
        }
        KeyCode::Up | KeyCode::Char('k') if plain => {
            if let Some(doctor) = state_mut(app) {
                doctor.scroll_by(-1, width, height);
            }
        }
        KeyCode::PageDown => {
            if let Some(doctor) = state_mut(app) {
                doctor.scroll_by(height.cast_signed(), width, height);
            }
        }
        KeyCode::PageUp => {
            if let Some(doctor) = state_mut(app) {
                doctor.scroll_by(-height.cast_signed(), width, height);
            }
        }
        KeyCode::Home | KeyCode::Char('g') if plain => {
            if let Some(doctor) = state_mut(app) {
                doctor.scroll = 0;
            }
        }
        KeyCode::End | KeyCode::Char('G') if plain => {
            if let Some(doctor) = state_mut(app) {
                doctor.scroll = usize::MAX;
                doctor.clamp(width, height);
            }
        }
        _ => {}
    }
    Effect::None
}

pub(crate) fn wheel(app: &mut App, delta: isize) -> Effect {
    let (width, height) = content_size(app);
    if let Some(doctor) = state_mut(app) {
        doctor.scroll_by(delta, width, height);
    }
    Effect::None
}

pub(crate) fn resize(app: &mut App) {
    let (width, height) = content_size(app);
    if let Some(doctor) = state_mut(app) {
        doctor.clamp(width, height);
    }
}

pub(crate) fn content_size(app: &App) -> (usize, usize) {
    (
        app.size().0.saturating_sub(4).max(1),
        app.pane_rows().saturating_sub(2).max(1),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fathomable_core::XdgDirs;
    use fathomable_testing::TempDir;

    use crate::app::{Popup, testing};

    use super::{Doctor, key, wrap};

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
