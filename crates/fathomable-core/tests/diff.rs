//! Behaviour of the line diff, the gutter status, and the diff layout
//! (ADR 0006).

use fathomable_core::diff::{Compare, Diff, DiffKind, LineStatus};
use fathomable_core::layout::{Face, Layout, Line};

fn statuses(diff: &Diff) -> Vec<Option<LineStatus>> {
    (1..=diff.new_lines())
        .map(|line| diff.status(line))
        .collect()
}

#[test]
fn identical_texts_have_no_hunks() {
    let diff = Diff::new("a\nb\n", "a\nb\n");
    assert!(diff.is_empty());
    assert_eq!(diff.counts(), (0, 0));
    assert_eq!(statuses(&diff), [None, None]);
    assert!(diff.next_hunk(1).is_none());
    // A missing trailing newline is not a change on its own.
    assert!(Diff::new("a\nb", "a\nb\n").is_empty());
}

#[test]
fn additions_modifications_and_removals_mark_the_right_lines() {
    let old = "one\ntwo\nthree\nfour\nfive\n";
    let new = "one\nTWO\nthree\nfive\nsix\n";
    let diff = Diff::new(old, new);
    assert_eq!(
        statuses(&diff),
        [
            None,
            Some(LineStatus::Modified),
            None,
            // "four" was removed above "five" (ADR 0010).
            Some(LineStatus::Removed),
            Some(LineStatus::Added),
        ]
    );
    assert_eq!(diff.counts(), (2, 2));
    let hunks: Vec<_> = diff
        .hunks()
        .iter()
        .map(|h| (h.old_range(), h.new_range()))
        .collect();
    assert_eq!(hunks, [(1..2, 1..2), (3..4, 3..3), (5..5, 4..5)]);
}

#[test]
fn removal_at_the_end_marks_the_last_line() {
    let diff = Diff::new("a\nb\nc\n", "a\nb\n");
    assert_eq!(statuses(&diff), [None, Some(LineStatus::Removed)]);
    assert_eq!(diff.hunks()[0].target_line(diff.new_lines()), 2);
}

#[test]
fn empty_base_means_every_line_is_added() {
    let diff = Diff::new("", "a\nb\n");
    assert_eq!(
        statuses(&diff),
        [Some(LineStatus::Added), Some(LineStatus::Added)]
    );
    let diff = Diff::new("a\nb\n", "");
    assert_eq!(diff.counts(), (0, 2));
    assert_eq!(diff.status(1), None, "no new lines to mark");
}

#[test]
fn completely_different_texts_are_one_replacement() {
    let diff = Diff::new("x\ny\n", "p\nq\nr\n");
    assert_eq!(diff.hunks().len(), 1);
    assert_eq!(diff.hunks()[0].old_range(), 0..2);
    assert_eq!(diff.hunks()[0].new_range(), 0..3);
}

#[test]
fn myers_finds_a_minimal_edit_on_shuffled_input() {
    // The classic example from Myers' paper: ABCABBA -> CBABAC.
    let old = "A\nB\nC\nA\nB\nB\nA\n";
    let new = "C\nB\nA\nB\nA\nC\n";
    let diff = Diff::new(old, new);
    let (added, removed) = diff.counts();
    assert_eq!(added + removed, 5, "shortest edit script has length 5");
    // Reconstruct the new text from the old one and the hunks.
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    for hunk in diff.hunks() {
        out.extend_from_slice(&a[i..hunk.old_range().start]);
        out.extend_from_slice(&b[hunk.new_range()]);
        i = hunk.old_range().end;
    }
    out.extend_from_slice(&a[i..]);
    assert_eq!(out, b);
}

#[test]
fn hunk_navigation_wraps_in_both_directions() {
    let diff = Diff::new("a\nb\nc\nd\ne\n", "a\nB\nc\nd\nE\n");
    let targets: Vec<_> = diff
        .hunks()
        .iter()
        .map(|h| h.target_line(diff.new_lines()))
        .collect();
    assert_eq!(targets, [2, 5]);
    assert_eq!(
        diff.next_hunk(1).map(|(h, w)| (h.target_line(5), w)),
        Some((2, false))
    );
    assert_eq!(
        diff.next_hunk(2).map(|(h, w)| (h.target_line(5), w)),
        Some((5, false))
    );
    assert_eq!(
        diff.next_hunk(5).map(|(h, w)| (h.target_line(5), w)),
        Some((2, true))
    );
    assert_eq!(
        diff.prev_hunk(5).map(|(h, w)| (h.target_line(5), w)),
        Some((2, false))
    );
    assert_eq!(
        diff.prev_hunk(2).map(|(h, w)| (h.target_line(5), w)),
        Some((5, true))
    );
}

#[test]
fn unified_listing_groups_nearby_hunks_with_context() {
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
    let new = "1\n2\n3\n4\n5\nsix\n7\n8\n9\n10\n11\n12\nthirteen\n";
    let diff = Diff::new(old, new);
    let lines = diff.unified(old, new, 3);
    let shown: Vec<String> = lines
        .iter()
        .map(|l| {
            let sign = match l.kind() {
                DiffKind::Header => "@",
                DiffKind::Context => " ",
                DiffKind::Added => "+",
                DiffKind::Removed => "-",
            };
            format!("{sign}{}", l.text())
        })
        .collect();
    assert_eq!(
        shown,
        [
            "@@@ -3,10 +3,11 @@",
            " 3",
            " 4",
            " 5",
            "-6",
            "+six",
            " 7",
            " 8",
            " 9",
            " 10",
            " 11",
            " 12",
            "+thirteen",
        ]
    );
    assert_eq!(lines[4].old_line(), Some(6));
    assert_eq!(lines[5].new_line(), Some(6));
    // Distant hunks get their own header.
    let far = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n";
    let far_new = "one\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\nsixteen\n";
    let headers = Diff::new(far, far_new)
        .unified(far, far_new, 3)
        .into_iter()
        .filter(|l| l.kind() == DiffKind::Header)
        .map(|l| l.text().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(headers, ["@@ -1,4 +1,4 @@", "@@ -13,4 +13,4 @@"]);
}

#[test]
fn diff_layout_carries_faces_and_new_text_sources() -> Result<(), Box<dyn std::error::Error>> {
    let old = "# Title\n\nold line\n";
    let new = "# Title\n\nnew line\nextra\n";
    let layout = Layout::diff(old, new, 40, Compare::default());
    let texts: Vec<String> = layout.lines().iter().map(Line::text).collect();
    assert_eq!(
        texts,
        [
            "@@ -1,3 +1,4 @@",
            " # Title",
            " ",
            "-old line",
            "+new line",
            "+extra",
        ]
    );
    let faces: Vec<Face> = layout
        .lines()
        .iter()
        .map(|l| l.spans()[0].style().face.clone())
        .collect();
    assert_eq!(
        faces,
        [
            Face::DiffHeader,
            Face::Text,
            Face::Text,
            Face::DiffRemoved,
            Face::DiffAdded,
            Face::DiffAdded,
        ]
    );
    // Gutter numbers are new-text lines; removed rows and headers have none.
    let numbers: Vec<Option<usize>> = layout.lines().iter().map(Line::source_line).collect();
    assert_eq!(numbers, [None, Some(1), Some(2), None, Some(3), Some(4)]);
    assert_eq!(layout.lines()[3].diff_old_line(), Some(3));
    assert_eq!(layout.lines()[3].diff_new_line(), None);
    assert_eq!(layout.lines()[4].diff_old_line(), None);
    assert_eq!(layout.lines()[4].diff_new_line(), Some(3));
    let range = layout.lines()[4]
        .source()
        .ok_or("added line has no source")?;
    assert_eq!(&new[range], "new line");
    // Column 1 of "+new line" is the first source byte of the line.
    assert_eq!(layout.lines()[4].source_at(1), Some(9));

    let same = Layout::diff(new, new, 40, Compare::default());
    assert_eq!(same.lines().len(), 1);
    assert!(same.lines()[0].source().is_none());
    Ok(())
}

#[test]
fn diff_layout_wraps_long_lines_under_their_sign() {
    let old = "";
    let new = "abcdefghijklmnopqrstuvwxyz\n";
    let layout = Layout::diff(old, new, 15, Compare::default());
    let texts: Vec<String> = layout.lines().iter().map(Line::text).collect();
    assert_eq!(texts, ["@@ -0,0 +1 @@", "+abcdefghijklmn", " opqrstuvwxyz"]);
    assert!(layout.lines().iter().all(|line| line.width() <= 15));
    assert_eq!(
        layout.lines()[1].source_line(),
        Some(1),
        "only the first visual row gets a gutter number"
    );
    assert_eq!(layout.lines()[2].source_line(), None);
    assert_eq!(layout.lines()[1].diff_new_line(), Some(1));
    assert_eq!(layout.lines()[2].diff_new_line(), Some(1));
}
