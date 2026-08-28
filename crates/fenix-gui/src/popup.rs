//! Generic anchored-popup positioning, shared by every floating panel the
//! editor draws on top of its content -- the which-key hint popup and the
//! autocompletion candidate popup. The only two things every one of these
//! needs in common:
//! an identity (so `TextPipeline` can keep a persistent glyph buffer per
//! popup, the same way it already does per split pane) and a resolved,
//! on-screen `Rect` that never runs off the window -- what's actually
//! drawn *inside* a popup stays specific to each one and lives in
//! `app.rs`.

use fenix_window::Rect;

/// Identifies one popup's glyph buffer in `TextPipeline`. A plain enum
/// rather than a stringly-typed id -- there are only ever a handful of
/// popup *kinds* in an editor UI, and a typo in a string key would be a
/// silent no-render bug instead of a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PopupId {
    WhichKey,
    /// The autocompletion candidate popup, anchored under the caret via
    /// `Anchor::BelowPoint`. Never coexists with `WhichKey` -- one only
    /// appears mid-Normal/-pending-sequence, the other only in Insert
    /// mode -- so no popup-overlap handling is needed anywhere this id
    /// is used.
    Completion,
    /// The Docker panel's contextual "view command options" popup
    /// (`x` on a Containers/Images/Volumes pane), anchored `TopRight`
    /// like `WhichKey`. Never coexists with `WhichKey`/`Completion`
    /// either -- it only shows while the Docker panel has focus, where
    /// neither of those other two popups can appear.
    DockerMenu,
    /// The Git panel's own contextual "view command options" popup
    /// (`x` on a Files/Branches/Commits/Stash pane) -- same shape and
    /// same non-coexistence reasoning as `DockerMenu`, just for the Git
    /// session instead.
    GitMenu,
    /// The Jira panel's own contextual "view command options" popup
    /// (`x` on an Issues/Detail pane) -- same shape and same non-
    /// coexistence reasoning as `DockerMenu`/`GitMenu`, just for the
    /// Jira session instead.
    JiraMenu,
    /// Whichever single-line prompt/confirmation is currently capturing
    /// input (Vim's own `:command`/`/`-`?`-search, the project-grep
    /// query, or one of this app's many others -- see `App::active_
    /// prompt_text`), anchored `BottomLeft` just above the modeline.
    /// Never coexists with any popup above: a prompt captures every
    /// keystroke before `WhichKey`'s pending-sequence state or
    /// `Completion`'s Insert-mode dispatch could ever become active at
    /// the same time, and `DockerMenu`/`GitMenu` are unconditionally
    /// cleared at the very top of `route_keypress`, before any prompt-
    /// capturing check runs.
    Prompt,
}

/// Where a popup wants to appear, before clamping to the window.
#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    /// A fixed distance from the window's top-right corner -- which-key's
    /// own "clear of both the content you're editing and the modeline"
    /// placement.
    TopRight { margin: f32 },
    /// Just below a point in window coordinates (e.g. the caret's current
    /// screen position) -- the completion popup's own placement, tracking
    /// where you're typing.
    BelowPoint { x: f32, y: f32 },
    /// A fixed distance from the window's bottom-left corner, sitting
    /// just above `avoid_bottom` (the modeline's own top edge) -- the
    /// prompt popup's placement: the same corner a capturing prompt's
    /// text used to occupy *inside* the modeline, just elevated into
    /// its own box above it instead of replacing the modeline's content.
    BottomLeft { margin: f32 },
}

/// Resolves `anchor` plus a popup's desired `width`/`height` into a
/// concrete on-screen `Rect`, clamped so it never extends past the right
/// edge of the window or past `avoid_bottom` (the modeline's top edge,
/// for every popup so far) -- the fix for which-key's previously-
/// unclamped top-right placement, which could run under the modeline
/// once enough leader groups existed to make its content taller than the
/// available space. `BelowPoint` additionally flips to render *above*
/// its anchor point when there isn't room below, the standard "flip when
/// it wouldn't fit" convention completion/tooltip popups use everywhere.
///
/// This only clamps *position*, not `width`/`height` themselves -- a
/// popup whose natural content is taller than `avoid_bottom` allows
/// should shrink its own content first (see `max_rows`) rather than be
/// silently cropped here.
pub fn resolve(anchor: Anchor, width: f32, height: f32, window_w: f32, avoid_bottom: f32) -> Rect {
    let max_x = (window_w - width).max(0.0);
    let max_y = (avoid_bottom - height).max(0.0);
    let (x, y) = match anchor {
        Anchor::TopRight { margin } => (window_w - width - margin, margin),
        Anchor::BelowPoint { x, y } => {
            let fits_below = y + height <= avoid_bottom;
            (x, if fits_below { y } else { y - height })
        }
        Anchor::BottomLeft { margin } => (margin, avoid_bottom - height - margin),
    };
    Rect { x: x.clamp(0.0, max_x), y: y.clamp(0.0, max_y), w: width, h: height }
}

/// How many `row_height`-tall rows can fit in a popup before it would run
/// under `avoid_bottom` (accounting for `margin` on both ends and
/// `padding` inside the panel itself) -- callers should truncate their
/// content to this many rows (with a "+N more" summary row, say) rather
/// than let `resolve`'s position-only clamping crop the last row in
/// half. Always at least 1, so there's never a zero-row popup that's
/// impossible to read anything useful from.
pub fn max_rows(avoid_bottom: f32, margin: f32, row_height: f32, padding: f32) -> usize {
    let available = (avoid_bottom - margin * 2.0 - padding).max(0.0);
    ((available / row_height).floor() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_right_places_the_popup_at_the_corner_minus_its_margin() {
        let r = resolve(Anchor::TopRight { margin: 10.0 }, 200.0, 50.0, 800.0, 600.0);
        assert_eq!(r.x, 800.0 - 200.0 - 10.0);
        assert_eq!(r.y, 10.0);
        assert_eq!(r.w, 200.0);
        assert_eq!(r.h, 50.0);
    }

    #[test]
    fn top_right_clamps_when_it_would_run_under_the_modeline() {
        // Content taller than the space above `avoid_bottom` -- position
        // clamps so the popup's bottom edge lands exactly at avoid_bottom
        // instead of extending past it (the bug this module fixes: a
        // which-key panel with enough leader groups to overflow used to
        // just draw under/past the modeline).
        let r = resolve(Anchor::TopRight { margin: 10.0 }, 200.0, 5000.0, 800.0, 600.0);
        assert_eq!(r.y, 0.0); // max_y clamps to 0 since height alone exceeds avoid_bottom
        assert!(r.y + r.h >= 600.0); // still taller than available -- caller must shrink content, not just position
    }

    #[test]
    fn top_right_clamps_when_the_window_is_narrower_than_the_popup_plus_margin() {
        let r = resolve(Anchor::TopRight { margin: 10.0 }, 200.0, 50.0, 150.0, 600.0);
        assert_eq!(r.x, 0.0); // would otherwise be negative
    }

    #[test]
    fn below_point_stays_below_when_there_is_room() {
        let r = resolve(Anchor::BelowPoint { x: 40.0, y: 100.0 }, 150.0, 60.0, 800.0, 600.0);
        assert_eq!(r.x, 40.0);
        assert_eq!(r.y, 100.0);
    }

    #[test]
    fn below_point_flips_above_when_it_would_overflow_the_bottom() {
        let r = resolve(Anchor::BelowPoint { x: 40.0, y: 580.0 }, 150.0, 60.0, 800.0, 600.0);
        assert_eq!(r.y, 580.0 - 60.0); // renders above the point instead
    }

    #[test]
    fn below_point_clamps_horizontally_to_the_window() {
        let r = resolve(Anchor::BelowPoint { x: 790.0, y: 100.0 }, 150.0, 60.0, 800.0, 600.0);
        assert_eq!(r.x, 800.0 - 150.0);
    }

    #[test]
    fn bottom_left_places_the_popup_just_above_avoid_bottom_minus_its_margin() {
        let r = resolve(Anchor::BottomLeft { margin: 10.0 }, 200.0, 50.0, 800.0, 600.0);
        assert_eq!(r.x, 10.0);
        assert_eq!(r.y, 600.0 - 50.0 - 10.0);
        assert_eq!(r.w, 200.0);
        assert_eq!(r.h, 50.0);
    }

    #[test]
    fn bottom_left_clamps_when_content_is_taller_than_available_space() {
        let r = resolve(Anchor::BottomLeft { margin: 10.0 }, 200.0, 5000.0, 800.0, 600.0);
        assert_eq!(r.y, 0.0); // max_y clamps to 0 since height alone exceeds avoid_bottom
        assert!(r.y + r.h >= 600.0); // still taller than available -- caller must shrink content, not just position
    }

    #[test]
    fn bottom_left_clamps_when_the_window_is_narrower_than_the_popup_plus_margin() {
        let r = resolve(Anchor::BottomLeft { margin: 10.0 }, 200.0, 50.0, 150.0, 600.0);
        assert_eq!(r.x, 0.0); // would otherwise sit past the window's own right edge
    }

    #[test]
    fn max_rows_fits_as_many_rows_as_the_available_space_allows() {
        // avoid_bottom=600, margin=10 (x2), padding=12 -> 568px available;
        // at 20px rows that's 28 full rows, the partial 29th not counted.
        assert_eq!(max_rows(600.0, 10.0, 20.0, 12.0), 28);
    }

    #[test]
    fn max_rows_never_returns_zero_even_with_almost_no_room() {
        assert_eq!(max_rows(15.0, 10.0, 20.0, 8.0), 1);
    }
}
