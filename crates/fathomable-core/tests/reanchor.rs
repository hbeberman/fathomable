//! Following an annotated range through edits (ADR 0019).

use fathomable_core::annotations::LineRange;
use fathomable_core::reanchor::{Mapping, map_range};

#[test]
fn an_edited_line_lands_on_its_replacement() {
    let old = "a\nb\nc\nd\n";
    assert_eq!(
        map_range(old, "a\nB\nc\nd\n", LineRange::new(2, 2)),
        Mapping::Edited(LineRange::new(2, 2))
    );
    // Lines inserted above shift the range; nothing in it changed.
    assert_eq!(
        map_range(old, "x\ny\na\nb\nc\nd\n", LineRange::new(2, 3)),
        Mapping::Moved(LineRange::new(4, 5))
    );
    // A range that is partly rewritten spans the survivors and the
    // replacement.
    assert_eq!(
        map_range(old, "a\nb\nC1\nC2\nd\n", LineRange::new(2, 3)),
        Mapping::Edited(LineRange::new(2, 3))
    );
}

#[test]
fn removed_lines_detach_and_shrinking_hunks_clamp() {
    let old = "a\nb\nc\nd\n";
    assert_eq!(
        map_range(old, "a\nd\n", LineRange::new(2, 3)),
        Mapping::Removed
    );
    // Three lines become one: both annotated lines land on it.
    assert_eq!(
        map_range("a\nb\nc\nd\ne\n", "a\nX\ne\n", LineRange::new(3, 4)),
        Mapping::Edited(LineRange::new(2, 2))
    );
    // One removed, one edited: the edited survivor carries the thread.
    assert_eq!(
        map_range(old, "a\nB\nd\n", LineRange::new(2, 3)),
        Mapping::Edited(LineRange::new(2, 2))
    );
    assert_eq!(map_range("", "", LineRange::new(1, 1)), Mapping::Removed);
    // A rewrite reaching well beyond the range is not an edit of it.
    assert_eq!(
        map_range("h\n\na\nb\nc\nd\ne\n", "h\n\ngone\n", LineRange::new(4, 5)),
        Mapping::Removed
    );
}
