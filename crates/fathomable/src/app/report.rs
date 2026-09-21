// @okf-doc: /decisions/0081-the-menu-bar.md
//! Shared wrapping and navigation for the live Status and Doctor reports.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::layout::{display_width, graphemes};

use super::view::Effect;
use super::{App, Popup, draw};

#[derive(Debug, Default)]
pub(crate) struct Scroll {
    offset: usize,
}

impl Scroll {
    pub(crate) const fn new() -> Self {
        Self { offset: 0 }
    }

    pub(crate) fn position(&self, total: usize, height: usize) -> usize {
        self.offset.min(total.saturating_sub(height))
    }

    fn scroll_by(&mut self, delta: isize, total: usize, height: usize) {
        self.offset = self
            .position(total, height)
            .saturating_add_signed(delta)
            .min(total.saturating_sub(height));
    }
}

pub(crate) fn status_lines(app: &App, width: usize) -> Vec<(String, String)> {
    let rows = app.status_lines();
    let width = width.max(1);
    let label_width = rows
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0)
        .min(width.saturating_sub(3) / 2);
    let mut lines = Vec::new();
    for (label, value) in rows {
        if label_width == 0 {
            lines.extend(
                wrap(&label, width)
                    .into_iter()
                    .map(|text| (text, String::new())),
            );
            lines.extend(
                wrap(&value, width)
                    .into_iter()
                    .map(|text| (String::new(), text)),
            );
            continue;
        }
        let labels = wrap(&label, label_width);
        let values = wrap(&value, width - label_width - 2);
        for index in 0..labels.len().max(values.len()) {
            let label = labels.get(index).map_or("", String::as_str);
            let padding = " ".repeat(label_width.saturating_sub(display_width(label)) + 2);
            lines.push((
                format!("{label}{padding}"),
                values.get(index).cloned().unwrap_or_default(),
            ));
        }
    }
    lines
}

pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    text.split('\n')
        .flat_map(|line| wrap_line(line, width.max(1)))
        .collect()
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
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
    for (_, grapheme) in graphemes(text) {
        let cells = display_width(grapheme);
        if used + cells > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push_str(grapheme);
        used += cells;
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn state_mut(app: &mut App) -> Option<&mut Scroll> {
    match app.popup.as_mut() {
        Some(Popup::Status(scroll)) => Some(scroll),
        Some(Popup::Doctor(doctor)) => Some(&mut doctor.scroll),
        _ => None,
    }
}

fn line_count(app: &App, width: usize) -> usize {
    match app.popup() {
        Some(Popup::Status(_)) => status_lines(app, width).len(),
        Some(Popup::Doctor(doctor)) => doctor.visual_lines(width).len(),
        _ => 0,
    }
}

pub(crate) fn key(app: &mut App, event: KeyEvent) -> Effect {
    let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    if event.code == KeyCode::Esc {
        app.close_popup();
        return Effect::None;
    }
    if event.code == KeyCode::Char('r') && plain && matches!(app.popup(), Some(Popup::Doctor(_))) {
        app.open_doctor();
        return Effect::None;
    }
    let (width, height) = content_size(app);
    let total = line_count(app, width);
    let Some(scroll) = state_mut(app) else {
        return Effect::None;
    };
    let page = isize::try_from(height).unwrap_or(isize::MAX);
    match event.code {
        KeyCode::Down | KeyCode::Char('j') if plain => scroll.scroll_by(1, total, height),
        KeyCode::Up | KeyCode::Char('k') if plain => scroll.scroll_by(-1, total, height),
        KeyCode::PageDown => scroll.scroll_by(page, total, height),
        KeyCode::PageUp => scroll.scroll_by(-page, total, height),
        KeyCode::Home | KeyCode::Char('g') if plain => scroll.offset = 0,
        KeyCode::End | KeyCode::Char('G') if plain => {
            scroll.offset = total.saturating_sub(height);
        }
        _ => {}
    }
    Effect::None
}

pub(crate) fn wheel(app: &mut App, delta: isize) -> Effect {
    let (width, height) = content_size(app);
    let total = line_count(app, width);
    if let Some(scroll) = state_mut(app) {
        scroll.scroll_by(delta, total, height);
    }
    Effect::None
}

pub(crate) fn resize(app: &mut App) {
    if matches!(app.popup(), Some(Popup::Status(_) | Popup::Doctor(_))) {
        wheel(app, 0);
    }
}

pub(crate) fn content_size(app: &App) -> (usize, usize) {
    let area = draw::report_area(app);
    (
        usize::from(area.width.saturating_sub(2)).max(1),
        usize::from(area.height.saturating_sub(2)).max(1),
    )
}

#[cfg(test)]
mod tests;
