// @okf-doc: /decisions/0081-the-menu-bar.md
//! Read-only setup steps for connecting an agent to Fathomable over MCP.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::layout::{Layout, Line};

use super::view::Effect;
use super::{App, Popup, draw};

const SETUP: &str = "\
Register Fathomable as a stdio MCP server in your agent host.
It binds to the launch workspace unless you supply a checkout path.

Copilot CLI
  copilot mcp add fathomable -- fathomable --mcp

Claude Code
  claude mcp add --scope user fathomable -- fathomable --mcp

Codex CLI
  codex mcp add fathomable -- fathomable --mcp

VS Code
  code --add-mcp '{\"name\":\"fathomable\",\"type\":\"stdio\",\"command\":\"fathomable\",\"args\":[\"--mcp\",\"${workspaceFolder}\"]}'

Launch the agent from the checkout you want to review. To bind an
explicit checkout, run fathomable --mcp /path/to/checkout instead.

Restart the agent host's MCP connection after changing the setup.
The server exposes threads, thread_start, and thread_reply.

Full guide
  https://github.com/hbeberman/fathomable/blob/main/docs/guide.md#connect-an-agent
";

#[derive(Debug)]
pub(crate) struct McpSetup {
    layout: Layout,
    scroll: usize,
}

impl McpSetup {
    fn new(width: usize) -> Self {
        Self {
            layout: Layout::source(SETUP, width),
            scroll: 0,
        }
    }

    pub(crate) fn visible_lines(&self, height: usize) -> impl Iterator<Item = &Line> {
        self.layout.lines().iter().skip(self.scroll).take(height)
    }

    fn scroll_by(&mut self, delta: isize, height: usize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.layout.lines().len().saturating_sub(height));
    }

    fn resize(&mut self, width: usize, height: usize) {
        if self.layout.width() != width {
            let offset = self
                .layout
                .lines()
                .get(self.scroll)
                .and_then(Line::source)
                .map_or(0, |range| range.start);
            self.layout = Layout::source(SETUP, width);
            self.scroll = self.layout.line_at_offset(offset).unwrap_or(0);
        }
        self.scroll_by(0, height);
    }
}

impl App {
    pub(crate) fn open_mcp_setup(&mut self) {
        self.park_draft();
        self.close_getting_started();
        let (width, _) = content_size(self);
        self.popup = Some(Popup::McpSetup(McpSetup::new(width)));
    }
}

pub(crate) fn key(app: &mut App, event: KeyEvent) -> Effect {
    if event.code == KeyCode::Esc {
        app.close_popup();
        return Effect::None;
    }
    let (_, height) = content_size(app);
    let Some(Popup::McpSetup(setup)) = app.popup.as_mut() else {
        return Effect::None;
    };
    let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    let page = isize::try_from(height).unwrap_or(isize::MAX);
    match event.code {
        KeyCode::Down | KeyCode::Char('j') if plain => setup.scroll_by(1, height),
        KeyCode::Up | KeyCode::Char('k') if plain => setup.scroll_by(-1, height),
        KeyCode::PageDown if plain => setup.scroll_by(page, height),
        KeyCode::PageUp if plain => setup.scroll_by(-page, height),
        KeyCode::Char('d') if event.modifiers == KeyModifiers::CONTROL => {
            setup.scroll_by((page / 2).max(1), height);
        }
        KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => {
            setup.scroll_by(-(page / 2).max(1), height);
        }
        KeyCode::Home | KeyCode::Char('g') if plain => setup.scroll = 0,
        KeyCode::End | KeyCode::Char('G') if plain => {
            setup.scroll = setup.layout.lines().len().saturating_sub(height);
        }
        _ => {}
    }
    Effect::None
}

pub(crate) fn wheel(app: &mut App, delta: isize) -> Effect {
    let (_, height) = content_size(app);
    if let Some(Popup::McpSetup(setup)) = app.popup.as_mut() {
        setup.scroll_by(delta, height);
    }
    Effect::None
}

pub(crate) fn resize(app: &mut App) {
    let (width, height) = content_size(app);
    if let Some(Popup::McpSetup(setup)) = app.popup.as_mut() {
        setup.resize(width, height);
    }
}

fn content_size(app: &App) -> (usize, usize) {
    let area = draw::report_area(app);
    (
        usize::from(area.width.saturating_sub(2)).max(1),
        usize::from(area.height.saturating_sub(2)).max(1),
    )
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use crossterm::event::KeyCode;

    use crate::app::menu_bar::{self, Submenu, Target};
    use crate::app::{Popup, testing};

    #[test]
    fn command_and_help_menu_open_mcp_setup() -> anyhow::Result<()> {
        let dir = testing::workspace("mcp-setup", testing::README)?;
        let mut app = testing::source_app(&dir)?;

        app.command("mcp");
        let screen = testing::screen(&app)?.join("\n");
        assert!(matches!(app.popup(), Some(Popup::McpSetup(_))));
        assert!(screen.contains("MCP Setup"));
        assert!(screen.contains("copilot mcp add fathomable"));
        assert!(screen.contains("threads, thread_start, and thread_reply"));
        testing::press_key(&mut app, KeyCode::Esc);
        assert!(app.popup().is_none());

        let rows = menu_bar::submenu_rows(&app, Submenu::Help);
        let item = rows
            .iter()
            .filter_map(menu_bar::Row::item)
            .find(|item| item.target == Target::McpSetup)
            .context("MCP Setup is missing from Help")?;
        assert_eq!(item.label, "MCP Setup");
        assert_eq!(item.hint, ":mcp");
        app.run_title_target(item.target);
        assert!(matches!(app.popup(), Some(Popup::McpSetup(_))));
        Ok(())
    }
}
