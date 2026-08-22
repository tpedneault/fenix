/// A single insertion point in a `Buffer`.
///
/// Buffer editing/movement operations take a `&mut Cursor` rather than owning
/// one internally, so a future multi-cursor mode can drive the same
/// primitives over a `Vec<Cursor>` without changing `Buffer`'s API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Absolute char offset into the buffer's rope.
    pub char_idx: usize,
    /// Column (in chars) the cursor "wants" to be at when moving through
    /// shorter lines vertically, mirroring Emacs's goal-column behavior.
    pub sticky_col: usize,
}

impl Cursor {
    pub fn at_start() -> Self {
        Self { char_idx: 0, sticky_col: 0 }
    }
}

impl Default for Cursor {
    fn default() -> Self {
        Self::at_start()
    }
}
