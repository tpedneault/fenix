//! The window's panels: the editor's panes, the sidebar, the terminal
//! and the docks sit on the theme's tray as rounded surfaces, a gap
//! apart, with no lines between them. Inside a panel, cards (an agent's
//! tool calls, a permission it asks for, a debugger column) and keys'
//! caps are rounded boxes of their own. This is the geometry of all of
//! that, kept apart from `redraw` so it can be tested without a GPU.

use fenix_window::Rect;

/// Half the gap between two panels, and the tray left at the window's
/// edges: two of these between panels, two at the edges.
pub(crate) const HALF_GAP: f32 = 3.0;

/// A panel's corners.
pub(crate) const PANEL_RADIUS: f32 = 8.0;

/// A card's corners, inside a panel.
pub(crate) const CARD_RADIUS: f32 = 6.0;

/// A key's cap.
pub(crate) const KEY_RADIUS: f32 = 4.0;

/// Which of a pane's edges are the pane area's own, rather than shared
/// with another pane: those keep the tray's edge, the others half a gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
}

/// `rect`, one of the panes laid out over `area`, inset to where its
/// panel is drawn: half a gap on each side, and a whole one at the
/// area's left, top and right edges when `outer` says that edge meets
/// the window rather than the sidebar. The bottom is always half a gap:
/// under it is the terminal, a dock or the modeline, each with its own.
pub(crate) fn framed(rect: Rect, area: Rect, outer: Edges) -> Rect {
    let near = |a: f32, b: f32| (a - b).abs() < 0.5;
    let left = if outer.left && near(rect.x, area.x) { HALF_GAP * 2.0 } else { HALF_GAP };
    let top = if outer.top && near(rect.y, area.y) { HALF_GAP * 2.0 } else { HALF_GAP };
    let right = if outer.right && near(rect.x + rect.w, area.x + area.w) { HALF_GAP * 2.0 } else { HALF_GAP };
    let bottom = HALF_GAP;
    Rect { x: rect.x + left, y: rect.y + top, w: (rect.w - left - right).max(0.0), h: (rect.h - top - bottom).max(0.0) }
}

/// The panel drawn for a strip across the window's width at `y`, `h`
/// high -- the terminal or the debug dock: the tray's edge at the sides,
/// half a gap above and below.
pub(crate) fn strip_panel(window_width: f32, y: f32, h: f32) -> Rect {
    Rect { x: HALF_GAP * 2.0, y: y + HALF_GAP, w: (window_width - HALF_GAP * 4.0).max(0.0), h: (h - HALF_GAP * 2.0).max(0.0) }
}

/// The sidebar's panel, in its `width` down to `bottom`.
pub(crate) fn sidebar_panel(width: f32, bottom: f32) -> Rect {
    Rect { x: HALF_GAP * 2.0, y: HALF_GAP * 2.0, w: (width - HALF_GAP * 3.0).max(0.0), h: (bottom - HALF_GAP * 3.0).max(0.0) }
}

/// A card around rows `first..=last` of a text column, rows `line_height`
/// high from `top`, across `x..x + w`: a little taller than its rows, so
/// their text isn't on its edge -- into the blank rows a card has around
/// it -- and as wide as it's given.
pub(crate) fn card_rect(x: f32, w: f32, top: f32, line_height: f32, first: usize, last: usize) -> Rect {
    let pad = (line_height * 0.25).round();
    let y = top + first as f32 * line_height;
    let h = (last.max(first) - first + 1) as f32 * line_height;
    Rect { x, y: y - pad, w, h: h + pad * 2.0 }
}

/// A key's cap behind columns `start..end` of a row at `y`: a char
/// either side wide less a little, so neighbouring caps stay apart, and
/// most of the row high.
pub(crate) fn key_cap(x0: f32, char_width: f32, y: f32, line_height: f32, start: usize, end: usize) -> Rect {
    let inset_y = (line_height * 0.12).round();
    let inset_x = (char_width * 0.25).round();
    let x = x0 + start as f32 * char_width - inset_x;
    let w = (end.max(start + 1) - start) as f32 * char_width + inset_x * 2.0;
    Rect { x, y: y + inset_y, w, h: line_height - inset_y * 2.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
    const ALL: Edges = Edges { left: true, top: true, right: true };

    #[test]
    fn one_pane_keeps_the_tray_at_the_window_edges() {
        let r = framed(AREA, AREA, ALL);
        assert_eq!(r, Rect { x: 6.0, y: 6.0, w: 788.0, h: 591.0 });
    }

    #[test]
    fn two_panes_side_by_side_are_a_whole_gap_apart() {
        let left = framed(Rect { x: 0.0, y: 0.0, w: 400.0, h: 600.0 }, AREA, ALL);
        let right = framed(Rect { x: 400.0, y: 0.0, w: 400.0, h: 600.0 }, AREA, ALL);
        assert_eq!(right.x - (left.x + left.w), HALF_GAP * 2.0);
        assert_eq!(left.x, HALF_GAP * 2.0, "the window's edge");
        assert_eq!(right.x + right.w, 800.0 - HALF_GAP * 2.0);
    }

    #[test]
    fn beside_the_sidebar_a_pane_leaves_the_gap_to_it() {
        let area = Rect { x: 200.0, ..AREA };
        let r = framed(Rect { x: 200.0, y: 0.0, w: 600.0, h: 600.0 }, area, Edges { left: false, ..ALL });
        assert_eq!(r.x, 200.0 + HALF_GAP);
        let sidebar = sidebar_panel(200.0, 600.0);
        assert_eq!(r.x - (sidebar.x + sidebar.w), HALF_GAP * 2.0, "a whole gap from the sidebar's panel");
    }

    #[test]
    fn a_pane_smaller_than_its_gaps_is_empty_not_negative() {
        let r = framed(Rect { x: 0.0, y: 0.0, w: 4.0, h: 4.0 }, AREA, ALL);
        assert_eq!((r.w, r.h), (0.0, 0.0));
    }

    #[test]
    fn a_strip_is_inset_from_the_window_and_from_what_is_above_and_below() {
        let s = strip_panel(800.0, 400.0, 100.0);
        assert_eq!(s, Rect { x: 6.0, y: 403.0, w: 788.0, h: 94.0 });
    }

    #[test]
    fn a_card_wraps_its_rows_and_a_cap_its_columns() {
        let card = card_rect(10.0, 300.0, 100.0, 20.0, 2, 4);
        assert!(card.y < 140.0 && card.y > 130.0 && card.y + card.h > 200.0 && card.y + card.h < 210.0, "around its rows, not into the next: {card:?}");
        let cap = key_cap(8.0, 10.0, 50.0, 20.0, 3, 4);
        assert!(cap.x < 38.0 && cap.x + cap.w > 48.0 && cap.y > 50.0 && cap.y + cap.h < 70.0, "{cap:?}");
    }
}
