//! The host half of the PDF reader (`reader` and `reader_sidebar` are
//! the pure halves): the open documents, each pane's view of one, the
//! pdfium worker's replies, the reader's keys, its sidebar, search and
//! marks.
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
//! back in its page cache, and drops what's scrolled far away. Its
//! sidebar -- outline, matches, marks -- sits down the pane's left edge.

use std::collections::BTreeMap;

use super::*;
use crate::reader::{self, Cmd, Layout, Outcome, Zoom};
use crate::reader_sidebar::{self, MatchRow, SideKey, Sidebar, Target};

/// How far `j`/`k`/`h`/`l` move, in pixels.
pub(super) const PAN_STEP_PX: u32 = 60;

/// Pages rendered ahead of the pane and behind it.
const AHEAD: usize = 2;
const BEHIND: usize = 1;

/// Rendered pages further than this from the pane are let go.
const KEEP: usize = 8;

/// A page not rendered yet: blank paper.
pub(super) const PAPER: [f32; 4] = [0.91, 0.905, 0.89, 1.0];

/// A search's matches on the page, and the one `n` last went to.
const MATCH_TINT: [f32; 4] = [1.0, 0.71, 0.28, 0.38];
const CURRENT_MATCH_TINT: [f32; 4] = [1.0, 0.42, 0.24, 0.5];

/// A view: the pane showing it, and the document's buffer.
pub(super) type ViewKey = (fenix_window::WindowId, BufferId);

/// A place in a document: a page, and how far down it (0..1).
pub(super) type Place = (u32, f32);

/// What a fetched outline is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OutlineFor {
    Sidebar,
    Picker,
}

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
    /// The bookmark tree, fetched the first time it's asked for.
    pub(super) outline: Option<Vec<fenix_pdf::outline::OutlineEntry>>,
    /// Asked for and not back yet, and what for.
    pub(super) outline_for: Option<OutlineFor>,
    /// The search under way or last made.
    pub(super) search_id: u64,
    pub(super) query: String,
    /// Its matches so far, in page order.
    pub(super) matches: Vec<fenix_pdf::search::PdfSearchMatch>,
    pub(super) search_done: bool,
    /// The match `n` last went to.
    pub(super) current_match: Option<usize>,
    /// Whether the matches are highlighted (`Esc` hides them).
    pub(super) highlights: bool,
    /// `Enter` in the search prompt before the first match came: go to
    /// the first one at or after here when it does.
    pub(super) jump_when_found: Option<u32>,
    /// `m{a}`.
    pub(super) marks: BTreeMap<char, Place>,
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
    /// The pages' part of the pane (the pane less a sidebar beside them)
    /// in pixels, and the window's scale factor, from the last frame
    /// drawn; `(0, 0)` until then.
    pub(super) pane: (f32, f32),
    pub(super) dpi: f32,
    /// The document laid out for `zoom`, `pane` and `dpi`.
    pub(super) layout: Layout,
    layout_for: Option<(Zoom, (f32, f32), f32, usize)>,
    /// Top-left of what's shown, in the layout's pixels.
    pub(super) scroll: (f32, f32),
    /// A place to show at the pane's top once the layout is known --
    /// where a new view starts, or a jump made before its pane was first
    /// drawn.
    pub(super) pending_jump: Option<Place>,
    pub(super) cache: HashMap<u32, CachedPage>,
    /// Renders asked for and not back yet: page -> (request, width).
    pub(super) pending: HashMap<u32, (u64, u32)>,
    /// The pages last asked to be kept, so a scroll that changes them
    /// tells the worker.
    wanted: Vec<u32>,
    /// The sidebar, when open.
    pub(super) sidebar: Option<Sidebar>,
    /// Its width in pixels, and whether it's over the pages (a narrow
    /// pane) rather than beside them.
    pub(super) side_px: f32,
    pub(super) side_over: bool,
    /// Where jumps came from and, after `Ctrl-o`, went to.
    back: Vec<Place>,
    forward: Vec<Place>,
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

/// What a PDF pane draws besides its pages: the sidebar's text and rects,
/// and the search's highlights.
#[derive(Default)]
pub(super) struct PdfChrome {
    pub(super) spans: RowSpans,
    /// Under the text: the sidebar's background and its selected row.
    pub(super) rects: Vec<((f32, f32, f32, f32), [f32; 4])>,
    /// Over the pages.
    pub(super) highlights: Vec<((f32, f32, f32, f32), [f32; 4])>,
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
                outline_for: None,
                search_id: 0,
                query: String::new(),
                matches: Vec::new(),
                search_done: true,
                current_match: None,
                highlights: false,
                jump_when_found: None,
                marks: BTreeMap::new(),
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
                    pending_jump: Some((page, 0.0)),
                    cache: HashMap::new(),
                    pending: HashMap::new(),
                    wanted: Vec::new(),
                    sidebar: None,
                    side_px: 0.0,
                    side_over: false,
                    back: Vec::new(),
                    forward: Vec::new(),
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
    /// shows it here -- so `SPC r n` from the code beside a document turns
    /// its page.
    pub(super) fn pdf_target(&mut self) -> Option<ViewKey> {
        let key = self.pdf_view_in_pane(self.focused_pane_id()).or_else(|| {
            let (pane, buffer) = self.pdf_last_view?;
            (self.windows().content(pane) == Some(&buffer)).then_some((pane, buffer))
        })?;
        self.pdf_ensure_view(key)?;
        self.pdf_last_view = Some(key);
        Some(key)
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
        if let Some((page, frac)) = view.pending_jump.take() {
            let page = (page as usize).min(pages.len() - 1);
            view.scroll.1 = if frac > 0.0 { view.layout.scroll_for((page, frac)) } else { view.layout.top_of(page) };
        }
        let (max_x, max_y) = view.layout.max_scroll(view.pane);
        view.scroll = (view.scroll.0.clamp(0.0, max_x), view.scroll.1.clamp(0.0, max_y));
    }

    /// Everything a frame does for a view before drawing it: note its
    /// pane's size (less a sidebar beside the pages), lay it out, ask for
    /// the pages it needs (the ones in sight first), cancel what it no
    /// longer needs and let go of pages far away. `cell` is the text's
    /// `(char width, line height)`, for the sidebar. Also what a test
    /// calls in place of drawing.
    pub(super) fn pdf_prepare_view(&mut self, key: ViewKey, pane: (f32, f32), cell: (f32, f32)) {
        let dpi = self.frame_scale();
        let Some(view) = self.pdf_ensure_view(key) else { return };
        let side_px = if view.sidebar.is_some() { (reader_sidebar::COLS as f32 * cell.0 + text::PAD_LEFT * 2.0).round() } else { 0.0 };
        view.side_px = side_px;
        view.side_over = side_px > 0.0 && pane.0 < reader_sidebar::BESIDE_MIN_COLS as f32 * cell.0;
        view.pane = (if view.side_over { pane.0 } else { (pane.0 - side_px).max(1.0) }, pane.1);
        view.dpi = dpi;
        self.pdf_relayout(key);
        self.pdf_settle_sidebar(key, ((pane.1 - text::PAD_TOP) / cell.1.max(1.0)).floor().max(2.0) as usize - 1);
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

    /// Where `key`'s pages start in a pane at `rect` (right of a sidebar
    /// beside them), and the part of the pane they may be drawn in.
    fn pdf_page_area(&self, key: ViewKey, rect: fenix_window::Rect) -> Option<(f32, fenix_window::Rect)> {
        let view = self.pdf_views.get(&key)?;
        let origin = if view.side_over { rect.x } else { rect.x + view.side_px };
        let clip = fenix_window::Rect { x: rect.x + view.side_px, y: rect.y, w: (rect.w - view.side_px).max(0.0), h: rect.h };
        Some((origin, clip))
    }

    /// The pages `key` shows in a pane at `rect`, clipped to where pages
    /// go.
    pub(super) fn pdf_draw_plan(&self, key: ViewKey, rect: fenix_window::Rect) -> Vec<PageDraw> {
        let (Some(view), Some((origin, clip))) = (self.pdf_views.get(&key), self.pdf_page_area(key, rect)) else { return Vec::new() };
        let mut draws = Vec::new();
        for page in view.layout.visible(view.scroll.1, rect.h) {
            let p = view.layout.pages[page];
            let (dx, dy) = (origin + p.x - view.scroll.0, rect.y + p.y - view.scroll.1);
            let (x0, y0) = (dx.max(clip.x), dy.max(clip.y));
            let (x1, y1) = ((dx + p.w).min(clip.x + clip.w), (dy + p.h).min(clip.y + clip.h));
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
            Some(view) => view.pending_jump.map(|(p, _)| p).unwrap_or(0),
            None => self.pdf_docs.get(&key.1).map(|doc| doc.last_place.0).unwrap_or(0),
        }
    }

    /// Where `key`'s pane top is, as a place to come back to.
    fn pdf_place(&self, key: ViewKey) -> Option<Place> {
        let view = self.pdf_views.get(&key)?;
        if let Some(place) = view.pending_jump {
            return Some(place);
        }
        (!view.layout.pages.is_empty()).then(|| {
            let (page, frac) = view.layout.anchor(view.scroll.1);
            (page as u32, frac)
        })
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
    /// closed, or for a render or search since superseded, is dropped.
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
                let Some(doc) = self.pdf_docs.get_mut(&buffer) else { return };
                let empty = entries.is_empty();
                doc.outline = Some(entries);
                match doc.outline_for.take() {
                    Some(OutlineFor::Picker) => self.pdf_open_heading_picker(buffer),
                    Some(OutlineFor::Sidebar) if empty => self.set_message("this PDF has no bookmarks"),
                    _ => {}
                }
                // An open sidebar goes to the section being read.
                let views: Vec<ViewKey> = self.pdf_views.keys().filter(|(_, b)| *b == buffer).copied().collect();
                for view_key in views {
                    self.pdf_sidebar_to_here(view_key);
                }
            }
            fenix_pdf::PdfResponse::SearchResults { key, request_id, matches, done } => {
                let Some(buffer) = self.pdf_doc_by_key(key) else { return };
                let Some(doc) = self.pdf_docs.get_mut(&buffer) else { return };
                if request_id != doc.search_id {
                    return;
                }
                doc.matches.extend(matches);
                doc.search_done = done;
                let pending_jump = doc.jump_when_found.and_then(|from| doc.matches.iter().position(|m| m.page_index >= from).map(|at| (from, at)));
                if done && doc.jump_when_found.is_some() && pending_jump.is_none() && !doc.matches.is_empty() {
                    doc.jump_when_found = None;
                    self.pdf_go_to_match(buffer, 0);
                } else if let Some((_, at)) = pending_jump {
                    doc.jump_when_found = None;
                    self.pdf_go_to_match(buffer, at);
                }
                if done {
                    let (count, query) = self.pdf_docs.get(&buffer).map(|d| (d.matches.len(), d.query.clone())).unwrap_or_default();
                    if self.pdf_search_prompt.is_none() {
                        if count == 0 {
                            self.set_message(format!("no matches for \"{query}\""));
                        } else {
                            self.set_message(format!("{count} match{} for \"{query}\" -- n and N go through them", if count == 1 { "" } else { "es" }));
                        }
                    }
                }
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
    /// of it and its views go with it. The buffer itself is the caller's
    /// to close.
    pub(super) fn pdf_close_doc(&mut self, buffer: BufferId) {
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
    /// doesn't want -- the leader, `:`, `Ctrl-w` -- goes on to the
    /// editor. With the sidebar focused, its keys come first.
    pub(super) fn reader_key(&mut self, keypress: KeyPress) -> bool {
        let Some(key) = self.pdf_view_in_pane(self.focused_pane_id()) else {
            self.pdf_keys.reset();
            return false;
        };
        if self.pdf_keys_view != Some(key) {
            self.pdf_keys.reset();
            self.pdf_keys_view = Some(key);
        }
        self.pdf_last_view = Some(key);
        if self.pdf_views.get(&key).and_then(|v| v.sidebar.as_ref()).is_some_and(|s| s.focused) {
            return self.pdf_sidebar_key(key, keypress);
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
            Cmd::SetMark(c) => self.pdf_set_mark(c),
            Cmd::GotoMark(c) => self.pdf_goto_mark(c),
            Cmd::Escape => self.pdf_escape(),
        }
        self.wake_caret();
    }

    /// What a count or a `g`/`z` typed so far reads as, for the modeline.
    pub(super) fn pdf_pending_keys(&self) -> Option<String> {
        self.pdf_keys.is_pending().then(|| self.pdf_keys.pending_text())
    }

    /// `Esc`: hides the search's highlights, else closes the sidebar.
    fn pdf_escape(&mut self) {
        let Some(key) = self.pdf_target() else { return };
        if let Some(doc) = self.pdf_docs.get_mut(&key.1).filter(|doc| doc.highlights) {
            doc.highlights = false;
            return;
        }
        if let Some(view) = self.pdf_views.get_mut(&key) {
            view.sidebar = None;
        }
    }

    // -- Moving -----------------------------------------------------------

    /// `J`/`K`, `SPC r n`/`SPC r p`: `delta` pages on from the one being
    /// read, at its top.
    pub(super) fn pdf_turn_page(&mut self, delta: i32) {
        let Some(key) = self.pdf_target() else { return };
        let current = self.pdf_current_page(key) as i64;
        self.pdf_show(key, ((current + delta as i64).max(0) as u32, 0.0));
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
    /// A jump: `Ctrl-o` comes back.
    pub(crate) fn pdf_goto_page(&mut self, page_number: u32) {
        let Some(key) = self.pdf_target() else { return };
        self.pdf_jump(key, (page_number.saturating_sub(1), 0.0));
    }

    /// Shows `place` at the top of `key`'s pane (a page past the end is
    /// the last). Before the view is laid out, it goes there once it is.
    fn pdf_show(&mut self, key: ViewKey, place: Place) {
        let count = self.pdf_docs.get(&key.1).map(|doc| doc.page_count()).unwrap_or(0);
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        let page = if count > 0 { place.0.min(count - 1) } else { place.0 };
        view.pending_jump = Some((page, place.1));
        self.pdf_relayout(key);
        if let Some(doc) = self.pdf_docs.get_mut(&key.1) {
            doc.last_place.0 = page;
        }
        self.wake_caret();
    }

    /// `pdf_show`, remembering where it came from for `Ctrl-o`.
    fn pdf_jump(&mut self, key: ViewKey, place: Place) {
        if let Some(from) = self.pdf_place(key) {
            if let Some(view) = self.pdf_views.get_mut(&key) {
                view.back.push(from);
                view.forward.clear();
            }
        }
        self.pdf_show(key, place);
    }

    /// `Ctrl-o` (`back`) and `Ctrl-i` in a PDF: to where the last jump
    /// came from, or back again. `false` when there's nowhere to go, so
    /// the editor's own jump list takes the key.
    pub(super) fn pdf_jump_history(&mut self, back: bool) -> bool {
        let Some(key) = self.pdf_view_in_pane(self.focused_pane_id()) else { return false };
        let here = self.pdf_place(key);
        let Some(view) = self.pdf_views.get_mut(&key) else { return false };
        let (from, to) = if back { (&mut view.back, &mut view.forward) } else { (&mut view.forward, &mut view.back) };
        let Some(place) = from.pop() else { return false };
        if let Some(here) = here {
            to.push(here);
        }
        self.pdf_show(key, place);
        true
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

    // -- Marks ------------------------------------------------------------

    fn pdf_set_mark(&mut self, c: char) {
        let Some(key) = self.pdf_target() else { return };
        let Some(place) = self.pdf_place(key) else { return };
        if let Some(doc) = self.pdf_docs.get_mut(&key.1) {
            doc.marks.insert(c, place);
        }
        self.set_message(format!("mark {c} on page {}", place.0 + 1));
    }

    fn pdf_goto_mark(&mut self, c: char) {
        let Some(key) = self.pdf_target() else { return };
        match self.pdf_docs.get(&key.1).and_then(|doc| doc.marks.get(&c).copied()) {
            Some(place) => self.pdf_jump(key, place),
            None => self.set_message(format!("no mark {c} in this document -- m{c} sets one")),
        }
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

    /// The modeline's page, zoom and search for the focused pane's
    /// document.
    pub(super) fn pdf_modeline_position(&self) -> Option<String> {
        let key = self.pdf_view_in_pane(self.focused_pane_id())?;
        let doc = self.pdf_docs.get(&key.1)?;
        let zoom = self.pdf_views.get(&key).map(|view| view.zoom).unwrap_or(doc.last_place.1);
        let pending = self.pdf_pending_keys().map(|keys| format!("{keys}   ")).unwrap_or_default();
        let search = match (doc.highlights, doc.current_match) {
            (true, Some(at)) => format!("   /{} {}/{}", doc.query, at + 1, doc.matches.len()),
            (true, None) if !doc.query.is_empty() => format!("   /{} {}", doc.query, doc.matches.len()),
            _ => String::new(),
        };
        Some(if doc.pages.is_empty() {
            format!("{pending}Opening...")
        } else {
            format!("{pending}Page {}/{}   {}{search}", self.pdf_current_page(key) + 1, doc.page_count(), reader::zoom_label(zoom))
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

    // -- Sidebar ----------------------------------------------------------

    /// `o`, `SPC r o`: opens the sidebar on the outline with the keyboard
    /// in it, or closes it.
    pub(crate) fn pdf_toggle_outline(&mut self) {
        let Some(key) = self.pdf_target() else { return };
        let Some(view) = self.pdf_views.get_mut(&key) else { return };
        if view.sidebar.is_some() {
            view.sidebar = None;
            return;
        }
        view.sidebar = Some(Sidebar::open(reader_sidebar::Tab::Outline));
        self.pdf_want_outline(key.1, OutlineFor::Sidebar);
        self.pdf_sidebar_to_here(key);
    }

    /// Asks the worker for `doc`'s outline if it hasn't come yet.
    fn pdf_want_outline(&mut self, doc: BufferId, what: OutlineFor) {
        let Some(d) = self.pdf_docs.get_mut(&doc) else { return };
        if d.outline.is_some() {
            return;
        }
        let asked = d.outline_for.is_some();
        if !asked || what == OutlineFor::Picker {
            d.outline_for = Some(what);
        }
        if !asked {
            if let Some(worker) = &self.pdf_worker {
                worker.send(fenix_pdf::PdfRequest::FetchOutline { key: d.doc_key });
            }
        }
    }

    /// The rows `key`'s sidebar shows now.
    fn pdf_sidebar_rows(&self, key: ViewKey) -> Vec<reader_sidebar::Row> {
        let (Some(view), Some(doc)) = (self.pdf_views.get(&key), self.pdf_docs.get(&key.1)) else { return Vec::new() };
        let Some(side) = &view.sidebar else { return Vec::new() };
        let outline = doc.outline.as_deref().unwrap_or(&[]);
        let matches: Vec<MatchRow> = doc.matches.iter().map(|m| MatchRow { page: m.page_index, context: m.context.clone() }).collect();
        let marks: Vec<(char, u32)> = doc.marks.iter().map(|(&c, &(page, _))| (c, page)).collect();
        side.rows(outline, &matches, &marks, self.pdf_current_page(key), doc.current_match)
    }

    /// Puts the outline's cursor on the section being read.
    fn pdf_sidebar_to_here(&mut self, key: ViewKey) {
        let rows = self.pdf_sidebar_rows(key);
        if let Some(side) = self.pdf_views.get_mut(&key).and_then(|v| v.sidebar.as_mut()) {
            side.to_here(&rows);
        }
    }

    fn pdf_settle_sidebar(&mut self, key: ViewKey, height: usize) {
        let count = self.pdf_sidebar_rows(key).len();
        if let Some(side) = self.pdf_views.get_mut(&key).and_then(|v| v.sidebar.as_mut()) {
            side.settle(count, height);
        }
    }

    /// A key while the sidebar has the keyboard.
    fn pdf_sidebar_key(&mut self, key: ViewKey, keypress: KeyPress) -> bool {
        let rows = self.pdf_sidebar_rows(key);
        let outline = self.pdf_docs.get(&key.1).and_then(|d| d.outline.clone()).unwrap_or_default();
        let Some(side) = self.pdf_views.get_mut(&key).and_then(|v| v.sidebar.as_mut()) else { return false };
        let result = side.key(keypress, &rows, &outline);
        match result {
            SideKey::Taken => {}
            SideKey::Close => {
                if let Some(view) = self.pdf_views.get_mut(&key) {
                    view.sidebar = None;
                }
            }
            SideKey::Go(target) => {
                side.focused = false;
                match target {
                    Target::Page(page) => self.pdf_jump(key, (page, 0.0)),
                    Target::Match(at) => self.pdf_go_to_match(key.1, at),
                    Target::Mark(c) => self.pdf_goto_mark(c),
                }
            }
            SideKey::Pass => return false,
        }
        self.wake_caret();
        true
    }

    /// What `key`'s pane at `rect` draws besides its pages: the sidebar
    /// (text, background, selected row) and the search's highlights.
    pub(super) fn pdf_chrome(&self, key: ViewKey, rect: fenix_window::Rect, cell: (f32, f32)) -> PdfChrome {
        let theme = self.theme;
        let mut chrome = PdfChrome::default();
        let (Some(view), Some(doc)) = (self.pdf_views.get(&key), self.pdf_docs.get(&key.1)) else { return chrome };

        if doc.highlights {
            if let (Some((origin, clip)), false) = (self.pdf_page_area(key, rect), view.layout.pages.is_empty()) {
                let visible = view.layout.visible(view.scroll.1, rect.h);
                let scale = view.layout.scale;
                for (i, m) in doc.matches.iter().enumerate().filter(|(_, m)| visible.contains(&(m.page_index as usize))) {
                    let p = view.layout.pages[m.page_index as usize];
                    let tint = if doc.current_match == Some(i) { CURRENT_MATCH_TINT } else { MATCH_TINT };
                    for r in &m.rects {
                        let x0 = (origin + p.x - view.scroll.0 + r[0] * scale).max(clip.x);
                        let y0 = (rect.y + p.y - view.scroll.1 + r[1] * scale).max(clip.y);
                        let x1 = (origin + p.x - view.scroll.0 + r[2] * scale).min(clip.x + clip.w);
                        let y1 = (rect.y + p.y - view.scroll.1 + r[3] * scale).min(clip.y + clip.h);
                        if x1 > x0 && y1 > y0 {
                            chrome.highlights.push(((x0, y0, x1 - x0, y1 - y0), tint));
                        }
                    }
                }
            }
        }

        let Some(side) = &view.sidebar else { return chrome };
        let (char_w, line_h) = cell;
        chrome.rects.push(((rect.x, rect.y, view.side_px, rect.h), theme.sidebar_bg));
        chrome.rects.push(((rect.x + view.side_px - 1.0, rect.y, 1.0, rect.h), theme.divider));
        let rows = self.pdf_sidebar_rows(key);
        let height = ((rect.h - text::PAD_TOP) / line_h.max(1.0)).floor().max(2.0) as usize - 1;
        let row_y = |row: usize| rect.y + text::PAD_TOP + row as f32 * line_h;
        let header = side.header(doc.matches.len(), doc.marks.len(), !doc.search_done);
        chrome.spans.push((format!("{header:.width$}", width = reader_sidebar::COLS), theme.caret_text, false));
        if rows.is_empty() {
            let empty = match (side.tab(), &doc.outline) {
                (reader_sidebar::Tab::Outline, None) => "loading the outline\u{2026}",
                (reader_sidebar::Tab::Outline, Some(_)) => "no bookmarks -- Tab for matches",
                (reader_sidebar::Tab::Matches, _) => "no matches -- / searches",
                (reader_sidebar::Tab::Marks, _) => "no marks -- m{a} sets one",
            };
            chrome.spans.push(("\n".to_string(), theme.fg, false));
            chrome.spans.push((empty.to_string(), theme.gutter_fg, false));
        }
        for (i, row) in rows.iter().enumerate().skip(side.scroll).take(height) {
            let shown = i - side.scroll + 1;
            chrome.spans.push(("\n".to_string(), theme.fg, false));
            let color = if row.here { theme.caret_text } else { theme.fg };
            chrome.spans.push((reader_sidebar::fit(row, reader_sidebar::COLS), color, false));
            if i == side.cursor {
                let tint = if side.focused { theme.selection } else { [theme.selection[0], theme.selection[1], theme.selection[2], theme.selection[3] * 0.45] };
                chrome.rects.push(((rect.x, row_y(shown), view.side_px - 1.0, line_h), tint));
            }
            if row.here {
                chrome.rects.push(((rect.x, row_y(shown), 2.0, line_h), theme.mode_normal));
            }
        }
        let _ = char_w;
        chrome
    }

    // -- Headings picker --------------------------------------------------

    /// `SPC r t`: a fuzzy picker over the document's headings.
    pub(crate) fn pdf_pick_heading(&mut self) {
        let Some((_, doc)) = self.pdf_target() else { return };
        if self.pdf_docs.get(&doc).is_some_and(|d| d.outline.is_some()) {
            self.pdf_open_heading_picker(doc);
        } else {
            self.pdf_want_outline(doc, OutlineFor::Picker);
        }
    }

    fn pdf_open_heading_picker(&mut self, doc: BufferId) {
        let entries = self.pdf_docs.get(&doc).and_then(|d| d.outline.clone()).unwrap_or_default();
        if entries.is_empty() {
            self.set_message("this PDF has no bookmarks");
            return;
        }
        let candidates = entries
            .iter()
            .map(|e| fenix_picker::Candidate::new(format!("{}{}   p.{}", "  ".repeat(e.depth as usize), e.title, e.page_index + 1), e.page_index))
            .collect();
        self.enter_picker(ActivePicker::PdfHeading(fenix_picker::PickerState::new(candidates)));
    }

    /// The heading picked: the document read to its page.
    pub(super) fn pdf_heading_picked(&mut self, page: u32) {
        let Some(key) = self.pdf_target() else { return };
        self.pdf_jump(key, (page, 0.0));
    }

    // -- Search -----------------------------------------------------------

    /// `/`, `SPC r /`: asks what to search for; the matches light up as
    /// you type.
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
            KeyCode::Named(FenixNamedKey::Escape) => {
                self.pdf_search_prompt = None;
                if let Some((_, doc)) = self.pdf_target() {
                    if let Some(d) = self.pdf_docs.get_mut(&doc) {
                        d.highlights = false;
                    }
                }
            }
            KeyCode::Named(FenixNamedKey::Enter) => {
                self.pdf_search_prompt = None;
                self.pdf_accept_search();
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
                let query = input.clone();
                self.pdf_dispatch_search(query);
            }
            KeyCode::Char(c) if key.mods == Mods::default() => {
                input.push(c);
                let query = input.clone();
                self.pdf_dispatch_search(query);
            }
            _ => {}
        }
        self.wake_caret();
    }

    pub(super) fn pdf_search_prompt_text(&self) -> Option<String> {
        let input = self.pdf_search_prompt.as_ref()?;
        let count = self.pdf_last_view.and_then(|(_, doc)| self.pdf_docs.get(&doc)).filter(|_| !input.is_empty()).map(|d| {
            format!("   {}{}", d.matches.len(), if d.search_done { " matches" } else { "\u{2026}" })
        });
        Some(format!("search pdf: {input}{}", count.unwrap_or_default()))
    }

    /// Searches the document read for `query`, from the start, replacing
    /// the last search. Smart-case: a capital makes it exact.
    fn pdf_dispatch_search(&mut self, query: String) {
        let Some((_, doc)) = self.pdf_target() else { return };
        let request_id = self.pdf_next_id();
        let Some(d) = self.pdf_docs.get_mut(&doc) else { return };
        d.search_id = request_id;
        d.query = query.clone();
        d.matches.clear();
        d.current_match = None;
        d.highlights = true;
        d.search_done = query.trim().is_empty();
        if query.trim().is_empty() {
            return;
        }
        let match_case = query.chars().any(char::is_uppercase);
        if let Some(worker) = &self.pdf_worker {
            worker.send(fenix_pdf::PdfRequest::Search { key: d.doc_key, request_id, query, match_case, from_page: 0 });
        }
    }

    /// `Enter` in the search prompt: to the first match at or after the
    /// page being read -- now, or when it's found.
    fn pdf_accept_search(&mut self) {
        let Some(key) = self.pdf_target() else { return };
        let current = self.pdf_current_page(key);
        let Some(doc) = self.pdf_docs.get(&key.1) else { return };
        if doc.query.trim().is_empty() {
            return;
        }
        let (at, done, any, query) = (doc.matches.iter().position(|m| m.page_index >= current), doc.search_done, !doc.matches.is_empty(), doc.query.clone());
        match at {
            Some(at) => self.pdf_go_to_match(key.1, at),
            None if !done => {
                if let Some(doc) = self.pdf_docs.get_mut(&key.1) {
                    doc.jump_when_found = Some(current);
                }
            }
            None if any => self.pdf_go_to_match(key.1, 0),
            None => self.set_message(format!("no matches for \"{query}\"")),
        }
    }

    /// `n`/`N`: the next (or previous) match, going round the end. With
    /// no match gone to yet, the first after the page being read.
    fn pdf_step_match(&mut self, forward: bool) {
        let Some(key) = self.pdf_target() else { return };
        let current_page = self.pdf_current_page(key);
        let Some(doc) = self.pdf_docs.get_mut(&key.1) else { return };
        if doc.matches.is_empty() {
            let message = if doc.query.is_empty() { "no search yet -- / searches the document".to_string() } else { format!("no matches for \"{}\"", doc.query) };
            self.set_message(message);
            return;
        }
        doc.highlights = true;
        let n = doc.matches.len();
        let at = match doc.current_match {
            Some(at) if forward => (at + 1) % n,
            Some(at) => (at + n - 1) % n,
            None if forward => doc.matches.iter().position(|m| m.page_index >= current_page).unwrap_or(0),
            None => doc.matches.iter().rposition(|m| m.page_index <= current_page).unwrap_or(n - 1),
        };
        self.pdf_go_to_match(key.1, at);
    }

    /// Match `at` of `doc`'s search: highlighted as the current one and
    /// brought into view, a third of the way down the pane.
    fn pdf_go_to_match(&mut self, doc: BufferId, at: usize) {
        let Some(d) = self.pdf_docs.get_mut(&doc) else { return };
        let Some(m) = d.matches.get(at) else { return };
        let (page, top_pts, total, page_h) = (m.page_index, m.rects.first().map(|r| r[1]).unwrap_or(0.0), d.matches.len(), d.pages.get(m.page_index as usize).map(|p| p.1).unwrap_or(792.0));
        d.current_match = Some(at);
        d.highlights = true;
        let key = self.pdf_target().filter(|(_, b)| *b == doc).or_else(|| {
            self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&doc)).map(|p| (p, doc))
        });
        if let Some(key) = key {
            let third = self.pdf_views.get(&key).filter(|v| v.layout.scale > 0.0).map(|v| v.pane.1 / 3.0 / v.layout.scale).unwrap_or(0.0);
            let frac = ((top_pts - third) / page_h.max(1.0)).max(0.0);
            self.pdf_jump(key, (page, frac));
        }
        self.set_message(format!("match {}/{total} on page {}", at + 1, page + 1));
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
    const CELL: (f32, f32) = (text::CHAR_WIDTH, text::LINE_HEIGHT);
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
        app.pdf_prepare_view(key, PANE, CELL);
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
        app.pdf_prepare_view(right, PANE, CELL);

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
        app.pdf_prepare_view((pane, key.1), PANE, CELL);
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
        app.pdf_prepare_view(key, PANE, CELL);
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
        app.pdf_prepare_view(key, PANE, CELL);

        assert!(!view(&app, key).pending.contains_key(&1), "page 2 isn't needed at the end");
        assert!(view(&app, key).pending.contains_key(&9));
        assert!(app.pdf_requests.values().all(|&(_, p)| p >= 7), "only the end and one behind: {:?}", app.pdf_requests);
        assert!(view(&app, key).cache.contains_key(&0), "8 pages away is still kept");
        app.pdf_docs.get_mut(&key.1).unwrap().pages = vec![(612.0, 792.0); 30];
        app.pdf_goto_page(30);
        app.pdf_prepare_view(key, PANE, CELL);
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

    // -- Jumps and marks --------------------------------------------------

    #[test]
    fn ctrl_o_goes_back_from_a_jump_and_ctrl_i_forward_again() {
        let (_guard, key, mut app) = test_open_pdf("pdf_jumps.pdf");
        app.pdf_scroll(100);
        keys(&mut app, "7G");
        assert_eq!(page(&app, key), 6);
        assert!(app.pdf_jump_history(true));
        assert_eq!(view(&app, key).scroll.1, 100.0, "back where the jump came from");
        assert!(app.pdf_jump_history(false));
        assert_eq!(page(&app, key), 6);
        assert!(!app.pdf_jump_history(false), "nothing further: the editor's own jump list takes it");
    }

    #[test]
    fn scrolling_isnt_a_jump() {
        let (_guard, _key, mut app) = test_open_pdf("pdf_not_a_jump.pdf");
        keys(&mut app, "5j");
        assert!(!app.pdf_jump_history(true));
    }

    #[test]
    fn marks_come_back_to_the_same_place() {
        let (_guard, key, mut app) = test_open_pdf("pdf_marks.pdf");
        app.pdf_goto_page(4);
        app.pdf_scroll(100);
        let place = view(&app, key).scroll.1;
        keys(&mut app, "ma");
        keys(&mut app, "G");
        keys(&mut app, "'a");
        assert!((view(&app, key).scroll.1 - place).abs() < 1.0);
        keys(&mut app, "'b");
        assert!(app.modeline_text().contains("no mark b"), "{}", app.modeline_text());
    }

    // -- Sidebar ----------------------------------------------------------

    fn sample_outline() -> Vec<fenix_pdf::outline::OutlineEntry> {
        vec![
            fenix_pdf::outline::OutlineEntry { title: "Chapter 1".to_string(), page_index: 0, depth: 0 },
            fenix_pdf::outline::OutlineEntry { title: "Chapter 2".to_string(), page_index: 6, depth: 0 },
        ]
    }

    #[test]
    fn o_opens_the_sidebar_on_the_outline_and_asks_for_it() {
        let (_guard, key, mut app) = test_open_pdf("pdf_sidebar.pdf");
        keys(&mut app, "o");
        let side = view(&app, key).sidebar.as_ref().expect("open");
        assert!(side.focused, "it has the keyboard");
        assert_eq!(app.pdf_docs[&key.1].outline_for, Some(OutlineFor::Sidebar), "asked for");
        assert_eq!(app.windows().window_count(), 1, "in the pane, not beside it");

        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.pdf_goto_page(8);
        app.apply_pdf_response(fenix_pdf::PdfResponse::Outline { key: doc_key, entries: sample_outline() });
        assert_eq!(view(&app, key).sidebar.as_ref().unwrap().cursor, 1, "on the section being read");
    }

    #[test]
    fn enter_in_the_sidebar_goes_there_and_gives_the_pages_the_keyboard() {
        let (_guard, key, mut app) = test_open_pdf("pdf_sidebar_enter.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(sample_outline());
        keys(&mut app, "o");
        assert!(app.reader_key(KeyPress::char('j')), "the sidebar's j");
        assert_eq!(page(&app, key), 0, "moving in the sidebar doesn't scroll");
        assert!(app.reader_key(KeyPress::named(FenixNamedKey::Enter)));
        assert_eq!(page(&app, key), 6);
        assert!(!view(&app, key).sidebar.as_ref().unwrap().focused);
        keys(&mut app, "j");
        assert!(view(&app, key).scroll.1 > 6.0 * STRIDE - 12.0, "j scrolls the pages again");
        keys(&mut app, "o");
        assert!(view(&app, key).sidebar.is_none(), "o closes it");
    }

    #[test]
    fn a_wide_pane_has_the_sidebar_beside_the_pages_and_a_narrow_one_over_them() {
        let (_guard, key, mut app) = test_open_pdf("pdf_sidebar_width.pdf");
        keys(&mut app, "o");
        app.pdf_prepare_view(key, (1200.0, 400.0), CELL);
        let side = view(&app, key).side_px;
        assert!(side > 0.0 && !view(&app, key).side_over);
        assert_eq!(view(&app, key).pane.0, 1200.0 - side, "the pages fit what's left");
        app.pdf_prepare_view(key, PANE, CELL);
        assert!(view(&app, key).side_over);
        assert_eq!(view(&app, key).pane.0, PANE.0, "the pages keep the whole width");
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: PANE.0, h: PANE.1 };
        assert!(app.pdf_draw_plan(key, rect).iter().all(|d| d.dest.0 >= side), "nothing drawn under it");
    }

    #[test]
    fn the_sidebar_draws_its_lists_header_and_rows() {
        let (_guard, key, mut app) = test_open_pdf("pdf_sidebar_draw.pdf");
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(sample_outline());
        keys(&mut app, "o");
        let chrome = app.pdf_chrome(key, fenix_window::Rect { x: 0.0, y: 0.0, w: 1200.0, h: 400.0 }, CELL);
        let text: String = chrome.spans.iter().map(|(t, _, _)| t.as_str()).collect();
        assert!(text.starts_with("[Outline] Matches 0 Marks 0"), "{text}");
        assert!(text.contains("Chapter 2"), "{text}");
        assert!(!chrome.rects.is_empty(), "its background and selected row");
    }

    #[test]
    fn spc_r_t_picks_a_heading_by_name() {
        let (_guard, key, mut app) = test_open_pdf("pdf_headings.pdf");
        app.pdf_pick_heading();
        assert_eq!(app.pdf_docs[&key.1].outline_for, Some(OutlineFor::Picker));
        let doc_key = app.pdf_docs[&key.1].doc_key;
        app.apply_pdf_response(fenix_pdf::PdfResponse::Outline { key: doc_key, entries: sample_outline() });
        let Some(ActivePicker::PdfHeading(picker)) = &mut app.active_picker else { panic!("the heading picker") };
        for c in "chapter 2".chars() {
            picker.push_char(c);
        }
        app.picker_confirm();
        assert_eq!(page(&app, key), 6);
    }

    // -- Search -----------------------------------------------------------

    fn hit(page: u32, y: f32, context: &str) -> fenix_pdf::search::PdfSearchMatch {
        fenix_pdf::search::PdfSearchMatch { page_index: page, char_index: 0, context: context.to_string(), rects: vec![[100.0, y, 160.0, y + 12.0]] }
    }

    fn found(app: &mut App, key: ViewKey, matches: Vec<fenix_pdf::search::PdfSearchMatch>, done: bool) {
        let (doc_key, request_id) = (app.pdf_docs[&key.1].doc_key, app.pdf_docs[&key.1].search_id);
        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id, matches, done });
    }

    #[test]
    fn typing_a_search_searches_as_you_go_with_smart_case() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_typing.pdf");
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('f'));
        let first = app.pdf_docs[&key.1].search_id;
        app.pdf_search_prompt_key(KeyPress::char('o'));
        let doc = &app.pdf_docs[&key.1];
        assert!(doc.search_id > first, "a new search for each change");
        assert_eq!(doc.query, "fo");
        assert!(doc.highlights && !doc.search_done);
        found(&mut app, key, vec![hit(0, 100.0, "fox")], false);
        assert_eq!(app.pdf_search_prompt_text(), Some("search pdf: fo   1\u{2026}".to_string()));
        found(&mut app, key, vec![hit(4, 100.0, "fog")], true);
        assert_eq!(app.pdf_search_prompt_text(), Some("search pdf: fo   2 matches".to_string()));
    }

    #[test]
    fn a_stale_searchs_results_are_dropped() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_stale.pdf");
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('a'));
        let doc_key = app.pdf_docs[&key.1].doc_key;
        let stale = app.pdf_docs[&key.1].search_id;
        app.pdf_search_prompt_key(KeyPress::char('b'));
        app.apply_pdf_response(fenix_pdf::PdfResponse::SearchResults { key: doc_key, request_id: stale, matches: vec![hit(0, 0.0, "a")], done: true });
        assert!(app.pdf_docs[&key.1].matches.is_empty());
    }

    #[test]
    fn enter_goes_to_the_first_match_from_here_even_before_it_is_found() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_enter.pdf");
        app.pdf_goto_page(4);
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('x'));
        app.pdf_search_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        found(&mut app, key, vec![hit(1, 100.0, "early")], false);
        assert_eq!(page(&app, key), 3, "page 2 is before here");
        found(&mut app, key, vec![hit(5, 300.0, "later")], true);
        assert_eq!(page(&app, key), 5);
        assert_eq!(app.pdf_docs[&key.1].current_match, Some(1));
    }

    #[test]
    fn n_and_capital_n_go_through_the_matches_one_by_one() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_n.pdf");
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('x'));
        app.pdf_search_prompt_key(KeyPress::named(FenixNamedKey::Escape));
        found(&mut app, key, vec![hit(2, 100.0, "a"), hit(2, 600.0, "b"), hit(8, 50.0, "c")], true);
        keys(&mut app, "n");
        assert_eq!((app.pdf_docs[&key.1].current_match, page(&app, key)), (Some(0), 2));
        keys(&mut app, "n");
        assert_eq!(app.pdf_docs[&key.1].current_match, Some(1), "two on one page, one at a time");
        keys(&mut app, "n");
        assert_eq!((app.pdf_docs[&key.1].current_match, page(&app, key)), (Some(2), 8));
        keys(&mut app, "n");
        assert_eq!(app.pdf_docs[&key.1].current_match, Some(0), "round the end");
        keys(&mut app, "N");
        assert_eq!(app.pdf_docs[&key.1].current_match, Some(2));
        assert!(app.modeline_text().contains("match 3/3 on page 9"), "{}", app.modeline_text());
        app.status_message = None;
        assert!(app.modeline_text().contains("/x 3/3"), "the position shows the search too: {}", app.modeline_text());
    }

    #[test]
    fn matches_are_highlighted_on_the_page_until_esc() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_highlights.pdf");
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('x'));
        app.pdf_search_prompt_key(KeyPress::named(FenixNamedKey::Enter));
        found(&mut app, key, vec![hit(0, 100.0, "a"), hit(0, 200.0, "b"), hit(9, 0.0, "far")], true);
        app.pdf_prepare_view(key, PANE, CELL);
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: PANE.0, h: PANE.1 };
        let chrome = app.pdf_chrome(key, rect, CELL);
        assert_eq!(chrome.highlights.len(), 2, "the two in sight");
        let current = chrome.highlights.iter().find(|(_, tint)| *tint == CURRENT_MATCH_TINT).expect("the current one stands out");
        assert_eq!(current.0.2, 60.0, "the match's width at 100%");
        assert!(app.reader_key(KeyPress::named(FenixNamedKey::Escape)));
        assert!(app.pdf_chrome(key, rect, CELL).highlights.is_empty());
    }

    #[test]
    fn esc_in_the_search_prompt_clears_the_highlights() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_esc.pdf");
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('x'));
        app.pdf_search_prompt_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.pdf_search_prompt.is_none());
        assert!(!app.pdf_docs[&key.1].highlights);
    }

    #[test]
    fn the_matches_list_goes_to_a_match() {
        let (_guard, key, mut app) = test_open_pdf("pdf_search_list.pdf");
        keys(&mut app, "/");
        app.pdf_search_prompt_key(KeyPress::char('x'));
        app.pdf_search_prompt_key(KeyPress::named(FenixNamedKey::Escape));
        found(&mut app, key, vec![hit(2, 100.0, "a"), hit(7, 100.0, "b")], true);
        app.pdf_docs.get_mut(&key.1).unwrap().outline = Some(Vec::new());
        keys(&mut app, "o");
        app.reader_key(KeyPress::named(FenixNamedKey::Tab));
        app.reader_key(KeyPress::char('j'));
        app.reader_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!((app.pdf_docs[&key.1].current_match, page(&app, key)), (Some(1), 7));
    }

    #[test]
    fn n_before_any_search_says_how_to_search() {
        let (_guard, _key, mut app) = test_open_pdf("pdf_keys_no_search.pdf");
        keys(&mut app, "n");
        assert!(app.modeline_text().contains("/ searches"), "{}", app.modeline_text());
    }
}
