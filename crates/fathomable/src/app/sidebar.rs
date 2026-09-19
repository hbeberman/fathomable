// @okf-doc: /decisions/0057-the-sidebar.md
//! The sidebar: the left column that holds the files pane above the
//! threads pane, each shown or hidden on its own (ADR 0049, named by
//! ADR 0057). Its state and its width rule live here; the panes' own
//! code is in `files_pane` and `threads::pane`.

use std::collections::HashSet;
use std::path::PathBuf;

use fathomable_core::config::SidebarConfig;

use super::threads::pane::PaneScope;
use super::{App, TEXT_MIN_WIDTH};

/// Narrowest the sidebar can be dragged.
pub(super) const SIDEBAR_MIN_WIDTH: usize = 8;

/// The sidebar's state (ADR 0049, ADR 0057): which of its panes are shown,
/// what the threads pane lists and which files it has folded (ADR 0066),
/// the split a drag set, and the configured sizes.
#[derive(Debug, Clone, Copy)]
struct PaneSet {
    tree: bool,
    threads: bool,
}

#[derive(Debug)]
pub(crate) struct Sidebar {
    /// The files pane is shown.
    pub(crate) tree: bool,
    /// The threads pane is shown.
    pub(crate) threads: bool,
    /// The files pane restored by the whole-sidebar toggle.
    restore: PaneSet,
    pub(crate) scope: PaneScope,
    /// Files the threads pane has folded to their row (ADR 0066).
    pub(crate) folded: HashSet<PathBuf>,
    /// Rows a drag gave the threads pane, over `config.split`.
    pub(crate) split: Option<usize>,
    pub(crate) config: SidebarConfig,
}

impl Sidebar {
    pub(crate) fn new(config: SidebarConfig) -> Self {
        let tree = config.visible && config.files;
        let threads = config.visible && config.threads;
        Self {
            tree,
            threads,
            restore: PaneSet {
                tree: config.files,
                threads: config.threads,
            },
            scope: PaneScope::default(),
            folded: HashSet::new(),
            split: None,
            config,
        }
    }

    pub(crate) fn shown(&self) -> bool {
        self.tree || self.threads
    }

    pub(crate) fn hide(&mut self) {
        if self.shown() {
            self.restore = PaneSet {
                tree: self.tree,
                threads: self.threads,
            };
            self.tree = false;
            self.threads = false;
        }
    }

    pub(crate) fn restore(&mut self) -> bool {
        if !self.restore.tree && !self.restore.threads {
            return false;
        }
        self.tree = self.restore.tree;
        self.threads = self.restore.threads;
        true
    }

    pub(crate) fn restores_tree(&self) -> bool {
        self.restore.tree
    }

    pub(crate) fn has_restore(&self) -> bool {
        self.restore.tree || self.restore.threads
    }

    pub(crate) fn show_tree(&mut self) {
        if !self.shown() {
            self.restore.threads = false;
        }
        self.tree = true;
        self.restore.tree = true;
    }

    pub(crate) fn show_threads(&mut self) {
        if !self.shown() {
            self.restore.tree = false;
        }
        self.threads = true;
        self.restore.threads = true;
    }

    pub(crate) fn hide_tree(&mut self) {
        self.tree = false;
        if self.threads {
            self.restore = PaneSet {
                tree: false,
                threads: true,
            };
        } else {
            self.restore = PaneSet {
                tree: true,
                threads: false,
            };
        }
    }

    pub(crate) fn hide_threads(&mut self) {
        self.threads = false;
        if self.tree {
            self.restore = PaneSet {
                tree: true,
                threads: false,
            };
        } else {
            self.restore = PaneSet {
                tree: false,
                threads: true,
            };
        }
    }
}

impl App {
    /// The sidebar's width in columns, 0 when neither of its panes is shown
    /// (ADR 0049).
    pub(crate) fn sidebar_width(&self) -> usize {
        if !self.sidebar.shown() {
            return 0;
        }
        let widest = self.width.saturating_sub(TEXT_MIN_WIDTH);
        self.sidebar_cols
            .map_or_else(
                || self.sidebar.config.width.min(self.width / 3),
                |cols| cols.min(widest),
            )
            .min(widest)
            .max(SIDEBAR_MIN_WIDTH)
    }

    /// `Space p s`: hide the whole sidebar, remembering its pane
    /// composition, or show that composition again.
    pub(crate) fn toggle_sidebar(&mut self) {
        if self.sidebar.shown() {
            self.sidebar.hide();
            if matches!(self.focus, super::Focus::Tree | super::Focus::ThreadsPane) {
                self.focus = self.displayed_main_focus();
            }
            self.relayout();
            return;
        }
        if self.sidebar.restores_tree() && !self.ensure_tree() {
            return;
        }
        if !self.sidebar.restore() {
            self.notice("sidebar has no lists; show File list or Thread list first");
            return;
        }
        if self.sidebar.tree && self.tree_target.is_some() {
            self.refresh_tree_target();
        }
        self.relayout();
    }
}
