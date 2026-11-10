//! How the pane a buffer is shown in lays its lines out on screen rows,
//! for the motions that go by rows: `gj`/`gk`, and with standard keys and
//! in Insert the arrows Up/Down and Home/End. Fenix-vim doesn't know a
//! pane's width; the editor tells it (`VimState::set_screen_rows`), and
//! without wrap a row is a line and these motions are the line ones.

use std::cell::RefCell;
use std::ops::Range;

use fenix_core::Buffer;

/// A pane's rows, as its editor lays them out.
pub trait ScreenRows: Send + Sync {
    /// The char `delta` rows below `at` (above, when negative), held to
    /// the first row and the last, at cell `x` of its row -- or, with no
    /// `x`, at the cell `at` is drawn at -- with the cell kept. `None`
    /// when a row is a line.
    fn row_move(&self, buffer: &Buffer, at: usize, delta: isize, x: Option<usize>) -> Option<(usize, usize)>;
    /// The chars on `at`'s row. `None` when a row is a line.
    fn row_bounds(&self, buffer: &Buffer, at: usize) -> Option<Range<usize>>;
}

/// A row a line: no wrap.
pub struct LineRows;

impl ScreenRows for LineRows {
    fn row_move(&self, _: &Buffer, _: usize, _: isize, _: Option<usize>) -> Option<(usize, usize)> {
        None
    }
    fn row_bounds(&self, _: &Buffer, _: usize) -> Option<Range<usize>> {
        None
    }
}

/// The cells runs of moves by row keep to, as the sticky column is kept
/// for `j`/`k`: one for each cursor (the char it's on, and the cell),
/// the last key's (`kept`) and this key's (`keeping`). A key that moves
/// no cursor by row forgets them (`VimState::begin_key`).
#[derive(Default)]
pub(crate) struct Goals {
    pub kept: RefCell<Vec<(usize, usize)>>,
    pub keeping: RefCell<Vec<(usize, usize)>>,
}

/// The rows to move by, and the cells kept.
pub(crate) struct Screen<'a> {
    pub rows: &'a dyn ScreenRows,
    pub goals: &'a Goals,
}

impl Screen<'_> {
    /// Where `delta` rows from `at` is, at the cell the cursor on `at`
    /// kept: the char and the cell. `None` when a row is a line.
    pub(crate) fn step(&self, buffer: &Buffer, at: usize, delta: isize) -> Option<(usize, usize)> {
        let x = self.goals.keeping.borrow().iter().chain(self.goals.kept.borrow().iter()).find(|(was, _)| *was == at).map(|&(_, x)| x);
        self.rows.row_move(buffer, at, delta, x)
    }

    /// The cell `x` kept for the cursor now on `at`.
    pub(crate) fn keep(&self, at: usize, x: usize) {
        let mut keeping = self.goals.keeping.borrow_mut();
        keeping.retain(|(was, _)| *was != at);
        keeping.push((at, x));
    }
}
