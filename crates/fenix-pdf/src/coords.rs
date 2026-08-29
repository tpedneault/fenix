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
}
