//! Fit-to-page pixel-size math -- pure, no pdfium/thread knowledge, same
//! split-out-pure-math shape as `fenix_vnc::coords` (this crate's own
//! closest precedent). A PDF page's "native size" is in points (1/72
//! inch), not pixels, so unlike VNC's pane-pixel <-> framebuffer-pixel
//! ratio, this is a scale-to-fit computation: the largest pixel size that
//! preserves the page's aspect ratio while fitting entirely inside the
//! pane.

/// The target render size, in pixels, for a page of `page_w_pts` x
/// `page_h_pts` (PDF points, 1/72 inch) so that it fits entirely inside a
/// pane of `pane_w_px` x `pane_h_px`, preserving aspect ratio. Returns
/// `(1, 1)` (never `(0, 0)` -- a zero-sized render target is invalid) if
/// either input dimension is non-positive, since there's no meaningful
/// fit in that case and this is only ever consulted once a page's real
/// size is known and a pane has been laid out.
pub fn fit_page_size(page_w_pts: f32, page_h_pts: f32, pane_w_px: u32, pane_h_px: u32) -> (u32, u32) {
    if page_w_pts <= 0.0 || page_h_pts <= 0.0 || pane_w_px == 0 || pane_h_px == 0 {
        return (1, 1);
    }
    let scale = (pane_w_px as f32 / page_w_pts).min(pane_h_px as f32 / page_h_pts);
    let width = (page_w_pts * scale).round().max(1.0) as u32;
    let height = (page_h_pts * scale).round().max(1.0) as u32;
    (width, height)
}

/// Same idea as `fit_page_size`, but only the pane's *width* constrains
/// the scale -- the page renders at exactly `pane_w_px` wide and however
/// tall that makes it, which can easily exceed `pane_h_px` (a tall page
/// scrolls vertically rather than shrinking to fit, unlike `fit_page_
/// size`). Degenerate inputs fall back to `(1, 1)` for the same reason
/// `fit_page_size` does.
pub fn fit_width_size(page_w_pts: f32, page_h_pts: f32, pane_w_px: u32) -> (u32, u32) {
    if page_w_pts <= 0.0 || page_h_pts <= 0.0 || pane_w_px == 0 {
        return (1, 1);
    }
    let scale = pane_w_px as f32 / page_w_pts;
    let height = (page_h_pts * scale).round().max(1.0) as u32;
    (pane_w_px.max(1), height)
}

/// The render size, in pixels, for a page of `page_w_pts` x `page_h_pts`
/// at `percent` of its "native" size -- defined here as `100` meaning one
/// pixel per PDF point (matches `fit_page_size`'s own pixels-per-point
/// convention when a pane happens to be exactly the page's point size).
/// Entirely pane-independent, unlike `fit_page_size`/`fit_width_size` --
/// this is what lets a percent zoom stay put across a window resize
/// instead of silently re-fitting. Degenerate inputs (non-positive page
/// size or `percent`) fall back to `(1, 1)`.
pub fn percent_size(page_w_pts: f32, page_h_pts: f32, percent: u32) -> (u32, u32) {
    if page_w_pts <= 0.0 || page_h_pts <= 0.0 || percent == 0 {
        return (1, 1);
    }
    let scale = percent as f32 / 100.0;
    let width = (page_w_pts * scale).round().max(1.0) as u32;
    let height = (page_h_pts * scale).round().max(1.0) as u32;
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_a_square_page_to_a_square_pane_exactly() {
        assert_eq!(fit_page_size(100.0, 100.0, 500, 500), (500, 500));
    }

    #[test]
    fn letterboxes_a_portrait_page_in_a_wider_pane() {
        // page is 2:1 tall (100x200 pts), pane is a 800x800 square --
        // height is the constraining axis (scale = 800/200 = 4), so
        // width comes out narrower than the pane, not stretched to fill it.
        assert_eq!(fit_page_size(100.0, 200.0, 800, 800), (400, 800));
    }

    #[test]
    fn letterboxes_a_landscape_page_in_a_taller_pane() {
        // page is 2:1 wide (200x100 pts), pane is a 400x800 tall
        // rectangle -- width is the constraining axis (scale = 400/200 = 2).
        assert_eq!(fit_page_size(200.0, 100.0, 400, 800), (400, 200));
    }

    #[test]
    fn scales_up_a_small_page_to_fill_a_large_pane() {
        assert_eq!(fit_page_size(72.0, 72.0, 720, 720), (720, 720));
    }

    #[test]
    fn degenerate_inputs_fall_back_to_a_1x1_target_instead_of_a_zero_sized_one() {
        assert_eq!(fit_page_size(0.0, 100.0, 500, 500), (1, 1));
        assert_eq!(fit_page_size(100.0, 0.0, 500, 500), (1, 1));
        assert_eq!(fit_page_size(100.0, 100.0, 0, 500), (1, 1));
        assert_eq!(fit_page_size(100.0, 100.0, 500, 0), (1, 1));
        assert_eq!(fit_page_size(-1.0, 100.0, 500, 500), (1, 1));
    }

    #[test]
    fn preserves_aspect_ratio_within_rounding() {
        let (w, h) = fit_page_size(612.0, 792.0, 1000, 1000); // US Letter
        let expected_ratio = 612.0 / 792.0;
        let actual_ratio = w as f32 / h as f32;
        assert!((actual_ratio - expected_ratio).abs() < 0.01, "w={w} h={h} ratio={actual_ratio}");
    }

    #[test]
    fn fit_width_matches_the_pane_width_exactly() {
        assert_eq!(fit_width_size(100.0, 200.0, 800).0, 800);
    }

    #[test]
    fn fit_width_can_produce_a_height_taller_than_any_given_pane() {
        // A tall page at full pane width easily exceeds a short pane's
        // height -- that's the whole point (vertical scroll), not a bug.
        let (w, h) = fit_width_size(100.0, 2000.0, 400);
        assert_eq!(w, 400);
        assert_eq!(h, 8000);
    }

    #[test]
    fn fit_width_degenerate_inputs_fall_back_to_1x1() {
        assert_eq!(fit_width_size(0.0, 100.0, 500), (1, 1));
        assert_eq!(fit_width_size(100.0, 0.0, 500), (1, 1));
        assert_eq!(fit_width_size(100.0, 100.0, 0), (1, 1));
    }

    #[test]
    fn percent_100_is_one_pixel_per_point() {
        assert_eq!(percent_size(612.0, 792.0, 100), (612, 792));
    }

    #[test]
    fn percent_scales_linearly() {
        assert_eq!(percent_size(100.0, 200.0, 50), (50, 100));
        assert_eq!(percent_size(100.0, 200.0, 200), (200, 400));
    }

    #[test]
    fn percent_degenerate_inputs_fall_back_to_1x1() {
        assert_eq!(percent_size(0.0, 100.0, 100), (1, 1));
        assert_eq!(percent_size(100.0, 100.0, 0), (1, 1));
    }
}
