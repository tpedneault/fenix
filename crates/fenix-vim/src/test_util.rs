use fenix_core::{Buffer, Cursor};

pub(crate) fn buf(s: &str) -> Buffer {
    let mut b = Buffer::empty();
    let mut c = Cursor::at_start();
    for ch in s.chars() {
        b.insert_char(&mut c, ch);
    }
    b
}

pub(crate) fn cur(idx: usize) -> Cursor {
    Cursor { char_idx: idx, sticky_col: 0 }
}
