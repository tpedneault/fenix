//! The host half of the reading view (`reading_page`), and the pictures
//! pages draw: a picture a page asks for (a file in a note, a diagram)
//! is decoded off the UI thread into BGRA, uploaded once as a texture
//! and drawn over the blank cells the page left for it.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::page::ImageKey;
use crate::pdf_texture::PdfTexture;
use crate::reading::{self, Action as RAction};
use crate::reading_page::{Action as ReadAction, ReadingPage};

/// A picture's pixels and, once uploaded, its texture.
pub(super) struct PageTex {
    pub(super) w: u32,
    pub(super) h: u32,
    /// Waiting to be uploaded.
    pub(super) bgra: Option<Vec<u8>>,
    pub(super) texture: Option<PdfTexture>,
}

/// The widest a picture in a note is kept; bigger ones are scaled down.
const MAX_PICTURE_WIDTH: u32 = 2400;

/// Reads a picture file into BGRA pixels.
pub(super) fn decode_picture(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let img = if img.width() > MAX_PICTURE_WIDTH { img.resize(MAX_PICTURE_WIDTH, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((w, h, rgba_to_bgra(rgba.into_raw())))
}

pub(super) fn rgba_to_bgra(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    px
}

/// Where a page's picture goes: `cell_box` is the cells the page left
/// for it and `clip` what of the pane is showing (x0, y0, x1, y1), both
/// in pixels. Fitted and no bigger than it is, or -- with a `view` --
/// fitted, times its zoom, around its centre. Returns the rect drawn
/// into, the part of the picture it shows (u0, v0, u1, v1), and where
/// the whole picture sits (partly off-screen when zoomed).
pub(super) type Placed = ((f32, f32, f32, f32), (f32, f32, f32, f32), (f32, f32, f32, f32));

pub(super) fn place_picture(cell_box: (f32, f32, f32, f32), clip: (f32, f32, f32, f32), tex: (u32, u32), view: Option<(f32, f32, f32)>) -> Option<Placed> {
    let (bx, by, bw, bh) = cell_box;
    let (tw, th) = (tex.0 as f32, tex.1 as f32);
    if tw <= 0.0 || th <= 0.0 || bw <= 0.0 || bh <= 0.0 {
        return None;
    }
    let fit = (bw / tw).min(bh / th);
    let (x, y, w, h) = match view {
        None => {
            let s = fit.min(1.0);
            let (w, h) = (tw * s, th * s);
            (bx, by + (bh - h) / 2.0, w, h)
        }
        Some((zoom, cx, cy)) => {
            let s = fit * zoom.max(0.1);
            let (w, h) = (tw * s, th * s);
            let place = |b: f32, bl: f32, len: f32, c: f32| if len <= bl { b + (bl - len) / 2.0 } else { (b + bl / 2.0 - c * len).clamp(b + bl - len, b) };
            (place(bx, bw, w, cx), place(by, bh, h, cy), w, h)
        }
    };
    let x0 = x.max(bx).max(clip.0);
    let y0 = y.max(by).max(clip.1);
    let x1 = (x + w).min(bx + bw).min(clip.2);
    let y1 = (y + h).min(by + bh).min(clip.3);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let uv = ((x0 - x) / w, (y0 - y) / h, (x1 - x) / w, (y1 - y) / h);
    Some(((x0, y0, x1 - x0, y1 - y0), uv, (x, y, w, h)))
}

/// What laying a reading page out found, before the page is borrowed.
pub(super) struct ReadingPre {
    rendered: Option<reading::Rendered>,
    seen: (u64, usize),
    follow: Option<usize>,
}

impl ReadingPre {
    /// Whether anything changed.
    pub(super) fn changed(&self) -> bool {
        self.rendered.is_some() || self.follow.is_some()
    }
}

impl App {
    /// `SPC m r` (`replace`) / `SPC m p`: the focused Markdown buffer
    /// rendered, in its place or beside it.
    pub(crate) fn open_reading_view(&mut self, replace: bool) {
        let source = self.focused_buffer_id();
        if let Some(PageModel::Reading(p)) = self.pages.get(&source).map(|s| &s.model) {
            // Already reading: back to the source.
            let (src, line) = (p.source, p.rendered.source_line_of(p.line).unwrap_or(0));
            self.reading_edit(source, src, line);
            return;
        }
        if !self.in_markdown() {
            self.set_error("the reading view is for Markdown");
            return;
        }
        let path = self.open().buffer.path().map(Path::to_path_buf);
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        // Beside: one reading pane per source; SPC m p again closes it.
        if !replace {
            let existing = self.find_page(|m| matches!(m, PageModel::Reading(p) if p.source == source && !p.replaces));
            if let Some(id) = existing {
                let pane = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&id));
                self.close_page(id);
                if let Some(p) = pane {
                    let back = self.focused_pane_id();
                    self.windows_mut().focus(p);
                    self.close_window();
                    if self.windows().windows().contains(&back) {
                        self.windows_mut().focus(back);
                    }
                }
                return;
            }
        }
        let mut page = ReadingPage::new(source, path, replace);
        page.followed = Some(line);
        if replace {
            self.open_page(PageModel::Reading(Box::new(page)));
        } else {
            let origin = self.focused_pane_id();
            self.open_page_beside(PageModel::Reading(Box::new(page)), 0.5);
            // Typing stays in the source; the view follows it.
            self.windows_mut().focus(origin);
        }
    }

    /// Works out a reading page's layout before the page is borrowed:
    /// laid out again when its source changed, and the line its source
    /// cursor moved to. `None` when it isn't a reading page.
    pub(super) fn reading_precompute(&self, id: BufferId, cols: usize, force: bool) -> Option<ReadingPre> {
        let Some(PageModel::Reading(p)) = self.pages.get(&id).map(|s| &s.model) else { return None };
        let ob = self.buffers.get(p.source)?;
        let width = cols.saturating_sub(2).max(20);
        let seen = (ob.buffer.edit_count(), width);
        let rendered = (force || p.seen != Some(seen)).then(|| self.with_reading_ctx(p.path.as_deref(), |ctx| reading::layout(&ob.buffer.text(), width, ctx)));
        let follow = if p.replaces || self.focused_buffer_id() == id {
            None
        } else {
            self.windows()
                .windows()
                .into_iter()
                .find(|&pane| self.windows().content(pane) == Some(&p.source))
                .map(|pane| ob.buffer.line_col(&self.pane_state(pane).cursor).0)
                .filter(|&l| Some(l) != p.followed)
        };
        Some(ReadingPre { rendered, seen, follow })
    }

    /// Applies what `reading_precompute` found and lays the page out.
    pub(super) fn reading_apply(p: &mut ReadingPage, pre: ReadingPre, cols: usize) -> crate::page::Page {
        let first = p.seen.is_none();
        if let Some(r) = pre.rendered {
            p.rendered = r;
            p.seen = Some(pre.seen);
            if p.target.is_some_and(|t| t >= p.rendered.targets.len()) {
                p.target = None;
            }
        }
        match (pre.follow, first, p.followed) {
            (Some(line), _, _) => p.follow(line),
            // The first layout starts where the source cursor was.
            (None, true, Some(line)) => p.follow(line),
            _ => {}
        }
        crate::reading_page::layout(p, cols)
    }

    pub(super) fn reading_action(&mut self, page_id: BufferId, action: ReadAction) {
        let Some(PageModel::Reading(p)) = self.pages.get(&page_id).map(|s| &s.model) else { return };
        let (source, path, replaces) = (p.source, p.path.clone(), p.replaces);
        match action {
            ReadAction::None => {}
            ReadAction::Close => {
                if replaces {
                    self.close_page(page_id);
                    self.open_buffer_in_focused_pane(source);
                } else {
                    let pane = self.focused_pane_id();
                    self.close_page(page_id);
                    if self.windows().content(pane) != Some(&page_id) {
                        self.close_window();
                    }
                }
            }
            ReadAction::Edit(line) => self.reading_edit(page_id, source, line),
            ReadAction::Target(RAction::Checkbox { line, done }) => {
                let Some(ob) = self.buffers.get_mut(source) else { return };
                let text = ob.buffer.line(line).to_string();
                if let Some(task) = fenix_notebook::meta::task_on(text.trim_end_matches(['\n', '\r']), line) {
                    let at = ob.buffer.line_start_char(line) + task.mark_col;
                    let mut scratch = Cursor::at_start();
                    ob.buffer.replace_range(&mut scratch, at, at + 1, if done { " " } else { "x" });
                    self.set_message(if done { "unticked" } else { "ticked" });
                }
            }
            ReadAction::Target(RAction::Link(fenix_notebook::blocks::LinkTo::Url(url))) => {
                if let Err(e) = fenix_fs::open_url(&url) {
                    self.set_error(format!("couldn't open {url}: {e}"));
                }
            }
            ReadAction::Target(RAction::Link(fenix_notebook::blocks::LinkTo::Wiki(link))) => {
                self.reading_to_source_pane(page_id, source);
                self.notebook_follow(link, path);
            }
            ReadAction::Target(RAction::Image(file)) => {
                if let Err(e) = fenix_fs::open_with_default(&file) {
                    self.set_error(format!("couldn't open {}: {e}", file.display()));
                }
            }
            ReadAction::Target(RAction::Diagram { line, embed }) => self.reading_open_diagram(page_id, source, line, embed),
        }
    }

    /// Where a reading view hands off to: the pane its source is in (or,
    /// when it replaced the source, its own pane showing the source).
    fn reading_to_source_pane(&mut self, page_id: BufferId, source: BufferId) {
        let replaces = matches!(self.pages.get(&page_id).map(|s| &s.model), Some(PageModel::Reading(p)) if p.replaces);
        if replaces {
            return;
        }
        if let Some(p) = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&source)) {
            self.windows_mut().focus(p);
        }
    }

    /// Back to editing the source at `line`.
    fn reading_edit(&mut self, page_id: BufferId, source: BufferId, line: usize) {
        let replaces = matches!(self.pages.get(&page_id).map(|s| &s.model), Some(PageModel::Reading(p)) if p.replaces);
        if replaces {
            self.close_page(page_id);
            self.open_buffer_in_focused_pane(source);
        } else {
            self.reading_to_source_pane(page_id, source);
        }
        if self.focused_buffer_id() == source {
            self.notebook_place_cursor(line, 0);
        }
    }

    /// A diagram in the reading view: its source, for now.
    pub(super) fn reading_open_diagram(&mut self, page_id: BufferId, source: BufferId, line: usize, embed: Option<fenix_notebook::Link>) {
        match embed {
            Some(link) => {
                let path = self.buffers.get(source).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
                self.reading_to_source_pane(page_id, source);
                self.notebook_follow(link, path);
            }
            None => self.reading_edit(page_id, source, line),
        }
    }

    // ------------------------------------------------------------------
    // Pictures on pages
    // ------------------------------------------------------------------

    /// Starts reading the pictures page `id` shows that aren't read yet.
    pub(super) fn request_page_images(&mut self, id: BufferId) {
        let Some(state) = self.pages.get(&id) else { return };
        let wanted: Vec<ImageKey> = state.page.images.iter().map(|i| i.key.clone()).filter(|k| !self.notebook.textures.contains_key(k) && !self.notebook.pending.contains(k)).collect();
        for key in wanted {
            match &key {
                ImageKey::File(path) => {
                    self.notebook.pending.insert(key.clone());
                    let path = path.clone();
                    let key2 = key.clone();
                    self.page_spawn(move |send| send(PageEvent::PageImage { key: key2, result: decode_picture(&path) }));
                }
                ImageKey::Diagram(_) => self.request_diagram_image(key.clone()),
            }
        }
    }

    /// A picture arrived: kept for upload, and pages showing it laid out
    /// again at its size.
    pub(super) fn apply_page_image(&mut self, key: ImageKey, result: Result<(u32, u32, Vec<u8>), String>) {
        self.notebook.pending.remove(&key);
        match result {
            Ok((w, h, bgra)) => {
                self.notebook.image_sizes.insert(key.clone(), (w, h));
                self.notebook.textures.insert(key, PageTex { w, h, bgra: Some(bgra), texture: None });
            }
            Err(e) => {
                // Remembered as failed (1x1, nothing drawn) so it isn't
                // asked for every frame.
                eprintln!("fenix: couldn't read a picture: {e}");
                self.notebook.textures.insert(key, PageTex { w: 0, h: 0, bgra: None, texture: None });
            }
        }
        for state in self.pages.values_mut() {
            if matches!(state.model, PageModel::Reading(_) | PageModel::Notebook(_)) {
                state.stale = true;
                if let PageModel::Reading(p) = &mut state.model {
                    p.seen = None;
                }
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_opens_beside_follows_and_ticks_in_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.md");
        std::fs::write(&path, "# Title\n\n- [ ] one\n- [ ] two\n").unwrap();
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        let source = app.focused_buffer_id();
        app.notebook_place_cursor(3, 0);
        app.open_reading_view(false);
        assert_eq!(app.focused_buffer_id(), source, "typing stays in the source");
        let page = app.find_page(|m| matches!(m, PageModel::Reading(_))).expect("the reading view");
        let pane = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&page)).unwrap();
        app.ensure_page_layout(page, pane, 60);
        let text = app.buffers.get(page).unwrap().buffer.text();
        assert!(text.contains("TITLE") && text.contains("[ ] two"), "{text}");
        let Some(PageModel::Reading(p)) = app.pages.get(&page).map(|s| &s.model) else { unreachable!() };
        assert_eq!(p.line, 3, "on the source cursor's block");
        app.reading_action(page, ReadAction::Target(RAction::Checkbox { line: 3, done: false }));
        assert_eq!(app.buffers.get(source).unwrap().buffer.text(), "# Title\n\n- [ ] one\n- [x] two\n");
        // Typing in the source lays the view out again.
        app.ensure_page_layout(page, pane, 60);
        assert!(app.buffers.get(page).unwrap().buffer.text().contains("[x] two"));
        // SPC m p again closes it.
        app.open_reading_view(false);
        assert!(app.find_page(|m| matches!(m, PageModel::Reading(_))).is_none());
    }

    #[test]
    fn pictures_decode_to_bgra() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        image::RgbaImage::from_raw(1, 1, vec![10, 20, 30, 255]).unwrap().save(&file).unwrap();
        assert_eq!(decode_picture(&file).unwrap(), (1, 1, vec![30, 20, 10, 255]));
    }

    #[test]
    fn pictures_fit_or_zoom_and_are_cut_to_the_pane() {
        // 200x100 into a 100x100 box: half size, centred vertically.
        let (dest, uv, whole) = place_picture((0.0, 0.0, 100.0, 100.0), (0.0, 0.0, 1000.0, 1000.0), (200, 100), None).unwrap();
        assert_eq!(whole, (0.0, 25.0, 100.0, 50.0));
        assert_eq!(dest, (0.0, 25.0, 100.0, 50.0));
        assert_eq!(uv, (0.0, 0.0, 1.0, 1.0));
        // Small pictures aren't blown up...
        let (_, _, whole) = place_picture((0.0, 0.0, 100.0, 100.0), (0.0, 0.0, 1000.0, 1000.0), (50, 50), None).unwrap();
        assert_eq!((whole.2, whole.3), (50.0, 50.0));
        // ...unless zoomed; at 2x on the left edge, half is shown.
        let (dest, uv, _) = place_picture((0.0, 0.0, 100.0, 100.0), (0.0, 0.0, 1000.0, 1000.0), (100, 100), Some((2.0, 0.0, 0.5))).unwrap();
        assert_eq!(dest, (0.0, 0.0, 100.0, 100.0));
        assert_eq!(uv, (0.0, 0.25, 0.5, 0.75));
        // Cut by the pane.
        let (dest, uv, _) = place_picture((0.0, 0.0, 100.0, 100.0), (0.0, 50.0, 1000.0, 1000.0), (100, 100), None).unwrap();
        assert_eq!((dest.1, dest.3, uv.1), (50.0, 50.0, 0.5));
    }
}
