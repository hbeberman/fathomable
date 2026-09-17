// @okf-doc: /decisions/0069-the-diffs-keys-on-the-bar.md
//! Key hints for the single viewer-wide comparison.

use super::App;
use super::draw::header::HintOf;
use super::input::bindings::{Action, Where};

/// Hints shown while the unified comparison is open.
pub(crate) fn diff_hints(app: &App) -> Vec<HintOf> {
    if !app.view().diff_view() || app.focus() != super::Focus::View {
        return Vec::new();
    }
    vec![
        HintOf::keyed(Where::View, Action::ComparisonBase, "base"),
        HintOf::keyed(Where::View, Action::ComparisonTarget, "target"),
        HintOf::keyed(Where::View, Action::ComparisonWhitespace, "whitespace"),
        HintOf::keyed(Where::View, Action::Escape, "close"),
    ]
}
