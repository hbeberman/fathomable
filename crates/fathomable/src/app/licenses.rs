// @okf-doc: /decisions/0088-bundled-licenses.md
//! Offline license notices in a scrollable, read-only pane.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::layout::{Layout, Line};

use super::view::Effect;
use super::{App, Popup, draw};

const NOTICES: &str = include_str!("../../assets/licenses.txt");

#[derive(Debug)]
pub(crate) struct Licenses {
    layout: Layout,
    scroll: usize,
}

impl Licenses {
    fn new(width: usize) -> Self {
        Self {
            layout: Layout::source(NOTICES, width),
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
            self.layout = Layout::source(NOTICES, width);
            self.scroll = self.layout.line_at_offset(offset).unwrap_or(0);
        }
        self.scroll_by(0, height);
    }
}

impl App {
    pub(crate) fn open_licenses(&mut self) {
        self.park_draft();
        self.close_getting_started();
        let (width, _) = content_size(self);
        self.popup = Some(Popup::Licenses(Licenses::new(width)));
    }
}

pub(crate) fn key(app: &mut App, event: KeyEvent) -> Effect {
    if event.code == KeyCode::Esc {
        app.close_popup();
        return Effect::None;
    }
    let (_, height) = content_size(app);
    let Some(Popup::Licenses(licenses)) = app.popup.as_mut() else {
        return Effect::None;
    };
    let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    let page = isize::try_from(height).unwrap_or(isize::MAX);
    match event.code {
        KeyCode::Down | KeyCode::Char('j') if plain => licenses.scroll_by(1, height),
        KeyCode::Up | KeyCode::Char('k') if plain => licenses.scroll_by(-1, height),
        KeyCode::PageDown if plain => licenses.scroll_by(page, height),
        KeyCode::PageUp if plain => licenses.scroll_by(-page, height),
        KeyCode::Char('d') if event.modifiers == KeyModifiers::CONTROL => {
            licenses.scroll_by((page / 2).max(1), height);
        }
        KeyCode::Char('u') if event.modifiers == KeyModifiers::CONTROL => {
            licenses.scroll_by(-(page / 2).max(1), height);
        }
        KeyCode::Home | KeyCode::Char('g') if plain => licenses.scroll = 0,
        KeyCode::End | KeyCode::Char('G') if plain => {
            licenses.scroll = licenses.layout.lines().len().saturating_sub(height);
        }
        _ => {}
    }
    Effect::None
}

pub(crate) fn wheel(app: &mut App, delta: isize) -> Effect {
    let (_, height) = content_size(app);
    if let Some(Popup::Licenses(licenses)) = app.popup.as_mut() {
        licenses.scroll_by(delta, height);
    }
    Effect::None
}

pub(crate) fn resize(app: &mut App) {
    let (width, height) = content_size(app);
    if let Some(Popup::Licenses(licenses)) = app.popup.as_mut() {
        licenses.resize(width, height);
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
mod tests;
