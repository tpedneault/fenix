//! A commit graph's rows as shapes: the Log page lays each row out as
//! marks in lanes (`page::GraphMark`), and this places them in pixels --
//! rails as thin bars, bends as a bar, a quarter circle and a bar, and
//! each commit as a dot.
//!
//! Lanes are spaced two cells apart while they fit the cells the page
//! gave the graph; past that they're drawn closer, down to `MIN_PITCH`,
//! and past that the rightmost fold into the last lane, drawn fainter --
//! a history with dozens of branches live at once keeps its messages in
//! view rather than pushing them off the page.

use crate::page::{GraphMark, NodeLook};

/// The closest lanes are drawn, in pixels.
pub(super) const MIN_PITCH: f32 = 6.0;

/// A shape to draw, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Shape {
    /// A bar: (x, y, w, h).
    Bar(f32, f32, f32, f32, [f32; 4]),
    /// A quarter circle's outline around (cx, cy), `r` out, toward
    /// right/down.
    Arc { cx: f32, cy: f32, r: f32, right: bool, down: bool, color: [f32; 4] },
    /// A dot at (cx, cy), filled or a ring.
    Dot { cx: f32, cy: f32, r: f32, ring: bool, color: [f32; 4] },
}

/// Where the lanes go across `width` pixels for `lanes` of them: the
/// distance between two, and how many have a place of their own (the
/// rest share the last).
pub(super) fn pitch(width: f32, lanes: usize, char_width: f32) -> (f32, usize) {
    let lanes = lanes.max(1);
    let widest = char_width * 2.0;
    let pitch = (width / lanes as f32).clamp(MIN_PITCH.min(widest), widest);
    let shown = ((width / pitch).floor() as usize).clamp(1, lanes);
    (pitch, shown)
}

/// How thick the lines are, for a font `char_width` wide.
pub(super) fn thickness(char_width: f32) -> f32 {
    (char_width * 0.2).clamp(1.25, 3.0)
}

/// The shapes for one row of the graph: `marks` across the cells from
/// `x` that are `width` wide, on the row from `y`, `line_height` tall,
/// for a graph of `lanes` lanes. `color` is each mark's colour; `ring`
/// HEAD's ring's.
#[allow(clippy::too_many_arguments)]
pub(super) fn shapes(x: f32, width: f32, y: f32, line_height: f32, char_width: f32, lanes: usize, marks: &[(GraphMark, [f32; 4])], ring: [f32; 4]) -> Vec<Shape> {
    let (pitch, shown) = pitch(width, lanes, char_width);
    let t = thickness(char_width);
    let folded = |lane: usize| lanes > shown && lane + 1 >= shown;
    let lane_x = |lane: usize| (x + pitch * (lane.min(shown - 1) as f32 + 0.5)).round();
    let faded = |lane: usize, c: [f32; 4]| if folded(lane) { [c[0], c[1], c[2], c[3] * 0.45] } else { c };
    let (top, mid, bottom) = (y, (y + line_height / 2.0).round(), y + line_height);
    let node_r = (line_height * 0.2).clamp(2.5, 5.5).min(pitch * 0.45).max(2.0);
    let mut out = Vec::new();
    let mut dots = Vec::new();
    for &(mark, color) in marks {
        match mark {
            GraphMark::Rail { lane, top: up, bottom: down, .. } => {
                let (from, to) = (if up { top } else { mid }, if down { bottom } else { mid });
                if to > from {
                    out.push(Shape::Bar(lane_x(lane) - t / 2.0, from, t, to - from, faded(lane, color)));
                }
            }
            GraphMark::Bend { from, to, into, .. } => {
                let color = faded(if into { from } else { to }, color);
                let (a, b) = (lane_x(from), lane_x(to));
                if (a - b).abs() < 0.5 {
                    // Folded into the same place: straight on.
                    let (y0, y1) = if into { (top, mid) } else { (mid, bottom) };
                    out.push(Shape::Bar(a - t / 2.0, y0, t, y1 - y0, color));
                    continue;
                }
                let r = (a - b).abs().min(line_height / 2.0).max(1.0);
                let right = b > a;
                if into {
                    // Down from the top at `a`, round the corner, across into
                    // the commit at `b`.
                    let (cx, cy) = (if right { a + r } else { a - r }, mid - r);
                    out.push(Shape::Bar(a - t / 2.0, top, t, (cy - top).max(0.0), color));
                    out.push(Shape::Arc { cx, cy, r, right: !right, down: true, color });
                    let (x0, x1) = if right { (cx, b) } else { (b, cx) };
                    out.push(Shape::Bar(x0, mid - t / 2.0, (x1 - x0).max(0.0), t, color));
                } else {
                    // Out of the commit at `a`, across, round the corner and
                    // down from `b` to the bottom.
                    let (cx, cy) = (if right { b - r } else { b + r }, mid + r);
                    let (x0, x1) = if right { (a, cx) } else { (cx, a) };
                    out.push(Shape::Bar(x0, mid - t / 2.0, (x1 - x0).max(0.0), t, color));
                    out.push(Shape::Arc { cx, cy, r, right, down: false, color });
                    out.push(Shape::Bar(b - t / 2.0, cy, t, (bottom - cy).max(0.0), color));
                }
            }
            GraphMark::Node { lane, look, .. } => {
                let (cx, color) = (lane_x(lane), faded(lane, color));
                match look {
                    NodeLook::Commit => dots.push(Shape::Dot { cx, cy: mid, r: node_r, ring: false, color }),
                    NodeLook::Merge => {
                        dots.push(Shape::Dot { cx, cy: mid, r: node_r, ring: true, color });
                        dots.push(Shape::Dot { cx, cy: mid, r: (node_r * 0.4).max(1.0), ring: false, color });
                    }
                    NodeLook::Head => {
                        dots.push(Shape::Dot { cx, cy: mid, r: node_r + t + 1.0, ring: true, color: ring });
                        dots.push(Shape::Dot { cx, cy: mid, r: node_r, ring: false, color });
                    }
                }
            }
        }
    }
    // The dots over the lines that meet them.
    out.extend(dots);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Role;

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const RING: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

    fn rail(lane: usize, top: bool, bottom: bool) -> (GraphMark, [f32; 4]) {
        (GraphMark::Rail { lane, top, bottom, role: Role::Text }, RED)
    }

    /// Lanes are two cells apart while they fit; closer when they don't;
    /// and past the closest, the rest share the last place.
    #[test]
    fn lanes_close_up_to_fit_then_fold() {
        assert_eq!(pitch(160.0, 4, 8.0), (16.0, 4), "two cells a lane");
        let (p, shown) = pitch(160.0, 20, 8.0);
        assert_eq!((p, shown), (8.0, 20), "closer, all shown");
        let (p, shown) = pitch(160.0, 100, 8.0);
        assert_eq!(p, MIN_PITCH);
        assert_eq!(shown, 26, "the rest fold into the last");
        assert_eq!(pitch(10.0, 0, 8.0).1, 1);
    }

    /// A rail runs the whole row, or the half toward the commit.
    #[test]
    fn a_rail_is_the_part_of_the_row_it_crosses() {
        let s = shapes(0.0, 64.0, 100.0, 20.0, 8.0, 4, &[rail(1, true, true), rail(0, true, false)], RING);
        let Shape::Bar(x, y, w, h, _) = s[0] else { panic!() };
        assert_eq!((x + w / 2.0, y, h), (24.0, 100.0, 20.0), "lane 1's middle, the whole row");
        let Shape::Bar(_, y, _, h, _) = s[1] else { panic!() };
        assert_eq!((y, h), (100.0, 10.0), "to the commit's middle");
    }

    /// A merge's second parent bends out of the commit and down its own
    /// lane; a branch ending here bends in from above.
    #[test]
    fn bends_turn_a_quarter_circle() {
        let out = (GraphMark::Bend { from: 0, to: 2, into: false, role: Role::Text }, RED);
        let s = shapes(0.0, 64.0, 0.0, 20.0, 8.0, 4, &[out], RING);
        assert_eq!(s.len(), 3);
        let Shape::Arc { cx, cy, r, right, down, .. } = s[1] else { panic!() };
        assert_eq!((cx, cy, r, right, down), (30.0, 20.0, 10.0, true, false), "turning down into lane 2 at x=40");
        let into = (GraphMark::Bend { from: 2, to: 0, into: true, role: Role::Text }, RED);
        let s = shapes(0.0, 64.0, 0.0, 20.0, 8.0, 4, &[into], RING);
        let Shape::Arc { cx, cy, right, down, .. } = s[1] else { panic!() };
        assert_eq!((cx, cy, right, down), (30.0, 0.0, true, true), "from the top of lane 2 round toward lane 0");
    }

    /// A commit is a dot over its lines; a merge a ring; HEAD ringed.
    #[test]
    fn a_commits_dot_is_drawn_last() {
        let marks = [(GraphMark::Node { lane: 0, look: NodeLook::Head, role: Role::Text }, RED), rail(0, true, true)];
        let s = shapes(0.0, 64.0, 0.0, 20.0, 8.0, 1, &marks, RING);
        assert!(matches!(s[0], Shape::Bar(..)));
        assert!(matches!(s[1], Shape::Dot { ring: true, color: RING, .. }));
        assert!(matches!(s[2], Shape::Dot { ring: false, color: RED, .. }));
    }

    /// A lane folded into the last place is drawn fainter.
    #[test]
    fn a_folded_lane_is_fainter() {
        let s = shapes(0.0, 12.0, 0.0, 20.0, 8.0, 50, &[rail(40, true, true), rail(0, true, true)], RING);
        let Shape::Bar(x40, .., c40) = s[0] else { panic!() };
        let Shape::Bar(x0, .., c0) = s[1] else { panic!() };
        assert!(x40 > x0 && c40[3] < c0[3], "{x40} {x0} {c40:?}");
    }
}
