use crate::{Rect, WindowTree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Ratios (not absolute pixels) determine relative position, so the actual
/// size used for this comparison doesn't matter -- just needs to be large
/// enough that float arithmetic on it stays comfortably precise.
const REFERENCE_SIZE: f32 = 10_000.0;

/// Picks the nearest of `candidates` lying in `dir` from the one keyed
/// `from`, using real layout geometry rather than any structural
/// adjacency (which wouldn't match what the user visually sees).
/// Standard directional-navigation heuristic: among candidates strictly
/// on that side, take whichever has the greatest edge overlap on the
/// perpendicular axis, tie-broken by closest distance. `None` when
/// nothing qualifies -- typically already at the edge.
///
/// Generic over the key, and over whatever coordinate space the rects
/// are in, so one rule serves both callers: `WindowTree::navigate`
/// passes one tree's panes in its own layout space, while the host app
/// passes *every* pane of *every* OS window in desktop coordinates, so
/// that walking off the left edge of one monitor's window continues
/// into the window on the monitor next door. Panes in the same window
/// are simply nearer than panes in the one beside it, so unifying the
/// two needs no special case at all.
pub fn pick<K: Copy + PartialEq>(candidates: &[(K, Rect)], from: K, dir: NavDirection) -> Option<K> {
    let &(_, from_rect) = candidates.iter().find(|(key, _)| *key == from)?;

    let mut best: Option<(K, f32, f32)> = None; // (key, overlap, distance): maximize overlap, then minimize distance
    for &(key, rect) in candidates {
        if key == from {
            continue;
        }
        let Some((overlap, distance)) = candidate_score(from_rect, rect, dir) else { continue };
        let better = match best {
            None => true,
            Some((_, best_overlap, best_distance)) => {
                overlap > best_overlap || (overlap == best_overlap && distance < best_distance)
            }
        };
        if better {
            best = Some((key, overlap, distance));
        }
    }
    best.map(|(key, ..)| key)
}

impl<T> WindowTree<T> {
    /// Moves focus to the nearest window in `dir` from the focused one.
    /// Returns `false` (no-op, focus unchanged) if nothing qualifies --
    /// e.g. already at the grid's edge. See `pick` for the rule.
    pub fn navigate(&mut self, dir: NavDirection) -> bool {
        let bounds = Rect { x: 0.0, y: 0.0, w: REFERENCE_SIZE, h: REFERENCE_SIZE };
        let rects = self.layout(bounds);
        match pick(&rects, self.focused_id(), dir) {
            Some(id) => {
                self.focus(id);
                true
            }
            None => false,
        }
    }
}

/// If `other` is strictly in `dir` from `focused` (allowing a small epsilon
/// for float slack at shared edges), returns `(perpendicular-axis overlap,
/// edge-to-edge distance)`; `None` if `other` isn't a candidate at all.
fn candidate_score(focused: Rect, other: Rect, dir: NavDirection) -> Option<(f32, f32)> {
    const EPS: f32 = 0.01;
    match dir {
        NavDirection::Right => {
            if other.x + EPS < focused.x + focused.w {
                return None;
            }
            let overlap = vertical_overlap(focused, other);
            (overlap > 0.0).then_some((overlap, other.x - (focused.x + focused.w)))
        }
        NavDirection::Left => {
            if other.x + other.w > focused.x + EPS {
                return None;
            }
            let overlap = vertical_overlap(focused, other);
            (overlap > 0.0).then_some((overlap, focused.x - (other.x + other.w)))
        }
        NavDirection::Down => {
            if other.y + EPS < focused.y + focused.h {
                return None;
            }
            let overlap = horizontal_overlap(focused, other);
            (overlap > 0.0).then_some((overlap, other.y - (focused.y + focused.h)))
        }
        NavDirection::Up => {
            if other.y + other.h > focused.y + EPS {
                return None;
            }
            let overlap = horizontal_overlap(focused, other);
            (overlap > 0.0).then_some((overlap, focused.y - (other.y + other.h)))
        }
    }
}

fn vertical_overlap(a: Rect, b: Rect) -> f32 {
    let lo = a.y.max(b.y);
    let hi = (a.y + a.h).min(b.y + b.h);
    (hi - lo).max(0.0)
}

fn horizontal_overlap(a: Rect, b: Rect) -> f32 {
    let lo = a.x.max(b.x);
    let hi = (a.x + a.w).min(b.x + b.w);
    (hi - lo).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SplitKind, WindowId};

    /// Builds a 2x2 grid: `Vertical(Horizontal(TL, BL), Horizontal(TR, BR))`.
    /// Returns the tree plus each quadrant's id.
    fn grid() -> (WindowTree<&'static str>, WindowId, WindowId, WindowId, WindowId) {
        let mut t = WindowTree::new("TL");
        let tl = t.focused_id();
        let bl = t.split(SplitKind::Horizontal, "BL");
        t.focus(tl);
        let tr = t.split(SplitKind::Vertical, "TR");
        let br = t.split(SplitKind::Horizontal, "BR");
        (t, tl, bl, tr, br)
    }

    /// Two side-by-side monitors' worth of rects in one desktop
    /// coordinate space: a 1920-wide screen at x=0 split into two
    /// panes, and a 1920-wide screen at x=1920 split into two panes.
    /// Keyed `(window, pane)` the way the host app keys them.
    fn two_screens() -> Vec<((u8, u8), Rect)> {
        vec![
            ((0, 0), Rect { x: 0.0, y: 0.0, w: 960.0, h: 1080.0 }),
            ((0, 1), Rect { x: 960.0, y: 0.0, w: 960.0, h: 1080.0 }),
            ((1, 0), Rect { x: 1920.0, y: 0.0, w: 960.0, h: 1080.0 }),
            ((1, 1), Rect { x: 2880.0, y: 0.0, w: 960.0, h: 1080.0 }),
        ]
    }

    #[test]
    fn pick_crosses_from_one_screens_leftmost_pane_into_the_next_screens_rightmost() {
        let screens = two_screens();
        // The whole point: going left off the edge of the right-hand
        // monitor lands on the *nearest* pane of the left-hand one --
        // its rightmost -- not its leftmost.
        assert_eq!(pick(&screens, (1, 0), NavDirection::Left), Some((0, 1)));
        // And symmetrically going the other way.
        assert_eq!(pick(&screens, (0, 1), NavDirection::Right), Some((1, 0)));
    }

    #[test]
    fn pick_stays_inside_a_screen_while_there_is_somewhere_to_go() {
        let screens = two_screens();
        assert_eq!(pick(&screens, (1, 1), NavDirection::Left), Some((1, 0)), "the neighbouring pane is nearer than the next screen");
        assert_eq!(pick(&screens, (0, 0), NavDirection::Right), Some((0, 1)));
    }

    #[test]
    fn pick_off_the_far_edge_of_the_outermost_screen_is_a_no_op() {
        let screens = two_screens();
        assert_eq!(pick(&screens, (0, 0), NavDirection::Left), None);
        assert_eq!(pick(&screens, (1, 1), NavDirection::Right), None);
        assert_eq!(pick(&screens, (0, 0), NavDirection::Up), None);
    }

    #[test]
    fn pick_skips_a_screen_that_does_not_overlap_vertically_at_all() {
        // A window on a monitor stacked above the others shares no
        // horizontal band with the one to its left, so a leftward move
        // from it has no candidate rather than jumping diagonally.
        let mut screens = two_screens();
        screens.push(((2, 0), Rect { x: 1920.0, y: -1080.0, w: 1920.0, h: 1080.0 }));
        assert_eq!(pick(&screens, (2, 0), NavDirection::Left), None);
        assert_eq!(pick(&screens, (2, 0), NavDirection::Down), Some((1, 0)));
    }

    #[test]
    fn pick_from_a_key_that_is_not_in_the_list_yields_nothing() {
        assert_eq!(pick(&two_screens(), (9, 9), NavDirection::Left), None);
    }

    #[test]
    fn navigate_right_and_left_across_the_grid() {
        let (mut t, tl, _bl, tr, _br) = grid();
        t.focus(tl);
        assert!(t.navigate(NavDirection::Right));
        assert_eq!(t.focused_id(), tr);
        assert!(t.navigate(NavDirection::Left));
        assert_eq!(t.focused_id(), tl);
    }

    #[test]
    fn navigate_down_and_up_across_the_grid() {
        let (mut t, tl, bl, ..) = grid();
        t.focus(tl);
        assert!(t.navigate(NavDirection::Down));
        assert_eq!(t.focused_id(), bl);
        assert!(t.navigate(NavDirection::Up));
        assert_eq!(t.focused_id(), tl);
    }

    #[test]
    fn navigate_diagonally_via_two_moves() {
        let (mut t, tl, _bl, _tr, br) = grid();
        t.focus(tl);
        assert!(t.navigate(NavDirection::Right));
        assert!(t.navigate(NavDirection::Down));
        assert_eq!(t.focused_id(), br);
    }

    #[test]
    fn navigate_off_the_grid_edge_is_a_no_op() {
        let (mut t, tl, ..) = grid();
        t.focus(tl);
        assert!(!t.navigate(NavDirection::Left));
        assert_eq!(t.focused_id(), tl);
        assert!(!t.navigate(NavDirection::Up));
        assert_eq!(t.focused_id(), tl);
    }

    #[test]
    fn navigate_on_a_single_window_tree_is_always_a_no_op() {
        let mut t = WindowTree::new("only");
        for dir in [NavDirection::Left, NavDirection::Right, NavDirection::Up, NavDirection::Down] {
            assert!(!t.navigate(dir));
        }
    }

    #[test]
    fn navigate_picks_the_window_with_greatest_perpendicular_overlap() {
        // A tall left column (full height) next to two stacked windows on
        // the right, resized so the top one occupies 80% of the right
        // column's height and the bottom one only 20%. From the left
        // window, Right should land on the top one -- it overlaps the left
        // window's height far more (80 units vs. 20), even though both
        // right-side windows are equally close (both touch the same shared
        // edge), so this specifically exercises the overlap comparison,
        // not the distance tie-break.
        let mut t = WindowTree::new("L");
        let l = t.focused_id();
        let tr = t.split(SplitKind::Vertical, "TR"); // Vertical(L, TR), 50/50
        t.split(SplitKind::Horizontal, "BR"); // splits TR -> Vertical(L, Horizontal(TR, BR))
        t.focus(tr);
        t.resize_focused(0.3); // TR/BR split ratio 0.5 -> 0.8: TR gets 80% of the right column's height
        t.focus(l);
        assert!(t.navigate(NavDirection::Right));
        assert_eq!(t.focused_id(), tr);
    }
}
