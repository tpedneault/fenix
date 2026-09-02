//! Pane-pixel <-> VM-framebuffer-pixel coordinate scaling -- used both to
//! send correctly-scaled pointer events to the server
//! (`pane_to_framebuffer`) and, on the rendering side, to size the
//! textured quad's destination rect (the inverse direction is just the
//! same ratio applied the other way, exposed here as
//! `framebuffer_to_pane` mainly so both directions have one obviously-
//! correct, tested implementation rather than each caller re-deriving
//! the ratio independently).
//!
//! Stays load-bearing even with `VncClient::request_resize` ("remote
//! resizing," matching the framebuffer to the pane instead of scaling
//! it): a server that doesn't support the extension, or hasn't answered
//! yet, still leaves pane and framebuffer sizes mismatched, and this is
//! what keeps that case rendering (scaled, not distorted-aspect or
//! cropped) instead of assuming they're always equal.

/// Maps a point in pane-local pixels to the corresponding pixel in the
/// VM's actual framebuffer, clamped to `0..fb_size` on each axis. Returns
/// `(0, 0)` if the pane has zero size or the framebuffer resolution isn't
/// known yet (`fb_size` of `(0, 0)`) -- there's no meaningful mapping in
/// either case, and this is only ever consulted while a VNC pane is
/// already focused/visible, so a degenerate input is a transient state
/// (e.g. a frame between resize and the next redraw), not a real error.
pub fn pane_to_framebuffer(pane_px: (f32, f32), pane_size: (f32, f32), fb_size: (u16, u16)) -> (u16, u16) {
    if pane_size.0 <= 0.0 || pane_size.1 <= 0.0 || fb_size.0 == 0 || fb_size.1 == 0 {
        return (0, 0);
    }
    let x = (pane_px.0 / pane_size.0) * fb_size.0 as f32;
    let y = (pane_px.1 / pane_size.1) * fb_size.1 as f32;
    let clamp = |v: f32, max: u16| v.round().clamp(0.0, max as f32 - 1.0) as u16;
    (clamp(x, fb_size.0), clamp(y, fb_size.1))
}

/// The inverse of `pane_to_framebuffer`: where a framebuffer pixel lands
/// in pane-local pixels, given the pane's current on-screen size. Used to
/// size the textured quad's destination rect (the whole framebuffer maps
/// onto the whole pane) rather than per-pixel, but exposed per-pixel here
/// so both directions share one tested ratio.
pub fn framebuffer_to_pane(fb_px: (u16, u16), pane_size: (f32, f32), fb_size: (u16, u16)) -> (f32, f32) {
    if fb_size.0 == 0 || fb_size.1 == 0 {
        return (0.0, 0.0);
    }
    let x = (fb_px.0 as f32 / fb_size.0 as f32) * pane_size.0;
    let y = (fb_px.1 as f32 / fb_size.1 as f32) * pane_size.1;
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_when_pane_and_framebuffer_are_the_same_size() {
        assert_eq!(pane_to_framebuffer((100.0, 50.0), (800.0, 600.0), (800, 600)), (100, 50));
    }

    #[test]
    fn scales_up_from_a_smaller_pane_to_a_larger_framebuffer() {
        // pane is half the framebuffer's size on each axis
        assert_eq!(pane_to_framebuffer((100.0, 100.0), (400.0, 300.0), (800, 600)), (200, 200));
    }

    #[test]
    fn scales_down_from_a_larger_pane_to_a_smaller_framebuffer() {
        // pane is 4x the framebuffer's size on each axis
        assert_eq!(pane_to_framebuffer((400.0, 400.0), (800.0, 800.0), (200, 200)), (100, 100));
    }

    #[test]
    fn clamps_to_the_last_valid_framebuffer_pixel_instead_of_overrunning() {
        assert_eq!(pane_to_framebuffer((800.0, 600.0), (800.0, 600.0), (800, 600)), (799, 599));
        assert_eq!(pane_to_framebuffer((-10.0, -10.0), (800.0, 600.0), (800, 600)), (0, 0));
    }

    #[test]
    fn degenerate_sizes_map_to_the_origin_instead_of_dividing_by_zero() {
        assert_eq!(pane_to_framebuffer((10.0, 10.0), (0.0, 0.0), (800, 600)), (0, 0));
        assert_eq!(pane_to_framebuffer((10.0, 10.0), (800.0, 600.0), (0, 0)), (0, 0));
    }

    #[test]
    fn round_trips_within_a_pixel_of_rounding_error() {
        let cases: [((f32, f32), (u16, u16)); 3] = [((400.0, 300.0), (800, 600)), ((100.0, 100.0), (100, 100)), ((37.0, 51.0), (1920, 1080))];
        for (pane_size, fb_size) in cases {
            for pane_px in [(0.0, 0.0), (pane_size.0 / 2.0, pane_size.1 / 2.0), (pane_size.0 - 1.0, pane_size.1 - 1.0)] {
                let fb_px = pane_to_framebuffer(pane_px, pane_size, fb_size);
                let back = framebuffer_to_pane(fb_px, pane_size, fb_size);
                assert!((back.0 - pane_px.0).abs() <= 1.0, "x: {pane_px:?} -> {fb_px:?} -> {back:?} (pane_size={pane_size:?}, fb_size={fb_size:?})");
                assert!((back.1 - pane_px.1).abs() <= 1.0, "y: {pane_px:?} -> {fb_px:?} -> {back:?} (pane_size={pane_size:?}, fb_size={fb_size:?})");
            }
        }
    }
}
