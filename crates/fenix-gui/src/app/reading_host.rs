//! The host half of the reading view (`reading_page`), and the pictures
//! pages draw: a picture a page asks for (a file in a note, a diagram)
//! is decoded off the UI thread into BGRA, uploaded once as a texture
//! and drawn over the blank cells the page left for it.
//!
//! What the reading view can't know by itself it asks the extensions
//! that serve it: a ```mermaid block drawn (`diagrams.render`, the
//! diagrams extension's), and what a `[[link]]` goes to or embeds
//! (`notebook.link`, `notebook.follow`, the notebook's). Without them a
//! diagram shows as its source and a link as a link.

use super::pages::{PageEvent, PageModel};
use super::*;
use crate::page::ImageKey;
use crate::surface::Surface;
use crate::reading::{self, Action as RAction};
use crate::reading_page::{Action as ReadAction, ReadingPage};

/// A picture's pixels and, once uploaded, its texture.
pub(super) struct PageTex {
    pub(super) w: u32,
    pub(super) h: u32,
    /// Waiting to be uploaded.
    pub(super) bgra: Option<Vec<u8>>,
    pub(super) texture: Option<Surface>,
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

/// A picture from its bytes, as `decode_picture` reads a file.
pub(super) fn decode_picture_bytes(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    let img = if img.width() > MAX_PICTURE_WIDTH { img.resize(MAX_PICTURE_WIDTH, u32::MAX, image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((w, h, rgba_to_bgra(rgba.into_raw())))
}

pub(super) fn rgba_to_bgra(mut px: Vec<u8>) -> Vec<u8> {
    for p in px.as_chunks_mut::<4>().0 {
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

    /// Whether the page is laid out anew: what it needs is asked then.
    pub(super) fn laid_out(&self) -> bool {
        self.rendered.is_some()
    }
}


/// Pictures pages show (a note's images, a diagram's drawing), read and
/// once drawn uploaded -- core, as the reading view is.
#[derive(Default)]
pub(super) struct PageImages {
    /// Picture sizes known so far, for the reading layout.
    pub(super) image_sizes: HashMap<crate::page::ImageKey, (u32, u32)>,
    /// Pictures pages draw, read and (once drawn) uploaded.
    pub(super) textures: HashMap<crate::page::ImageKey, PageTex>,
    /// Pictures being read.
    pub(super) pending: std::collections::HashSet<crate::page::ImageKey>,
    /// Diagrams asked of the extension that draws them, by their key's
    /// hash: drawn (or being), or why not.
    pub(super) diagrams: HashMap<u64, Drawing>,
    /// What `[[links]]` go to, as the notebook extension said, by
    /// `link_key`; `None` while it's being asked.
    pub(super) links: HashMap<String, Option<LinkSaid>>,
}

/// A diagram as the diagrams extension drew it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Drawing {
    Asked,
    Drawn,
    Wrong(String),
}

/// What the notebook extension says a link goes to.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub(super) struct LinkSaid {
    #[serde(default)]
    pub(super) exists: bool,
    /// `note`, `diagram`, `missing` or `other`.
    #[serde(default)]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) text: String,
}

/// A link as the notebook extension is sent it.
fn link_json(link: &crate::md::Link) -> serde_json::Value {
    serde_json::json!({ "target": link.target, "heading": link.heading, "markdown": link.markdown, "embed": link.embed })
}

/// What a link from file `from` is kept under.
fn link_key(from: Option<&Path>, link: &crate::md::Link) -> String {
    format!("{}|{}|{}|{}|{}", from.map(|p| p.display().to_string()).unwrap_or_default(), link.target, link.heading.as_deref().unwrap_or(""), link.markdown, link.embed)
}

/// The key a diagram's source, drawn in theme `theme`, is kept under.
pub(super) fn diagram_hash(source: &str, theme: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    source.trim_end().hash(&mut h);
    theme.hash(&mut h);
    h.finish()
}

impl App {
    /// Whether the focused buffer is Markdown.
    pub(super) fn in_markdown(&self) -> bool {
        self.focused_language() == Some(fenix_syntax::LanguageId::Markdown)
    }

    /// Opens `model` in a new pane to the right, `share` of the width.
    pub(super) fn open_page_beside(&mut self, model: PageModel, share: f32) -> BufferId {
        self.split_vertical();
        let id = self.open_page(model);
        let pane = self.focused_pane_id();
        self.workspaces.active_pane_tabs_mut().insert(pane, vec![id]);
        // The split starts even; this side (the right) gets `share`.
        self.windows_mut().set_split_near(pane, 1.0 - share);
        id
    }

    /// The extension that draws diagrams (`diagrams.render`), if one's
    /// installed.
    pub(super) fn diagram_server(&self) -> Option<String> {
        self.code_server("diagrams.render", None)
    }

    /// The key a diagram's source is drawn under, in this theme.
    pub(super) fn diagram_key_of(&self, source: &str) -> u64 {
        diagram_hash(source, self.theme.name)
    }

    /// What the reading layout needs, borrowing only what it reads.
    pub(super) fn with_reading_ctx<R>(&self, base: Option<&Path>, f: impl FnOnce(&reading::Ctx) -> R) -> R {
        let sizes = &self.page_images.image_sizes;
        let (cw, lh) = match &self.text {
            Some(t) => (t.char_width(), t.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };
        let base_dir = base.and_then(Path::parent).map(Path::to_path_buf);
        let base_file = base.map(Path::to_path_buf);
        let drawing = self.diagram_server().is_some();
        let highlight = |lang: &str, text: &str| highlight_block(lang, text);
        let image_size = |key: &ImageKey| sizes.get(key).copied();
        let diagram_key = |source: &str| ImageKey::Diagram(self.diagram_key_of(source));
        let diagram_error = |source: &str| match self.page_images.diagrams.get(&self.diagram_key_of(source)) {
            Some(Drawing::Wrong(why)) => Some(why.clone()),
            _ => None,
        };
        let drawn = |_: &str| drawing;
        let image_path = |src: &str| {
            if src.contains("://") {
                return None;
            }
            let decoded = src.replace("%20", " ");
            let p = PathBuf::from(&decoded);
            let path = if p.is_absolute() { p } else { base_dir.clone().unwrap_or_default().join(p) };
            path.exists().then_some(path)
        };
        // `[[links]]` are the notebook extension's: as it said, else a
        // link as a link and an embed by its name.
        let said = |link: &crate::md::Link| self.page_images.links.get(&link_key(base_file.as_deref(), link)).cloned().flatten();
        let embed = |link: &crate::md::Link| match said(link) {
            Some(s) if s.kind == "diagram" => reading::Embedded::Diagram { name: s.name, source: s.text },
            Some(s) if s.kind == "note" => reading::Embedded::Note { name: s.name, text: s.text },
            Some(s) if !s.exists => reading::Embedded::Missing(link.target.clone()),
            _ => reading::Embedded::Unresolved(link.target.clone()),
        };
        let link_exists = |link: &crate::md::Link| said(link).is_none_or(|s| s.exists);
        let ctx = reading::Ctx {
            highlight: &highlight,
            image_size: &image_size,
            diagram_key: &diagram_key,
            diagram_error: &diagram_error,
            drawn: &drawn,
            image_path: &image_path,
            embed: &embed,
            link_exists: &link_exists,
            cell: (cw, lh),
        };
        f(&ctx)
    }

    /// Asks the extensions what reading page `id` needs and nobody's said
    /// yet: its diagrams drawn, its links' targets. Each answer lays the
    /// page out again.
    pub(super) fn reading_ask(&mut self, id: BufferId) {
        let Some(PageModel::Reading(p)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let Some(ob) = self.buffers.get(p.source) else { return };
        let from = p.path.clone();
        let text = ob.buffer.text();
        let mut sources = Vec::new();
        let mut links = Vec::new();
        for b in crate::md::blocks::parse(&text) {
            match b.block {
                crate::md::blocks::Block::Mermaid { source } => sources.push(source),
                crate::md::blocks::Block::Embed { link } => links.push(link),
                _ => {}
            }
        }
        links.extend(crate::md::meta::links(&text).into_iter().filter(|l| !l.markdown));
        self.ask_diagrams(sources);
        self.ask_links(from, links);
    }

    /// Diagrams drawn by the extension that draws them, the ones not asked
    /// for yet.
    pub(super) fn ask_diagrams(&mut self, sources: Vec<String>) {
        let Some(server) = self.diagram_server() else { return };
        for source in sources {
            let h = self.diagram_key_of(&source);
            if self.page_images.diagrams.contains_key(&h) {
                continue;
            }
            self.page_images.diagrams.insert(h, Drawing::Asked);
            let payload = serde_json::json!({ "source": source }).to_string();
            let then = move |app: &mut App, said: Result<String, String>| app.diagram_drawn(h, said);
            self.code_serve(&server, "diagrams.render", payload, super::wasm_host::Answer::Fenix(Box::new(then)));
        }
    }

    /// A diagram came back: its picture decoded off the UI thread, or why
    /// it doesn't draw.
    fn diagram_drawn(&mut self, h: u64, said: Result<String, String>) {
        let png = said.and_then(|text| {
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let b64 = v.get("png").and_then(|p| p.as_str()).ok_or("no picture")?;
            base64_decode(b64)
        });
        match png {
            Ok(bytes) => {
                self.page_images.diagrams.insert(h, Drawing::Drawn);
                let key = ImageKey::Diagram(h);
                self.page_images.pending.insert(key.clone());
                self.page_spawn(move |send| send(PageEvent::PageImage { key, result: decode_picture_bytes(&bytes) }));
            }
            Err(why) => {
                self.page_images.diagrams.insert(h, Drawing::Wrong(why));
                self.page_pictures_arrived();
            }
        }
    }

    /// What links from `from` go to, asked of the notebook extension.
    fn ask_links(&mut self, from: Option<PathBuf>, links: Vec<crate::md::Link>) {
        let Some(server) = self.code_server("notebook.link", None) else { return };
        let mut asked: Vec<(String, crate::md::Link)> = Vec::new();
        for link in links {
            let key = link_key(from.as_deref(), &link);
            if self.page_images.links.contains_key(&key) || asked.iter().any(|(k, _)| *k == key) {
                continue;
            }
            asked.push((key, link));
        }
        if asked.is_empty() {
            return;
        }
        for (key, _) in &asked {
            self.page_images.links.insert(key.clone(), None);
        }
        let payload = serde_json::json!({ "from": from, "links": asked.iter().map(|(_, l)| link_json(l)).collect::<Vec<_>>() }).to_string();
        let keys: Vec<String> = asked.into_iter().map(|(k, _)| k).collect();
        let then = move |app: &mut App, said: Result<String, String>| {
            let said: Vec<LinkSaid> = said.ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            for (i, key) in keys.into_iter().enumerate() {
                // What it didn't say is a link as a link, not asked again.
                let one = said.get(i).cloned().unwrap_or(LinkSaid { exists: true, kind: "other".into(), name: String::new(), text: String::new() });
                app.page_images.links.insert(key, Some(one));
            }
            app.page_pictures_arrived();
        };
        self.code_serve(&server, "notebook.link", payload, super::wasm_host::Answer::Fenix(Box::new(then)));
    }

    /// What's known of links and diagrams is forgotten (a note saved, the
    /// theme changed): asked again as pages show them.
    pub(crate) fn reading_forget(&mut self) {
        self.page_images.links.clear();
        self.page_images.diagrams.retain(|_, d| *d == Drawing::Asked);
    }

    /// Follows a `[[link]]` from the reading view, through the notebook
    /// extension; a link to a file in Markdown is opened without it.
    fn follow_wiki_link(&mut self, link: crate::md::Link, from: Option<PathBuf>) {
        if let Some(server) = self.code_server("notebook.follow", None) {
            let payload = serde_json::json!({ "from": from, "link": link_json(&link) }).to_string();
            let then = |app: &mut App, said: Result<String, String>| {
                if let Err(e) = said {
                    app.set_error(e);
                }
            };
            return self.code_serve(&server, "notebook.follow", payload, super::wasm_host::Answer::Fenix(Box::new(then)));
        }
        if link.markdown && !link.target.is_empty() {
            let base = from.as_deref().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
            let path = base.join(link.target.replace("%20", " "));
            if path.exists() {
                return self.open_file_as(&path, true);
            }
            return self.set_error(format!("no file {}", path.display()));
        }
        self.set_message(format!("[[{}]] is a note's -- the notebook is an extension (SPC h x installs it)", link.target));
    }

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
        // Notes may have come and gone since links were last asked about.
        self.reading_forget();
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
        let (source, path, replaces, back) = (p.source, p.path.clone(), p.replaces, p.back);
        match action {
            ReadAction::None => {}
            // A page Fenix wrote: gone with its source, back where it came from.
            ReadAction::Close if back.is_some() => {
                self.close_page(page_id);
                self.buffers.close(source);
                if let Some(back) = back.filter(|b| self.pages.contains_key(b)) {
                    self.open_buffer_in_focused_pane(back);
                }
            }
            ReadAction::Edit(_) if back.is_some() => self.set_message("Fenix wrote this page -- there's nothing to edit"),
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
                if let Some(task) = crate::md::meta::task_on(text.trim_end_matches(['\n', '\r']), line) {
                    let at = ob.buffer.line_start_char(line) + task.mark_col;
                    let mut scratch = Cursor::at_start();
                    ob.buffer.replace_range(&mut scratch, at, at + 1, if done { " " } else { "x" });
                    self.set_message(if done { "unticked" } else { "ticked" });
                }
            }
            ReadAction::Target(RAction::Link(crate::md::blocks::LinkTo::Url(url))) => {
                if let Err(e) = fenix_fs::open_url(&url) {
                    self.set_error(format!("couldn't open {url}: {e}"));
                }
            }
            ReadAction::Target(RAction::Link(crate::md::blocks::LinkTo::Wiki(link))) => {
                self.reading_to_source_pane(page_id, source);
                self.follow_wiki_link(link, path);
            }
            ReadAction::Target(RAction::Image(file)) => {
                // An extension's that opens it (a picture viewer), first.
                if let Some(open) = self.opener_for(&file) {
                    return open(self, &file, false);
                }
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
            self.place_cursor(line, 0);
        }
    }

    /// A diagram in the reading view: its source, for now.
    pub(super) fn reading_open_diagram(&mut self, page_id: BufferId, source: BufferId, line: usize, embed: Option<crate::md::Link>) {
        match embed {
            Some(link) => {
                let path = self.buffers.get(source).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
                self.reading_to_source_pane(page_id, source);
                self.follow_wiki_link(link, path);
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
        let wanted: Vec<ImageKey> = state.page.images.iter().map(|i| i.key.clone()).filter(|k| !self.page_images.textures.contains_key(k) && !self.page_images.pending.contains(k)).collect();
        for key in wanted {
            match &key {
                ImageKey::File(path) => {
                    self.page_images.pending.insert(key.clone());
                    let path = path.clone();
                    let key2 = key.clone();
                    self.page_spawn(move |send| send(PageEvent::PageImage { key: key2, result: decode_picture(&path) }));
                }
                // Asked of the diagrams extension with the page's other
                // needs (`reading_ask`); it comes when it's drawn.
                ImageKey::Diagram(_) => {}
                // An extension's: it comes when it's loaded, or not at all.
                ImageKey::Code(_) => {}
            }
        }
    }

    /// A picture arrived: kept for upload, and pages showing it laid out
    /// again at its size.
    pub(super) fn apply_page_image(&mut self, key: ImageKey, result: Result<(u32, u32, Vec<u8>), String>) {
        self.page_images.pending.remove(&key);
        match result {
            Ok((w, h, bgra)) => {
                self.page_images.image_sizes.insert(key.clone(), (w, h));
                self.page_images.textures.insert(key, PageTex { w, h, bgra: Some(bgra), texture: None });
            }
            Err(e) => {
                // Remembered as failed (1x1, nothing drawn) so it isn't
                // asked for every frame.
                eprintln!("fenix: couldn't read a picture: {e}");
                self.page_images.textures.insert(key, PageTex { w: 0, h: 0, bgra: None, texture: None });
            }
        }
        self.page_pictures_arrived();
    }

    /// Pictures came in (a file read, a diagram drawn): every page that
    /// shows them is laid out again.
    pub(super) fn page_pictures_arrived(&mut self) {
        for state in self.pages.values_mut() {
            match &mut state.model {
                PageModel::Reading(p) => {
                    p.seen = None;
                    state.stale = true;
                }
                PageModel::Preview(_) => state.stale = true,
                _ => {}
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// Base64 back to bytes: a picture an extension sent.
fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        let v = T.iter().position(|&b| b == c).ok_or("not base64")? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// A capture name from `fenix-syntax` as one the page roles can hold.
fn static_capture(name: &str) -> &'static str {
    match name.split('.').next().unwrap_or(name) {
        "keyword" | "repeat" | "conditional" | "storageclass" | "include" | "tag" => "keyword",
        "string" | "escape" => "string",
        "comment" | "spell" => "comment",
        "function" | "method" => "function",
        "type" | "namespace" | "module" => "type",
        "number" | "float" => "number",
        "constant" | "boolean" => "constant",
        "variable" | "property" | "label" | "field" | "parameter" => "variable",
        "operator" => "operator",
        "punctuation" => "punctuation",
        "attribute" | "constructor" | "preproc" | "embedded" => "attribute",
        _ => "text",
    }
}

/// The grammar a fenced block's info word names.
fn language_for_fence(lang: &str) -> Option<fenix_syntax::LanguageId> {
    let ext = match lang {
        "rust" => "rs",
        "python" | "py3" => "py",
        "bash" | "shell" | "sh" | "zsh" | "console" => "sh",
        "javascript" | "node" => "js",
        "typescript" => "ts",
        "c++" | "cxx" | "cc" => "cpp",
        "yml" => "yaml",
        "csharp" | "c#" => "cs",
        "golang" => "go",
        "markdown" => "md",
        "arduino" => "ino",
        other => other,
    };
    fenix_syntax::detect_language(ext)
}

/// A code block's colours, per line: (columns, capture).
pub(super) fn highlight_block(lang: &str, text: &str) -> Vec<Vec<(std::ops::Range<usize>, &'static str)>> {
    let Some(language) = language_for_fence(lang) else { return Vec::new() };
    highlight_lines(language, text)
}

/// `text`'s colours as `language`, per line: (columns, capture).
pub(super) fn highlight_lines(language: fenix_syntax::LanguageId, text: &str) -> Vec<Vec<(std::ops::Range<usize>, &'static str)>> {
    let state = fenix_syntax::SyntaxState::new(language, text);
    let spans = state.highlights_in_range(text, 0..text.len());
    let mut starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let mut out: Vec<Vec<(std::ops::Range<usize>, &'static str)>> = vec![Vec::new(); starts.len()];
    for (range, name) in spans {
        let line = match starts.binary_search(&range.start) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = starts[line];
        let line_end = starts.get(line + 1).map(|e| e - 1).unwrap_or(text.len());
        let end = range.end.min(line_end);
        if end <= range.start {
            continue;
        }
        let a = text[line_start..range.start].chars().count();
        let b = a + text[range.start..end].chars().count();
        out[line].push((a..b, static_capture(name)));
    }
    out
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
        app.place_cursor(3, 0);
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

    /// Without the extensions a diagram shows as its source and an embed
    /// by its name; with them, the diagram is drawn (`diagrams.render`)
    /// and the embed is what the notebook says it is (`notebook.link`).
    #[test]
    fn diagrams_and_embeds_are_the_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.md");
        std::fs::write(&path, "# Title\n\n```mermaid\nflowchart TD\n  a --> b\n```\n\n![[Ground station]]\n").unwrap();
        let mut app = App::with_file(Some(path.to_string_lossy().into_owned()));
        app.open_reading_view(false);
        let page = app.find_page(|m| matches!(m, PageModel::Reading(_))).expect("the reading view");
        let pane = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&page)).unwrap();
        app.ensure_page_layout(page, pane, 60);
        let text = app.buffers.get(page).unwrap().buffer.text();
        assert!(text.contains("flowchart TD") && text.contains("↳ Ground station"), "{text}");

        let mut ext = super::super::wasm_host::tests::hello("");
        ext.manifest = fenix_extensions::Manifest::parse(
            "[extension]\nid = \"test.both\"\nname = \"Both\"\nversion = \"1.0.0\"\nruntime = \"wasm\"\n\n[engine]\napi = \"0.7\"\nfenix = \">=1.0, <2\"\n\n[contributes]\nnamespaces = [\"diagrams\", \"notebook\"]\nservices = [\"diagrams.render\", \"notebook.link\"]\n",
        )
        .unwrap();
        ext.id = "test.both".into();
        app.wasm.installed = vec![ext];
        app.reading_forget();
        if let Some(state) = app.pages.get_mut(&page) {
            if let PageModel::Reading(p) = &mut state.model {
                p.seen = None;
            }
            state.stale = true;
        }
        let started = std::time::Instant::now();
        loop {
            app.ensure_page_layout(page, pane, 60);
            app.pump_wasm();
            let text = app.buffers.get(page).unwrap().buffer.text();
            let drawn = app.page_images.image_sizes.keys().any(|k| matches!(k, ImageKey::Diagram(_)));
            if drawn && text.contains("Antenna notes") {
                assert!(!text.contains("flowchart TD"), "drawn, not its source: {text}");
                break;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(60), "{text}\n{}", app.extension_output_text());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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
