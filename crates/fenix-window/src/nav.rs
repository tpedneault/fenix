use crate::{Rect, WindowId, WindowTree};

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

impl<T> WindowTree<T> {
    /// Moves focus to the nearest window in `dir` from the focused one,
    /// using real layout geometry rather than tree adjacency (which
    /// wouldn't match what the user visually sees). Standard directional-
    /// window-nav heuristic: among windows strictly on that side, picks
    /// whichever has the greatest edge overlap on the perpendicular axis,
    /// tie-broken by closest distance. Returns `false` (no-op, focus
    /// unchanged) if nothing qualifies -- e.g. already at the grid's edge.
    pub fn navigate(&mut self, dir: NavDirection) -> bool {
        let bounds = Rect { x: 0.0, y: 0.0, w: REFERENCE_SIZE, h: REFERENCE_SIZE };
        let rects = self.layout(bounds);
        let focused_id = self.focused_id();
        let Some(&(_, focused_rect)) = rects.iter().find(|(id, _)| *id == focused_id) else {
            return false;
        };

        let mut best: Option<(WindowId, f32, f32)> = None; // (id, overlap, distance): maximize overlap, then minimize distance
        for &(id, rect) in &rects {
            if id == focused_id {
                continue;
            }
            let Some((overlap, distance)) = candidate_score(focused_rect, rect, dir) else { continue };
            let better = match best {
                None => true,
                Some((_, best_overlap, best_distance)) => {
                    overlap > best_overlap || (overlap == best_overlap && distance < best_distance)
                }
            };
            if better {
                best = Some((id, overlap, distance));
            }
        }

        match best {
            Some((id, ..)) => {
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
    use crate::SplitKind;

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
