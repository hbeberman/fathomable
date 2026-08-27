//! Behaviour of the comment buffer: edits, motion, wrapping, and mouse
//! placement (ADR 0018).

use fathomable_core::editor::{Buffer, Cell, Cursor, Edit, Motion, Row};

fn moved(buffer: &mut Buffer, motion: Motion) -> Cursor {
    buffer.apply(Edit::Move(motion));
    buffer.cursor()
}

#[test]
fn typing_and_deleting_around_the_cursor() {
    let mut buffer = Buffer::new();
    assert!(buffer.is_empty());
    assert_eq!(buffer.cursor(), Cursor::default());
    buffer.insert("hello world");
    buffer.apply(Edit::Move(Motion::WordBack));
    buffer.insert("big ");
    assert_eq!(buffer.text(), "hello big world");
    buffer.apply(Edit::DeleteForward);
    assert_eq!(buffer.text(), "hello big orld");
    buffer.apply(Edit::DeleteBack);
    assert_eq!(buffer.text(), "hello bigorld");
    buffer.apply(Edit::DeleteWordBack);
    assert_eq!(buffer.text(), "hello orld");
    buffer.apply(Edit::DeleteWordBack);
    assert_eq!(buffer.text(), "orld");
    assert_eq!(buffer.cursor(), Cursor { line: 0, column: 0 });
    // Nothing to delete is a no-op, not a panic.
    buffer.apply(Edit::DeleteBack);
    buffer.apply(Edit::Move(Motion::Left));
    assert_eq!(buffer.text(), "orld");
}

#[test]
fn line_kills_and_newlines() {
    let mut buffer = Buffer::from_text("first line\nsecond");
    assert_eq!(buffer.cursor(), Cursor { line: 1, column: 6 });
    buffer.apply(Edit::Move(Motion::Up));
    assert_eq!(buffer.cursor(), Cursor { line: 0, column: 6 });
    buffer.apply(Edit::DeleteToLineEnd);
    assert_eq!(buffer.text(), "first \nsecond");
    buffer.apply(Edit::DeleteToLineStart);
    assert_eq!(buffer.text(), "\nsecond");
    buffer.apply(Edit::Newline);
    buffer.insert("x");
    assert_eq!(buffer.text(), "\nx\nsecond");
    assert_eq!(buffer.lines().count(), 3);
    // Ctrl-w at a line start joins with the line above.
    buffer.apply(Edit::Move(Motion::Down));
    buffer.apply(Edit::Move(Motion::LineStart));
    buffer.apply(Edit::DeleteWordBack);
    assert_eq!(buffer.text(), "\nsecond");
}

#[test]
fn vertical_motion_keeps_the_wanted_column() {
    let mut buffer = Buffer::from_text("a long line\nab\nanother long line");
    buffer.set_cursor(Cursor { line: 0, column: 9 });
    assert_eq!(
        moved(&mut buffer, Motion::Down),
        Cursor { line: 1, column: 2 }
    );
    assert_eq!(
        moved(&mut buffer, Motion::Down),
        Cursor { line: 2, column: 9 }
    );
    // Off the end goes to the very end; off the top to the very start.
    assert_eq!(
        moved(&mut buffer, Motion::Down),
        Cursor {
            line: 2,
            column: 17
        }
    );
    buffer.set_cursor(Cursor { line: 0, column: 3 });
    assert_eq!(
        moved(&mut buffer, Motion::Up),
        Cursor { line: 0, column: 0 }
    );
    // A horizontal move forgets the wanted column.
    buffer.set_cursor(Cursor { line: 0, column: 9 });
    buffer.apply(Edit::Move(Motion::Down));
    buffer.apply(Edit::Move(Motion::Left));
    assert_eq!(
        moved(&mut buffer, Motion::Down),
        Cursor { line: 2, column: 1 }
    );
    assert_eq!(
        moved(&mut buffer, Motion::LineEnd),
        Cursor {
            line: 2,
            column: 17
        }
    );
    assert_eq!(
        moved(&mut buffer, Motion::WordBack),
        Cursor {
            line: 2,
            column: 13
        }
    );
    assert_eq!(
        moved(&mut buffer, Motion::WordForward),
        Cursor {
            line: 2,
            column: 17
        }
    );
}

#[test]
fn graphemes_move_as_units() {
    // "é" as e + combining acute, then a CJK character two cells wide.
    let mut buffer = Buffer::from_text("e\u{301}中x");
    assert_eq!(buffer.cursor(), Cursor { line: 0, column: 3 });
    assert_eq!(
        moved(&mut buffer, Motion::Left),
        Cursor { line: 0, column: 2 }
    );
    assert_eq!(
        moved(&mut buffer, Motion::Left),
        Cursor { line: 0, column: 1 }
    );
    buffer.apply(Edit::DeleteBack);
    assert_eq!(buffer.text(), "中x");
    assert_eq!(buffer.cursor_cell(10), Cell { row: 0, column: 0 });
    buffer.apply(Edit::Move(Motion::Right));
    assert_eq!(buffer.cursor_cell(10), Cell { row: 0, column: 2 });
}

#[test]
fn wrapping_rows_and_the_cursor_cell_agree() {
    let mut buffer = Buffer::from_text("abcdefgh\nij");
    let rows = buffer.rows(3);
    let texts: Vec<&str> = rows.iter().map(|row| buffer.row_text(*row)).collect();
    assert_eq!(texts, ["abc", "def", "gh", "ij"]);
    assert_eq!(rows.iter().map(Row::line).collect::<Vec<_>>(), [0, 0, 0, 1]);
    assert_eq!(buffer.cursor_cell(3), Cell { row: 3, column: 2 });
    buffer.set_cursor(Cursor { line: 0, column: 3 });
    // The end of a wrapped row and the start of the next share an offset;
    // the cursor draws at the start of the next row.
    assert_eq!(buffer.cursor_cell(3), Cell { row: 1, column: 0 });
    buffer.place_cursor(3, Cell { row: 2, column: 1 });
    assert_eq!(buffer.cursor(), Cursor { line: 0, column: 7 });
    // Past the row's end clamps to it; past the last row clamps to the end.
    buffer.place_cursor(3, Cell { row: 0, column: 9 });
    assert_eq!(buffer.cursor(), Cursor { line: 0, column: 3 });
    buffer.place_cursor(3, Cell { row: 9, column: 9 });
    assert_eq!(buffer.cursor(), Cursor { line: 1, column: 2 });
    // A wide character never splits: two cells on a one-column box.
    let narrow = Buffer::from_text("中");
    assert_eq!(narrow.rows(1).len(), 1);
    assert_eq!(Buffer::new().rows(5).len(), 1);
}
