//! Following an annotated range through edits (ADR 0019).

use fathomable_core::annotations::{Anchor, LineRange};
use fathomable_core::context::{Context, MAX_CONTEXT_BYTES, map_context};
use fathomable_core::reanchor::{Mapping, map_range};

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

#[test]
fn truncated_context_cannot_reanchor_to_a_selected_line_prefix() -> Result {
    for unit in ["x", "\u{00e9}"] {
        let original = unit.repeat(MAX_CONTEXT_BYTES + 100);
        let range = LineRange::new(1, 1);
        let context = Context::capture(&original, range).ok_or("context")?;
        let anchor = Anchor::capture(&original, range).ok_or("anchor")?;
        let current = format!("{}\n{original} edited\n", context.snippet());

        assert!(context.is_truncated());
        assert!(context.text().len() <= MAX_CONTEXT_BYTES);
        assert_eq!(context.omitted_selected_lines(), 0);
        assert_eq!(anchor.locate(&current, range), None);
        assert_eq!(map_context(&context, &current, range), Mapping::Removed);

        let restored: Context = serde_json::from_str(&serde_json::to_string(&context)?)?;
        assert_eq!(map_context(&restored, &current, range), Mapping::Removed);
    }
    Ok(())
}

#[test]
fn complete_context_at_the_byte_limit_still_follows_edits() -> Result {
    let original = "x".repeat(MAX_CONTEXT_BYTES - 1);
    let range = LineRange::new(1, 1);
    let context = Context::capture(&original, range).ok_or("context")?;

    assert!(!context.is_truncated());
    assert_eq!(context.text().len(), MAX_CONTEXT_BYTES);
    assert_eq!(
        map_context(&context, &format!("{original} edited\n"), range),
        Mapping::Edited(range)
    );
    Ok(())
}

#[test]
fn truncated_context_preserves_exact_anchors_but_declines_heuristic_placement() -> Result {
    let original = "a long surrounding line\nkeep\nselected\nafter\n";
    let range = LineRange::new(3, 3);
    let context = Context::capture_bounded(original, range, 20).ok_or("context")?;
    let anchor = Anchor::capture(original, range).ok_or("anchor")?;

    assert!(context.is_truncated());
    assert_eq!(context.snippet(), "selected");
    assert_eq!(context.omitted_selected_lines(), 0);
    assert_eq!(
        anchor.locate("keep\nselected\nafter\n", range),
        Some(LineRange::new(2, 2))
    );
    assert_eq!(
        map_context(&context, "keep\nSELECTED\nafter\n", range),
        Mapping::Removed
    );
    Ok(())
}

#[test]
fn truncated_context_with_empty_or_omitted_selected_lines_cannot_place() -> Result {
    for (original, range, budget) in [
        ("selected\n", LineRange::new(1, 1), 0),
        ("\u{00e9}\n", LineRange::new(1, 1), 2),
        ("\n\n\n\n", LineRange::new(1, 4), 2),
    ] {
        let context = Context::capture_bounded(original, range, budget).ok_or("context")?;
        assert!(context.is_truncated());
        assert!(context.text().len() <= budget);
        assert_eq!(map_context(&context, "\n\n", range), Mapping::Removed);
    }
    Ok(())
}

#[test]
fn persisted_context_without_omission_metadata_respects_truncation() -> Result {
    let range = LineRange::new(1, 1);
    let truncated: Context = serde_json::from_str(r#"{"lines":["prefix"],"truncated":true}"#)?;
    assert_eq!(truncated.omitted_selected_lines(), 0);
    assert_eq!(
        map_context(&truncated, "prefix\nprefix and the rest\n", range),
        Mapping::Removed
    );

    let complete: Context = serde_json::from_str(r#"{"lines":["one"]}"#)?;
    assert_eq!(
        map_context(&complete, "ONE\n", range),
        Mapping::Edited(range)
    );
    Ok(())
}

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
