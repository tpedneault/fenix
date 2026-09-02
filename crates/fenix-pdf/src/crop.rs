//! Extracting a visible sub-rectangle out of a full page render for
//! panning. Unlike `fenix_vnc::framebuffer::blit_rect` (copies a small
//! dirty-rect *into* a persistent full-size buffer), this crate never
//! needs dirty-rects -- a rendered PDF page always arrives whole -- so
//! the direction is reversed: cropping a possibly-larger-than-the-pane
//! full bitmap *down* to just the pane-sized window `scroll_offset`
//! currently has scrolled to, which is what actually gets uploaded to the
//! GPU texture each frame (see `fenix-gui`'s own pane-classification/
//! redraw loop, this crate's own top-level doc comment on the
//! `winit`/`wgpu`-free split).

/// Copies a `crop_w` x `crop_h` window out of `src` (a tightly-packed
/// BGRA buffer of `src_w` x `src_h` pixels), starting at `(x, y)`,
/// returning a fresh tightly-packed BGRA buffer of exactly `crop_w` x
/// `crop_h` pixels. Convenience wrapper over `crop_bgra_into` for call
/// sites that don't have a buffer to reuse (tests, mostly) -- the
/// per-frame one in `fenix-gui` does, and goes through `crop_bgra_into`
/// directly so panning a large page doesn't allocate a fresh
/// multi-megabyte `Vec` on every keypress.
pub fn crop_bgra(src: &[u8], src_w: u32, src_h: u32, x: u32, y: u32, crop_w: u32, crop_h: u32) -> Vec<u8> {
    let mut dest = Vec::new();
    crop_bgra_into(&mut dest, src, src_w, src_h, x, y, crop_w, crop_h);
    dest
}

/// `crop_bgra`'s actual body, writing into a caller-owned `dest` that's
/// resized (and reused, keeping its allocation) rather than freshly
/// allocated -- the crop window is pane-sized, so it changes size rarely
/// (a resize/zoom) but gets rewritten often (every pan step), which is
/// exactly the shape a reused scratch buffer is for. `dest`'s previous
/// contents are entirely overwritten; nothing is read out of it.
///
/// Clamped to `src`'s actual bounds rather than panicking (same "never
/// trust the caller's math to be exactly right" posture as
/// `fenix_vnc::framebuffer::blit_rect`) -- any pixel the crop window
/// would have covered past `src`'s edge comes back as opaque black
/// (`[0, 0, 0, 255]`) instead of leaving stale bytes from a previous
/// call, since this is meant to be uploaded straight to a texture every
/// call, not blitted onto something that already has content underneath
/// it. That out-of-bounds fill deliberately only touches the rows/
/// columns the copy below doesn't reach: filling the *whole* buffer
/// first and then overwriting almost all of it again is a second full
/// pass over several megabytes for nothing, on a path that runs on every
/// single pan keypress.
pub fn crop_bgra_into(dest: &mut Vec<u8>, src: &[u8], src_w: u32, src_h: u32, x: u32, y: u32, crop_w: u32, crop_h: u32) {
    const BYTES_PER_PIXEL: usize = 4;
    let dest_stride = crop_w as usize * BYTES_PER_PIXEL;
    dest.clear();
    dest.resize(crop_h as usize * dest_stride, 0);
    let (copy_w, copy_h) = if x >= src_w || y >= src_h {
        (0, 0)
    } else {
        (crop_w.min(src_w - x) as usize, crop_h.min(src_h - y) as usize)
    };
    // Opaque black, not transparent, for whatever the copy below won't
    // cover -- see this function's own doc comment on why alpha 0 would
    // be misleading, and why only the uncovered region is touched.
    for row in 0..crop_h as usize {
        let row_start = row * dest_stride;
        let uncovered = if row < copy_h { copy_w * BYTES_PER_PIXEL } else { 0 };
        for alpha_byte in (row_start + uncovered + BYTES_PER_PIXEL - 1..row_start + dest_stride).step_by(BYTES_PER_PIXEL) {
            dest[alpha_byte] = 255;
        }
    }
    if copy_w == 0 || copy_h == 0 {
        return;
    }
    let src_stride = src_w as usize * BYTES_PER_PIXEL;
    for row in 0..copy_h {
        let src_start = (y as usize + row) * src_stride + x as usize * BYTES_PER_PIXEL;
        let src_end = src_start + copy_w * BYTES_PER_PIXEL;
        let Some(src_row) = src.get(src_start..src_end) else { break };
        let dest_start = row * dest_stride;
        let dest_end = dest_start + copy_w * BYTES_PER_PIXEL;
        let Some(dest_row) = dest.get_mut(dest_start..dest_end) else { break };
        dest_row.copy_from_slice(src_row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Vec<u8> {
        bgra.repeat(w as usize * h as usize)
    }

    fn pixel_at(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * w as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn crops_the_full_source_when_the_window_matches_exactly() {
        let src = solid(4, 4, [1, 2, 3, 4]);
        let cropped = crop_bgra(&src, 4, 4, 0, 0, 4, 4);
        assert_eq!(cropped, src);
    }

    #[test]
    fn crops_a_window_at_a_nonzero_offset() {
        // 4x4 source, top-left quadrant is one color, rest is another.
        let mut src = solid(4, 4, [9, 9, 9, 255]);
        for y in 0..2u32 {
            for x in 0..2u32 {
                let i = (y as usize * 4 + x as usize) * 4;
                src[i..i + 4].copy_from_slice(&[1, 2, 3, 255]);
            }
        }
        let cropped = crop_bgra(&src, 4, 4, 0, 0, 2, 2);
        assert_eq!(pixel_at(&cropped, 2, 0, 0), [1, 2, 3, 255]);
        assert_eq!(pixel_at(&cropped, 2, 1, 1), [1, 2, 3, 255]);

        let cropped2 = crop_bgra(&src, 4, 4, 2, 2, 2, 2);
        assert_eq!(pixel_at(&cropped2, 2, 0, 0), [9, 9, 9, 255]);
    }

    #[test]
    fn a_window_starting_outside_the_source_returns_opaque_black_not_a_panic() {
        let src = solid(4, 4, [1, 2, 3, 4]);
        let cropped = crop_bgra(&src, 4, 4, 10, 10, 2, 2);
        assert!(cropped.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn a_window_overrunning_the_sources_edge_is_clamped_not_a_panic() {
        let src = solid(4, 4, [5, 6, 7, 8]);
        // window is 4x4 starting at (2,2) in a 4x4 source -- overruns by
        // 2 on each axis, so only the top-left 2x2 of the result is real.
        let cropped = crop_bgra(&src, 4, 4, 2, 2, 4, 4);
        assert_eq!(cropped.len(), 4 * 4 * 4);
        assert_eq!(pixel_at(&cropped, 4, 0, 0), [5, 6, 7, 8]);
        assert_eq!(pixel_at(&cropped, 4, 1, 1), [5, 6, 7, 8]);
        // the overrun portion is opaque black, not garbage/zero-alpha.
        assert_eq!(pixel_at(&cropped, 4, 3, 3), [0, 0, 0, 255]);
    }

    #[test]
    fn zero_sized_crop_returns_an_empty_buffer_without_panicking() {
        let src = solid(4, 4, [1, 2, 3, 4]);
        let cropped = crop_bgra(&src, 4, 4, 0, 0, 0, 0);
        assert!(cropped.is_empty());
    }

    #[test]
    fn crop_into_reuses_a_dirty_buffer_without_leaking_its_old_contents() {
        let src = solid(4, 4, [1, 2, 3, 4]);
        let mut dest = vec![0xABu8; 4 * 4 * 4];
        // Same window twice, the second time into a buffer that was
        // already the right size but full of unrelated bytes.
        crop_bgra_into(&mut dest, &src, 4, 4, 0, 0, 2, 2);
        assert_eq!(dest, crop_bgra(&src, 4, 4, 0, 0, 2, 2));
        crop_bgra_into(&mut dest, &src, 4, 4, 10, 10, 2, 2);
        assert!(dest.chunks_exact(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn crop_into_fills_only_the_overrun_with_opaque_black_and_copies_the_rest() {
        let src = solid(4, 4, [5, 6, 7, 8]);
        let mut dest = Vec::new();
        crop_bgra_into(&mut dest, &src, 4, 4, 2, 2, 4, 4);
        assert_eq!(dest, crop_bgra(&src, 4, 4, 2, 2, 4, 4));
        assert_eq!(pixel_at(&dest, 4, 0, 0), [5, 6, 7, 8]);
        assert_eq!(pixel_at(&dest, 4, 2, 0), [0, 0, 0, 255]);
        assert_eq!(pixel_at(&dest, 4, 0, 2), [0, 0, 0, 255]);
    }
}
