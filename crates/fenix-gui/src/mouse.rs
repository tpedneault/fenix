//! The mouse's own arithmetic, apart from the editor: counting clicks,
//! the pointer's column back to a character, where a dragged tab drops,
//! and the context menu's rows. `app/mouse.rs` puts it to work.

use std::time::{Duration, Instant};

use fenix_window::Rect;

/// Clicks closer together than this, in time, count as one double or
/// triple click.
pub const MULTI_CLICK: Duration = Duration::from_millis(450);

/// ...and closer than this, in pixels, either way.
pub const MULTI_CLICK_SLOP: f32 = 4.0;

/// Counts a press as the first, second or third of a run of clicks: one
/// soon enough after the last and near enough to it goes on the run, a
/// fourth starts over at one.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClickCounter {
    last: Option<(Instant, (f32, f32), u8)>,
}

impl ClickCounter {
    /// The press at `pos` at `now`: 1, 2 or 3.
    pub fn press(&mut self, now: Instant, pos: (f32, f32)) -> u8 {
        let count = match self.last {
            Some((at, (x, y), n))
                if now.duration_since(at) < MULTI_CLICK && (pos.0 - x).abs() <= MULTI_CLICK_SLOP && (pos.1 - y).abs() <= MULTI_CLICK_SLOP =>
            {
                n % 3 + 1
            }
            _ => 1,
        };
        self.last = Some((now, pos, count));
        count
    }
}

/// The character column drawn at visual column `vcol`, by `col_map` (char
/// column -> visual column, one entry more than the line has chars, as
/// `tabstops::expand_line` and `decorations::layout_line` give it): the
/// last char starting at or before it -- so a click anywhere on a tab's
/// spaces, or on an inlay hint, lands on the char it belongs to. And
/// whether `vcol` is past the line's end.
pub fn char_col_at(col_map: &[usize], vcol: usize) -> (usize, bool) {
    let len = col_map.len().saturating_sub(1);
    let width = col_map.last().copied().unwrap_or(0);
    if vcol >= width {
        return (len, true);
    }
    // `col_map` only grows, so the chars starting at or before `vcol`
    // are a prefix of it.
    let starting = col_map[..len].partition_point(|&v| v <= vcol);
    (starting.saturating_sub(1), false)
}

/// Where in a pane a dragged tab is let go: in the middle it joins the
/// pane's tabs; near an edge it splits the pane there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Middle,
    Left,
    Right,
    Top,
    Bottom,
}

/// How much of a pane's width or height, from each edge, splits it.
pub const DROP_EDGE_SHARE: f32 = 0.2;

/// Which zone of `rect` `pos` is in: the edge it's nearest, when that's
/// within `DROP_EDGE_SHARE` of it, else the middle.
pub fn drop_zone(rect: Rect, pos: (f32, f32)) -> DropZone {
    if rect.w <= 0.0 || rect.h <= 0.0 {
        return DropZone::Middle;
    }
    let (fx, fy) = ((pos.0 - rect.x) / rect.w, (pos.1 - rect.y) / rect.h);
    let edges = [(fx, DropZone::Left), (1.0 - fx, DropZone::Right), (fy, DropZone::Top), (1.0 - fy, DropZone::Bottom)];
    let (near, zone) = edges.into_iter().fold((f32::MAX, DropZone::Middle), |best, e| if e.0 < best.0 { e } else { best });
    if near < DROP_EDGE_SHARE {
        zone
    } else {
        DropZone::Middle
    }
}

/// One row of a context menu: what it says, the key that does the same
/// (shown beside it, as which-key shows it), and what choosing it does.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem<A> {
    pub label: String,
    pub key: String,
    pub action: A,
}

/// A context menu, open at a point: its rows and the one highlighted.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu<A> {
    pub items: Vec<MenuItem<A>>,
    /// The pointer's spot it opened at, in window pixels.
    pub at: (f32, f32),
    pub selected: Option<usize>,
}

/// The space inside a menu's box, around its rows -- half above the
/// first, half under the last, as the other popups have it.
pub const MENU_PADDING: f32 = 8.0;

/// Columns between an item's label and its key.
const MENU_GAP: usize = 3;

impl<A: Clone> Menu<A> {
    pub fn new(items: Vec<MenuItem<A>>, at: (f32, f32)) -> Self {
        Self { items, at, selected: None }
    }

    /// The widest label and the widest key, in columns.
    fn columns(&self) -> (usize, usize) {
        let label = self.items.iter().map(|i| i.label.chars().count()).max().unwrap_or(0);
        let key = self.items.iter().map(|i| i.key.chars().count()).max().unwrap_or(0);
        (label, key)
    }

    /// Each row's text: the label, then the key at the right.
    pub fn rows(&self) -> Vec<(String, String)> {
        let (label_cols, key_cols) = self.columns();
        self.items
            .iter()
            .map(|item| {
                let pad = label_cols - item.label.chars().count() + MENU_GAP + key_cols - item.key.chars().count();
                (format!("{}{}", item.label, " ".repeat(pad)), item.key.clone())
            })
            .collect()
    }

    /// The menu's box: just right of and under the pointer, moved left or
    /// up to stay inside `window_w` by `bottom` (the modeline's top).
    pub fn rect(&self, char_width: f32, line_height: f32, pad_left: f32, window_w: f32, bottom: f32) -> Rect {
        let (label_cols, key_cols) = self.columns();
        let w = (label_cols + MENU_GAP + key_cols) as f32 * char_width + 2.0 * pad_left;
        let h = self.items.len() as f32 * line_height + MENU_PADDING;
        let x = if self.at.0 + w <= window_w { self.at.0 } else { (self.at.0 - w).max(0.0) };
        let y = if self.at.1 + h <= bottom { self.at.1 } else { (self.at.1 - h).max(0.0) };
        Rect { x, y, w, h }
    }

    /// The row under `pos` in a menu drawn at `rect`.
    pub fn row_at(&self, rect: Rect, line_height: f32, pos: (f32, f32)) -> Option<usize> {
        if !rect.contains_point(pos.0, pos.1) {
            return None;
        }
        let row = ((pos.1 - rect.y - MENU_PADDING / 2.0) / line_height).floor();
        (row >= 0.0 && (row as usize) < self.items.len()).then_some(row as usize)
    }

    /// Moves the highlight `by` rows, round from the last to the first;
    /// from nothing highlighted, down starts at the first and up at the
    /// last.
    pub fn step(&mut self, by: isize) {
        let n = self.items.len() as isize;
        if n == 0 {
            return;
        }
        let next = match self.selected {
            Some(i) => (i as isize + by).rem_euclid(n),
            None if by >= 0 => 0,
            None => n - 1,
        };
        self.selected = Some(next as usize);
    }

    /// What the highlighted row does.
    pub fn chosen(&self) -> Option<A> {
        self.selected.and_then(|i| self.items.get(i)).map(|item| item.action.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_close_in_time_and_place_count_up_to_three_then_start_over() {
        let mut clicks = ClickCounter::default();
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        assert_eq!(clicks.press(t, (10.0, 10.0)), 1);
        assert_eq!(clicks.press(ms(200), (12.0, 9.0)), 2);
        assert_eq!(clicks.press(ms(400), (12.0, 9.0)), 3);
        assert_eq!(clicks.press(ms(600), (12.0, 9.0)), 1, "a fourth is a new first");
        assert_eq!(clicks.press(ms(1200), (12.0, 9.0)), 1, "too late");
        assert_eq!(clicks.press(ms(1300), (30.0, 9.0)), 1, "too far");
    }

    #[test]
    fn a_visual_column_lands_on_the_char_drawn_there() {
        // "a\tb" with tabs of 4: a at 0, the tab over 1..4, b at 4.
        let map = [0, 1, 4, 5];
        assert_eq!(char_col_at(&map, 0), (0, false));
        assert_eq!(char_col_at(&map, 1), (1, false));
        assert_eq!(char_col_at(&map, 3), (1, false), "anywhere on the tab's spaces is the tab");
        assert_eq!(char_col_at(&map, 4), (2, false));
        assert_eq!(char_col_at(&map, 5), (3, true), "past the end");
        assert_eq!(char_col_at(&map, 40), (3, true));
        assert_eq!(char_col_at(&[0], 0), (0, true), "an empty line");
        // An inlay of three columns before char 1: what's clicked on it
        // is the char before.
        assert_eq!(char_col_at(&[0, 4, 5], 2), (0, false));
        // Hidden text: chars 1 and 2 drawn at no width.
        assert_eq!(char_col_at(&[0, 1, 1, 1, 2], 1), (3, false));
    }

    #[test]
    fn a_tab_dropped_near_an_edge_splits_there_and_in_the_middle_joins() {
        let rect = Rect { x: 100.0, y: 0.0, w: 400.0, h: 200.0 };
        assert_eq!(drop_zone(rect, (300.0, 100.0)), DropZone::Middle);
        assert_eq!(drop_zone(rect, (110.0, 100.0)), DropZone::Left);
        assert_eq!(drop_zone(rect, (490.0, 100.0)), DropZone::Right);
        assert_eq!(drop_zone(rect, (300.0, 10.0)), DropZone::Top);
        assert_eq!(drop_zone(rect, (300.0, 195.0)), DropZone::Bottom);
        assert_eq!(drop_zone(rect, (105.0, 195.0)), DropZone::Left, "the nearer edge wins in a corner");
    }

    fn menu() -> Menu<u8> {
        let item = |label: &str, key: &str, action| MenuItem { label: label.into(), key: key.into(), action };
        Menu::new(vec![item("Go to definition", "gd", 1), item("Rename", "SPC c r", 2), item("Copy", "y", 3)], (700.0, 500.0))
    }

    #[test]
    fn a_menus_keys_line_up_at_the_right() {
        let rows = menu().rows();
        assert_eq!(rows[0], ("Go to definition".to_string() + &" ".repeat(3 + 5), "gd".to_string()));
        let widths: Vec<usize> = rows.iter().map(|(l, k)| l.chars().count() + k.chars().count()).collect();
        assert!(widths.iter().all(|&w| w == widths[0]), "{rows:?}");
    }

    #[test]
    fn a_menu_opens_beside_the_pointer_and_stays_in_the_window() {
        let m = Menu { at: (100.0, 50.0), ..menu() };
        let rect = m.rect(10.0, 20.0, 8.0, 800.0, 580.0);
        assert_eq!((rect.x, rect.y), (100.0, 50.0));
        assert_eq!((rect.w, rect.h), ((16 + 3 + 7) as f32 * 10.0 + 16.0, 3.0 * 20.0 + MENU_PADDING));
        let rect = menu().rect(10.0, 20.0, 8.0, 800.0, 520.0);
        assert_eq!((rect.x, rect.y), (700.0 - rect.w, 500.0 - rect.h), "flipped left and up at the window's edge");
    }

    #[test]
    fn the_pointer_and_the_keys_pick_a_row() {
        let mut m = Menu { at: (0.0, 0.0), ..menu() };
        let rect = m.rect(10.0, 20.0, 8.0, 800.0, 580.0);
        assert_eq!(m.row_at(rect, 20.0, (5.0, 4.0 + 25.0)), Some(1));
        assert_eq!(m.row_at(rect, 20.0, (5.0, 2.0)), None, "the padding");
        assert_eq!(m.row_at(rect, 20.0, (rect.w + 1.0, 30.0)), None, "outside");
        assert_eq!(m.chosen(), None);
        m.step(1);
        assert_eq!(m.chosen(), Some(1));
        m.step(-1);
        assert_eq!(m.chosen(), Some(3), "round to the last");
        m.selected = None;
        m.step(-1);
        assert_eq!(m.selected, Some(2), "up from nothing starts at the bottom");
    }
}
