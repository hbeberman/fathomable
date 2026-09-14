// @okf-doc: /decisions/0079-list-focus-language.md
//! List selection is separate from the pane that currently owns navigation.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::author::CURSOR_BAR;
use super::{Theme, on_surface};
use crate::app::{App, Focus};

/// Whether navigation keys reach a list, rather than another pane or overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Navigation {
    Active,
    Inactive,
}

impl Navigation {
    pub(super) fn for_pane(app: &App, pane: Focus) -> Self {
        if app.focus() == pane && app.popup().is_none() && app.prefix().is_empty() {
            Self::Active
        } else {
            Self::Inactive
        }
    }

    pub(super) fn selection(self, selected: bool) -> Selection {
        if selected {
            match self {
                Self::Active => Selection::Active,
                Self::Inactive => Selection::Inactive,
            }
        } else {
            Selection::None
        }
    }
}

/// A row's remembered selection, and whether its list currently owns keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Selection {
    None,
    Active,
    Inactive,
}

impl Selection {
    pub(super) fn style(self, theme: &Theme) -> Style {
        match self {
            Self::None => Style::default(),
            Self::Active => theme.list_active,
            Self::Inactive => theme.list_inactive,
        }
    }

    pub(super) fn marker(self, theme: &Theme) -> Span<'static> {
        match self {
            Self::Active => {
                Span::styled(CURSOR_BAR, on_surface(Style::default(), theme.list_cursor))
            }
            Self::None | Self::Inactive => Span::raw(" "),
        }
    }

    /// Patch every span, including explicit backgrounds, so the row is one band.
    pub(super) fn paint<'a>(self, theme: &Theme, mut line: Line<'a>) -> Line<'a> {
        let style = self.style(theme);
        line.style = line.style.patch(style);
        for span in &mut line.spans {
            span.style = span.style.patch(style);
        }
        // A selection foreground must not recolour the navigation marker.
        if self == Self::Active
            && let Some(marker) = line.spans.first_mut()
        {
            *marker = self.marker(theme);
        }
        line
    }
}

/// An ancestor is context, not another keyboard cursor.
pub(super) fn context_marker(theme: &Theme, marked: bool) -> Span<'static> {
    if marked {
        Span::styled(CURSOR_BAR, theme.info)
    } else {
        Span::raw(" ")
    }
}

#[cfg(test)]
mod tests;
