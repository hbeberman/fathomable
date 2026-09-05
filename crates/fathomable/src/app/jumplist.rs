//! The jumplist (ADR 0049): the positions far moves leave behind, walked
//! back with `Alt-Left` and forward with `Alt-Right`.
//!
//! A far move — opening another file, a search jump, `gg`/`G`, `:N`, a
//! thread or hunk step — records the position it left. Going back the
//! first time records where the reader stands, so forward returns there;
//! a far move after going back drops the forward part, as an editor's
//! undo drops its redo. Neighbouring duplicates collapse and the list is
//! capped, so a repeated jump costs nothing and the oldest fall off.

use std::path::PathBuf;

/// A place in the workspace: a file and a source line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Position {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
}

/// Positions kept at most.
const CAP: usize = 100;

/// The positions far moves left, oldest first, and where the reader
/// stands among them.
#[derive(Debug, Default)]
pub(crate) struct Jumplist {
    entries: Vec<Position>,
    /// The entry the reader stands on after going back; `None` at the
    /// tip, where every far move lands.
    index: Option<usize>,
}

impl Jumplist {
    /// A far move left `position`: keep it, dropping any forward part.
    pub(crate) fn record(&mut self, position: Position) {
        if let Some(index) = self.index.take() {
            self.entries.truncate(index + 1);
        }
        self.push(position);
    }

    /// `Alt-Left` from `here`: the previous position, or `None` at the
    /// oldest. The first step back keeps `here` so forward returns to it.
    pub(crate) fn back(&mut self, here: Position) -> Option<&Position> {
        let index = if let Some(index) = self.index {
            index
        } else {
            self.push(here);
            self.entries.len() - 1
        };
        if index == 0 {
            self.index = Some(0);
            return None;
        }
        self.index = Some(index - 1);
        self.entries.get(index - 1)
    }

    /// `Alt-Right`: the next position, or `None` at the newest.
    pub(crate) fn forward(&mut self) -> Option<&Position> {
        let next = self.index? + 1;
        if next >= self.entries.len() {
            return None;
        }
        self.index = Some(next);
        self.entries.get(next)
    }

    /// How many positions are kept.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn push(&mut self, position: Position) {
        if self.entries.last() != Some(&position) {
            self.entries.push(position);
        }
        if self.entries.len() > CAP {
            self.entries.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{CAP, Jumplist, Position};

    fn at(path: &str, line: usize) -> Position {
        Position {
            path: Path::new(path).to_path_buf(),
            line,
        }
    }

    #[test]
    fn back_returns_to_what_far_moves_left_and_forward_undoes_it() {
        let mut jumps = Jumplist::default();
        assert!(jumps.back(at("a.md", 1)).is_none(), "nothing to go back to");
        assert!(jumps.forward().is_none());

        jumps.record(at("a.md", 1));
        jumps.record(at("a.md", 9));
        // Standing at b.md:3, back lands on the newest position left.
        assert_eq!(jumps.back(at("b.md", 3)), Some(&at("a.md", 9)));
        assert_eq!(jumps.back(at("a.md", 9)), Some(&at("a.md", 1)));
        assert!(jumps.back(at("a.md", 1)).is_none());
        assert_eq!(jumps.forward(), Some(&at("a.md", 9)));
        assert_eq!(
            jumps.forward(),
            Some(&at("b.md", 3)),
            "the first step back kept where it left"
        );
        assert!(jumps.forward().is_none());
    }

    #[test]
    fn a_far_move_after_going_back_drops_the_forward_part() {
        let mut jumps = Jumplist::default();
        jumps.record(at("a.md", 1));
        jumps.record(at("a.md", 5));
        assert_eq!(jumps.back(at("a.md", 9)), Some(&at("a.md", 5)));
        jumps.record(at("a.md", 5));
        assert!(jumps.forward().is_none(), "a.md:9 is gone");
        assert_eq!(jumps.back(at("c.md", 1)), Some(&at("a.md", 5)));
        assert_eq!(jumps.len(), 3);
    }

    #[test]
    fn neighbouring_duplicates_collapse_and_the_oldest_fall_off() {
        let mut jumps = Jumplist::default();
        jumps.record(at("a.md", 1));
        jumps.record(at("a.md", 1));
        assert_eq!(jumps.len(), 1);
        for line in 0..CAP * 2 {
            jumps.record(at("a.md", line));
        }
        assert_eq!(jumps.len(), CAP);
        assert_eq!(jumps.back(at("z.md", 1)), Some(&at("a.md", CAP * 2 - 1)));
    }
}
