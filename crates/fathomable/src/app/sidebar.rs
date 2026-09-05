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
const SIDEBAR_MIN_WIDTH: usize = 8;

/// The sidebar's state (ADR 0049, ADR 0057): which of its panes are shown,
/// what the threads pane lists and which files it has folded (ADR 0066),
/// the split a drag set, and the configured sizes.
#[derive(Debug)]
pub(crate) struct Sidebar {
    /// The files pane is shown.
    pub(crate) tree: bool,
    /// The threads pane is shown.
    pub(crate) threads: bool,
    pub(crate) scope: PaneScope,
    /// Files the threads pane has folded to their row (ADR 0066).
    pub(crate) folded: HashSet<PathBuf>,
    /// Rows a drag gave the threads pane, over `config.split`.
    pub(crate) split: Option<usize>,
    pub(crate) config: SidebarConfig,
}

impl Sidebar {
    pub(crate) fn new(config: SidebarConfig) -> Self {
        Self {
            tree: false,
            threads: false,
            scope: PaneScope::default(),
            folded: HashSet::new(),
            split: None,
            config,
        }
    }
}

impl App {
    /// The sidebar's width in columns, 0 when neither of its panes is shown
    /// (ADR 0049).
    pub(crate) fn sidebar_width(&self) -> usize {
        if !self.sidebar.tree && !self.sidebar.threads {
            return 0;
        }
        let widest = self.width.saturating_sub(TEXT_MIN_WIDTH);
        self.sidebar_cols
            .map_or_else(
                || self.sidebar.config.width.min(self.width / 3),
                |cols| cols.min(widest),
            )
            .max(SIDEBAR_MIN_WIDTH)
    }
}
