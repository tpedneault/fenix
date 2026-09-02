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

/// Smallest `(x0, y0, x1, y1)` half-open box containing both inputs.
fn union_rect(a: (u16, u16, u16, u16), b: (u16, u16, u16, u16)) -> (u16, u16, u16, u16) {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// One VNC session's live CPU-side pixel buffer -- safe to mutate from a
/// background reader thread and read from a render thread once wrapped
/// in `Arc<Mutex<_>>` by the caller. This type itself has no threading
/// or GPU awareness: every method is plain and synchronous, so keep it
/// that way -- a slow or blocking method here would reintroduce, behind
/// a mutex instead of a winit event queue, the exact main-thread-stall
/// problem this type exists to avoid (see `fenix-gui`'s `VncReader` and
/// `App::redraw` for how the two sides actually share one of these).
pub struct VncFramebuffer {
    pixels: Vec<u8>,
    width: u16,
    height: u16,
    /// Bumped on every `apply_rect`/`apply_copy`. Not what a renderer
    /// should key off -- see `commit`: mid-update the buffer is
    /// deliberately allowed to be inconsistent, so this counts writes,
    /// not showable frames.
    generation: u64,
    /// Bounding box of everything written since the last `commit`, in
    /// pixels -- `(x0, y0, x1, y1)` half-open. Merged into
    /// `committed_dirty` by `commit`.
    pending_dirty: Option<(u16, u16, u16, u16)>,
    /// Bounding box of everything committed but not yet uploaded, taken
    /// (and cleared) by `take_dirty`. Kept separate from `pending_dirty`
    /// so a renderer that skips a frame doesn't lose the region it hadn't
    /// drawn yet.
    committed_dirty: Option<(u16, u16, u16, u16)>,
}

impl VncFramebuffer {
    pub fn new() -> Self {
        Self { pixels: Vec::new(), width: 0, height: 0, generation: 0, pending_dirty: None, committed_dirty: None }
    }

    /// See `generation`'s own doc comment.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Marks everything written since the last call as one finished,
    /// self-consistent frame -- call this on `VncFrame::UpdateEnd`, and
    /// only there.
    ///
    /// RFB updates are atomic per *message*, not per rectangle:
    /// `CopyRect` is defined against the framebuffer as it stood at the
    /// start of the update, so a frame shown partway through one can
    /// legitimately have content in two places at once, or regions that
    /// should already have been erased. Committing at the boundary is
    /// what lets `take_dirty` hand a renderer only whole frames.
    pub fn commit(&mut self) {
        let Some(pending) = self.pending_dirty.take() else { return };
        self.committed_dirty = Some(match self.committed_dirty {
            Some(existing) => union_rect(existing, pending),
            None => pending,
        });
    }

    /// The bounding box of everything committed since the last call,
    /// clamped to the buffer, as `(x, y, width, height)` -- `None` when
    /// there's nothing new to draw. Clears it, so a caller that takes a
    /// region owns uploading it.
    ///
    /// A single bounding box rather than the individual rectangles: in
    /// the common case (a window redrawing, a cursor moving, text
    /// scrolling) it's a tight region, and in the pathological case
    /// (two opposite corners touched) it degrades to the whole
    /// framebuffer -- which is exactly what the previous
    /// upload-everything-every-time behaviour did unconditionally.
    pub fn take_dirty(&mut self) -> Option<(u16, u16, u16, u16)> {
        let (x0, y0, x1, y1) = self.committed_dirty.take()?;
        let x1 = x1.min(self.width);
        let y1 = y1.min(self.height);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    /// Copies one sub-rectangle out into `out` as tightly-packed BGRA
    /// (stride `width * 4`), which is the layout a GPU texture upload
    /// wants -- `pixels` itself is strided by the full framebuffer
    /// width, so it can't be handed over directly. `out` is cleared
    /// first; reuse the same `Vec` across calls to avoid reallocating
    /// every frame.
    pub fn copy_region(&self, x: u16, y: u16, width: u16, height: u16, out: &mut Vec<u8>) {
        out.clear();
        let stride = self.width as usize * 4;
        let row_bytes = width as usize * 4;
        out.reserve(row_bytes * height as usize);
        for row in 0..height as usize {
            let start = (y as usize + row) * stride + x as usize * 4;
            match self.pixels.get(start..start + row_bytes) {
                Some(bytes) => out.extend_from_slice(bytes),
                // Truncated/out-of-bounds row: pad rather than bail, so
                // `out` still matches the `width * height` the caller is
                // about to upload.
                None => out.resize(out.len() + row_bytes, 0),
            }
        }
    }

    /// Reallocates to `width x height` BGRA pixels, all zero -- called
    /// once per `VncFrame::Resolution`. The caller's next full-buffer
    /// upload (once its GPU texture is recreated at the new size) covers
    /// the whole buffer unconditionally anyway, so this drops any
    /// outstanding dirty region rather than trying to rebase it onto the
    /// new dimensions.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.pixels = vec![0u8; width as usize * height as usize * 4];
        self.pending_dirty = None;
        self.committed_dirty = None;
    }

    /// Applies one dirty rectangle (see `blit_rect`).
    pub fn apply_rect(&mut self, x: u16, y: u16, width: u16, height: u16, bgra: &[u8]) {
        blit_rect(&mut self.pixels, self.width, self.height, x, y, width, height, bgra);
        self.mark_written(x, y, width, height);
    }

    /// `CopyRect`: copies already-decoded pixels from `src` to `dst`
    /// within this same buffer. Extracts `src` into a scratch buffer
    /// first -- `self.pixels` can't be borrowed as both source and
    /// destination at once, and the two regions can legitimately
    /// overlap (e.g. scrolling content down a few rows in place). A
    /// no-op if no resolution is known yet.
    pub fn apply_copy(&mut self, dst: (u16, u16, u16, u16), src: (u16, u16, u16, u16)) {
        if self.width == 0 {
            return;
        }
        let (dst_x, dst_y, w, h) = dst;
        let (src_x, src_y, _, _) = src;
        let stride = self.width as usize * 4;
        let mut scratch = vec![0u8; w as usize * h as usize * 4];
        for row in 0..h as usize {
            let src_off = (src_y as usize + row) * stride + src_x as usize * 4;
            let dst_off = row * w as usize * 4;
            if let Some(row_bytes) = self.pixels.get(src_off..src_off + w as usize * 4) {
                scratch[dst_off..dst_off + w as usize * 4].copy_from_slice(row_bytes);
            }
        }
        blit_rect(&mut self.pixels, self.width, self.height, dst_x, dst_y, w, h, &scratch);
        self.mark_written(dst_x, dst_y, w, h);
    }

    fn mark_written(&mut self, x: u16, y: u16, width: u16, height: u16) {
        self.generation += 1;
        // Saturating: a rect running past the buffer's edge is clamped by
        // `blit_rect` on the way in, and `take_dirty` clamps again on the
        // way out, so overflowing here would only ever widen the box.
        let written = (x, y, x.saturating_add(width), y.saturating_add(height));
        self.pending_dirty = Some(match self.pending_dirty {
            Some(existing) => union_rect(existing, written),
            None => written,
        });
    }

    pub fn dimensions(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// A clone of the current pixel data, for the screenshot feature
    /// (`SPC v s`) -- callers that don't need to hold the lock while
    /// encoding a PNG.
    pub fn snapshot(&self) -> Vec<u8> {
        self.pixels.clone()
    }

}

impl Default for VncFramebuffer {
    fn default() -> Self {
        Self::new()
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

    // -- VncFramebuffer ---------------------------------------------------

    fn pixel_at(pixels: &[u8], width: u16, x: u16, y: u16) -> [u8; 4] {
        let i = (y as usize * width as usize + x as usize) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn new_starts_empty_with_no_dimensions_and_generation_zero() {
        let fb = VncFramebuffer::new();
        assert_eq!(fb.dimensions(), (0, 0));
        assert!(fb.pixels().is_empty());
        assert_eq!(fb.generation(), 0);
    }

    #[test]
    fn resize_reallocates_zeroed() {
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        fb.apply_rect(0, 0, 2, 2, &solid_rect(2, 2, [1, 2, 3, 4]));

        fb.resize(2, 2);
        assert_eq!(fb.dimensions(), (2, 2));
        assert_eq!(fb.pixels(), &[0u8; 2 * 2 * 4][..]);
    }

    #[test]
    fn apply_rect_writes_pixels_and_bumps_generation() {
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        assert_eq!(fb.generation(), 0);

        fb.apply_rect(1, 1, 2, 2, &solid_rect(2, 2, [10, 20, 30, 255]));
        assert_eq!(pixel_at(fb.pixels(), 4, 1, 1), [10, 20, 30, 255]);
        assert_eq!(pixel_at(fb.pixels(), 4, 2, 2), [10, 20, 30, 255]);
        assert_eq!(pixel_at(fb.pixels(), 4, 0, 0), [0, 0, 0, 0]);
        assert_eq!(fb.generation(), 1);

        fb.apply_rect(0, 0, 1, 1, &solid_rect(1, 1, [1, 1, 1, 1]));
        assert_eq!(fb.generation(), 2);
    }

    #[test]
    fn apply_rect_before_any_resize_is_a_safe_no_op_that_still_bumps_generation() {
        let mut fb = VncFramebuffer::new();
        fb.apply_rect(0, 0, 2, 2, &solid_rect(2, 2, [1, 2, 3, 4]));
        // `blit_rect` bails out immediately against a zero-sized dest,
        // but this still counts as a write -- there's simply nothing in
        // `pixels()` for a later upload to show for it.
        assert!(fb.pixels().is_empty());
        assert_eq!(fb.generation(), 1);
    }

    #[test]
    fn apply_copy_moves_pixels_from_src_to_dst() {
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        fb.apply_rect(0, 0, 2, 2, &solid_rect(2, 2, [5, 6, 7, 8]));

        fb.apply_copy((2, 2, 2, 2), (0, 0, 2, 2));
        assert_eq!(pixel_at(fb.pixels(), 4, 2, 2), [5, 6, 7, 8]);
        assert_eq!(pixel_at(fb.pixels(), 4, 3, 3), [5, 6, 7, 8]);
        // source region is untouched by the copy
        assert_eq!(pixel_at(fb.pixels(), 4, 0, 0), [5, 6, 7, 8]);
    }

    #[test]
    fn apply_copy_handles_overlapping_src_and_dst_regions_correctly() {
        // A 3x3 block at (0,0) with distinct per-row colors, copied one
        // row down onto itself (src (0,0), dst (0,1)) -- overlapping,
        // so a naive in-place copy through the same buffer (without
        // extracting the source first) would corrupt row 1 before it's
        // ever read as a source for row 2.
        let mut fb = VncFramebuffer::new();
        fb.resize(3, 3);
        let mut src = Vec::new();
        for row in 0..3u8 {
            src.extend_from_slice(&[row, row, row, 255].repeat(3));
        }
        fb.apply_rect(0, 0, 3, 3, &src);

        fb.apply_copy((0, 1, 3, 2), (0, 0, 3, 2));

        // row 0 unchanged
        assert_eq!(pixel_at(fb.pixels(), 3, 0, 0), [0, 0, 0, 255]);
        // row 1 now holds what was originally row 0
        assert_eq!(pixel_at(fb.pixels(), 3, 0, 1), [0, 0, 0, 255]);
        // row 2 now holds what was originally row 1 (not the
        // already-overwritten row 1) -- this is what a naive
        // borrow-through-self copy would get wrong.
        assert_eq!(pixel_at(fb.pixels(), 3, 0, 2), [1, 1, 1, 255]);
    }

    #[test]
    fn apply_copy_before_any_resolution_is_known_is_a_no_op() {
        let mut fb = VncFramebuffer::new();
        fb.apply_copy((0, 0, 2, 2), (2, 2, 2, 2));
        assert!(fb.pixels().is_empty());
        assert_eq!(fb.generation(), 0);
    }

    // -- commit / dirty tracking ------------------------------------------

    #[test]
    fn nothing_is_takeable_until_the_update_is_committed() {
        // The core of the torn-frame fix: writes are invisible to a
        // renderer until `commit` marks them as one finished update.
        let mut fb = VncFramebuffer::new();
        fb.resize(8, 8);
        fb.apply_rect(1, 1, 2, 2, &solid_rect(2, 2, [1, 2, 3, 4]));
        assert_eq!(fb.take_dirty(), None);

        fb.commit();
        assert_eq!(fb.take_dirty(), Some((1, 1, 2, 2)));
        // Taken once, then nothing until the next committed update.
        assert_eq!(fb.take_dirty(), None);
    }

    #[test]
    fn commit_unions_every_rect_in_the_update_into_one_box() {
        let mut fb = VncFramebuffer::new();
        fb.resize(16, 16);
        fb.apply_rect(1, 2, 2, 2, &solid_rect(2, 2, [1, 1, 1, 1]));
        fb.apply_rect(10, 12, 3, 3, &solid_rect(3, 3, [2, 2, 2, 2]));
        fb.commit();
        // (1,2)..(3,4) unioned with (10,12)..(13,15) -> (1,2)..(13,15)
        assert_eq!(fb.take_dirty(), Some((1, 2, 12, 13)));
    }

    #[test]
    fn a_skipped_frame_does_not_lose_its_dirty_region() {
        // A renderer that doesn't get to draw between two updates must
        // still end up repainting both regions, not just the later one.
        let mut fb = VncFramebuffer::new();
        fb.resize(16, 16);
        fb.apply_rect(0, 0, 2, 2, &solid_rect(2, 2, [1, 1, 1, 1]));
        fb.commit();
        fb.apply_rect(10, 10, 2, 2, &solid_rect(2, 2, [2, 2, 2, 2]));
        fb.commit();
        assert_eq!(fb.take_dirty(), Some((0, 0, 12, 12)));
    }

    #[test]
    fn writes_after_a_commit_stay_pending_for_the_next_one() {
        let mut fb = VncFramebuffer::new();
        fb.resize(16, 16);
        fb.apply_rect(0, 0, 2, 2, &solid_rect(2, 2, [1, 1, 1, 1]));
        fb.commit();
        // Mid-next-update write: committed region is only the first one.
        fb.apply_rect(8, 8, 2, 2, &solid_rect(2, 2, [2, 2, 2, 2]));
        assert_eq!(fb.take_dirty(), Some((0, 0, 2, 2)));
        assert_eq!(fb.take_dirty(), None);

        fb.commit();
        assert_eq!(fb.take_dirty(), Some((8, 8, 2, 2)));
    }

    #[test]
    fn commit_with_nothing_pending_is_a_no_op() {
        // Servers can legitimately answer an incremental request with an
        // update containing no rectangles at all.
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        fb.commit();
        assert_eq!(fb.take_dirty(), None);
    }

    #[test]
    fn a_dirty_box_is_clamped_to_the_buffer() {
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        // Overruns the right/bottom edge; `blit_rect` clamps the pixels,
        // and the reported box has to be clamped to match or the caller
        // would upload past the texture.
        fb.apply_rect(2, 2, 8, 8, &solid_rect(8, 8, [9, 9, 9, 9]));
        fb.commit();
        assert_eq!(fb.take_dirty(), Some((2, 2, 2, 2)));
    }

    #[test]
    fn resize_drops_any_outstanding_dirty_region() {
        let mut fb = VncFramebuffer::new();
        fb.resize(8, 8);
        fb.apply_rect(0, 0, 4, 4, &solid_rect(4, 4, [1, 2, 3, 4]));
        fb.commit();

        fb.resize(2, 2);
        // Stale coordinates against the old size would be uploaded
        // against a freshly created (and already fully uploaded) texture.
        assert_eq!(fb.take_dirty(), None);
    }

    #[test]
    fn copy_region_extracts_tightly_packed_rows() {
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        // Distinct value per row so a stride mistake is visible.
        for row in 0..4u8 {
            fb.apply_rect(0, row as u16, 4, 1, &[row, row, row, 255].repeat(4));
        }

        let mut out = Vec::new();
        fb.copy_region(1, 1, 2, 2, &mut out);
        assert_eq!(out.len(), 2 * 2 * 4);
        assert_eq!(&out[0..4], &[1, 1, 1, 255]); // row 1
        assert_eq!(&out[4..8], &[1, 1, 1, 255]);
        assert_eq!(&out[8..12], &[2, 2, 2, 255]); // row 2
        assert_eq!(&out[12..16], &[2, 2, 2, 255]);
    }

    #[test]
    fn copy_region_reuses_the_output_buffer_without_appending() {
        let mut fb = VncFramebuffer::new();
        fb.resize(4, 4);
        let mut out = vec![0xAA; 999];
        fb.copy_region(0, 0, 2, 2, &mut out);
        assert_eq!(out.len(), 2 * 2 * 4);
    }
}
