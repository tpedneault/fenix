//! The host half of the PDF reader (`reader` is the pure half): the open
//! documents, each pane's view of one, the pdfium worker's replies, the
//! reader's keys, and the outline and search panes.
//!
//! A document belongs to its buffer, the way a file's text does: opening
//! a PDF gives it a tab in the focused pane like any file, the tab can be
//! moved, closed and reopened, and the same document can be shown in two
//! panes. Where you are in it -- page, zoom, scroll -- belongs to the
//! *view*: one per pane and tab (`ViewKey`), made the first time a pane
//! shows the document, starting where the last view left off.

use super::*;
use crate::reader::{self, Cmd, Outcome, Zoom};

/// How far `j`/`k`/`h`/`l` move, in rendered pixels.
pub(super) const PAN_STEP_PX: u32 = 60;

/// A view: the pane showing it, and the document's buffer.
pub(super) type ViewKey = (fenix_window::WindowId, BufferId);

/// One open document -- `pdf_docs[buffer]`.
pub(super) struct PdfDoc {
    /// Canonical, so opening it again finds this one.
    pub(super) path: PathBuf,
    /// The file name, for the tab and the buffer list.
    pub(super) name: String,
    /// The worker's name for the document.
    pub(super) doc_key: fenix_pdf::PdfDocKey,
    /// 0 until the worker has opened it.
    pub(super) page_count: u32,
    /// Page 0's size in points -- what a view without a render of its own
    /// yet fits to.
    pub(super) page_point_size: (f32, f32),
    /// The bookmark tree, fetched the first time the outline is asked for.
    pub(super) outline: Option<Vec<fenix_pdf::outline::OutlineEntry>>,
    /// The latest search sent; a reply to any earlier one is stale.
    pub(super) pending_search_request_id: u64,
    pub(super) last_search_query: String,
    /// The pages with a match of the last search, in order -- what `n`
    /// and `N` go through.
    pub(super) match_pages: Vec<u32>,
    /// Where the last view used was, so a new view -- the document shown
    /// in another pane, or reopened after its tab was closed -- continues
    /// there instead of at page 1.
    pub(super) last_place: (u32, Zoom),
}

/// One pane's view of a document -- `pdf_views[(pane, buffer)]`.
pub(super) struct PdfView {
    /// The worker's name for this view, so two views of one document
    /// don't cancel each other's renders.
    pub(super) id: u64,
    pub(super) current_page: u32,
    /// The latest render asked for; a reply to any earlier one is stale.
    pub(super) pending_request_id: u64,
    /// The pixel size of the latest render asked for -- compared every
    /// frame with what the zoom now calls for, to notice a resize.
    /// `(0, 0)` until the first.
    pub(super) last_requested_size: (u32, u32),
    /// This view's page's size in points, once a render of it has come
    /// back; `(0, 0)` until then (the document's page 0 stands in).
    pub(super) page_point_size: (f32, f32),
    pub(super) zoom: Zoom,
    /// The pane's size in pixels, from the last frame drawn.
    pub(super) last_pane_size: (u32, u32),
    /// Top-left of what's shown of `full_bgra`; clamped when drawn.
    pub(super) scroll_offset: (u32, u32),
    /// The next render starts at the bottom of its page: set by
    /// scrolling up past a page's top, so it continues onto the bottom
    /// of the page before.
    pub(super) land_at_bottom: bool,
    /// The whole latest render, kept so panning re-crops it.
    pub(super) full_bgra: Option<(u32, u32, Vec<u8>)>,
    /// `(w, h, x, y)` of the crop in `texture`, so an unchanged frame
    /// skips the upload.
    pub(super) last_uploaded: Option<(u32, u32, u32, u32)>,
    /// Made by the frame that draws the view, when it first does.
    pub(super) texture: Option<PdfTexture>,
}

impl App {
    pub(super) fn looks_like_pdf(path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    }

    fn pdf_next_id(&mut self) -> u64 {
        self.pdf_next_id += 1;
        self.pdf_next_id
    }

    /// Opens `path` as a PDF in a tab of the focused pane.
    pub(crate) fn open_pdf_path(&mut self, path: &Path) {
        self.open_pdf_path_as(path, false);
    }

    /// `open_pdf_path`, in the pane's preview tab when `preview` -- what a
    /// jump uses. A document already open gets another tab (or its own
    /// tab here back) and isn't loaded again.
    pub(super) fn open_pdf_path_as(&mut self, path: &Path, preview: bool) {
        let canonical = fenix_project::plain_path(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        let buffer = match self.pdf_docs.iter().find(|(_, doc)| doc.path == canonical).map(|(id, _)| *id) {
            Some(buffer) => buffer,
            None => self.pdf_load(&canonical),
        };
        self.open_buffer_in_focused_pane_as(buffer, preview);
        let key = (self.focused_pane_id(), buffer);
        self.pdf_ensure_view(key);
        self.pdf_last_view = Some(key);
        self.refresh_project_root();
        self.record_recent_file(&canonical);
        self.main_view = MainView::Editor;
        self.wake_caret();
    }

    /// A buffer for `path` and the worker's `Open` for it. The page count
    /// and first render come later, through `apply_pdf_response`.
    fn pdf_load(&mut self, path: &Path) -> BufferId {
        if self.pdf_worker.is_none() {
            let worker = match self.event_proxy.clone() {
                Some(proxy) => fenix_pdf::PdfWorker::spawn(move |response| {
                    let _ = proxy.send_event(FenixUserEvent::PdfResponse(response));
                }),
                // No event loop to report to (tests): the worker still
                // runs, so pdfium stays on its one thread; its replies
                // just go nowhere.
                None => fenix_pdf::PdfWorker::spawn(|_| {}),
            };
            self.pdf_worker = Some(worker);
        }
        let buffer = self.buffers.open_pdf();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
        let doc_key = fenix_pdf::PdfDocKey::new();
        if let Some(worker) = &self.pdf_worker {
            worker.send(fenix_pdf::PdfRequest::Open { key: doc_key, path: path.to_path_buf() });
        }
        self.pdf_docs.insert(
            buffer,
            PdfDoc {
                path: path.to_path_buf(),
                name,
                doc_key,
                page_count: 0,
                page_point_size: (0.0, 0.0),
                outline: None,
                pending_search_request_id: 0,
                last_search_query: String::new(),
                match_pages: Vec::new(),
                last_place: (0, reader::DEFAULT_ZOOM),
            },
        );
        buffer
    }

    /// The view for `key`, made if the pane hasn't shown the document
    /// before. `None` when `key`'s buffer isn't a document.
    pub(super) fn pdf_ensure_view(&mut self, key: ViewKey) -> Option<&mut PdfView> {
        let (page, zoom) = self.pdf_docs.get(&key.1)?.last_place;
        if !self.pdf_views.contains_key(&key) {
            let id = self.pdf_next_id();
            self.pdf_views.insert(
                key,
                PdfView {
                    id,
                    current_page: page,
                    pending_request_id: 0,
                    last_requested_size: (0, 0),
                    page_point_size: (0.0, 0.0),
                    zoom,
                    last_pane_size: (0, 0),
                    scroll_offset: (0, 0),
                    land_at_bottom: false,
                    full_bgra: None,
                    last_uploaded: None,
                    texture: None,
                },
            );
        }
        self.pdf_views.get_mut(&key)
    }

    /// The document shown in `pane`, as a view key.
    pub(super) fn pdf_view_in_pane(&self, pane: fenix_window::WindowId) -> Option<ViewKey> {
        let buffer = *self.windows().content(pane)?;
        self.pdf_docs.contains_key(&buffer).then_some((pane, buffer))
    }

    /// The view the reader's commands act on: the focused pane's when it
    /// shows a document, otherwise the one read last while its pane still
    /// shows it here -- so `SPC r n` from the outline next to a document,
    /// or from the code beside it, turns its page.
    pub(super) fn pdf_target(&mut self) -> Option<ViewKey> {
        let key = self.pdf_view_in_pane(self.focused_pane_id()).or_else(|| {
            let (pane, buffer) = self.pdf_last_view?;
            (self.windows().content(pane) == Some(&buffer)).then_some((pane, buffer))
        })?;
        self.pdf_ensure_view(key)?;
        self.pdf_last_view = Some(key);
        Some(key)
    }

    /// A pane in this workspace showing `doc`, as a view: the one read
    /// last if it still does, else the first.
    fn pdf_view_of_doc(&self, doc: BufferId) -> Option<ViewKey> {
        if let Some((pane, buffer)) = self.pdf_last_view {
            if buffer == doc && self.windows().content(pane) == Some(&doc) {
                return Some((pane, buffer));
            }
        }
        self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&doc)).map(|p| (p, doc))
    }

    /// A view's page's size in points: its own once rendered, else the
    /// document's first page's.
    fn pdf_page_pts(&self, key: ViewKey) -> (f32, f32) {
        match (self.pdf_views.get(&key), self.pdf_docs.get(&key.1)) {
            (Some(view), _) if view.page_point_size.0 > 0.0 => view.page_point_size,
            (_, Some(doc)) => doc.page_point_size,
            _ => (0.0, 0.0),
        }
    }

    /// The size to render `key`'s page at, for its zoom and pane now.
    pub(super) fn pdf_target_size(&self, key: ViewKey) -> (u32, u32) {
        let Some(view) = self.pdf_views.get(&key) else { return (0, 0) };
        reader::target_size(view.zoom, self.pdf_page_pts(key), view.last_pane_size, self.frame_scale())
    }

    /// Asks the worker for `key`'s page at `w` x `h`. Nothing before the
    /// document's page count is known, or for a degenerate size.
    fn pdf_dispatch_render(&mut self, key: ViewKey, w: u32, h: u32) {
        if w == 0 || h == 0 || self.pdf_docs.get(&key.1).is_none_or(|doc| doc.page_count == 0) {
            return;
        }
        let request_id = self.pdf_next_id();
        let Some(doc_key) = self.pdf_docs.get(&key.1).map(|doc| doc.doc_key) else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        view.pending_request_id = request_id;
        view.last_requested_size = (w, h);
        let (view_id, page_index) = (view.id, view.current_page);
        if let Some(worker) = &self.pdf_worker {
            worker.send(fenix_pdf::PdfRequest::RenderPage { key: doc_key, view: view_id, request_id, page_index, target_w: w, target_h: h });
        }
    }

    /// Renders `key` again for its current page and zoom.
    pub(super) fn pdf_render(&mut self, key: ViewKey) {
        let (w, h) = self.pdf_target_size(key);
        self.pdf_dispatch_render(key, w, h);
        self.pdf_remember_place(key);
    }

    fn pdf_remember_place(&mut self, key: ViewKey) {
        let Some(place) = self.pdf_views.get(&key).map(|view| (view.current_page, view.zoom)) else { return };
        if let Some(doc) = self.pdf_docs.get_mut(&key.1) {
            doc.last_place = place;
        }
    }

    /// The document whose worker name is `doc_key`.
    fn pdf_doc_by_key(&self, doc_key: fenix_pdf::PdfDocKey) -> Option<BufferId> {
        self.pdf_docs.iter().find(|(_, doc)| doc.doc_key == doc_key).map(|(id, _)| *id)
    }

    /// A reply from the worker. Anything for a document or view since
    /// closed, or for a request since superseded, is dropped.
    pub(super) fn apply_pdf_response(&mut self, response: fenix_pdf::PdfResponse) {
        match response {
            fenix_pdf::PdfResponse::Opened { key, page_count, page_width_pts, page_height_pts } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                if let Some(doc) = self.pdf_docs.get_mut(&buffer) {
                    doc.page_count = page_count;
                    doc.page_point_size = (page_width_pts, page_height_pts);
                    doc.last_place.0 = doc.last_place.0.min(page_count.saturating_sub(1));
                }
                // Views made before the count was known may be past the
                // end; the first render is asked for by the next frame.
                for (_, view) in self.pdf_views.iter_mut().filter(|((_, b), _)| *b == buffer) {
                    view.current_page = view.current_page.min(page_count.saturating_sub(1));
                }
            }
            fenix_pdf::PdfResponse::OpenFailed { key, message } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                let path = self.pdf_docs.get(&buffer).map(|doc| doc.path.display().to_string()).unwrap_or_default();
                self.set_error(format!("couldn't open {path}: {message}"));
            }
            fenix_pdf::PdfResponse::PageRendered { key, request_id, page_index, width, height, bgra, page_width_pts, page_height_pts } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                let Some(view_key) = self.pdf_views.iter().find(|((_, b), v)| *b == buffer && v.pending_request_id == request_id).map(|(k, _)| *k) else { return };
                let Some(view) = self.pdf_views.get_mut(&view_key) else { return };
                if page_index != view.current_page {
                    return;
                }
                view.page_point_size = (page_width_pts, page_height_pts);
                // `u32::MAX`: drawing clamps it to this render's bottom.
                view.scroll_offset = if view.land_at_bottom { (0, u32::MAX) } else { (0, 0) };
                view.land_at_bottom = false;
                view.full_bgra = Some((width, height, bgra));
                view.last_uploaded = None;
                // A page of another size than the one this render was
                // sized for: render it again at its own size. Within a
                // pixel is close enough -- real documents vary their pages
                // in the second decimal, and a re-render for that is waste.
                let corrected = self.pdf_target_size(view_key);
                if corrected != (0, 0) && (corrected.0.abs_diff(width) > 1 || corrected.1.abs_diff(height) > 1) {
                    self.pdf_dispatch_render(view_key, corrected.0, corrected.1);
                }
            }
            fenix_pdf::PdfResponse::RenderFailed { key, request_id, message } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                if self.pdf_views.iter().any(|((_, b), v)| *b == buffer && v.pending_request_id == request_id) {
                    self.set_error(format!("couldn't render page: {message}"));
                }
            }
            fenix_pdf::PdfResponse::Outline { key, entries } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                if let Some(doc) = self.pdf_docs.get_mut(&buffer) {
                    doc.outline = Some(entries.clone());
                }
                // Only ever fetched to be shown.
                self.pdf_open_outline_pane(buffer, &entries);
            }
            fenix_pdf::PdfResponse::SearchResults { key, request_id, matches } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                let Some(doc) = self.pdf_docs.get_mut(&buffer) else { return };
                if request_id != doc.pending_search_request_id {
                    return;
                }
                let mut pages: Vec<u32> = matches.iter().map(|m| m.page_index).collect();
                pages.dedup();
                doc.match_pages = pages;
                let query = doc.last_search_query.clone();
                self.set_message(format!("{} match{} for \"{query}\" -- n and N go through them", matches.len(), if matches.len() == 1 { "" } else { "es" }));
                self.pdf_open_or_update_search_pane(buffer, &query, &matches);
            }
        }
        self.wake_caret();
    }

    /// Takes the documents out of a frame being closed: its views go (and
    /// their textures, made by its GPU); the documents stay open, as
    /// their buffers do.
    pub(super) fn pdf_forget_views_in(&mut self, panes: &[fenix_window::WindowId]) {
        self.pdf_views.retain(|(pane, _), _| !panes.contains(pane));
        if self.pdf_last_view.is_some_and(|(pane, _)| panes.contains(&pane)) {
            self.pdf_last_view = None;
        }
    }

    /// Closes the document of a buffer being killed: the worker lets go
    /// of it, and its views and outline and search panes go with it. The
    /// buffer itself is the caller's to close.
    pub(super) fn pdf_close_doc(&mut self, buffer: BufferId) {
        self.pdf_close_outline_pane(buffer);
        self.pdf_close_search_pane(buffer);
        let Some(doc) = self.pdf_docs.remove(&buffer) else { return };
        if let Some(worker) = &self.pdf_worker {
            worker.send(fenix_pdf::PdfRequest::Close { key: doc.doc_key });
        }
        self.pdf_views.retain(|(_, b), _| *b != buffer);
        if self.pdf_last_view.is_some_and(|(_, b)| b == buffer) {
            self.pdf_last_view = None;
        }
    }

    // -- Keys -------------------------------------------------------------

    /// A key in a PDF pane: `true` when the reader took it. What it
    /// doesn't want -- the leader, `:`, `Ctrl-w`, `Ctrl-o` -- goes on to
    /// the editor.
    pub(super) fn reader_key(&mut self, keypress: KeyPress) -> bool {
        let Some(key) = self.pdf_view_in_pane(self.focused_pane_id()) else {
            self.pdf_keys.reset();
            return false;
        };
        if self.pdf_keys_view != Some(key) {
            self.pdf_keys.reset();
            self.pdf_keys_view = Some(key);
        }
        match self.pdf_keys.key(keypress) {
            Outcome::Pending | Outcome::Dropped => {
                self.wake_caret();
                true
            }
            Outcome::Run(cmd) => {
                self.pdf_run(cmd);
                true
            }
            Outcome::Pass => false,
        }
    }

    fn pdf_run(&mut self, cmd: Cmd) {
        let pane_h = self.pdf_target().and_then(|key| self.pdf_views.get(&key)).map(|view| view.last_pane_size.1 as i32).filter(|&h| h > 0).unwrap_or(400);
        match cmd {
            Cmd::Scroll(n) => self.pdf_scroll(n.saturating_mul(PAN_STEP_PX as i32)),
            Cmd::HalfScreen(n) => self.pdf_scroll(n.saturating_mul(pane_h / 2)),
            Cmd::Screen(n) => self.pdf_scroll(n.saturating_mul((pane_h - PAN_STEP_PX as i32).max(PAN_STEP_PX as i32))),
            Cmd::Pan(n) => self.pdf_pan(n, 0),
            Cmd::Page(n) => {
                self.pdf_turn_page(n);
            }
            Cmd::GotoPage(n) => self.pdf_goto_page(n),
            Cmd::LastPage => self.pdf_last_page(),
            Cmd::Tab(mv) => self.move_tab(mv),
            Cmd::ZoomIn => self.pdf_zoom_in(),
            Cmd::ZoomOut => self.pdf_zoom_out(),
            Cmd::ToggleFit => {
                let fit_width = self.pdf_target().and_then(|key| self.pdf_views.get(&key)).is_some_and(|view| view.zoom == Zoom::FitWidth);
                self.pdf_set_zoom(if fit_width { Zoom::FitPage } else { Zoom::FitWidth });
            }
            Cmd::FitWidth => self.pdf_zoom_fit_width(),
            Cmd::FitPage => self.pdf_zoom_fit_page(),
            Cmd::ActualSize => self.pdf_set_zoom(Zoom::Percent(100)),
            Cmd::Outline => self.pdf_toggle_outline(),
            Cmd::Search => self.start_pdf_search_prompt(),
            Cmd::NextMatch => self.pdf_step_match(true),
            Cmd::PrevMatch => self.pdf_step_match(false),
        }
        self.wake_caret();
    }

    /// What a count or a `g`/`z` typed so far reads as, for the modeline.
    pub(super) fn pdf_pending_keys(&self) -> Option<String> {
        self.pdf_keys.is_pending().then(|| self.pdf_keys.pending_text())
    }

    // -- Moving -----------------------------------------------------------

    /// `J`/`K`, `SPC r n`/`SPC r p`: `delta` pages on, at the top of the
    /// page. Whether the page changed.
    pub(super) fn pdf_turn_page(&mut self, delta: i32) -> bool {
        self.pdf_turn_page_landing(delta, false)
    }

    /// `pdf_turn_page`, landing at the bottom of the new page when
    /// `land_at_bottom` -- scrolling up past a page's top.
    fn pdf_turn_page_landing(&mut self, delta: i32, land_at_bottom: bool) -> bool {
        let Some(key) = self.pdf_target() else { return false };
        let page_count = self.pdf_docs.get(&key.1).map(|doc| doc.page_count).unwrap_or(0);
        let Some(view) = self.pdf_views.get_mut(&key) else { return false };
        if page_count == 0 {
            return false;
        }
        let page = (view.current_page as i64 + delta as i64).clamp(0, page_count as i64 - 1) as u32;
        if page == view.current_page {
            return false;
        }
        view.current_page = page;
        view.land_at_bottom = land_at_bottom;
        self.pdf_render(key);
        self.wake_caret();
        true
    }

    pub(crate) fn pdf_next_page(&mut self) {
        self.pdf_turn_page(1);
    }

    pub(crate) fn pdf_prev_page(&mut self) {
        self.pdf_turn_page(-1);
    }

    pub(crate) fn pdf_first_page(&mut self) {
        self.pdf_goto_page(1);
    }

    pub(crate) fn pdf_last_page(&mut self) {
        let Some(key) = self.pdf_target() else { return };
        let Some(page_count) = self.pdf_docs.get(&key.1).map(|doc| doc.page_count) else { return };
        self.pdf_goto_page(page_count);
    }

    /// Scrolls `delta_px` down the page (up when negative); past the
    /// page's edge, onto the next or previous one. A page that fits the
    /// pane has nothing to scroll, so there every scroll turns it.
    pub(super) fn pdf_scroll(&mut self, delta_px: i32) {
        let Some(key) = self.pdf_target() else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let full_h = view.full_bgra.as_ref().map(|(_, h, _)| *h).unwrap_or(0);
        let max_y = full_h.saturating_sub(full_h.min(view.last_pane_size.1));
        let current = view.scroll_offset.1.min(max_y);
        let next = (current as i64 + delta_px as i64).clamp(0, max_y as i64) as u32;
        if next != current {
            view.scroll_offset.1 = next;
            self.wake_caret();
            return;
        }
        if delta_px > 0 {
            self.pdf_turn_page_landing(1, false);
        } else if delta_px < 0 {
            self.pdf_turn_page_landing(-1, true);
        }
    }

    /// Page `page_number`, counting from 1; past the end is the last page.
    pub(crate) fn pdf_goto_page(&mut self, page_number: u32) {
        let Some(key) = self.pdf_target() else { return };
        self.pdf_jump_to_page(key, page_number.saturating_sub(1));
    }

    /// `key` to page `page_index` (from 0), at its top.
    fn pdf_jump_to_page(&mut self, key: ViewKey, page_index: u32) {
        let page_count = self.pdf_docs.get(&key.1).map(|doc| doc.page_count).unwrap_or(0);
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        if page_count == 0 {
            return;
        }
        let page = page_index.min(page_count - 1);
        if page != view.current_page {
            view.current_page = page;
            view.land_at_bottom = false;
            self.pdf_render(key);
        } else {
            view.scroll_offset.1 = 0;
        }
        self.wake_caret();
    }

    /// `h`/`l`: pans by `PAN_STEP_PX` steps. Clamped when drawn.
    pub(super) fn pdf_pan(&mut self, dx: i32, dy: i32) {
        let Some(key) = self.pdf_target() else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let step = PAN_STEP_PX as i64;
        let (x, y) = view.scroll_offset;
        view.scroll_offset = ((x.min(u32::MAX / 2) as i64 + dx as i64 * step).max(0) as u32, (y.min(u32::MAX / 2) as i64 + dy as i64 * step).max(0) as u32);
        self.wake_caret();
    }

    /// `n`/`N`: the next (or previous) page with a match of the last
    /// search, going round the end.
    fn pdf_step_match(&mut self, forward: bool) {
        let Some(key) = self.pdf_target() else { return };
        let Some(doc) = self.pdf_docs.get(&key.1) else { return };
        if doc.match_pages.is_empty() {
            let message = if doc.last_search_query.is_empty() { "no search yet -- / searches the document".to_string() } else { format!("no matches for \"{}\"", doc.last_search_query) };
            self.set_message(message);
            return;
        }
        let current = self.pdf_views.get(&key).map(|view| view.current_page).unwrap_or(0);
        let pages = &doc.match_pages;
        let at = if forward {
            pages.iter().position(|&p| p > current).unwrap_or(0)
        } else {
            pages.iter().rposition(|&p| p < current).unwrap_or(pages.len() - 1)
        };
        let (page, total) = (pages[at], pages.len());
        self.pdf_jump_to_page(key, page);
        self.set_message(format!("match on page {} ({}/{total} pages)", page + 1, at + 1));
    }

    // -- Zoom -------------------------------------------------------------

    /// `+`/`-`: the next zoom step in or out, from wherever the view is --
    /// a fitted view steps from the percentage it's fitted at.
    fn pdf_zoom_step(&mut self, in_: bool) {
        let Some(key) = self.pdf_target() else { return };
        let page_w = self.pdf_page_pts(key).0;
        let scale = self.frame_scale();
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let current = match view.zoom {
            Zoom::Percent(p) => p,
            _ => reader::effective_percent(view.last_requested_size.0, page_w, scale),
        };
        view.zoom = Zoom::Percent(reader::step_zoom(current, in_));
        self.pdf_render(key);
        self.wake_caret();
    }

    pub(crate) fn pdf_zoom_in(&mut self) {
        self.pdf_zoom_step(true);
    }

    pub(crate) fn pdf_zoom_out(&mut self) {
        self.pdf_zoom_step(false);
    }

    fn pdf_set_zoom(&mut self, zoom: Zoom) {
        let Some(key) = self.pdf_target() else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        view.zoom = zoom;
        self.pdf_render(key);
        self.wake_caret();
    }

    pub(crate) fn pdf_zoom_fit_page(&mut self) {
        self.pdf_set_zoom(Zoom::FitPage);
    }

    pub(crate) fn pdf_zoom_fit_width(&mut self) {
        self.pdf_set_zoom(Zoom::FitWidth);
    }

    /// The modeline's page and zoom for the focused pane's document.
    pub(super) fn pdf_modeline_position(&self) -> Option<String> {
        let key = self.pdf_view_in_pane(self.focused_pane_id())?;
        let doc = self.pdf_docs.get(&key.1)?;
        let (page, zoom) = match self.pdf_views.get(&key) {
            Some(view) => (view.current_page, view.zoom),
            None => doc.last_place,
        };
        let pending = self.pdf_pending_keys().map(|keys| format!("{keys}   ")).unwrap_or_default();
        Some(if doc.page_count == 0 {
            format!("{pending}Opening...")
        } else {
            format!("{pending}Page {}/{}   {}", page + 1, doc.page_count, reader::zoom_label(zoom))
        })
    }

    // -- Go to page -------------------------------------------------------

    /// `SPC r g`: asks for a page number.
    pub(crate) fn start_pdf_goto_page_prompt(&mut self) {
        if self.pdf_target().is_none() {
            return;
        }
        self.pdf_goto_page_prompt = Some(String::new());
        self.wake_caret();
    }

    /// A key while `SPC r g` asks: digits, Enter, Backspace, Esc.
    pub(super) fn pdf_goto_page_prompt_key(&mut self, key: KeyPress) {
        let Some(input) = &mut self.pdf_goto_page_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.pdf_goto_page_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let input = self.pdf_goto_page_prompt.take().unwrap_or_default();
                if let Ok(page_number) = input.parse::<u32>() {
                    self.pdf_goto_page(page_number);
                }
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() && c.is_ascii_digit() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    pub(super) fn pdf_goto_page_prompt_text(&self) -> Option<String> {
        self.pdf_goto_page_prompt.as_ref().map(|input| format!("go to page: {input}"))
    }

    // -- Outline ----------------------------------------------------------

    /// Closes `doc`'s outline pane, if it has one, and goes back to the
    /// document.
    pub(super) fn pdf_close_outline_pane(&mut self, doc: BufferId) {
        let Some(pane) = self.pdf_outline_panes.remove(&doc) else { return };
        self.pdf_close_companion(pane);
        if let Some((reader_pane, _)) = self.pdf_view_of_doc(doc) {
            self.windows_mut().focus(reader_pane);
        }
        self.wake_caret();
    }

    /// Closes an outline or search pane and forgets its buffer.
    fn pdf_close_companion(&mut self, pane: fenix_window::WindowId) {
        let buffer = self.windows().content(pane).copied();
        if self.windows().windows().contains(&pane) {
            self.windows_mut().focus(pane);
            if self.windows_mut().close_focused() {
                self.workspaces.active_pane_states_mut().remove(&pane);
                self.workspaces.active_scroll_anims_mut().remove(&pane);
                self.workspaces.active_pane_tabs_mut().remove(&pane);
            }
        }
        if let Some(buffer) = buffer {
            self.buffers.close(buffer);
            self.pdf_outline_lines.remove(&buffer);
            self.pdf_outline_source.remove(&buffer);
            self.pdf_search_result_lines.remove(&buffer);
            self.pdf_search_source.remove(&buffer);
        }
    }

    /// A split next to `doc`'s pane showing `entries`.
    fn pdf_open_outline_pane(&mut self, doc: BufferId, entries: &[fenix_pdf::outline::OutlineEntry]) {
        let Some((reader_pane, _)) = self.pdf_view_of_doc(doc) else { return };
        self.windows_mut().focus(reader_pane);
        let (text, lines) = pdf_outline::render(entries);
        let buffer = self.buffers.open_pdf_outline(&text);
        let pane = self.windows_mut().split(SplitKind::Vertical, buffer);
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
        self.pane_titles.insert(pane, "Outline".to_string());
        self.pdf_outline_lines.insert(buffer, lines);
        self.pdf_outline_source.insert(buffer, doc);
        self.pdf_outline_panes.insert(doc, pane);
        self.wake_caret();
    }

    /// `o`, `SPC r o`: opens or closes the outline of the document read
    /// -- from the document or from the outline itself.
    pub(crate) fn pdf_toggle_outline(&mut self) {
        let focused = self.focused_buffer_id();
        if let Some(doc) = self.pdf_outline_source.get(&focused).copied() {
            self.pdf_close_outline_pane(doc);
            return;
        }
        let Some((_, doc)) = self.pdf_target() else { return };
        if self.pdf_outline_panes.contains_key(&doc) {
            self.pdf_close_outline_pane(doc);
            return;
        }
        if let Some(entries) = self.pdf_docs.get(&doc).and_then(|d| d.outline.clone()) {
            self.pdf_open_outline_pane(doc, &entries);
            return;
        }
        if let (Some(worker), Some(d)) = (&self.pdf_worker, self.pdf_docs.get(&doc)) {
            worker.send(fenix_pdf::PdfRequest::FetchOutline { key: d.doc_key });
        }
    }

    /// `Enter` in the outline: its document to that entry's page.
    pub(super) fn pdf_outline_activate_selected(&mut self) {
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let buffer = self.focused_buffer_id();
        let Some(page_index) = self.pdf_outline_lines.get(&buffer).and_then(|lines| lines.get(line)).and_then(|meta| meta.as_ref()).map(|meta| meta.page_index) else { return };
        let Some(doc) = self.pdf_outline_source.get(&buffer).copied() else { return };
        let Some(key) = self.pdf_view_of_doc(doc) else { return };
        self.pdf_ensure_view(key);
        self.pdf_jump_to_page(key, page_index);
    }

    // -- Search -----------------------------------------------------------

    /// `/`, `SPC r /`: asks what to search for.
    pub(crate) fn start_pdf_search_prompt(&mut self) {
        if self.pdf_target().is_none() {
            return;
        }
        self.pdf_search_prompt = Some(String::new());
        self.wake_caret();
    }

    pub(super) fn pdf_search_prompt_key(&mut self, key: KeyPress) {
        let Some(input) = &mut self.pdf_search_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.pdf_search_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let query = self.pdf_search_prompt.take().unwrap_or_default();
                self.pdf_dispatch_search(query);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    pub(super) fn pdf_search_prompt_text(&self) -> Option<String> {
        self.pdf_search_prompt.as_ref().map(|input| format!("search pdf: {input}"))
    }

    /// Sends a search of the document read. A blank query does nothing.
    fn pdf_dispatch_search(&mut self, query: String) {
        if query.trim().is_empty() {
            return;
        }
        let Some((_, doc)) = self.pdf_target() else { return };
        let request_id = self.pdf_next_id();
        let Some(d) = self.pdf_docs.get_mut(&doc) else { return };
        d.pending_search_request_id = request_id;
        d.last_search_query = query.clone();
        d.match_pages.clear();
        if let Some(worker) = &self.pdf_worker {
            worker.send(fenix_pdf::PdfRequest::Search { key: d.doc_key, request_id, query });
        }
        self.set_message("searching...");
    }

    pub(super) fn pdf_close_search_pane(&mut self, doc: BufferId) {
        let Some(pane) = self.pdf_search_panes.remove(&doc) else { return };
        self.pdf_close_companion(pane);
        if let Some((reader_pane, _)) = self.pdf_view_of_doc(doc) {
            self.windows_mut().focus(reader_pane);
        }
        self.wake_caret();
    }

    /// The results of a search of `doc`: into its search pane if it has
    /// one, else a new split next to it.
    fn pdf_open_or_update_search_pane(&mut self, doc: BufferId, query: &str, matches: &[fenix_pdf::search::PdfSearchMatch]) {
        let (text, lines) = pdf_search::render(query, matches);
        if let Some(buffer) = self.pdf_search_panes.get(&doc).and_then(|&pane| self.windows().content(pane).copied()) {
            if let Some(ob) = self.buffers.get_mut(buffer) {
                let end = ob.buffer.len_chars();
                let mut scratch = Cursor::at_start();
                ob.buffer.replace_range(&mut scratch, 0, end, &text);
            }
            self.pdf_search_result_lines.insert(buffer, lines);
            for p in self.windows().windows() {
                if self.windows().content(p) == Some(&buffer) {
                    *self.pane_state_mut(p) = PaneState::seeded_at(Cursor::at_start());
                }
            }
            self.wake_caret();
            return;
        }
        let Some((reader_pane, _)) = self.pdf_view_of_doc(doc) else { return };
        self.windows_mut().focus(reader_pane);
        let buffer = self.buffers.open_pdf_search_results(&text);
        let pane = self.windows_mut().split(SplitKind::Vertical, buffer);
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(Cursor::at_start()));
        self.pane_titles.insert(pane, "Search results".to_string());
        self.pdf_search_result_lines.insert(buffer, lines);
        self.pdf_search_source.insert(buffer, doc);
        self.pdf_search_panes.insert(doc, pane);
        self.wake_caret();
    }

    /// `Enter` on a search result: its document to that page.
    pub(super) fn pdf_search_activate_selected(&mut self) {
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let buffer = self.focused_buffer_id();
        let Some(page_index) = self.pdf_search_result_lines.get(&buffer).and_then(|lines| lines.get(line)).and_then(|meta| meta.as_ref()).map(|meta| meta.page_index) else { return };
        let Some(doc) = self.pdf_search_source.get(&buffer).copied() else { return };
        let Some(key) = self.pdf_view_of_doc(doc) else { return };
        self.pdf_ensure_view(key);
        self.pdf_jump_to_page(key, page_index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each `App` below spawns a real `PdfWorker`, which binds pdfium --
    /// a process-wide library that crashes when two threads bring it up
    /// or tear it down at once, as parallel tests would.
    static PDF_WORKER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// An app with `path` open as a PDF, as if the worker had answered:
    /// 10 US Letter pages, in a 612 x 792 pane. Returns the guard first so
    /// the app (and its worker) is dropped before the lock is released.
    fn test_open_pdf(path: &str) -> (std::sync::MutexGuard<'static, ()>, ViewKey, App) {
        let guard = PDF_WORKER_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = App::with_file(None);
        app.open_pdf_path(Path::new(path));
        let key = app.pdf_view_in_pane(app.focused_pane_id()).expect("the PDF is in the focused pane");
        let doc = app.pdf_docs.get_mut(&key.1).unwrap();
        doc.page_count = 10;
        doc.page_point_size = (612.0, 792.0);
        app.pdf_views.get_mut(&key).unwrap().last_pane_size = (612, 792);
        (guard, key, app)
    }

    fn view(app: &App, key: ViewKey) -> &PdfView {
        app.pdf_views.get(&key).expect("the view")
    }

    fn keys(app: &mut App, text: &str) {
        for c in text.chars() {
            assert!(app.reader_key(KeyPress::char(c)), "{c}");
        }
    }

    fn rendered(app: &mut App, key: ViewKey, request_id: u64) {
        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.apply_pdf_response(fenix_pdf::PdfResponse::PageRendered {
            key: doc_key,
            request_id,
            page_index: 0,
            width: 612,
            height: 792,
            bgra: vec![0u8; 612 * 792 * 4],
            page_width_pts: 612.0,
            page_height_pts: 792.0,
        });
    }

    // -- A tab like any file ----------------------------------------------

    #[test]
    fn a_pdf_opens_as_a_tab_in_the_focused_pane_not_a_workspace_of_its_own() {
        let (_guard, key, app) = test_open_pdf("pdf_tab.pdf");
        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(app.focused_buffer_id(), key.1);
        assert!(app.workspaces.active_workspace().pane_tabs.get(&key.0).is_some_and(|tabs| tabs.contains(&key.1)), "it has a tab");
        assert!(!app.pane_titles.contains_key(&key.0), "a pane title would stop the tab keys");
        assert_eq!(view(&app, key).zoom, Zoom::FitWidth, "fit width by default");
    }

    #[test]
    fn opening_an_open_pdf_again_reuses_the_document() {
        let (_guard, key, mut app) = test_open_pdf("pdf_reopen.pdf");
        let scratch = app.buffers.open_scratch();
        app.open_buffer_in_focused_pane(scratch);
        app.open_pdf_path(Path::new("pdf_reopen.pdf"));
        assert_eq!(app.pdf_docs.len(), 1);
        assert_eq!(app.focused_buffer_id(), key.1);
    }

    #[test]
    fn two_panes_show_one_document_at_their_own_pages() {
        let (_guard, left, mut app) = test_open_pdf("pdf_two_views.pdf");
        let right_pane = app.windows_mut().split(SplitKind::Vertical, left.1);
        app.windows_mut().focus(right_pane);
        let right = app.pdf_target().expect("the split shows the document");
        assert_eq!(right, (right_pane, left.1));
        app.pdf_views.get_mut(&right).unwrap().last_pane_size = (612, 792);

        app.pdf_goto_page(7);

        assert_eq!(view(&app, right).current_page, 6);
        assert_eq!(view(&app, left).current_page, 0, "the other view stays where it was");
        assert_ne!(view(&app, left).id, view(&app, right).id, "each view gets its own renders");
        assert_eq!(app.pdf_docs.len(), 1);
    }

    #[test]
    fn a_new_view_starts_where_the_last_one_left_off() {
        let (_guard, key, mut app) = test_open_pdf("pdf_last_place.pdf");
        app.pdf_goto_page(5);
        app.pdf_zoom_fit_page();
        let pane = app.windows_mut().split(SplitKind::Vertical, key.1);
        let other = app.pdf_ensure_view((pane, key.1)).unwrap();
        assert_eq!((other.current_page, other.zoom), (4, Zoom::FitPage));
    }

    #[test]
    fn a_render_goes_to_the_view_that_asked_for_it_and_a_stale_one_is_dropped() {
        let (_guard, left, mut app) = test_open_pdf("pdf_render_routing.pdf");
        let right_pane = app.windows_mut().split(SplitKind::Vertical, left.1);
        let right = (right_pane, left.1);
        app.pdf_ensure_view(right).unwrap().last_pane_size = (612, 792);
        app.pdf_render(left);
        let stale = view(&app, left).pending_request_id;
        app.pdf_render(left);
        app.pdf_render(right);
        let request_id = view(&app, right).pending_request_id;
        assert_ne!(request_id, view(&app, left).pending_request_id);

        rendered(&mut app, right, request_id);
        rendered(&mut app, left, stale);

        assert!(view(&app, right).full_bgra.is_some());
        assert!(view(&app, left).full_bgra.is_none(), "neither the other view's nor a superseded one");
    }

    #[test]
    fn killing_the_buffer_closes_the_document_and_its_views() {
        let (_guard, key, mut app) = test_open_pdf("pdf_kill.pdf");
        app.kill_buffer_now();
        assert!(app.pdf_docs.is_empty());
        assert!(app.pdf_views.is_empty());
        assert!(app.buffers.get(key.1).is_none());
        assert_eq!(app.workspaces.len(), 1, "no workspace goes with it");
    }

    #[test]
    fn closing_a_frame_drops_its_views_and_keeps_the_document() {
        let (_guard, key, mut app) = test_open_pdf("pdf_frame_close.pdf");
        let elsewhere = app.buffers.open_scratch();
        let second = app.test_add_frame(elsewhere);
        app.activate_frame(second);
        app.drop_frame(0);
        assert!(!app.pdf_views.contains_key(&key), "its texture belonged to that frame");
        assert!(app.pdf_docs.contains_key(&key.1), "the buffer is still open");
    }

    #[test]
    fn spc_r_acts_on_the_document_read_last_from_another_pane() {
        let (_guard, key, mut app) = test_open_pdf("pdf_last_read.pdf");
        let scratch = app.buffers.open_scratch();
        let code = app.windows_mut().split(SplitKind::Vertical, scratch);
        app.windows_mut().focus(code);
        app.pdf_next_page();
        assert_eq!(view(&app, key).current_page, 1);
    }

    // -- Keys -------------------------------------------------------------

    #[test]
    fn counts_and_vim_page_keys() {
        let (_guard, key, mut app) = test_open_pdf("pdf_keys_pages.pdf");
        keys(&mut app, "3J");
        assert_eq!(view(&app, key).current_page, 3);
        keys(&mut app, "K");
        assert_eq!(view(&app, key).current_page, 2);
        keys(&mut app, "G");
        assert_eq!(view(&app, key).current_page, 9);
        keys(&mut app, "gg");
        assert_eq!(view(&app, key).current_page, 0);
        keys(&mut app, "7G");
        assert_eq!(view(&app, key).current_page, 6);
    }

    #[test]
    fn g_waits_so_gt_moves_to_the_next_tab() {
        let (_guard, key, mut app) = test_open_pdf("pdf_keys_gt.pdf");
        let scratch = app.buffers.open_scratch();
        app.open_buffer_in_focused_pane(scratch);
        app.set_pane_content_as(key.0, key.1, false);
        assert_eq!(app.focused_buffer_id(), key.1);

        keys(&mut app, "g");
        assert_eq!(app.focused_buffer_id(), key.1, "g alone does nothing yet");
        keys(&mut app, "t");

        assert_ne!(app.focused_buffer_id(), key.1, "gt went to another tab");
    }

    #[test]
    fn the_leader_and_colon_are_not_the_readers() {
        let (_guard, _key, mut app) = test_open_pdf("pdf_keys_pass.pdf");
        assert!(!app.reader_key(KeyPress::char(' ')));
        assert!(!app.reader_key(KeyPress::char(':')));
        assert!(!app.reader_key(KeyPress::char('w').with_ctrl()));
    }

    #[test]
    fn equals_toggles_fit_width_and_fit_page() {
        let (_guard, key, mut app) = test_open_pdf("pdf_keys_fit.pdf");
        keys(&mut app, "=");
        assert_eq!(view(&app, key).zoom, Zoom::FitPage);
        keys(&mut app, "=");
        assert_eq!(view(&app, key).zoom, Zoom::FitWidth);
        keys(&mut app, "z0");
        assert_eq!(view(&app, key).zoom, Zoom::Percent(100));
    }

    #[test]
    fn n_and_capital_n_go_through_the_pages_with_matches() {
        let (_guard, key, mut app) = test_open_pdf("pdf_keys_matches.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().match_pages = vec![2, 5, 8];
        keys(&mut app, "n");
        assert_eq!(view(&app, key).current_page, 2);
        keys(&mut app, "n");
        assert_eq!(view(&app, key).current_page, 5);
        keys(&mut app, "N");
        assert_eq!(view(&app, key).current_page, 2);
        keys(&mut app, "N");
        assert_eq!(view(&app, key).current_page, 8, "round the start to the last");
    }

    #[test]
    fn n_before_any_search_says_how_to_search() {
        let (_guard, _key, mut app) = test_open_pdf("pdf_keys_no_search.pdf");
        keys(&mut app, "n");
        assert!(app.modeline_text().contains("/ searches"), "{}", app.modeline_text());
    }

    #[test]
    fn the_modeline_shows_the_page_zoom_and_a_count_being_typed() {
        let (_guard, _key, mut app) = test_open_pdf("pdf_modeline.pdf");
        app.pdf_goto_page(4);
        let modeline = app.modeline_text();
        assert!(modeline.contains("Page 4/10") && modeline.contains("Fit width") && !modeline.contains("Ln "), "{modeline}");
        keys(&mut app, "12");
        assert!(app.modeline_text().contains("12   Page 4/10"), "{}", app.modeline_text());
    }

    // -- Scrolling and pages ----------------------------------------------

    /// A render taller than the pane, so there's something to scroll.
    fn seed_scrollable_render(app: &mut App, key: ViewKey) {
        let view = app.pdf_views.get_mut(&key).unwrap();
        view.last_pane_size = (612, 400);
        view.full_bgra = Some((612, 792, vec![0u8; 612 * 792 * 4]));
        view.scroll_offset = (0, 0);
    }

    #[test]
    fn scrolling_moves_within_a_page_taller_than_the_pane() {
        let (_guard, key, mut app) = test_open_pdf("pdf_scroll_within.pdf");
        seed_scrollable_render(&mut app, key);
        keys(&mut app, "j");
        assert_eq!(view(&app, key).scroll_offset.1, PAN_STEP_PX);
        assert_eq!(view(&app, key).current_page, 0);
        keys(&mut app, "2j");
        assert_eq!(view(&app, key).scroll_offset.1, 3 * PAN_STEP_PX);
    }

    #[test]
    fn scrolling_past_the_bottom_goes_on_to_the_top_of_the_next_page() {
        let (_guard, key, mut app) = test_open_pdf("pdf_scroll_past_bottom.pdf");
        seed_scrollable_render(&mut app, key);
        app.pdf_views.get_mut(&key).unwrap().scroll_offset = (0, 392);
        app.pdf_scroll(PAN_STEP_PX as i32);
        assert_eq!(view(&app, key).current_page, 1);
        assert!(!view(&app, key).land_at_bottom);
    }

    #[test]
    fn scrolling_past_the_top_goes_back_to_the_bottom_of_the_page_before() {
        let (_guard, key, mut app) = test_open_pdf("pdf_scroll_past_top.pdf");
        seed_scrollable_render(&mut app, key);
        app.pdf_views.get_mut(&key).unwrap().current_page = 3;
        app.pdf_scroll(-(PAN_STEP_PX as i32));
        assert_eq!(view(&app, key).current_page, 2);
        assert!(view(&app, key).land_at_bottom);
    }

    #[test]
    fn a_page_that_fits_turns_on_every_scroll_and_stops_at_the_last() {
        let (_guard, key, mut app) = test_open_pdf("pdf_scroll_fitting.pdf");
        app.pdf_views.get_mut(&key).unwrap().full_bgra = Some((612, 792, vec![0u8; 612 * 792 * 4]));
        app.pdf_scroll(PAN_STEP_PX as i32);
        assert_eq!(view(&app, key).current_page, 1);
        app.pdf_views.get_mut(&key).unwrap().current_page = 9;
        app.pdf_scroll(PAN_STEP_PX as i32);
        assert_eq!(view(&app, key).current_page, 9);
    }

    #[test]
    fn a_render_lands_at_the_bottom_only_when_scrolling_back_asked_for_it() {
        let (_guard, key, mut app) = test_open_pdf("pdf_landing.pdf");
        app.pdf_render(key);
        app.pdf_views.get_mut(&key).unwrap().land_at_bottom = true;
        let request_id = view(&app, key).pending_request_id;
        rendered(&mut app, key, request_id);
        assert_eq!(view(&app, key).scroll_offset, (0, u32::MAX), "clamped to the bottom when drawn");
        assert!(!view(&app, key).land_at_bottom);
    }

    #[test]
    fn going_to_a_page_counts_from_1_and_stops_at_the_last() {
        let (_guard, key, mut app) = test_open_pdf("pdf_goto.pdf");
        app.pdf_goto_page(5);
        assert_eq!(view(&app, key).current_page, 4);
        assert_eq!(view(&app, key).last_requested_size, (612, 792), "rendered at fit width");
        app.pdf_goto_page(9999);
        assert_eq!(view(&app, key).current_page, 9);
        app.pdf_first_page();
        assert_eq!(view(&app, key).current_page, 0);
    }

    #[test]
    fn the_go_to_page_prompt_takes_digits_and_esc_cancels() {
        let (_guard, key, mut app) = test_open_pdf("pdf_goto_prompt.pdf");
        app.start_pdf_goto_page_prompt();
        for c in "7x".chars() {
            app.pdf_goto_page_prompt_key(KeyPress::char(c));
        }
        assert_eq!(app.pdf_goto_page_prompt_text(), Some("go to page: 7".to_string()));
        app.pdf_goto_page_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!(view(&app, key).current_page, 6);

        app.start_pdf_goto_page_prompt();
        app.pdf_goto_page_prompt_key(KeyPress::char('2'));
        app.pdf_goto_page_prompt_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.pdf_goto_page_prompt.is_none());
        assert_eq!(view(&app, key).current_page, 6);
    }

    #[test]
    fn prompts_need_a_pdf() {
        let _guard = PDF_WORKER_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = App::with_file(None);
        app.start_pdf_goto_page_prompt();
        app.start_pdf_search_prompt();
        assert!(app.pdf_goto_page_prompt.is_none() && app.pdf_search_prompt.is_none());
    }

    // -- Zoom and panning -------------------------------------------------

    #[test]
    fn zoom_steps_from_where_the_view_is_and_stops_at_the_ends() {
        let (_guard, key, mut app) = test_open_pdf("pdf_zoom.pdf");
        app.pdf_views.get_mut(&key).unwrap().last_requested_size = (612, 792);
        app.pdf_zoom_in();
        assert_eq!(view(&app, key).zoom, Zoom::Percent(110), "a fitted 100% steps to 110%");
        app.pdf_views.get_mut(&key).unwrap().zoom = Zoom::Percent(400);
        app.pdf_zoom_in();
        assert_eq!(view(&app, key).zoom, Zoom::Percent(400));
        app.pdf_views.get_mut(&key).unwrap().zoom = Zoom::Percent(25);
        app.pdf_zoom_out();
        assert_eq!(view(&app, key).zoom, Zoom::Percent(25));
        app.pdf_zoom_fit_page();
        assert_eq!(view(&app, key).zoom, Zoom::FitPage);
    }

    #[test]
    fn panning_moves_and_never_goes_below_zero() {
        let (_guard, key, mut app) = test_open_pdf("pdf_pan.pdf");
        app.pdf_views.get_mut(&key).unwrap().scroll_offset = (100, 100);
        keys(&mut app, "l");
        assert_eq!(view(&app, key).scroll_offset, (100 + PAN_STEP_PX, 100));
        app.pdf_pan(-5, -5);
        assert_eq!(view(&app, key).scroll_offset, (0, 0));
    }

    // -- Outline ----------------------------------------------------------

    fn sample_outline() -> Vec<fenix_pdf::outline::OutlineEntry> {
        vec![
            fenix_pdf::outline::OutlineEntry { title: "Chapter 1".to_string(), page_index: 0, depth: 0 },
            fenix_pdf::outline::OutlineEntry { title: "Chapter 2".to_string(), page_index: 6, depth: 0 },
        ]
    }

    #[test]
    fn o_opens_the_outline_beside_the_document_and_closes_it_again() {
        let (_guard, key, mut app) = test_open_pdf("pdf_outline.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(sample_outline());
        let panes = app.windows().window_count();

        keys(&mut app, "o");
        assert_eq!(app.windows().window_count(), panes + 1);
        let outline = app.focused_buffer_id();
        assert_eq!(app.buffers.get(outline).unwrap().kind, BufferKind::PdfOutline);
        assert_eq!(app.pdf_outline_source.get(&outline), Some(&key.1));
        assert_eq!(app.open().buffer.text(), "Chapter 1\nChapter 2\n");

        app.pdf_toggle_outline();
        assert_eq!(app.windows().window_count(), panes);
        assert!(app.pdf_outline_panes.is_empty() && app.pdf_outline_lines.is_empty() && app.pdf_outline_source.is_empty());
        assert_eq!(app.focused_pane_id(), key.0);
    }

    #[test]
    fn an_outline_not_fetched_yet_is_asked_for_and_opens_when_it_comes() {
        let (_guard, key, mut app) = test_open_pdf("pdf_outline_fetch.pdf");
        let panes = app.windows().window_count();
        app.pdf_toggle_outline();
        assert_eq!(app.windows().window_count(), panes, "nothing to show yet");

        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.apply_pdf_response(fenix_pdf::PdfResponse::Outline { key: doc_key, entries: sample_outline() });
        assert_eq!(app.pdf_docs[&key.1].outline, Some(sample_outline()));
        assert_eq!(app.windows().window_count(), panes + 1);
    }

    #[test]
    fn enter_on_an_outline_entry_takes_the_document_there() {
        let (_guard, key, mut app) = test_open_pdf("pdf_outline_enter.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(sample_outline());
        app.pdf_toggle_outline();
        let start = app.open().buffer.line_start_char(1);
        app.test_set_cursor(Cursor { char_idx: start, sticky_col: 0 });
        app.pdf_outline_activate_selected();
        assert_eq!(view(&app, key).current_page, 6);
    }

    #[test]
    fn killing_the_document_closes_its_outline_and_search_panes() {
        let (_guard, key, mut app) = test_open_pdf("pdf_kill_companions.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(sample_outline());
        app.pdf_toggle_outline();
        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.pdf_docs.get_mut(&key.1).unwrap().pending_search_request_id = 1;
        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id: 1, matches: sample_matches() });
        app.windows_mut().focus(key.0);

        app.kill_buffer_now();

        assert!(app.pdf_outline_panes.is_empty() && app.pdf_outline_source.is_empty());
        assert!(app.pdf_search_panes.is_empty() && app.pdf_search_source.is_empty());
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn killing_the_outline_buffer_closes_its_pane_and_goes_back_to_the_document() {
        let (_guard, key, mut app) = test_open_pdf("pdf_outline_kill.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(sample_outline());
        app.pdf_toggle_outline();
        app.kill_buffer_now();
        assert!(app.pdf_outline_panes.is_empty());
        assert_eq!(app.focused_pane_id(), key.0);
        assert!(app.pdf_docs.contains_key(&key.1));
    }

    // -- Search -----------------------------------------------------------

    fn sample_matches() -> Vec<fenix_pdf::search::PdfSearchMatch> {
        vec![
            fenix_pdf::search::PdfSearchMatch { page_index: 0, char_index: 5, context: "the quick brown fox".to_string() },
            fenix_pdf::search::PdfSearchMatch { page_index: 6, char_index: 0, context: "jumps over the lazy dog".to_string() },
            fenix_pdf::search::PdfSearchMatch { page_index: 6, char_index: 40, context: "another dog".to_string() },
        ]
    }

    #[test]
    fn slash_asks_for_a_query_and_sends_the_search() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_prompt.pdf");
        keys(&mut app, "/");
        for c in "fox".chars() {
            app.pdf_search_prompt_key(KeyPress::char(c));
        }
        assert_eq!(app.pdf_search_prompt_text(), Some("search pdf: fox".to_string()));
        app.pdf_search_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        let doc = &app.pdf_docs[&key.1];
        assert!(doc.pending_search_request_id > 0);
        assert_eq!(doc.last_search_query, "fox");
    }

    #[test]
    fn search_results_open_beside_the_document_and_enter_goes_to_a_match() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_results.pdf");
        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.pdf_docs.get_mut(&key.1).unwrap().pending_search_request_id = 1;
        let panes = app.windows().window_count();

        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id: 1, matches: sample_matches() });

        assert_eq!(app.windows().window_count(), panes + 1);
        let results = app.focused_buffer_id();
        assert_eq!(app.pdf_search_source.get(&results), Some(&key.1));
        assert!(app.open().buffer.text().contains("p.  7  jumps over the lazy dog"));
        assert_eq!(app.pdf_docs[&key.1].match_pages, vec![0, 6], "one entry per page");

        let start = app.open().buffer.line_start_char(1);
        app.test_set_cursor(Cursor { char_idx: start, sticky_col: 0 });
        app.pdf_search_activate_selected();
        assert_eq!(view(&app, key).current_page, 6);
    }

    #[test]
    fn a_stale_search_is_dropped_and_a_new_one_reuses_the_results_pane() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_second.pdf");
        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.pdf_docs.get_mut(&key.1).unwrap().pending_search_request_id = 2;
        let panes = app.windows().window_count();
        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id: 1, matches: sample_matches() });
        assert_eq!(app.windows().window_count(), panes, "stale");

        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id: 2, matches: sample_matches() });
        let results_pane = app.pdf_search_panes[&key.1];
        app.pdf_docs.get_mut(&key.1).unwrap().pending_search_request_id = 3;
        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults {
            key: doc_key,
            request_id: 3,
            matches: vec![fenix_pdf::search::PdfSearchMatch { page_index: 2, char_index: 0, context: "banana".to_string() }],
        });
        assert_eq!(app.windows().window_count(), panes + 1);
        assert_eq!(app.pdf_search_panes[&key.1], results_pane);
        let buffer = *app.windows().content(results_pane).unwrap();
        assert!(app.buffers.get(buffer).unwrap().buffer.text().contains("banana"));
    }

    #[test]
    fn no_matches_say_so_and_enter_there_does_nothing() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_none.pdf");
        let doc_key = app.pdf_docs[&key.1].doc_key;
        {
            let doc = app.pdf_docs.get_mut(&key.1).unwrap();
            doc.pending_search_request_id = 1;
            doc.last_search_query = "xylophone".to_string();
        }
        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id: 1, matches: Vec::new() });
        assert_eq!(app.open().buffer.text(), "(no matches for \"xylophone\")");
        app.pdf_search_activate_selected();
        assert_eq!(view(&app, key).current_page, 0);
    }
}
