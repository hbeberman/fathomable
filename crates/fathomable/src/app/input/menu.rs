// @okf-doc: /decisions/0050-mouse-menus-and-gestures.md
//! Context and pane-settings menus (ADR 0050, ADR 0068): the actions
//! that apply under the pointer or to the Files pane, each showing the
//! key the binding table gives it, and the grids that the drawn menus
//! and the mouse share so a click lands on the drawn entry.
//!
//! A [`Menu`] is one more [`Popup`]. Its entries act on the cursor the
//! right-click placed, so they are ordinary [`Action`]s run through
//! [`App::act`]; the menu needs no target of its own.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::config::DiffMode;

use super::bindings::{self, Action, Chord, Keys, Match, MenuSection, Where};
use crate::app::threads::pane::{PanePoint, PaneScope};
use crate::app::threads::words::Words;
use crate::app::view::{Effect, Mode};
use crate::app::{App, Focus, Popup};
use fathomable_core::layout::display_width;
use fathomable_core::tree::Tree;

/// One row of a context menu.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The key as the table spells it, shown beside the label.
    key: String,
    /// The chords that key is, matched against what is typed.
    keys: Keys,
    label: String,
    /// What the entry runs. The key shown may belong to a sibling
    /// action: `dd` arms a delete, the entry deletes at once.
    action: Option<Action>,
    /// `Some` for a persistent toggle row, active or inactive.
    checked: Option<bool>,
    enabled: bool,
    active: bool,
}

impl Entry {
    #[must_use]
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub(crate) fn action(&self) -> Option<Action> {
        self.action
    }

    #[must_use]
    pub(crate) fn is_separator(&self) -> bool {
        self.action.is_none()
    }

    #[must_use]
    pub(crate) fn checked(&self) -> Option<bool> {
        self.checked
    }

    #[must_use]
    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub(crate) const fn active(&self) -> bool {
        self.active
    }
}

/// A context menu: what it acts on, its entries, and the cell it opened
/// at.
#[derive(Debug, Clone)]
pub(crate) struct Menu {
    title: String,
    place: Where,
    entries: Vec<Entry>,
    column: usize,
    row: usize,
    right_aligned: bool,
}

impl Menu {
    fn new(title: impl Into<String>, place: Where, column: usize, row: usize) -> Self {
        Self {
            title: title.into(),
            place,
            entries: Vec::new(),
            column,
            row,
            right_aligned: false,
        }
    }

    /// A pane-title menu whose top border sits immediately below its header.
    fn below_header(title: impl Into<String>, place: Where, column: usize, row: usize) -> Self {
        Self::new(title, place, column, row.saturating_add(1))
    }

    fn under_right_edge(title: impl Into<String>, place: Where, right: usize, row: usize) -> Self {
        let mut menu = Self::new(title, place, right, row.saturating_add(1));
        menu.right_aligned = true;
        menu
    }

    /// Add an entry showing `shown`'s key and running `run`; an action
    /// the table does not bind on this place adds nothing, so no entry
    /// is ever keyless.
    fn push(&mut self, shown: Action, run: Action, label: impl Into<String>) {
        self.push_entry(shown, run, label, None);
    }

    /// Add a persistent toggle entry carrying its current checked state.
    fn push_toggle(&mut self, shown: Action, run: Action, label: impl Into<String>, active: bool) {
        self.push_entry(shown, run, label, Some(active));
    }

    fn push_toggle_enabled(
        &mut self,
        shown: Action,
        run: Action,
        label: impl Into<String>,
        active: bool,
        enabled: bool,
    ) {
        self.push_toggle(shown, run, label, active);
        if let Some(entry) = self.entries.last_mut() {
            entry.enabled = enabled;
        }
    }

    fn push_entry(
        &mut self,
        shown: Action,
        run: Action,
        label: impl Into<String>,
        checked: Option<bool>,
    ) {
        if let Some(keys) = bindings::first_keys(self.place, shown) {
            self.entries.push(Entry {
                key: bindings::menu_spell(keys),
                keys,
                label: label.into(),
                action: Some(run),
                checked,
                enabled: true,
                active: false,
            });
        }
    }

    fn push_choice(&mut self, action: Action, label: impl Into<String>, active: bool) {
        if let Some(keys) = bindings::first_keys(self.place, action) {
            self.entries.push(Entry {
                key: bindings::menu_spell(keys),
                keys,
                label: label.into(),
                action: Some(action),
                checked: None,
                enabled: true,
                active,
            });
        }
    }

    /// Add a visual grouping rule that cannot be selected or invoked.
    fn separator(&mut self) {
        self.entries.push(Entry {
            key: String::new(),
            keys: &[],
            label: String::new(),
            action: None,
            checked: None,
            enabled: false,
            active: false,
        });
    }

    /// The row in the pill colour naming what the menu acts on.
    #[must_use]
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What `typed` means here: an entry's whole key fires it, the start
    /// of one waits, anything else is a miss that closes the menu.
    #[must_use]
    pub(crate) fn typed(&self, typed: &[Chord]) -> Match {
        if let Some(action) = self
            .entries
            .iter()
            .filter(|entry| entry.enabled)
            .find(|entry| entry.keys == typed)
            .and_then(Entry::action)
        {
            return Match::Exact(action);
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.keys.len() > typed.len() && entry.keys.starts_with(typed))
        {
            return Match::Prefix;
        }
        Match::Miss
    }

    /// Where the menu sits on a `width` by `height` screen: its top-left
    /// corner at the pointer, shifted left or up to stay inside.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn grid(&self, width: usize, height: usize) -> Grid {
        self.grid_in(width, 0, height)
    }

    /// Place the menu inside a vertical area beginning at `top`.
    #[must_use]
    pub(crate) fn grid_in(&self, width: usize, top: usize, height: usize) -> Grid {
        let (key_width, label_width) = measure(
            self.entries
                .iter()
                .map(|entry| (entry.key.as_str(), entry.label.as_str())),
        );
        let marker_width = usize::from(
            self.entries
                .iter()
                .any(|entry| entry.checked.is_some() || entry.active),
        ) * 2;
        let action_width = marker_width + label_width + 1 + key_width;
        let box_width = (action_width + 2)
            .max(display_width(&self.title) + 4)
            .min(width);
        let box_height = (self.entries.len() + 2).min(height);
        Grid {
            x: if self.right_aligned {
                self.column
                    .saturating_sub(box_width)
                    .min(width.saturating_sub(box_width))
            } else {
                self.column.min(width.saturating_sub(box_width))
            },
            y: self
                .row
                .max(top)
                .min(top + height.saturating_sub(box_height)),
            width: box_width,
            height: box_height,
            rows: self.entries.len(),
            columns: 1,
            key_width,
            label_width,
            count: self.entries.len(),
        }
    }
}

/// The compact mode chooser anchored to a File or Reviews header control.
#[derive(Debug)]
pub(crate) struct ModeMenu {
    menu: Menu,
    selected: usize,
}

impl ModeMenu {
    pub(crate) fn new(app: &App, right: usize, row: usize) -> Self {
        let mut menu = Menu::under_right_edge("Diff", Where::Any, right, row);
        let mode = app.diff_mode();
        for (action, label, choice) in [
            (Action::DiffNormal, "Normal diff", DiffMode::Normal),
            (Action::DiffUnified, "Unified diff", DiffMode::Unified),
            (Action::DiffOff, "Diff off", DiffMode::Off),
        ] {
            menu.push_choice(action, label, mode == choice);
        }
        let selected = match mode {
            DiffMode::Normal => 0,
            DiffMode::Unified => 1,
            DiffMode::Off => 2,
        };
        Self { menu, selected }
    }

    #[must_use]
    pub(crate) const fn menu(&self) -> &Menu {
        &self.menu
    }

    #[must_use]
    pub(crate) const fn selected(&self) -> usize {
        self.selected
    }

    fn move_by(&mut self, delta: isize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.menu.entries.len().saturating_sub(1));
    }

    fn selected_action(&self) -> Option<Action> {
        self.menu.entries.get(self.selected).and_then(Entry::action)
    }
}

/// The widest key and the widest label among `entries`.
fn measure<'a>(entries: impl Iterator<Item = (&'a str, &'a str)>) -> (usize, usize) {
    entries.fold((1, 1), |(key, label), (k, l)| {
        (key.max(display_width(k)), label.max(display_width(l)))
    })
}

/// Where a key menu's rows sit inside a rounded titled border. The drawing
/// lays the entries out by it and the mouse reads it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rows: usize,
    pub(crate) columns: usize,
    pub(crate) key_width: usize,
    pub(crate) label_width: usize,
    pub(crate) count: usize,
}

impl Grid {
    /// The cells one column of entries takes.
    #[must_use]
    pub(crate) fn column_width(key_width: usize, label_width: usize) -> usize {
        key_width + 2 + label_width + 3
    }
}

/// Preferred body height for a compact which-key card.
///
/// Taller cards are allowed when a narrower terminal cannot hold enough
/// readable columns.
const PREFERRED_HINT_ROWS: usize = 8;

/// Smallest comfortable label column before the card grows vertically.
const MIN_HINT_LABEL_WIDTH: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HintCell {
    Entry(usize),
    Rule,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HintColumn {
    pub(crate) width: usize,
    pub(crate) key_width: usize,
    pub(crate) label_width: usize,
    pub(crate) cells: Vec<HintCell>,
}

/// Responsive geometry for a pending-prefix hint card.
///
/// Callers provide semantic sections only. The constructor chooses columns,
/// splits sections only between entries, and keeps drawing and hit-testing on
/// the same layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HintGrid {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rows: usize,
    pub(crate) columns: Vec<HintColumn>,
    pub(crate) insufficient_space: bool,
}

impl HintGrid {
    #[must_use]
    pub(crate) fn bottom(
        sections: &[MenuSection],
        title: &str,
        x: usize,
        y: usize,
        pane_width: usize,
        pane_height: usize,
    ) -> Self {
        let entries = sections
            .iter()
            .flat_map(MenuSection::entries)
            .collect::<Vec<_>>();
        let section_ids = sections
            .iter()
            .enumerate()
            .flat_map(|(section, group)| std::iter::repeat_n(section, group.entries().len()))
            .collect::<Vec<_>>();
        let entry_widths = entries
            .iter()
            .map(|entry| (display_width(&entry.key()), display_width(entry.label())))
            .collect::<Vec<_>>();
        let full_label_width = entry_widths
            .iter()
            .map(|(_, label_width)| *label_width)
            .max()
            .unwrap_or(1);
        let available_rows = pane_height.saturating_sub(2);
        let plan = responsive_hint_plan(
            &section_ids,
            &entry_widths,
            full_label_width,
            pane_width,
            available_rows,
        );
        let Some(plan) = plan else {
            let height = pane_height.min(3);
            return Self {
                x,
                y: (y + pane_height).saturating_sub(height),
                width: pane_width,
                height,
                rows: height.saturating_sub(2),
                columns: Vec::new(),
                insufficient_space: true,
            };
        };

        let mut columns = Vec::with_capacity(plan.ends.len());
        let mut start = 0;
        for end in plan.ends {
            let (key_width, label_width) =
                hint_column_dimensions(&entry_widths, start, end, plan.label_width);
            let mut cells = Vec::with_capacity(plan.rows);
            for index in start..end {
                if index > start && section_ids[index] != section_ids[index - 1] {
                    cells.push(HintCell::Rule);
                }
                cells.push(HintCell::Entry(index));
            }
            cells.resize(plan.rows, HintCell::Empty);
            columns.push(HintColumn {
                width: hint_column_width(key_width, label_width),
                key_width,
                label_width,
                cells,
            });
            start = end;
        }
        let natural_width = hint_box_width(&columns);
        if let Some(first) = columns.first_mut() {
            let title_width = display_width(title).saturating_add(2);
            let title_padding = title_width
                .saturating_sub(first.width)
                .min(pane_width.saturating_sub(natural_width));
            first.width = first.width.saturating_add(title_padding);
        }
        let width = hint_box_width(&columns);
        let height = plan.rows + 2;
        Self {
            x: x + pane_width.saturating_sub(width),
            y: (y + pane_height).saturating_sub(height),
            width,
            height,
            rows: plan.rows,
            columns,
            insufficient_space: false,
        }
    }

    #[must_use]
    pub(crate) fn contains(&self, column: usize, row: usize) -> bool {
        column >= self.x
            && column < self.x + self.width
            && row >= self.y
            && row < self.y + self.height
    }

    #[must_use]
    pub(crate) fn column_x(&self, column: usize) -> Option<usize> {
        let before = self.columns.get(..column)?;
        Some(self.x + 1 + before.iter().map(|column| column.width + 1).sum::<usize>())
    }

    #[must_use]
    pub(crate) fn entry_at(&self, column: usize, row: usize) -> Option<usize> {
        let body_row = row.checked_sub(self.y + 1)?;
        if body_row >= self.rows {
            return None;
        }
        self.columns
            .iter()
            .enumerate()
            .find_map(|(index, hint_column)| {
                let start = self.column_x(index)?;
                (column >= start && column < start + hint_column.width)
                    .then(|| hint_column.cells.get(body_row).copied())
                    .flatten()
                    .and_then(|cell| match cell {
                        HintCell::Entry(entry) => Some(entry),
                        HintCell::Rule | HintCell::Empty => None,
                    })
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HintPlan {
    label_width: usize,
    rows: usize,
    ends: Vec<usize>,
    cuts: usize,
    squared_heights: usize,
}

fn responsive_hint_plan(
    section_ids: &[usize],
    entry_widths: &[(usize, usize)],
    full_label_width: usize,
    pane_width: usize,
    available_rows: usize,
) -> Option<HintPlan> {
    let preferred_rows = available_rows.min(PREFERRED_HINT_ROWS);
    let comfortable_label_width = full_label_width.min(MIN_HINT_LABEL_WIDTH);
    hint_plan(
        section_ids,
        entry_widths,
        full_label_width,
        comfortable_label_width,
        pane_width,
        preferred_rows,
    )
    .or_else(|| {
        hint_plan(
            section_ids,
            entry_widths,
            full_label_width,
            comfortable_label_width,
            pane_width,
            available_rows,
        )
    })
    .or_else(|| {
        (0..comfortable_label_width).rev().find_map(|label_width| {
            hint_plan_at_label_width(
                section_ids,
                entry_widths,
                label_width,
                pane_width,
                preferred_rows,
            )
            .or_else(|| {
                hint_plan_at_label_width(
                    section_ids,
                    entry_widths,
                    label_width,
                    pane_width,
                    available_rows,
                )
            })
        })
    })
}

fn hint_plan(
    section_ids: &[usize],
    entry_widths: &[(usize, usize)],
    full_label_width: usize,
    minimum_label_width: usize,
    pane_width: usize,
    row_limit: usize,
) -> Option<HintPlan> {
    if section_ids.is_empty() || row_limit == 0 {
        return None;
    }
    for label_width in (minimum_label_width..=full_label_width).rev() {
        if let Some(plan) = hint_plan_at_label_width(
            section_ids,
            entry_widths,
            label_width,
            pane_width,
            row_limit,
        ) {
            return Some(plan);
        }
    }
    None
}

fn hint_plan_at_label_width(
    section_ids: &[usize],
    entry_widths: &[(usize, usize)],
    label_width: usize,
    pane_width: usize,
    row_limit: usize,
) -> Option<HintPlan> {
    let mut best = None;
    for columns in 1..=section_ids.len() {
        let Some(partition) = best_partition(
            section_ids,
            entry_widths,
            label_width,
            columns,
            row_limit,
            pane_width,
        ) else {
            continue;
        };
        let candidate = HintPlan {
            label_width,
            rows: partition.tallest,
            ends: partition.ends,
            cuts: partition.cuts,
            squared_heights: partition.squared_heights,
        };
        if best
            .as_ref()
            .is_none_or(|current| better_hint_plan(&candidate, current))
        {
            best = Some(candidate);
        }
    }
    best
}

fn better_hint_plan(candidate: &HintPlan, current: &HintPlan) -> bool {
    candidate.cuts < current.cuts
        || (candidate.cuts == current.cuts
            && (candidate.ends.len() < current.ends.len()
                || (candidate.ends.len() == current.ends.len()
                    && (candidate.squared_heights < current.squared_heights
                        || (candidate.squared_heights == current.squared_heights
                            && candidate.ends > current.ends)))))
}

fn hint_box_width(columns: &[HintColumn]) -> usize {
    columns
        .iter()
        .map(|column| column.width)
        .sum::<usize>()
        .saturating_add(columns.len().saturating_sub(1))
        .saturating_add(2)
}

fn hint_column_width(key_width: usize, label_width: usize) -> usize {
    key_width + usize::from(label_width > 0) * (2 + label_width + 3)
}

fn hint_column_dimensions(
    entry_widths: &[(usize, usize)],
    start: usize,
    end: usize,
    label_width: usize,
) -> (usize, usize) {
    entry_widths[start..end]
        .iter()
        .fold((1, 0), |(key, label), (entry_key, entry_label)| {
            (
                key.max(*entry_key),
                label.max((*entry_label).min(label_width)),
            )
        })
}

fn best_partition(
    section_ids: &[usize],
    entry_widths: &[(usize, usize)],
    label_width: usize,
    columns: usize,
    row_limit: usize,
    pane_width: usize,
) -> Option<Partition> {
    #[derive(Debug, Clone)]
    struct Candidate(Partition);

    fn better(candidate: &Candidate, current: &Candidate) -> bool {
        candidate.0.cuts < current.0.cuts
            || (candidate.0.cuts == current.0.cuts
                && (candidate.0.squared_heights < current.0.squared_heights
                    || (candidate.0.squared_heights == current.0.squared_heights
                        && candidate.0.ends > current.0.ends)))
    }

    fn same_layout(candidate: &Candidate, current: &Candidate) -> bool {
        candidate.0.cuts == current.0.cuts
            && candidate.0.squared_heights == current.0.squared_heights
            && candidate.0.ends == current.0.ends
    }

    fn insert_candidate(candidates: &mut Vec<Candidate>, candidate: Candidate) {
        if candidates.iter().any(|current| {
            current.0.body_width <= candidate.0.body_width
                && (better(current, &candidate) || same_layout(current, &candidate))
        }) {
            return;
        }
        candidates.retain(|current| {
            candidate.0.body_width > current.0.body_width
                || (!better(&candidate, current) && !same_layout(&candidate, current))
        });
        candidates.push(candidate);
    }

    if columns == 0 || columns > section_ids.len() || section_ids.len() != entry_widths.len() {
        return None;
    }
    let reserved = columns.saturating_sub(1).saturating_add(2);
    let max_body_width = pane_width.checked_sub(reserved)?;
    let entries = section_ids.len();
    let mut plans = vec![vec![Vec::new(); entries + 1]; columns + 1];
    plans[0][0].push(Candidate(Partition {
        ends: Vec::new(),
        cuts: 0,
        squared_heights: 0,
        tallest: 0,
        body_width: 0,
    }));
    for used in 1..=columns {
        for end in used..=entries {
            for start in used - 1..end {
                let height = segment_height(section_ids, start, end);
                if height > row_limit {
                    continue;
                }
                let (key_width, segment_label_width) =
                    hint_column_dimensions(entry_widths, start, end, label_width);
                let segment_width = hint_column_width(key_width, segment_label_width);
                for previous in plans[used - 1][start].clone() {
                    let mut candidate = previous;
                    candidate.0.body_width = candidate.0.body_width.saturating_add(segment_width);
                    if candidate.0.body_width > max_body_width {
                        continue;
                    }
                    candidate.0.cuts +=
                        usize::from(start > 0 && section_ids[start - 1] == section_ids[start]);
                    candidate.0.squared_heights = candidate
                        .0
                        .squared_heights
                        .saturating_add(height.saturating_mul(height));
                    candidate.0.tallest = candidate.0.tallest.max(height);
                    candidate.0.ends.push(end);
                    insert_candidate(&mut plans[used][end], candidate);
                }
            }
        }
    }
    plans[columns][entries]
        .iter()
        .min_by(|left, right| {
            if better(left, right) {
                std::cmp::Ordering::Less
            } else if better(right, left) {
                std::cmp::Ordering::Greater
            } else {
                left.0.body_width.cmp(&right.0.body_width)
            }
        })
        .cloned()
        .map(|candidate| candidate.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Partition {
    ends: Vec<usize>,
    cuts: usize,
    squared_heights: usize,
    tallest: usize,
    body_width: usize,
}

fn segment_height(section_ids: &[usize], start: usize, end: usize) -> usize {
    end.saturating_sub(start)
        + (start + 1..end)
            .filter(|index| section_ids[*index] != section_ids[*index - 1])
            .count()
}

impl Grid {
    /// Whether the cell is inside the box, border included.
    #[must_use]
    pub(crate) fn contains(&self, column: usize, row: usize) -> bool {
        column >= self.x
            && column < self.x + self.width
            && row >= self.y
            && row < self.y + self.height
    }

    /// The entry drawn at the cell, if any.
    #[must_use]
    pub(crate) fn entry_at(&self, column: usize, row: usize) -> Option<usize> {
        if !self.contains(column, row)
            || row == self.y
            || row + 1 == self.y + self.height
            || column == self.x
            || column + 1 == self.x + self.width
        {
            return None;
        }
        let r = row - self.y - 1;
        let c = if self.columns == 1 {
            0
        } else {
            (column.checked_sub(self.x + 1)?) / Self::column_width(self.key_width, self.label_width)
        };
        if r >= self.rows || c >= self.columns {
            return None;
        }
        let index = c * self.rows + r;
        (index < self.count).then_some(index)
    }
}

impl App {
    /// The open context menu, if one is.
    #[must_use]
    pub(crate) fn menu(&self) -> Option<&Menu> {
        match self.popup() {
            Some(Popup::Menu(menu)) => Some(menu),
            _ => None,
        }
    }

    fn open_menu(&mut self, menu: Menu) {
        self.take_prefix();
        self.cancel_delete();
        self.park_draft();
        self.popup = Some(Popup::Menu(menu));
    }

    /// Open the compact mode chooser under a header control.
    pub(crate) fn open_diff_mode_menu(&mut self, right: usize, row: usize) {
        self.take_prefix();
        self.cancel_delete();
        self.park_draft();
        self.popup = Some(Popup::DiffMode(ModeMenu::new(self, right, row)));
    }

    /// Handle navigation and selection in the compact mode chooser.
    pub(super) fn mode_menu_key(&mut self, event: KeyEvent) -> Effect {
        let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
        match event.code {
            KeyCode::Esc => self.close_popup(),
            KeyCode::Down | KeyCode::Char('j') if plain => {
                if let Some(Popup::DiffMode(menu)) = self.popup.as_mut() {
                    menu.move_by(1);
                }
            }
            KeyCode::Up | KeyCode::Char('k') if plain => {
                if let Some(Popup::DiffMode(menu)) = self.popup.as_mut() {
                    menu.move_by(-1);
                }
            }
            KeyCode::Enter => {
                let action = match self.popup.as_ref() {
                    Some(Popup::DiffMode(menu)) => menu.selected_action(),
                    _ => None,
                };
                self.close_popup();
                if let Some(action) = action {
                    return self.act(action);
                }
            }
            _ => {}
        }
        Effect::None
    }

    /// Select one row in the compact mode chooser.
    pub(super) fn mode_menu_click(&mut self, index: usize) -> Effect {
        let action = match self.popup.as_ref() {
            Some(Popup::DiffMode(menu)) => menu.menu.entries.get(index).and_then(Entry::action),
            _ => None,
        };
        self.close_popup();
        action.map_or(Effect::None, |action| self.act(action))
    }

    /// A key while the menu is open: an entry's key runs it, the start
    /// of one waits, anything else closes the menu and is swallowed.
    pub(super) fn menu_key(&mut self, chord: Chord) -> Effect {
        let mut typed = self.take_prefix();
        typed.push(chord);
        let Some(menu) = self.menu() else {
            return Effect::None;
        };
        match menu.typed(&typed) {
            Match::Exact(action) => {
                self.close_popup();
                self.act(action)
            }
            Match::Prefix => {
                self.set_prefix(typed);
                Effect::None
            }
            Match::Miss => {
                self.close_popup();
                Effect::None
            }
        }
    }

    /// A click on entry `index` of the open menu.
    pub(super) fn menu_click(&mut self, index: usize) -> Effect {
        let Some(action) = self
            .menu()
            .and_then(|menu| menu.entries.get(index))
            .filter(|entry| entry.enabled)
            .and_then(Entry::action)
        else {
            return Effect::None;
        };
        self.close_popup();
        self.act(action)
    }

    /// A right-click in the text at screen `(column, row)`, `screen_row`
    /// rows into the text and `col` cells into it: a selection the
    /// pointer is in stays, else the cursor goes there; then the menu
    /// for what is under the cursor.
    pub(super) fn open_view_menu(
        &mut self,
        screen_row: usize,
        col: usize,
        column: usize,
        row: usize,
    ) {
        self.focus_pane(Focus::View);
        let view = self.view();
        let inside = view.mode() == Mode::Select
            && view
                .selection()
                .is_some_and(|selection| selection.contains(view.scroll() + screen_row, col));
        if !inside {
            self.view_mut().click(screen_row, col);
        }
        let menu = self.view_menu(column, row);
        self.open_menu(menu);
    }

    fn view_menu(&self, column: usize, row: usize) -> Menu {
        let place = Where::View;
        let view = self.view();
        if view.mode() == Mode::Select && view.selection().is_some() {
            let mut menu = Menu::new("selection", place, column, row);
            menu.push(Action::Comment, Action::Comment, "comment on selection");
            menu.push(
                Action::NewThread,
                Action::NewThread,
                "new thread on selection",
            );
            menu.push(Action::Yank, Action::Yank, "copy selection");
            menu.push(Action::Escape, Action::Escape, "clear selection");
            return menu;
        }
        let threads = self.threads_at_cursor();
        let title = if threads.is_empty() {
            view.cursor_source_line()
                .map_or_else(|| "line".to_owned(), |line| format!("line {line}"))
        } else {
            "thread".to_owned()
        };
        let mut menu = Menu::new(title, place, column, row);
        if let Some(id) = threads.first() {
            let on_thread_row = self.cursor_on_thread_row(id);
            let on_expanded = self.expanded_row_message(view.cursor().row).is_some();
            menu.push(
                Action::Fold,
                Action::Fold,
                if on_expanded {
                    "fold thread"
                } else {
                    "expand thread"
                },
            );
            if on_thread_row {
                menu.push(Action::Comment, Action::Comment, "reply");
            } else {
                menu.push(Action::Reply, Action::Reply, "reply");
            }
            let resolved = self
                .thread(id)
                .is_some_and(|thread| Words::of(None, thread).is_resolved());
            if !resolved {
                let enabled = self
                    .thread(id)
                    .is_some_and(|thread| thread.auto_resolve().is_enabled());
                menu.push(
                    Action::ToggleAutoResolve,
                    Action::ToggleAutoResolve,
                    if enabled {
                        "disable auto-resolve"
                    } else {
                        "enable auto-resolve"
                    },
                );
            }
            menu.push(
                Action::ToggleResolved,
                Action::ToggleResolved,
                if resolved {
                    "reopen thread"
                } else {
                    "resolve thread"
                },
            );
            if resolved {
                menu.push(
                    Action::ArchiveThread,
                    Action::ArchiveThread,
                    "archive thread",
                );
            }
            if self.thread_message_editable() {
                menu.push(Action::EditMessage, Action::EditMessage, "edit message");
            }
            menu.push(Action::Delete, Action::DeleteThread, "delete thread");
        }
        if self.reference_here() {
            menu.push(Action::GotoFile, Action::GotoFile, "open linked file/URL");
        }
        menu.push(Action::Comment, Action::Comment, "comment on line");
        menu.push(Action::ExtendLine, Action::ExtendLine, "select line");
        menu.push(Action::Yank, Action::Yank, "copy line");
        menu
    }

    /// A left-click on the Files title opens display settings below the header.
    pub(super) fn open_files_menu(&mut self, row: usize) {
        let mut menu = Menu::below_header("File list", Where::Tree, 0, row);
        for action in [
            Action::FilesChanged,
            Action::FilesReviews,
            Action::FilesUntracked,
            Action::FilesIgnored,
            Action::FilesAutoUnfold,
        ] {
            menu.push_toggle_enabled(
                action,
                action,
                Self::files_setting_label(action),
                self.files_setting_checked(action),
                action != Action::FilesChanged || self.diff_mode() != DiffMode::Off,
            );
        }
        self.open_menu(menu);
    }

    /// A left-click on the Threads title opens view settings below the header.
    pub(super) fn open_threads_menu(&mut self, row: usize) {
        let mut menu = Menu::below_header("Threads", Where::ThreadsPane, 0, row);
        menu.push_toggle(
            Action::PaneScope,
            Action::PaneScope,
            "only current file",
            self.sidebar_scope() == PaneScope::File,
        );
        menu.push_toggle(
            Action::AllThreads,
            Action::AllThreads,
            "all threads",
            self.all_threads(),
        );
        menu.push_toggle(
            Action::ReviewResolved,
            Action::ReviewResolved,
            "show resolved",
            self.review().resolved,
        );
        self.open_menu(menu);
    }

    /// A left-click on the Reviews title opens view settings below the header.
    pub(super) fn open_reviews_settings_menu(&mut self, row: usize) {
        let mut menu = Menu::below_header("Threads", Where::Review, self.sidebar_width(), row);
        menu.push(Action::FileView, Action::FileView, "open file");
        menu.separator();
        menu.push_toggle(
            Action::FileOnly,
            Action::FileOnly,
            "only current file",
            self.review().file_only,
        );
        menu.push_toggle_enabled(
            Action::AllThreads,
            Action::AllThreads,
            "all threads",
            self.all_threads(),
            self.review().view == crate::app::threads::list::ReviewView::Board,
        );
        menu.push_toggle(
            Action::ReviewResolved,
            Action::ReviewResolved,
            "show resolved",
            self.review().resolved,
        );
        self.open_menu(menu);
    }

    /// A left-click on the File title opens navigation and display settings.
    pub(super) fn open_file_menu(&mut self, row: usize) {
        let mut menu = Menu::below_header("File", Where::View, self.sidebar_width(), row);
        menu.push(Action::Review, Action::Review, "open reviews");
        menu.separator();
        menu.push_toggle_enabled(
            Action::SourceView,
            Action::SourceView,
            "rendered view",
            !self.view().source_view() && !self.view().diff_view(),
            self.source_view_available(),
        );
        menu.push_toggle(
            Action::StubsToggle,
            Action::StubsToggle,
            "show inline threads",
            self.stubs_shown(),
        );
        menu.push_toggle(
            Action::StubResolvedToggle,
            Action::StubResolvedToggle,
            "show resolved threads",
            self.stubs_resolved(),
        );
        self.open_menu(menu);
    }

    /// A right-click on tree row `tree_row` at screen `(column, row)`:
    /// the highlight moves there, showing the file as the wheel does,
    /// and the menu offers what the row can do.
    pub(super) fn open_tree_menu(&mut self, tree_row: usize, column: usize, row: usize) {
        self.tree_point(tree_row);
        let Some(current) = self.tree().and_then(Tree::current) else {
            return;
        };
        let is_dir = current.is_dir();
        let mut menu = Menu::new(current.name().to_owned(), Where::Tree, column, row);
        if is_dir {
            menu.push(
                Action::Confirm,
                Action::Confirm,
                if current.expanded() {
                    "collapse"
                } else {
                    "expand"
                },
            );
        } else {
            menu.push(Action::Confirm, Action::Confirm, "open");
            menu.push(Action::FileComment, Action::FileComment, "file comment");
        }
        menu.push(Action::CopyPath, Action::CopyPath, "copy path");
        menu.push(Action::CopyFullPath, Action::CopyFullPath, "copy full path");
        self.open_menu(menu);
    }

    /// A right-click on a threads pane row at screen `(column, row)`:
    /// on a thread the cursor moves there and the thread's menu opens;
    /// on a file row the cursor goes to its first thread and the file's
    /// menu opens (ADR 0066).
    pub(super) fn open_threads_pane_menu(&mut self, entry_row: usize, column: usize, row: usize) {
        match self.threads_pane_point(entry_row) {
            Some(PanePoint::File(path)) => {
                let menu = self.file_menu(Where::ThreadsPane, &path, column, row);
                self.open_menu(menu);
            }
            Some(PanePoint::Thread) => {
                let menu = self.thread_menu(Where::ThreadsPane, column, row);
                self.open_menu(menu);
            }
            None => {}
        }
    }

    /// A right-click on a review list row at screen `(column, row)`: a
    /// file row's menu, or the thread's.
    pub(super) fn open_review_menu(&mut self, list_row: usize, column: usize, row: usize) {
        if let Some(path) = self.review_point(list_row) {
            let menu = self.file_menu(Where::Review, &path, column, row);
            self.open_menu(menu);
            return;
        }
        self.review_click(list_row);
        if self.thread_cursor().thread().is_some() {
            let menu = self.thread_menu(Where::Review, column, row);
            self.open_menu(menu);
        }
    }

    /// Whether `z` on a thread row of `place` folds the thread's file
    /// (ADR 0066): only in the pane in workspace scope; in the list `z`
    /// folds the thread (ADR 0076).
    fn folds_files(&self, place: Where) -> bool {
        match place {
            Where::ThreadsPane => self.sidebar_scope() == PaneScope::Workspace,
            _ => false,
        }
    }

    /// The menu for a file row (ADR 0066): fold or unfold it, in the
    /// pane fold or unfold every file (the list's `Z` folds threads, ADR
    /// 0076), then open the file. The review list also carries its resolved
    /// toggle; the pane keeps that setting in its title menu.
    fn file_menu(&self, place: Where, path: &Path, column: usize, row: usize) -> Menu {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut menu = Menu::new(name, place, column, row);
        let collapsed = match place {
            Where::Review => self.review_file_is_collapsed(path),
            Where::ThreadsPane => self.threads_pane_file_is_collapsed(path),
            _ => false,
        };
        menu.push(
            Action::Fold,
            Action::Fold,
            if collapsed { "expand" } else { "collapse" },
        );
        if place == Where::ThreadsPane {
            let any_folded = self
                .review_entries(false)
                .iter()
                .any(|entry| self.threads_pane_file_is_collapsed(entry.path()));
            menu.push(
                Action::FoldAll,
                Action::FoldAll,
                if any_folded {
                    "expand all"
                } else {
                    "collapse all"
                },
            );
        }
        menu.push(Action::Confirm, Action::Confirm, "open file");
        if place == Where::Review {
            menu.push(
                Action::ReviewResolved,
                Action::ReviewResolved,
                if self.review().resolved {
                    "hide resolved"
                } else {
                    "show resolved"
                },
            );
        }
        menu
    }

    /// The menu for the thread cursor's thread on a thread surface; in
    /// the list it folds and expands the thread as the text's does (ADR
    /// 0076).
    fn thread_menu(&self, place: Where, column: usize, row: usize) -> Menu {
        let mut menu = Menu::new("thread", place, column, row);
        if place == Where::Review {
            let collapsed = self
                .thread_cursor()
                .thread()
                .is_some_and(|id| self.review_thread_is_collapsed(id));
            menu.push(
                Action::Fold,
                Action::Fold,
                if collapsed {
                    "expand thread"
                } else {
                    "collapse thread"
                },
            );
        }
        menu.push(Action::Confirm, Action::Confirm, "go to");
        let archived = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .is_some_and(fathomable_core::annotations::Thread::is_archived);
        if archived {
            menu.push(
                Action::RestoreThread,
                Action::RestoreThread,
                "restore thread",
            );
            return menu;
        }
        menu.push(Action::Reply, Action::Reply, "reply");
        let resolved = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .is_some_and(|thread| Words::of(None, thread).is_resolved());
        if !resolved {
            let enabled = self
                .thread_cursor()
                .thread()
                .and_then(|id| self.thread(id))
                .is_some_and(|thread| thread.auto_resolve().is_enabled());
            menu.push(
                Action::ToggleAutoResolve,
                Action::ToggleAutoResolve,
                if enabled {
                    "disable auto-resolve"
                } else {
                    "enable auto-resolve"
                },
            );
        }
        menu.push(
            Action::ToggleResolved,
            Action::ToggleResolved,
            if resolved {
                "reopen thread"
            } else {
                "resolve thread"
            },
        );
        if resolved {
            menu.push(
                Action::ArchiveThread,
                Action::ArchiveThread,
                "archive thread",
            );
        }
        if place == Where::Review {
            if self.thread_message_editable() {
                menu.push(Action::EditMessage, Action::EditMessage, "edit message");
            }
        } else {
            menu.push(
                Action::EditNewestOwn,
                Action::EditNewestOwn,
                "edit your newest message",
            );
        }
        menu.push(Action::Delete, Action::DeleteThread, "delete thread");
        if self.folds_files(place) {
            menu.push(Action::Fold, Action::Fold, "fold file");
        }
        menu
    }
}

#[cfg(test)]
mod tests;
