// @okf-doc: /decisions/0069-the-diffs-keys-on-the-bar.md
//! Key hints for the single viewer-wide comparison.

use super::App;
use super::draw::header::HintOf;
use super::input::bindings::{Action, Where};

/// The workspace comparison traversal hint.
pub(crate) fn diff_hints(app: &App) -> Vec<HintOf> {
    if !app.has_change_stops() || app.focus() != super::Focus::View {
        return Vec::new();
    }
    vec![HintOf::paired(
        Where::Any,
        Action::ChangePrev,
        Action::ChangeNext,
        "diffs",
    )]
}
