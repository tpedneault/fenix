//! The host half of the PDF reader (`reader` is the pure half): the open
//! documents, each pane's view of one, the pdfium worker's replies, the
//! reader's keys, and the outline and search panes.
//!
//! A document belongs to its buffer, the way a file's text does: opening
//! a PDF gives it a tab in the focused pane like any file, the tab can be
//! moved, closed and reopened, and the same document can be shown in two
//! panes. Where you are in it belongs to the *view*: one per pane and tab
//! (`ViewKey`), made the first time a pane shows the document, starting
//! where the last view left off.
//!
//! A view shows the whole document as one column of pages
//! (`reader::Layout`) scrolled to a point in it. Each frame it asks the
//! worker for the pages in sight and a few either side, keeps what comes
//! back in its page cache, and drops what's scrolled far away.

use super::*;
use crate::reader::{self, Cmd, Layout, Outcome, Zoom};

/// How far `j`/`k`/`h`/`l` move, in pixels.
pub(super) const PAN_STEP_PX: u32 = 60;

/// Pages rendered ahead of the pane and behind it.
const AHEAD: usize = 2;
const BEHIND: usize = 1;

/// Rendered pages further than this from the pane are let go.
const KEEP: usize = 8;

/// A page not rendered yet: blank paper.
pub(super) const PAPER: [f32; 4] = [0.91, 0.905, 0.89, 1.0];

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
    /// Every page's size in points; empty until the worker has opened it.
    pub(super) pages: Vec<(f32, f32)>,
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

impl PdfDoc {
    pub(super) fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }
}

/// A page the worker has rendered for a view.
pub(super) struct CachedPage {
    /// The size it was rendered at -- drawn stretched if the layout has
    /// since changed, until the sharp one comes.
    pub(super) w: u32,
    pub(super) h: u32,
    /// The pixels until the frame drawing the view has put them in
    /// `texture`.
    pub(super) bgra: Option<Vec<u8>>,
    pub(super) texture: Option<PdfTexture>,
}

/// One pane's view of a document -- `pdf_views[(pane, buffer)]`.
pub(super) struct PdfView {
    /// The worker's name for this view, so two views of one document
    /// don't cancel each other's renders.
    pub(super) id: u64,
    pub(super) zoom: Zoom,
    /// The pane's size in pixels and the window's scale factor, from the
    /// last frame drawn; `(0, 0)` until then.
    pub(super) pane: (f32, f32),
    pub(super) dpi: f32,
    /// The document laid out for `zoom`, `pane` and `dpi`.
    pub(super) layout: Layout,
    layout_for: Option<(Zoom, (f32, f32), f32, usize)>,
    /// Top-left of what's shown, in the layout's pixels.
    pub(super) scroll: (f32, f32),
    /// A page to show the top of once the layout is known -- where a new
    /// view starts, or a jump made before its pane was first drawn.
    pub(super) pending_jump: Option<u32>,
    pub(super) cache: HashMap<u32, CachedPage>,
    /// Renders asked for and not back yet: page -> (request, width).
    pub(super) pending: HashMap<u32, (u64, u32)>,
    /// The pages last asked to be kept, so a scroll that changes them
    /// tells the worker.
    wanted: Vec<u32>,
}

/// One page to draw in a pane: where, and which part of its render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PageDraw {
    pub(super) page: u32,
    /// Clipped to the pane.
    pub(super) dest: (f32, f32, f32, f32),
    /// The part of the page `dest` shows, 0..1 each way.
    pub(super) uv: (f32, f32, f32, f32),
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

    /// A buffer for `path` and the worker's `Open` for it. The page sizes
    /// come later, through `apply_pdf_response`.
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
                pages: Vec::new(),
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
                    zoom,
                    pane: (0.0, 0.0),
                    dpi: 1.0,
                    layout: Layout::default(),
                    layout_for: None,
                    scroll: (0.0, 0.0),
                    pending_jump: Some(page),
                    cache: HashMap::new(),
                    pending: HashMap::new(),
                    wanted: Vec::new(),
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

    // -- Layout and rendering ---------------------------------------------

    /// Lays `key` out again if its zoom, pane, scale factor or document
    /// changed, keeping the same place at the pane's top; then applies a
    /// pending jump and keeps the scroll in bounds.
    pub(super) fn pdf_relayout(&mut self, key: ViewKey) {
        let Some(pages) = self.pdf_docs.get(&key.1).map(|doc| doc.pages.clone()) else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        if view.pane.0 <= 0.0 || view.pane.1 <= 0.0 || pages.is_empty() {
            return;
        }
        let inputs = (view.zoom, view.pane, view.dpi, pages.len());
        if view.layout_for != Some(inputs) {
            let old = std::mem::take(&mut view.layout);
            let anchor = (!old.pages.is_empty()).then(|| old.anchor(view.scroll.1));
            let x_ratio = if old.width > 0.0 { (view.scroll.0 + view.pane.0 / 2.0) / old.width } else { 0.5 };
            view.layout = reader::layout(&pages, view.zoom, view.pane, view.dpi);
            view.layout_for = Some(inputs);
            if let Some(anchor) = anchor {
                view.scroll.1 = view.layout.scroll_for(anchor);
            }
            view.scroll.0 = x_ratio * view.layout.width - view.pane.0 / 2.0;
        }
        if let Some(page) = view.pending_jump.take() {
            let page = (page as usize).min(pages.len() - 1);
            view.scroll.1 = view.layout.top_of(page);
        }
        let (max_x, max_y) = view.layout.max_scroll(view.pane);
        view.scroll = (view.scroll.0.clamp(0.0, max_x), view.scroll.1.clamp(0.0, max_y));
    }

    /// Everything a frame does for a view before drawing it: note its
    /// pane's size, lay it out, ask for the pages it needs (the ones in
    /// sight first), cancel what it no longer needs and let go of pages
    /// far away. Also what a test calls in place of drawing.
    pub(super) fn pdf_prepare_view(&mut self, key: ViewKey, pane: (f32, f32)) {
        let dpi = self.frame_scale();
        let Some(view) = self.pdf_ensure_view(key) else { return };
        view.pane = pane;
        view.dpi = dpi;
        self.pdf_relayout(key);
        let Some(doc_key) = self.pdf_docs.get(&key.1).map(|doc| doc.doc_key) else { return };
        let Some(view) = self.pdf_views.get(&key) else { return };
        if view.layout.pages.is_empty() {
            return;
        }
        let visible = view.layout.visible(view.scroll.1, view.pane.1);
        let count = view.layout.pages.len();
        let mut wanted: Vec<u32> = visible.clone().map(|p| p as u32).collect();
        wanted.extend((visible.end..(visible.end + AHEAD).min(count)).map(|p| p as u32));
        wanted.extend((visible.start.saturating_sub(BEHIND)..visible.start).rev().map(|p| p as u32));
        let asks: Vec<(u32, (u32, u32))> = wanted
            .iter()
            .map(|&p| (p, view.layout.pages[p as usize].px()))
            .filter(|&(p, (w, _))| view.cache.get(&p).is_none_or(|c| c.w != w) && view.pending.get(&p).is_none_or(|&(_, pw)| pw != w))
            .collect();
        let changed = view.wanted != wanted;
        let view_id = view.id;
        let (lo, hi) = (visible.start.saturating_sub(KEEP) as u32, (visible.end + KEEP) as u32);

        for (page, (w, h)) in asks {
            let request_id = self.pdf_next_id();
            self.pdf_requests.insert(request_id, (key, page));
            if let Some(view) = self.pdf_views.get_mut(&key) {
                view.pending.insert(page, (request_id, w));
            }
            if let Some(worker) = &self.pdf_worker {
                worker.send(fenix_pdf::PdfRequest::RenderPage { key: doc_key, view: view_id, request_id, page_index: page, target_w: w, target_h: h });
            }
        }
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        if changed {
            let dropped: Vec<u64> = view.pending.iter().filter(|(p, _)| !wanted.contains(p)).map(|(_, (id, _))| *id).collect();
            view.pending.retain(|p, _| wanted.contains(p));
            view.wanted = wanted.clone();
            for id in dropped {
                self.pdf_requests.remove(&id);
            }
            if let Some(worker) = &self.pdf_worker {
                worker.send(fenix_pdf::PdfRequest::Cancel { key: doc_key, view: view_id, keep: wanted });
            }
        }
        if let Some(view) = self.pdf_views.get_mut(&key) {
            view.cache.retain(|&p, _| p >= lo && p < hi);
        }
        self.pdf_remember_place(key);
    }

    /// The pages `key` shows in a pane at `rect`, clipped to it.
    pub(super) fn pdf_draw_plan(&self, key: ViewKey, rect: fenix_window::Rect) -> Vec<PageDraw> {
        let Some(view) = self.pdf_views.get(&key) else { return Vec::new() };
        let mut draws = Vec::new();
        for page in view.layout.visible(view.scroll.1, rect.h) {
            let p = view.layout.pages[page];
            let (dx, dy) = (rect.x + p.x - view.scroll.0, rect.y + p.y - view.scroll.1);
            let (x0, y0) = (dx.max(rect.x), dy.max(rect.y));
            let (x1, y1) = ((dx + p.w).min(rect.x + rect.w), (dy + p.h).min(rect.y + rect.h));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            draws.push(PageDraw {
                page: page as u32,
                dest: (x0, y0, x1 - x0, y1 - y0),
                uv: ((x0 - dx) / p.w, (y0 - dy) / p.h, (x1 - dx) / p.w, (y1 - dy) / p.h),
            });
        }
        draws
    }

    /// The page `key` is reading: the one across the middle of its pane.
    pub(super) fn pdf_current_page(&self, key: ViewKey) -> u32 {
        match self.pdf_views.get(&key) {
            Some(view) if !view.layout.pages.is_empty() => view.layout.current(view.scroll.1, view.pane) as u32,
            Some(view) => view.pending_jump.unwrap_or(0),
            None => self.pdf_docs.get(&key.1).map(|doc| doc.last_place.0).unwrap_or(0),
        }
    }

    fn pdf_remember_place(&mut self, key: ViewKey) {
        let page = self.pdf_current_page(key);
        let Some(zoom) = self.pdf_views.get(&key).map(|view| view.zoom) else { return };
        if let Some(doc) = self.pdf_docs.get_mut(&key.1) {
            doc.last_place = (page, zoom);
        }
    }

    /// The document whose worker name is `doc_key`.
    fn pdf_doc_by_key(&self, doc_key: fenix_pdf::PdfDocKey) -> Option<BufferId> {
        self.pdf_docs.iter().find(|(_, doc)| doc.doc_key == doc_key).map(|(id, _)| *id)
    }

    /// A reply from the worker. Anything for a document or view since
    /// closed, or for a render since superseded, is dropped.
    pub(super) fn apply_pdf_response(&mut self, response: fenix_pdf::PdfResponse) {
        match response {
            fenix_pdf::PdfResponse::Opened { key, pages } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                if let Some(doc) = self.pdf_docs.get_mut(&buffer) {
                    doc.last_place.0 = doc.last_place.0.min((pages.len() as u32).saturating_sub(1));
                    doc.pages = pages;
                }
                // The views lay themselves out on the next frame.
            }
            fenix_pdf::PdfResponse::OpenFailed { key, message } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                let path = self.pdf_docs.get(&buffer).map(|doc| doc.path.display().to_string()).unwrap_or_default();
                self.set_error(format!("couldn't open {path}: {message}"));
            }
            fenix_pdf::PdfResponse::PageRendered { request_id, page_index, width, height, bgra, .. } => {
                let Some((view_key, page)) = self.pdf_requests.remove(&request_id) else { return };
                let Some(view) = self.pdf_views.get_mut(&view_key) else { return };
                if page != page_index || view.pending.get(&page).is_none_or(|&(id, _)| id != request_id) {
                    return;
                }
                view.pending.remove(&page);
                view.cache.insert(page, CachedPage { w: width, h: height, bgra: Some(bgra), texture: None });
            }
            fenix_pdf::PdfResponse::RenderFailed { request_id, message, .. } => {
                let Some((view_key, page)) = self.pdf_requests.remove(&request_id) else { return };
                if let Some(view) = self.pdf_views.get_mut(&view_key) {
                    view.pending.remove(&page);
                }
                self.set_error(format!("couldn't render page {}: {message}", page + 1));
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
        self.pdf_requests.retain(|_, ((pane, _), _)| !panes.contains(pane));
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
        self.pdf_requests.retain(|_, ((_, b), _)| *b != buffer);
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
        let pane_h = self.pdf_target().and_then(|key| self.pdf_views.get(&key)).map(|view| view.pane.1 as i32).filter(|&h| h > 0).unwrap_or(400);
        match cmd {
            Cmd::Scroll(n) => self.pdf_scroll(n.saturating_mul(PAN_STEP_PX as i32)),
            Cmd::HalfScreen(n) => self.pdf_scroll(n.saturating_mul(pane_h / 2)),
            Cmd::Screen(n) => self.pdf_scroll(n.saturating_mul((pane_h - PAN_STEP_PX as i32).max(PAN_STEP_PX as i32))),
            Cmd::Pan(n) => self.pdf_pan(n, 0),
            Cmd::Page(n) => self.pdf_turn_page(n),
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

    /// `J`/`K`, `SPC r n`/`SPC r p`: `delta` pages on from the one being
    /// read, at its top.
    pub(super) fn pdf_turn_page(&mut self, delta: i32) {
        let Some(key) = self.pdf_target() else { return };
        let current = self.pdf_current_page(key) as i64;
        self.pdf_jump_to_page(key, (current + delta as i64).max(0) as u32);
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
        let Some(count) = self.pdf_docs.get(&key.1).map(|doc| doc.page_count()) else { return };
        self.pdf_goto_page(count);
    }

    /// Scrolls `delta_px` down the column (up when negative).
    pub(super) fn pdf_scroll(&mut self, delta_px: i32) {
        let Some(key) = self.pdf_target() else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let max_y = view.layout.max_scroll(view.pane).1;
        view.scroll.1 = (view.scroll.1 + delta_px as f32).clamp(0.0, max_y);
        self.pdf_remember_place(key);
        self.wake_caret();
    }

    /// Page `page_number`, counting from 1; past the end is the last page.
    pub(crate) fn pdf_goto_page(&mut self, page_number: u32) {
        let Some(key) = self.pdf_target() else { return };
        self.pdf_jump_to_page(key, page_number.saturating_sub(1));
    }

    /// `key` to the top of page `page` (from 0; past the end is the
    /// last). Before the view is laid out, it goes there once it is.
    fn pdf_jump_to_page(&mut self, key: ViewKey, page: u32) {
        let count = self.pdf_docs.get(&key.1).map(|doc| doc.page_count()).unwrap_or(0);
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let page = if count > 0 { page.min(count - 1) } else { page };
        view.pending_jump = Some(page);
        self.pdf_relayout(key);
        if let Some(doc) = self.pdf_docs.get_mut(&key.1) {
            doc.last_place.0 = page;
        }
        self.wake_caret();
    }

    /// `h`/`l`: pans by `PAN_STEP_PX` steps.
    pub(super) fn pdf_pan(&mut self, dx: i32, dy: i32) {
        let Some(key) = self.pdf_target() else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let step = PAN_STEP_PX as f32;
        let (max_x, max_y) = view.layout.max_scroll(view.pane);
        view.scroll = ((view.scroll.0 + dx as f32 * step).clamp(0.0, max_x), (view.scroll.1 + dy as f32 * step).clamp(0.0, max_y));
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
        let current = self.pdf_current_page(key);
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
        let Some(view) = self.pdf_views.get(&key) else { return };
        let current = match view.zoom {
            Zoom::Percent(p) => p,
            _ if view.layout.scale > 0.0 => (view.layout.scale / view.dpi.max(0.1) * 100.0).round() as u32,
            _ => 100,
        };
        self.pdf_set_zoom(Zoom::Percent(reader::step_zoom(current, in_)));
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
        self.pdf_relayout(key);
        self.pdf_remember_place(key);
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
        let zoom = self.pdf_views.get(&key).map(|view| view.zoom).unwrap_or(doc.last_place.1);
        let pending = self.pdf_pending_keys().map(|keys| format!("{keys}   ")).unwrap_or_default();
        Some(if doc.pages.is_empty() {
            format!("{pending}Opening...")
        } else {
            format!("{pending}Page {}/{}   {}", self.pdf_current_page(key) + 1, doc.page_count(), reader::zoom_label(zoom))
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

    /// A pane that fits a US Letter page's width at 100%, with the gaps.
    const PANE: (f32, f32) = (636.0, 400.0);
    /// How far down the column each page starts: its height and a gap.
    const STRIDE: f32 = 792.0 + 12.0;

    /// An app with `path` open as a PDF, as if the worker had answered
    /// with 10 US Letter pages, drawn once in a `PANE`-sized pane. Returns
    /// the guard first so the app (and its worker) is dropped before the
    /// lock is released.
    fn test_open_pdf(path: &str) -> (std::sync::MutexGuard<'static, ()>, ViewKey, App) {
        let guard = PDF_WORKER_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = App::with_file(None);
        app.open_pdf_path(Path::new(path));
        let key = app.pdf_view_in_pane(app.focused_pane_id()).expect("the PDF is in the focused pane");
        app.pdf_docs.get_mut(&key.1).unwrap().pages = vec![(612.0, 792.0); 10];
        app.pdf_prepare_view(key, PANE);
        (guard, key, app)
    }

    fn view(app: &App, key: ViewKey) -> &PdfView {
        app.pdf_views.get(&key).expect("the view")
    }

    fn page(app: &App, key: ViewKey) -> u32 {
        app.pdf_current_page(key)
    }

    fn keys(app: &mut App, text: &str) {
        for c in text.chars() {
            assert!(app.reader_key(KeyPress::char(c)), "{c}");
        }
    }

    /// The worker's answer to request `request_id` for `page`.
    fn rendered(app: &mut App, request_id: u64, page: u32, w: u32) {
        let doc_key = app.pdf_docs.values().next().unwrap().doc_key;
        app.apply_pdf_response(fenix_pdf::PdfResponse::PageRendered {
            key: doc_key,
            request_id,
            page_index: page,
            width: w,
            height: 10,
            bgra: vec![0u8; w as usize * 10 * 4],
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
        app.pdf_prepare_view(right, PANE);

        app.pdf_goto_page(7);

        assert_eq!(page(&app, right), 6);
        assert_eq!(page(&app, left), 0, "the other view stays where it was");
        assert_ne!(view(&app, left).id, view(&app, right).id, "each view gets its own renders");
        assert_eq!(app.pdf_docs.len(), 1);
    }

    #[test]
    fn a_new_view_starts_where_the_last_one_left_off() {
        let (_guard, key, mut app) = test_open_pdf("pdf_last_place.pdf");
        app.pdf_goto_page(5);
        app.pdf_zoom_fit_page();
        let pane = app.windows_mut().split(SplitKind::Vertical, key.1);
        app.pdf_prepare_view((pane, key.1), PANE);
        assert_eq!(page(&app, (pane, key.1)), 4);
        assert_eq!(view(&app, (pane, key.1)).zoom, Zoom::FitPage);
    }

    #[test]
    fn killing_the_buffer_closes_the_document_and_its_views() {
        let (_guard, key, mut app) = test_open_pdf("pdf_kill.pdf");
        app.kill_buffer_now();
        assert!(app.pdf_docs.is_empty() && app.pdf_views.is_empty() && app.pdf_requests.is_empty());
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
        assert!(!app.pdf_views.contains_key(&key), "its textures belonged to that frame");
        assert!(app.pdf_docs.contains_key(&key.1), "the buffer is still open");
    }

    #[test]
    fn spc_r_acts_on_the_document_read_last_from_another_pane() {
        let (_guard, key, mut app) = test_open_pdf("pdf_last_read.pdf");
        let scratch = app.buffers.open_scratch();
        let code = app.windows_mut().split(SplitKind::Vertical, scratch);
        app.windows_mut().focus(code);
        app.pdf_next_page();
        assert_eq!(page(&app, key), 1);
    }

    // -- One column of pages ----------------------------------------------

    #[test]
    fn a_view_asks_for_the_pages_in_sight_and_a_few_ahead() {
        let (_guard, key, app) = test_open_pdf("pdf_asks.pdf");
        let mut asked: Vec<u32> = view(&app, key).pending.keys().copied().collect();
        asked.sort();
        assert_eq!(asked, vec![0, 1, 2], "page 1 in sight, two ahead, none behind the first");
        assert_eq!(app.pdf_requests.len(), 3);
        assert!(view(&app, key).pending.values().all(|&(_, w)| w == 612), "at the width fit width gives them");
    }

    #[test]
    fn a_rendered_page_goes_into_the_view_that_asked_and_a_superseded_one_is_dropped() {
        let (_guard, key, mut app) = test_open_pdf("pdf_rendered.pdf");
        let (first, _) = view(&app, key).pending[&0];
        app.pdf_set_zoom(Zoom::Percent(50));
        app.pdf_prepare_view(key, PANE);
        let (second, w) = view(&app, key).pending[&0];
        assert_ne!(first, second, "a new size asks again");

        rendered(&mut app, first, 0, 612);
        assert!(!view(&app, key).cache.contains_key(&0), "the old size's answer is dropped");
        rendered(&mut app, second, 0, w);
        assert_eq!(view(&app, key).cache[&0].w, w);
        assert!(!view(&app, key).pending.contains_key(&0));
    }

    #[test]
    fn scrolling_away_cancels_what_is_no_longer_needed_and_lets_far_pages_go() {
        let (_guard, key, mut app) = test_open_pdf("pdf_cancel.pdf");
        let (request, _) = view(&app, key).pending[&0];
        rendered(&mut app, request, 0, 612);
        assert!(view(&app, key).cache.contains_key(&0));

        app.pdf_goto_page(10);
        app.pdf_prepare_view(key, PANE);

        assert!(!view(&app, key).pending.contains_key(&1), "page 2 isn't needed at the end");
        assert!(view(&app, key).pending.contains_key(&9));
        assert!(app.pdf_requests.values().all(|&(_, p)| p >= 7), "only the end and one behind: {:?}", app.pdf_requests);
        assert!(view(&app, key).cache.contains_key(&0), "8 pages away is still kept");
        app.pdf_docs.get_mut(&key.1).unwrap().pages = vec![(612.0, 792.0); 30];
        app.pdf_goto_page(30);
        app.pdf_prepare_view(key, PANE);
        assert!(!view(&app, key).cache.contains_key(&0), "far away is let go");
    }

    #[test]
    fn the_draw_plan_cuts_pages_to_the_pane() {
        let (_guard, key, mut app) = test_open_pdf("pdf_plan.pdf");
        app.pdf_scroll(700);
        let rect = fenix_window::Rect { x: 100.0, y: 50.0, w: PANE.0, h: PANE.1 };
        let plan = app.pdf_draw_plan(key, rect);
        assert_eq!(plan.iter().map(|d| d.page).collect::<Vec<_>>(), vec![0, 1]);
        let first = plan[0];
        assert_eq!(first.dest, (112.0, 50.0, 612.0, 12.0 + 792.0 - 700.0));
        assert!((first.uv.1 - 688.0 / 792.0).abs() < 1e-4, "the bottom of page 1");
        assert_eq!(first.uv.3, 1.0);
        let second = plan[1];
        assert_eq!(second.dest.1, 50.0 + STRIDE + 12.0 - 700.0);
        assert!(second.dest.1 + second.dest.3 <= rect.y + rect.h, "cut at the pane's bottom");
    }

    #[test]
    fn scrolling_goes_straight_through_page_edges() {
        let (_guard, key, mut app) = test_open_pdf("pdf_scroll.pdf");
        keys(&mut app, "j");
        assert_eq!(view(&app, key).scroll.1, PAN_STEP_PX as f32);
        keys(&mut app, "2j");
        assert_eq!(view(&app, key).scroll.1, 3.0 * PAN_STEP_PX as f32);
        app.pdf_scroll(900);
        assert_eq!(page(&app, key), 1, "on into page 2 without a page turn");
        app.pdf_scroll(-100_000);
        assert_eq!(view(&app, key).scroll.1, 0.0, "stops at the top");
        app.pdf_scroll(100_000);
        assert_eq!(page(&app, key), 9, "and at the end");
    }

    #[test]
    fn half_and_whole_screens() {
        let (_guard, key, mut app) = test_open_pdf("pdf_screens.pdf");
        app.reader_key(KeyPress::char('d').with_ctrl());
        assert_eq!(view(&app, key).scroll.1, 200.0);
        app.reader_key(KeyPress::char('f').with_ctrl());
        assert_eq!(view(&app, key).scroll.1, 200.0 + 400.0 - PAN_STEP_PX as f32, "a screen, less a step to keep your place");
    }

    #[test]
    fn a_zoom_keeps_the_same_place_at_the_top() {
        let (_guard, key, mut app) = test_open_pdf("pdf_zoom_anchor.pdf");
        app.pdf_goto_page(6);
        app.pdf_scroll(300);
        let before = view(&app, key).layout.anchor(view(&app, key).scroll.1);
        keys(&mut app, "+");
        let after = view(&app, key).layout.anchor(view(&app, key).scroll.1);
        assert_eq!(view(&app, key).zoom, Zoom::Percent(110));
        assert_eq!(before.0, after.0);
        assert!((before.1 - after.1).abs() < 0.01, "{before:?} {after:?}");
    }

    // -- Keys -------------------------------------------------------------

    #[test]
    fn counts_and_vim_page_keys() {
        let (_guard, key, mut app) = test_open_pdf("pdf_keys_pages.pdf");
        keys(&mut app, "3J");
        assert_eq!(page(&app, key), 3);
        assert_eq!(view(&app, key).scroll.1, 3.0 * STRIDE, "at the page's top");
        keys(&mut app, "K");
        assert_eq!(page(&app, key), 2);
        keys(&mut app, "G");
        assert_eq!(page(&app, key), 9);
        keys(&mut app, "gg");
        assert_eq!(page(&app, key), 0);
        keys(&mut app, "7G");
        assert_eq!(page(&app, key), 6);
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
    fn zoom_steps_from_where_the_view_is_and_stops_at_the_ends() {
        let (_guard, key, mut app) = test_open_pdf("pdf_zoom.pdf");
        app.pdf_zoom_in();
        assert_eq!(view(&app, key).zoom, Zoom::Percent(110), "fit width is 100% here");
        app.pdf_set_zoom(Zoom::Percent(400));
        app.pdf_zoom_in();
        assert_eq!(view(&app, key).zoom, Zoom::Percent(400));
        app.pdf_set_zoom(Zoom::Percent(25));
        app.pdf_zoom_out();
        assert_eq!(view(&app, key).zoom, Zoom::Percent(25));
    }

    #[test]
    fn panning_works_when_the_page_is_wider_than_the_pane() {
        let (_guard, key, mut app) = test_open_pdf("pdf_pan.pdf");
        keys(&mut app, "l");
        assert_eq!(view(&app, key).scroll.0, 0.0, "fit width: nothing to pan");
        app.pdf_set_zoom(Zoom::Percent(200));
        let x = view(&app, key).scroll.0;
        keys(&mut app, "l");
        assert_eq!(view(&app, key).scroll.0, x + PAN_STEP_PX as f32);
        app.pdf_pan(-100, 0);
        assert_eq!(view(&app, key).scroll.0, 0.0);
    }

    #[test]
    fn n_and_capital_n_go_through_the_pages_with_matches() {
        let (_guard, key, mut app) = test_open_pdf("pdf_keys_matches.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().match_pages = vec![2, 5, 8];
        keys(&mut app, "n");
        assert_eq!(page(&app, key), 2);
        keys(&mut app, "n");
        assert_eq!(page(&app, key), 5);
        keys(&mut app, "N");
        assert_eq!(page(&app, key), 2);
        keys(&mut app, "N");
        assert_eq!(page(&app, key), 8, "round the start to the last");
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

    #[test]
    fn the_go_to_page_prompt_takes_digits_and_esc_cancels() {
        let (_guard, key, mut app) = test_open_pdf("pdf_goto_prompt.pdf");
        app.start_pdf_goto_page_prompt();
        for c in "7x".chars() {
            app.pdf_goto_page_prompt_key(KeyPress::char(c));
        }
        assert_eq!(app.pdf_goto_page_prompt_text(), Some("go to page: 7".to_string()));
        app.pdf_goto_page_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!(page(&app, key), 6);

        app.start_pdf_goto_page_prompt();
        app.pdf_goto_page_prompt_key(KeyPress::char('2'));
        app.pdf_goto_page_prompt_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.pdf_goto_page_prompt.is_none());
        assert_eq!(page(&app, key), 6);
    }

    #[test]
    fn prompts_need_a_pdf() {
        let _guard = PDF_WORKER_TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = App::with_file(None);
        app.start_pdf_goto_page_prompt();
        app.start_pdf_search_prompt();
        assert!(app.pdf_goto_page_prompt.is_none() && app.pdf_search_prompt.is_none());
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
        assert_eq!(page(&app, key), 6);
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
        assert_eq!(page(&app, key), 6);
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
        assert_eq!(page(&app, key), 0);
    }
}
