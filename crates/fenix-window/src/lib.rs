mod nav;

use std::sync::atomic::{AtomicU32, Ordering};

pub use nav::{pick, NavDirection};

/// Identity of one leaf window in a `WindowTree`, stable across splits and
/// closes of *other* windows (only closing a window invalidates its own id).
///
/// Unique across *every* tree in the process, not just within one -- ids
/// come from `alloc_id`'s single counter rather than a per-tree one. The
/// host app keys several of its own side tables by this id (`App::pane_
/// titles`, `pdf_outline_panes`, `pdf_search_panes`, every panel session's
/// pane fields), and those tables span every workspace -- and, once one
/// process drives several OS windows, every window too. A per-tree counter
/// made two different panes in two different trees genuinely
/// indistinguishable in those maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(u32);

/// Hands out the next never-yet-used `WindowId`. `Relaxed` is enough:
/// the only thing being ordered against is this counter itself, and all
/// that's required of it is that no two reads ever see the same value
/// (which `fetch_add` guarantees on its own, at any ordering).
fn alloc_id() -> WindowId {
    static NEXT_ID: AtomicU32 = AtomicU32::new(0);
    WindowId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

/// `Horizontal` stacks two windows with a horizontal divider between them
/// (Vim's `:split`); `Vertical` places them side by side with a vertical
/// divider (Vim's `:vsplit`) -- Vim's own naming, not "which axis the
/// windows are arranged along" (which would read backwards to a Vim user).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitKind {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Point-in-rect hit test -- left/top edges inclusive, right/bottom
    /// exclusive, so two adjacent rects sharing an edge never both
    /// claim the same pixel.
    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[cfg(test)]
mod rect_tests {
    use super::Rect;

    #[test]
    fn contains_point_is_true_inside_and_false_outside() {
        let r = Rect { x: 10.0, y: 20.0, w: 100.0, h: 50.0 };
        assert!(r.contains_point(10.0, 20.0)); // top-left corner, inclusive
        assert!(r.contains_point(50.0, 40.0)); // interior
        assert!(!r.contains_point(110.0, 40.0)); // right edge, exclusive
        assert!(!r.contains_point(50.0, 70.0)); // bottom edge, exclusive
        assert!(!r.contains_point(9.9, 40.0)); // just left of the rect
        assert!(!r.contains_point(50.0, 19.9)); // just above the rect
    }
}

enum Node<T> {
    Leaf { id: WindowId, content: T },
    Split { kind: SplitKind, ratio: f32, first: Box<Node<T>>, second: Box<Node<T>> },
}

impl<T> Node<T> {
    fn contains(&self, target: WindowId) -> bool {
        match self {
            Node::Leaf { id, .. } => *id == target,
            Node::Split { first, second, .. } => first.contains(target) || second.contains(target),
        }
    }

    fn first_leaf_id(&self) -> WindowId {
        match self {
            Node::Leaf { id, .. } => *id,
            Node::Split { first, .. } => first.first_leaf_id(),
        }
    }

    fn find(&self, target: WindowId) -> Option<&T> {
        match self {
            Node::Leaf { id, content } if *id == target => Some(content),
            Node::Leaf { .. } => None,
            Node::Split { first, second, .. } => {
                if first.contains(target) { first.find(target) } else { second.find(target) }
            }
        }
    }

    fn find_mut(&mut self, target: WindowId) -> Option<&mut T> {
        match self {
            Node::Leaf { id, content } if *id == target => Some(content),
            Node::Leaf { .. } => None,
            Node::Split { first, second, .. } => {
                if first.contains(target) { first.find_mut(target) } else { second.find_mut(target) }
            }
        }
    }

    /// In-place overwrite of one leaf's payload -- doesn't need to move the
    /// old content out (just drops it via assignment), so unlike
    /// `split_leaf`/`remove_from_split` below this needs no by-value
    /// consume-and-rebuild dance.
    fn set_content(&mut self, target: WindowId, content: T) {
        match self {
            Node::Leaf { id, content: c } if *id == target => *c = content,
            Node::Leaf { .. } => {}
            Node::Split { first, second, .. } => {
                if first.contains(target) {
                    first.set_content(target, content);
                } else {
                    second.set_content(target, content);
                }
            }
        }
    }

    /// Consumes `self` and returns a new tree with `target` (a leaf
    /// somewhere in this subtree) replaced by a `Split` of the old leaf
    /// and a new one. Consuming `self` by value (rather than `&mut self`)
    /// sidesteps needing a placeholder `Node<T>` to satisfy `mem::replace`
    /// for a fully generic, unconstrained `T` -- `contains` (a cheap
    /// `&self` read) picks which branch to recurse into *before* anything
    /// is moved, so `new_content` is only ever consumed along the one path
    /// that actually reaches `target`.
    fn split_leaf(self, target: WindowId, kind: SplitKind, new_id: WindowId, new_content: T) -> Node<T> {
        match self {
            Node::Leaf { id, content } if id == target => Node::Split {
                kind,
                ratio: 0.5,
                first: Box::new(Node::Leaf { id, content }),
                second: Box::new(Node::Leaf { id: new_id, content: new_content }),
            },
            Node::Leaf { .. } => self,
            Node::Split { kind: k, ratio, first, second } => {
                if first.contains(target) {
                    Node::Split { kind: k, ratio, first: Box::new(first.split_leaf(target, kind, new_id, new_content)), second }
                } else {
                    Node::Split { kind: k, ratio, first, second: Box::new(second.split_leaf(target, kind, new_id, new_content)) }
                }
            }
        }
    }

    /// `self` must be a `Split` containing `target` somewhere in it.
    /// Removes `target`, collapsing the `Split` it was directly under into
    /// its sibling. Returns the resulting subtree plus a sensible new
    /// focus (the first leaf of whichever side survived nearest the cut).
    fn remove_from_split(self, target: WindowId) -> (Node<T>, WindowId) {
        let Node::Split { kind, ratio, first, second } = self else {
            unreachable!("remove_from_split called on a Leaf")
        };
        let first_is_target = matches!(&*first, Node::Leaf { id, .. } if *id == target);
        let second_is_target = matches!(&*second, Node::Leaf { id, .. } if *id == target);
        if first_is_target {
            let focus = second.first_leaf_id();
            (*second, focus)
        } else if second_is_target {
            let focus = first.first_leaf_id();
            (*first, focus)
        } else if first.contains(target) {
            let (new_first, focus) = first.remove_from_split(target);
            (Node::Split { kind, ratio, first: Box::new(new_first), second }, focus)
        } else {
            let (new_second, focus) = second.remove_from_split(target);
            (Node::Split { kind, ratio, first, second: Box::new(new_second) }, focus)
        }
    }

    fn into_leaf_content(self, target: WindowId) -> T {
        match self {
            Node::Leaf { id, content } if id == target => content,
            Node::Leaf { .. } => unreachable!("target window id not found in tree"),
            Node::Split { first, second, .. } => {
                if first.contains(target) { first.into_leaf_content(target) } else { second.into_leaf_content(target) }
            }
        }
    }

    fn resize_near(&mut self, target: WindowId, delta: f32) -> bool {
        match self {
            Node::Leaf { .. } => false,
            Node::Split { ratio, first, second, .. } => {
                let first_is_target = matches!(&**first, Node::Leaf { id, .. } if *id == target);
                let second_is_target = matches!(&**second, Node::Leaf { id, .. } if *id == target);
                if first_is_target || second_is_target {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    true
                } else if first.resize_near(target, delta) {
                    true
                } else {
                    second.resize_near(target, delta)
                }
            }
        }
    }

    fn balance(&mut self) {
        if let Node::Split { ratio, first, second, .. } = self {
            *ratio = 0.5;
            first.balance();
            second.balance();
        }
    }

    fn collect_ids(&self, out: &mut Vec<WindowId>) {
        match self {
            Node::Leaf { id, .. } => out.push(*id),
            Node::Split { first, second, .. } => {
                first.collect_ids(out);
                second.collect_ids(out);
            }
        }
    }

    fn layout_into(&self, bounds: Rect, out: &mut Vec<(WindowId, Rect)>) {
        match self {
            Node::Leaf { id, .. } => out.push((*id, bounds)),
            Node::Split { kind, ratio, first, second } => {
                let (a, b) = split_rect(bounds, *kind, *ratio);
                first.layout_into(a, out);
                second.layout_into(b, out);
            }
        }
    }
}

fn split_rect(bounds: Rect, kind: SplitKind, ratio: f32) -> (Rect, Rect) {
    match kind {
        SplitKind::Vertical => {
            let w1 = bounds.w * ratio;
            (
                Rect { x: bounds.x, y: bounds.y, w: w1, h: bounds.h },
                Rect { x: bounds.x + w1, y: bounds.y, w: bounds.w - w1, h: bounds.h },
            )
        }
        SplitKind::Horizontal => {
            let h1 = bounds.h * ratio;
            (
                Rect { x: bounds.x, y: bounds.y, w: bounds.w, h: h1 },
                Rect { x: bounds.x, y: bounds.y + h1, w: bounds.w, h: bounds.h - h1 },
            )
        }
    }
}

/// A binary tree of horizontal/vertical splits, leaves holding an opaque
/// `T` (in practice a buffer handle) -- generic and host-agnostic, no
/// knowledge of buffers, GPU, or rendering. Mirrors `fenix-picker`'s role
/// as a small, independently-testable engine reused by the host app.
///
/// `root` is `Option<Node<T>>` purely as a `take()`/put-back vehicle for
/// the handful of operations (`split`, `close_focused`, `only`) that need
/// to consume-and-rebuild a subtree by value -- it is `Some` at every
/// point control returns to a caller; every public method upholds and
/// relies on that invariant.
pub struct WindowTree<T> {
    root: Option<Node<T>>,
    focused: WindowId,
}

impl<T> WindowTree<T> {
    pub fn new(content: T) -> Self {
        let id = alloc_id();
        Self { root: Some(Node::Leaf { id, content }), focused: id }
    }

    fn root(&self) -> &Node<T> {
        self.root.as_ref().expect("WindowTree::root invariant: always Some between calls")
    }

    fn root_mut(&mut self) -> &mut Node<T> {
        self.root.as_mut().expect("WindowTree::root invariant: always Some between calls")
    }

    pub fn focused_id(&self) -> WindowId {
        self.focused
    }

    pub fn focus(&mut self, id: WindowId) -> bool {
        if self.root().contains(id) {
            self.focused = id;
            true
        } else {
            false
        }
    }

    pub fn content(&self, id: WindowId) -> Option<&T> {
        self.root().find(id)
    }

    pub fn content_mut(&mut self, id: WindowId) -> Option<&mut T> {
        self.root_mut().find_mut(id)
    }

    pub fn focused_content(&self) -> &T {
        self.content(self.focused).expect("focused window always has content")
    }

    pub fn focused_content_mut(&mut self) -> &mut T {
        let id = self.focused;
        self.content_mut(id).expect("focused window always has content")
    }

    pub fn set_content(&mut self, id: WindowId, content: T) {
        self.root_mut().set_content(id, content);
    }

    /// Splits the focused window, placing the new one after (right of, for
    /// `Vertical`; below, for `Horizontal`) the old one -- Doom Emacs's own
    /// `splitright`/`splitbelow` default. The new window becomes focused.
    pub fn split(&mut self, kind: SplitKind, content: T) -> WindowId {
        let new_id = alloc_id();
        let focused = self.focused;
        let old_root = self.root.take().expect("WindowTree::root invariant");
        self.root = Some(old_root.split_leaf(focused, kind, new_id, content));
        self.focused = new_id;
        new_id
    }

    /// Closes the focused window. A no-op returning `false` if it's the
    /// only window left (nothing to collapse into).
    pub fn close_focused(&mut self) -> bool {
        if self.window_count() <= 1 {
            return false;
        }
        let focused = self.focused;
        let old_root = self.root.take().expect("WindowTree::root invariant");
        let (new_root, next_focus) = old_root.remove_from_split(focused);
        self.root = Some(new_root);
        self.focused = next_focus;
        true
    }

    /// Closes every window except the focused one -- a permanent
    /// `:only`/`SPC w o`, not Doom's reversible temporary "enlarge"
    /// (that needs layout history this tree doesn't keep, a disclosed cut).
    pub fn only(&mut self) {
        let focused = self.focused;
        let old_root = self.root.take().expect("WindowTree::root invariant");
        let content = old_root.into_leaf_content(focused);
        self.root = Some(Node::Leaf { id: focused, content });
    }

    /// Focuses the next window in a stable pre-order traversal, wrapping
    /// around -- `SPC w w`.
    pub fn cycle_next(&mut self) -> WindowId {
        let ids = self.windows();
        let pos = ids.iter().position(|&id| id == self.focused).unwrap_or(0);
        let next = ids[(pos + 1) % ids.len()];
        self.focused = next;
        next
    }

    /// Adjusts the ratio of the focused window's nearest ancestor split by
    /// `delta`, clamped to `[0.1, 0.9]` so neither side can be squeezed to
    /// nothing. A no-op on a single-window tree.
    pub fn resize_focused(&mut self, delta: f32) {
        let focused = self.focused;
        self.root_mut().resize_near(focused, delta);
    }

    /// Resets every split's ratio to an even 0.5 -- `SPC w =`.
    pub fn balance(&mut self) {
        self.root_mut().balance();
    }

    pub fn window_count(&self) -> usize {
        self.windows().len()
    }

    /// All window ids in a stable pre-order traversal.
    pub fn windows(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        self.root().collect_ids(&mut out);
        out
    }

    /// Computes each leaf's pixel `Rect` within `bounds`, recursively
    /// subdividing at each split's ratio. Content is fetched separately
    /// via `content(id)` -- keeps layout decoupled from `T`'s ownership.
    pub fn layout(&self, bounds: Rect) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        self.root().layout_into(bounds, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tree_has_one_window_holding_the_given_content() {
        let t = WindowTree::new("a");
        assert_eq!(t.window_count(), 1);
        assert_eq!(t.focused_content(), &"a");
    }

    #[test]
    fn split_adds_a_window_and_focuses_it() {
        let mut t = WindowTree::new("a");
        let new_id = t.split(SplitKind::Vertical, "b");
        assert_eq!(t.window_count(), 2);
        assert_eq!(t.focused_id(), new_id);
        assert_eq!(t.focused_content(), &"b");
    }

    #[test]
    fn splitting_twice_yields_three_windows_all_reachable() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b");
        t.split(SplitKind::Horizontal, "c");
        assert_eq!(t.window_count(), 3);
        let contents: Vec<&str> = t.windows().iter().map(|&id| *t.content(id).unwrap()).collect();
        let mut sorted = contents.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["a", "b", "c"]);
    }

    #[test]
    fn close_focused_returns_false_on_the_last_window() {
        let mut t = WindowTree::new("a");
        assert!(!t.close_focused());
        assert_eq!(t.window_count(), 1);
    }

    #[test]
    fn close_focused_collapses_back_to_one_window() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b"); // focused: b
        assert!(t.close_focused());
        assert_eq!(t.window_count(), 1);
        assert_eq!(t.focused_content(), &"a");
    }

    #[test]
    fn close_focused_promotes_the_sibling_and_focuses_it() {
        let mut t = WindowTree::new("a");
        let b = t.split(SplitKind::Vertical, "b");
        t.split(SplitKind::Horizontal, "c"); // splits b -> focused: c, sibling: b
        t.focus(b);
        assert!(t.close_focused()); // closes b, its sibling c should be promoted+focused
        assert_eq!(t.window_count(), 2);
        assert_eq!(t.focused_content(), &"c");
        let remaining: Vec<&str> = t.windows().iter().map(|&id| *t.content(id).unwrap()).collect();
        let mut sorted = remaining.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["a", "c"]);
    }

    #[test]
    fn only_closes_every_other_window() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b");
        t.split(SplitKind::Horizontal, "c"); // focused: c
        t.only();
        assert_eq!(t.window_count(), 1);
        assert_eq!(t.focused_content(), &"c");
    }

    #[test]
    fn focus_moves_to_an_existing_window_and_rejects_unknown_ids() {
        let mut t = WindowTree::new("a");
        let b = t.split(SplitKind::Vertical, "b");
        assert!(t.focus(b));
        assert_eq!(t.focused_content(), &"b");

        // An id this tree never allocated. Ids are process-wide unique
        // (see `alloc_id`), so a real id from another tree is guaranteed
        // to be one this tree has never seen -- no need to fabricate an
        // out-of-range one, and this now exercises the case that
        // actually happens in the host app (a pane id belonging to some
        // other workspace, or some other OS window's tree).
        let other = WindowTree::new("elsewhere");
        assert!(!t.focus(other.focused_id()));
    }

    #[test]
    fn two_trees_never_hand_out_the_same_window_id() {
        let mut a = WindowTree::new("a");
        a.split(SplitKind::Vertical, "a2");
        let mut b = WindowTree::new("b");
        b.split(SplitKind::Vertical, "b2");

        for id in a.windows() {
            assert!(!b.windows().contains(&id), "{id:?} was handed out by both trees");
        }
    }

    #[test]
    fn set_content_overwrites_in_place_without_changing_shape() {
        let mut t = WindowTree::new("a");
        let b = t.split(SplitKind::Vertical, "b");
        t.set_content(b, "b2");
        assert_eq!(t.content(b), Some(&"b2"));
        assert_eq!(t.window_count(), 2);
    }

    #[test]
    fn content_mut_allows_in_place_mutation() {
        let mut t = WindowTree::new(String::from("a"));
        t.focused_content_mut().push('!');
        assert_eq!(t.focused_content(), "a!");
    }

    #[test]
    fn cycle_next_wraps_around() {
        let mut t = WindowTree::new("a");
        let b = t.split(SplitKind::Vertical, "b"); // focused: b
        let a = t.windows().into_iter().find(|&id| id != b).unwrap();
        assert_eq!(t.cycle_next(), a); // wraps from b back to a in a 2-window tree
        assert_eq!(t.cycle_next(), b);
    }

    #[test]
    fn resize_focused_clamps_the_nearest_ancestor_ratio() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b"); // focused: b, ratio 0.5
        t.resize_focused(0.3);
        let rects = t.layout(Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 });
        let a_rect = rects.iter().find(|(_, r)| r.x == 0.0).unwrap().1;
        assert!((a_rect.w - 80.0).abs() < 0.01); // ratio 0.8 of width 100

        t.resize_focused(1.0); // clamps at 0.9, not 1.0
        let rects = t.layout(Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 });
        let a_rect = rects.iter().find(|(_, r)| r.x == 0.0).unwrap().1;
        assert!((a_rect.w - 90.0).abs() < 0.01);
    }

    #[test]
    fn balance_resets_every_split_to_half() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b");
        t.resize_focused(0.3);
        t.balance();
        let rects = t.layout(Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 });
        for (_, r) in rects {
            assert!((r.w - 50.0).abs() < 0.01);
        }
    }

    #[test]
    fn layout_vertical_split_divides_width() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b");
        let rects = t.layout(Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 });
        assert_eq!(rects.len(), 2);
        for (_, r) in &rects {
            assert_eq!(r.h, 100.0);
            assert_eq!(r.w, 100.0);
        }
        let xs: Vec<f32> = rects.iter().map(|(_, r)| r.x).collect();
        assert!(xs.contains(&0.0) && xs.contains(&100.0));
    }

    #[test]
    fn layout_horizontal_split_divides_height() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Horizontal, "b");
        let rects = t.layout(Rect { x: 0.0, y: 0.0, w: 100.0, h: 200.0 });
        assert_eq!(rects.len(), 2);
        for (_, r) in &rects {
            assert_eq!(r.w, 100.0);
            assert_eq!(r.h, 100.0);
        }
        let ys: Vec<f32> = rects.iter().map(|(_, r)| r.y).collect();
        assert!(ys.contains(&0.0) && ys.contains(&100.0));
    }

    #[test]
    fn layout_covers_the_full_bounds_with_no_overlap_in_a_nested_tree() {
        let mut t = WindowTree::new("a");
        t.split(SplitKind::Vertical, "b");
        t.split(SplitKind::Horizontal, "c"); // splits b
        let bounds = Rect { x: 0.0, y: 0.0, w: 200.0, h: 200.0 };
        let rects = t.layout(bounds);
        assert_eq!(rects.len(), 3);
        let total_area: f32 = rects.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((total_area - bounds.w * bounds.h).abs() < 0.01);
    }
}
