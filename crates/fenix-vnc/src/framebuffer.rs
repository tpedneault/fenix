//! Copying one decoded VNC update rectangle into a session's full-size
//! CPU-side framebuffer. `vnc-rs` delivers updates as per-dirty-rectangle
//! `RawImage(rect, data)` events (BGRA, tightly packed), not whole
//! frames -- the caller keeps one persistent `width*height*4` buffer per
//! session (recreated on `VncFrame::Resolution`) and blits each rect into
//! it as it arrives, both to keep a full picture for texture creation on
//! the first frame after a resolution change and so a partial redraw only
//! needs to re-upload what actually changed.

/// Copies `src` (a tightly-packed BGRA rect of `width`x`height` pixels)
/// into `dest`, a `dest_width`-pixel-wide BGRA buffer, at `(x, y)`.
/// Clamped to `dest`'s actual bounds rather than panicking: a
/// misbehaving or momentarily-out-of-sync server sending a rect that
/// overruns the framebuffer shouldn't be able to crash the editor over
/// it, it should just have its overrun portion silently dropped.
pub fn blit_rect(dest: &mut [u8], dest_width: u16, dest_height: u16, x: u16, y: u16, width: u16, height: u16, src: &[u8]) {
    const BYTES_PER_PIXEL: usize = 4;
    if x >= dest_width || y >= dest_height {
        return;
    }
    let copy_width = width.min(dest_width - x) as usize;
    let copy_height = height.min(dest_height - y) as usize;
    let src_stride = width as usize * BYTES_PER_PIXEL;
    let dest_stride = dest_width as usize * BYTES_PER_PIXEL;
    for row in 0..copy_height {
        let src_start = row * src_stride;
        let src_end = src_start + copy_width * BYTES_PER_PIXEL;
        let Some(src_row) = src.get(src_start..src_end) else { break };
        let dest_start = (y as usize + row) * dest_stride + x as usize * BYTES_PER_PIXEL;
        let dest_end = dest_start + copy_width * BYTES_PER_PIXEL;
        let Some(dest_row) = dest.get_mut(dest_start..dest_end) else { break };
        dest_row.copy_from_slice(src_row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rect(width: u16, height: u16, bgra: [u8; 4]) -> Vec<u8> {
        bgra.repeat(width as usize * height as usize)
    }

    #[test]
    fn copies_a_rect_into_the_right_place_at_a_nonzero_offset() {
        let mut dest = vec![0u8; 4 * 4 * 4]; // 4x4 BGRA, all zero
        let src = solid_rect(2, 2, [10, 20, 30, 255]);
        blit_rect(&mut dest, 4, 4, 1, 1, 2, 2, &src);

        let pixel_at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 4 + x) * 4;
            [dest[i], dest[i + 1], dest[i + 2], dest[i + 3]]
        };
        assert_eq!(pixel_at(1, 1), [10, 20, 30, 255]);
        assert_eq!(pixel_at(2, 1), [10, 20, 30, 255]);
        assert_eq!(pixel_at(1, 2), [10, 20, 30, 255]);
        assert_eq!(pixel_at(2, 2), [10, 20, 30, 255]);
        // untouched corner stays zero
        assert_eq!(pixel_at(0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel_at(3, 3), [0, 0, 0, 0]);
    }

    #[test]
    fn a_rect_starting_outside_the_buffer_is_dropped_entirely_not_a_panic() {
        let mut dest = vec![0u8; 4 * 4 * 4];
        let src = solid_rect(2, 2, [1, 2, 3, 4]);
        blit_rect(&mut dest, 4, 4, 10, 10, 2, 2, &src);
        assert!(dest.iter().all(|&b| b == 0));
    }

    #[test]
    fn a_rect_overrunning_the_buffers_edge_is_clamped_not_a_panic() {
        let mut dest = vec![0u8; 4 * 4 * 4];
        let src = solid_rect(4, 4, [5, 6, 7, 8]);
        // a 4x4 rect placed at (2,2) in a 4x4 dest overruns by 2 on each axis
        blit_rect(&mut dest, 4, 4, 2, 2, 4, 4, &src);
        let pixel_at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 4 + x) * 4;
            [dest[i], dest[i + 1], dest[i + 2], dest[i + 3]]
        };
        assert_eq!(pixel_at(2, 2), [5, 6, 7, 8]);
        assert_eq!(pixel_at(3, 3), [5, 6, 7, 8]);
        // nothing beyond the dest bounds was written (would have panicked otherwise)
        assert_eq!(dest.len(), 4 * 4 * 4);
    }

    #[test]
    fn a_truncated_source_buffer_stops_early_instead_of_panicking() {
        // Claims to be a 2x2 rect but only has enough data for one whole
        // row -- the second (missing) row is silently skipped rather
        // than panicking or drawing a partial, corrupted pixel from
        // whatever garbage would follow the buffer's actual end.
        let mut dest = vec![0u8; 4 * 4 * 4];
        let src = solid_rect(2, 1, [9, 9, 9, 9]); // one full row's worth of data
        blit_rect(&mut dest, 4, 4, 0, 0, 2, 2, &src);
        let pixel_at = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 4 + x) * 4;
            [dest[i], dest[i + 1], dest[i + 2], dest[i + 3]]
        };
        assert_eq!(pixel_at(0, 0), [9, 9, 9, 9]);
        assert_eq!(pixel_at(1, 0), [9, 9, 9, 9]);
        assert_eq!(pixel_at(0, 1), [0, 0, 0, 0]);
    }
}
