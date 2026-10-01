//! The host half of diagrams (`diagram_page`, `fenix-diagram`): a
//! diagram's source is drawn off the UI thread, keyed by the source and
//! the theme, so the same picture is drawn once however many places
//! show it (the preview, the notebook page, a note's reading view). The
//! last drawing that worked stays up while you type through an error,
//! and the error is a diagnostic on its line. Also: new diagrams (type,
//! starter, name), the preview beside a `.mmd`, the viewer, and themes.

use std::cell::RefCell;

use super::notebook_host::{NotebookPick, NotebookPrompt, NotebookPromptKind};
use super::pages::{PageEvent, PageModel};
use super::reading_host::PageTex;
use super::*;
use crate::diagram_page::{Action as DgAction, DiagramPage, Drawn};
use crate::page::ImageKey;
use fenix_notebook::Kind as NbKind;

/// The scale diagrams are drawn at, over their natural size: sharp on
/// high-DPI screens and when zoomed a little.
const DIAGRAM_SCALE: f32 = 2.0;

/// What a diagram's drawing came to.
pub(super) type DrawResult = Result<Drawn, (usize, String)>;

/// The diagram side of `NotebookState`.
#[derive(Default)]
pub(super) struct DiagramState {
    /// The source (and theme shown instead, if any) behind each key.
    pub(super) sources: RefCell<HashMap<u64, (String, Option<String>)>>,
    /// Each key's drawing, once it's done.
    pub(super) results: HashMap<u64, DrawResult>,
    pub(super) pending: std::collections::HashSet<u64>,
    /// Problems, by file, for the editor's diagnostics.
    pub(super) diagnostics: HashMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// Each page pane's height in rows, for pages that fill it.
    pub(super) rows: HashMap<BufferId, usize>,
}

/// What laying a diagram page out found, before it's borrowed.
pub(super) struct DiagramPre {
    want: Option<u64>,
    seen: Option<u64>,
    ready: Option<DrawResult>,
    cursor_node: Option<Option<usize>>,
    rows: usize,
}

impl App {
    /// The themes, as the settings and the editor theme make them.
    pub(super) fn diagram_themes(&self) -> fenix_diagram::Themes {
        let t = self.theme;
        let rgb = |c: glyphon::Color| [c.r(), c.g(), c.b()];
        let rgbf = |c: [f32; 4]| [(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8];
        let editor = fenix_diagram::EditorColors {
            bg: rgbf(t.bg),
            fg: rgb(t.fg),
            panel: rgbf(t.bg_modeline),
            accent: rgbf(t.caret),
            muted: rgb(t.gutter_fg),
            tint: rgbf(t.hl_line),
            warn: rgb(t.git_modified),
        };
        let custom = self
            .config
            .diagrams_themes
            .iter()
            .filter(|row| !row.first().map(|n| n.trim().is_empty()).unwrap_or(true))
            .map(|row| {
                let names = ["primaryColor", "primaryBorderColor", "primaryTextColor", "lineColor", "secondaryColor", "background"];
                fenix_diagram::ThemeSpec {
                    name: row[0].trim().to_string(),
                    base: row.get(1).cloned().filter(|b| !b.trim().is_empty()).unwrap_or_else(|| "base".into()),
                    vars: names.iter().enumerate().filter_map(|(i, k)| row.get(i + 2).map(|v| (k.to_string(), v.trim().to_string()))).collect(),
                }
            })
            .collect();
        fenix_diagram::Themes {
            default: self.config.diagrams_theme.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "fenix".into()),
            custom,
            editor,
            font: self.config.diagrams_font.clone().filter(|s| !s.trim().is_empty()),
        }
    }

    /// A diagram's key: its source, the theme it's shown in instead (if
    /// any), and everything that decides its colours. Remembers the
    /// source so the picture can be drawn.
    pub(super) fn diagram_key(&self, source: &str, over: Option<&str>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut h);
        over.hash(&mut h);
        self.theme.name.hash(&mut h);
        self.config.diagrams_theme.hash(&mut h);
        self.config.diagrams_themes.hash(&mut h);
        self.config.diagrams_font.hash(&mut h);
        let key = h.finish();
        self.notebook.diagrams.sources.borrow_mut().entry(key).or_insert_with(|| (source.to_string(), over.map(str::to_string)));
        key
    }

    /// Starts drawing a diagram picture, when it isn't drawn or drawing.
    pub(super) fn request_diagram_image(&mut self, key: ImageKey) {
        let ImageKey::Diagram(h) = key else { return };
        if self.notebook.diagrams.results.contains_key(&h) || self.notebook.diagrams.pending.contains(&h) {
            return;
        }
        let Some((source, over)) = self.notebook.diagrams.sources.borrow().get(&h).cloned() else { return };
        self.notebook.diagrams.pending.insert(h);
        let themes = self.diagram_themes();
        self.page_spawn(move |send| {
            let result = fenix_diagram::render(&source, &themes, over.as_deref()).map_err(|d| (d.line, d.message)).and_then(|r| {
                let (w, h, rgba) = fenix_diagram::rasterize(&r.svg, DIAGRAM_SCALE, fenix_diagram::Background::Theme).map_err(|e| (0, e))?;
                Ok((Drawn { size: (r.width, r.height), nodes: r.nodes, theme: r.theme }, w, h, super::reading_host::rgba_to_bgra(rgba)))
            });
            send(PageEvent::DiagramDrawn { key: h, result });
        });
    }

    /// A drawing arrived: its picture kept for upload, pages showing it
    /// laid out again, its problem (or none) on its file.
    pub(super) fn apply_diagram_drawn(&mut self, h: u64, result: Result<(Drawn, u32, u32, Vec<u8>), (usize, String)>) {
        self.notebook.diagrams.pending.remove(&h);
        let key = ImageKey::Diagram(h);
        let stored = match result {
            Ok((drawn, w, hh, bgra)) => {
                self.notebook.image_sizes.insert(key.clone(), (drawn.size.0.round() as u32, drawn.size.1.round() as u32));
                self.notebook.textures.insert(key.clone(), PageTex { w, h: hh, bgra: Some(bgra), texture: None });
                Ok(drawn)
            }
            Err(e) => Err(e),
        };
        self.notebook.diagrams.results.insert(h, stored);
        self.evict_pictures();
        for state in self.pages.values_mut() {
            match &mut state.model {
                PageModel::Reading(p) => {
                    p.seen = None;
                    state.stale = true;
                }
                PageModel::Notebook(_) | PageModel::Diagram(_) => state.stale = true,
                _ => {}
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Keeps the pictures pages still show; lets go of the rest once
    /// there are many.
    fn evict_pictures(&mut self) {
        if self.notebook.textures.len() < 48 {
            return;
        }
        let mut keep: std::collections::HashSet<ImageKey> = self.pages.values().flat_map(|s| s.page.images.iter().map(|i| i.key.clone())).collect();
        for s in self.pages.values() {
            if let PageModel::Diagram(p) = &s.model {
                keep.extend(p.key.clone());
                keep.extend(p.want.clone());
            }
        }
        self.notebook.textures.retain(|k, _| keep.contains(k));
        self.notebook.image_sizes.retain(|k, _| keep.contains(k));
        let hashes: std::collections::HashSet<u64> = keep.iter().filter_map(|k| if let ImageKey::Diagram(h) = k { Some(*h) } else { None }).collect();
        self.notebook.diagrams.results.retain(|h, _| hashes.contains(h));
        self.notebook.diagrams.sources.borrow_mut().retain(|h, _| hashes.contains(h));
    }

    /// Why `source` doesn't draw, once it's been tried.
    pub(super) fn diagram_problem(&self, source: &str) -> Option<String> {
        let h = self.diagram_key(source, None);
        match self.notebook.diagrams.results.get(&h) {
            Some(Err((line, message))) => Some(format!("line {}: {message}", line + 1)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------
    // The page
    // ------------------------------------------------------------------

    /// Before a diagram page is borrowed: what it should show now.
    pub(super) fn diagram_precompute(&self, id: BufferId) -> Option<DiagramPre> {
        let Some(PageModel::Diagram(p)) = self.pages.get(&id).map(|s| &s.model) else { return None };
        let rows = self.notebook.diagrams.rows.get(&id).copied().unwrap_or(30);
        let ob = p.source.and_then(|s| self.buffers.get(s))?;
        let edits = ob.buffer.edit_count();
        let (want, seen) = if p.seen != Some(edits) || p.want.is_none() {
            let text = ob.buffer.text();
            (Some(self.diagram_key(&text, p.theme_over.as_deref())), Some(edits))
        } else {
            (None, None)
        };
        let target = want.or(match &p.want {
            Some(ImageKey::Diagram(h)) => Some(*h),
            _ => None,
        });
        let ready = target.and_then(|h| self.notebook.diagrams.results.get(&h).cloned());
        // The node the source cursor is on, when the source is in a pane.
        let cursor_node = p.source.and_then(|src| {
            let pane = self.windows().windows().into_iter().find(|&w| self.windows().content(w) == Some(&src))?;
            let line = ob.buffer.line_col(&self.pane_state(pane).cursor).0;
            let drawn = match &ready {
                Some(Ok(d)) => d,
                _ => &p.drawn,
            };
            Some(drawn.nodes.iter().position(|n| n.line == Some(line)))
        });
        Some(DiagramPre { want, seen, ready, cursor_node, rows })
    }

    /// Whether what `diagram_precompute` found changes the page.
    pub(super) fn diagram_changed(p: &DiagramPage, pre: &DiagramPre) -> bool {
        pre.want.is_some() || pre.ready.as_ref().is_some_and(|r| match r {
            Ok(d) => *d != p.drawn || p.error.is_some(),
            Err(e) => p.error.as_ref() != Some(e),
        }) || pre.cursor_node.is_some_and(|c| c != p.cursor_node)
    }

    pub(super) fn diagram_apply(p: &mut DiagramPage, pre: DiagramPre, cols: usize) -> crate::page::Page {
        if let Some(h) = pre.want {
            p.want = Some(ImageKey::Diagram(h));
        }
        if let Some(seen) = pre.seen {
            p.seen = Some(seen);
        }
        match pre.ready {
            Some(Ok(drawn)) => {
                if p.drawn.nodes.len() != drawn.nodes.len() {
                    p.selected = None;
                }
                p.drawn = drawn;
                p.key = p.want.clone();
                p.error = None;
            }
            Some(Err(e)) => p.error = Some(e),
            None => {}
        }
        if let Some(c) = pre.cursor_node {
            p.cursor_node = c;
        }
        crate::diagram_page::layout(p, cols, pre.rows)
    }

    /// After a diagram page's layout: its problem on its file, and the
    /// drawing it wants started.
    pub(super) fn diagram_after_layout(&mut self, id: BufferId) {
        let Some(PageModel::Diagram(p)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let (want, error, source) = (p.want.clone(), p.error.clone(), p.source);
        if let Some(want) = want {
            self.request_diagram_image(want);
        }
        let Some(path) = source.and_then(|s| self.buffers.get(s)).and_then(|ob| ob.buffer.path()).map(|p| fenix_lsp::normalize(std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))) else { return };
        match error {
            Some((line, message)) => {
                let d = lsp_types::Diagnostic {
                    range: lsp_types::Range { start: lsp_types::Position { line: line as u32, character: 0 }, end: lsp_types::Position { line: line as u32, character: 200 } },
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    source: Some("diagram".into()),
                    message,
                    ..Default::default()
                };
                self.notebook.diagrams.diagnostics.insert(path, vec![d]);
            }
            None => {
                self.notebook.diagrams.diagnostics.remove(&path);
            }
        }
    }

    /// Records how tall a page's pane is; a page that fills it is laid
    /// out again when that changes.
    pub(super) fn note_page_rows(&mut self, id: BufferId, rows: usize) {
        if self.notebook.diagrams.rows.insert(id, rows) != Some(rows) {
            if let Some(state) = self.pages.get_mut(&id) {
                if matches!(state.model, PageModel::Diagram(_)) {
                    state.stale = true;
                }
            }
        }
    }

    /// Whether the focused buffer is a Mermaid file.
    pub(super) fn in_mermaid(&self) -> bool {
        self.focused_language() == Some(fenix_syntax::LanguageId::Mermaid)
    }

    /// `SPC m p` in a diagram: its preview beside it (again closes it).
    pub(crate) fn toggle_diagram_preview(&mut self) {
        let source = self.focused_buffer_id();
        if let Some(PageModel::Diagram(p)) = self.pages.get(&source).map(|s| &s.model) {
            // On the preview: close it.
            let _ = p;
            self.diagram_action(source, DgAction::Close);
            return;
        }
        if let Some(id) = self.find_page(|m| matches!(m, PageModel::Diagram(p) if p.source == Some(source) && !p.viewer)) {
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
        self.open_diagram_preview(source);
    }

    /// Opens the preview of buffer `source` beside it, keeping focus.
    pub(super) fn open_diagram_preview(&mut self, source: BufferId) {
        if self.find_page(|m| matches!(m, PageModel::Diagram(p) if p.source == Some(source) && !p.viewer)).is_some() {
            return;
        }
        let title = self.diagram_title(source);
        let origin = self.focused_pane_id();
        self.open_page_beside(PageModel::Diagram(Box::new(DiagramPage::new(Some(source), title, false))), 0.5);
        self.windows_mut().focus(origin);
    }

    /// A diagram's name: the notebook's, else its file's.
    fn diagram_title(&self, source: BufferId) -> String {
        let path = self.buffers.get(source).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
        match path.as_deref().and_then(|p| self.notebook.book.as_ref().and_then(|nb| nb.by_path(p))) {
            Some(e) => e.name.clone(),
            None => path.and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "diagram".into()),
        }
    }

    /// `v` / `SPC m v`: the diagram on its own, in this pane.
    pub(crate) fn open_diagram_viewer(&mut self) {
        let source = match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            Some(PageModel::Diagram(p)) => p.source,
            _ => Some(self.focused_buffer_id()),
        };
        let Some(source) = source else { return };
        let title = self.diagram_title(source);
        let mut page = DiagramPage::new(Some(source), title, true);
        page.zoom = 1.0;
        self.open_page(PageModel::Diagram(Box::new(page)));
    }

    /// Opens the viewer for a notebook diagram.
    pub(super) fn open_entry_viewer(&mut self, id: &str) {
        let Some(path) = self.notebook.book.as_ref().and_then(|nb| nb.get(id)).map(|e| e.path.clone()) else { return };
        let buffer = match self.buffers.id_for_path(&path) {
            Some(b) => b,
            None => {
                let b = self.buffers.open_path(&path);
                self.note_disk_state(b);
                b
            }
        };
        let title = self.diagram_title(buffer);
        self.open_page(PageModel::Diagram(Box::new(DiagramPage::new(Some(buffer), title, true))));
    }

    pub(super) fn diagram_action(&mut self, page_id: BufferId, action: DgAction) {
        let Some(PageModel::Diagram(p)) = self.pages.get(&page_id).map(|s| &s.model) else { return };
        let (source, viewer) = (p.source, p.viewer);
        match action {
            DgAction::None => {}
            DgAction::Close => {
                let pane = self.focused_pane_id();
                self.close_page(page_id);
                if viewer {
                    return;
                }
                if self.windows().content(pane) != Some(&page_id) && self.windows().window_count() > 1 {
                    self.close_window();
                }
                if let Some(src) = source {
                    if let Some(p) = self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&src)) {
                        self.windows_mut().focus(p);
                    }
                }
            }
            DgAction::Edit | DgAction::Line(_) => {
                let Some(src) = source else { return };
                match self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&src)) {
                    Some(p) => {
                        self.windows_mut().focus(p);
                    }
                    None => self.open_buffer_in_focused_pane(src),
                }
                if let DgAction::Line(line) = action {
                    self.notebook_place_cursor(line, 0);
                }
            }
            DgAction::CycleTheme => {
                let names = self.diagram_themes().names();
                if let Some(PageModel::Diagram(p)) = self.pages.get_mut(&page_id).map(|s| {
                    s.stale = true;
                    &mut s.model
                }) {
                    let current = p.theme_over.clone().unwrap_or_else(|| p.drawn.theme.clone());
                    let at = names.iter().position(|n| *n == current).map(|i| (i + 1) % names.len()).unwrap_or(0);
                    p.theme_over = Some(names[at].clone());
                    p.want = None;
                    p.seen = None;
                    p.note = Some((format!("shown in {} -- t sets it", names[at]), false));
                }
            }
            DgAction::PickTheme => {
                if let Some(src) = source {
                    self.diagram_pick_theme_for(src);
                }
            }
            DgAction::Export => {
                if let Some(src) = source {
                    self.diagram_export_buffer(src);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // New diagrams
    // ------------------------------------------------------------------

    /// `SPC n d`: pick a type, a starter, then a name.
    pub(crate) fn notebook_new_diagram_flow(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        let candidates = fenix_diagram::KINDS
            .iter()
            .enumerate()
            .map(|(i, k)| fenix_picker::Candidate::new(format!("{:<7} {:<19} {}", k.tag, k.header, k.about), NotebookPick::DiagramKind(i)))
            .collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "DIAGRAM", picker: fenix_picker::PickerState::new(candidates) });
    }

    pub(super) fn diagram_kind_picked(&mut self, i: usize) {
        let Some(kind) = fenix_diagram::KINDS.get(i) else { return };
        let mut candidates: Vec<fenix_picker::Candidate<NotebookPick>> = fenix_diagram::starters(kind)
            .into_iter()
            .map(|(name, text)| fenix_picker::Candidate::new(format!("{name:<10} {}", text.lines().nth(1).unwrap_or("").trim()), NotebookPick::Starter(text.to_string())))
            .collect();
        // Yours: Mermaid snippets whose trigger starts with the type.
        let catalog = self.snippet_layers(self.project_root.as_ref().map(|r| r.join(".fenix").join("snippets")).as_deref());
        for s in &catalog.snippets {
            if s.scopes.iter().any(|x| x == "mermaid" || x == "mmd") {
                let body = s.template.render(&Default::default(), &fenix_snippets::Context::current(None), "").text;
                if fenix_diagram::kind(&body).map(|k| k.header) == Some(kind.header) {
                    candidates.push(fenix_picker::Candidate::new(format!("{:<10} yours ({})", s.name, s.trigger), NotebookPick::Starter(body)));
                }
            }
        }
        self.enter_picker(ActivePicker::Notebook { title: "STARTER", picker: fenix_picker::PickerState::new(candidates) });
    }

    pub(super) fn diagram_starter_picked(&mut self, text: String) {
        self.notebook.prompt = Some(NotebookPrompt { kind: NotebookPromptKind::DiagramName { text }, input: String::new() });
        self.wake_caret();
    }

    /// Makes a diagram called `name` holding `text`, opens it and its
    /// preview.
    pub(super) fn notebook_create_diagram(&mut self, name: &str, text: &str) {
        let Some(nb) = self.notebook() else { return };
        if let Some(existing) = nb.by_name(name) {
            let id = existing.id.clone();
            self.set_message(format!("{name} is in the notebook already -- opened it"));
            self.open_notebook_entry(&id);
            return;
        }
        match nb.create(NbKind::Diagram, name, text) {
            Ok(id) => {
                self.open_notebook_entry(&id);
                // The cursor on the blank line under the header.
                let line = text.lines().position(|l| l.trim().is_empty()).unwrap_or(1);
                self.notebook_place_cursor(line, 4);
                self.refresh_notebook_pages();
                self.set_message(format!("{name} -- draws as you type; saves itself"));
            }
            Err(e) => self.set_error(format!("couldn't make the diagram: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // Themes
    // ------------------------------------------------------------------

    /// `SPC m t` / `t`: a theme to write into the diagram in `source`.
    pub(super) fn diagram_pick_theme_for(&mut self, source: BufferId) {
        let themes = self.diagram_themes();
        let current = self.buffers.get(source).map(|ob| themes.name_for(&ob.buffer.text(), None)).unwrap_or_default();
        let about = |n: &str| match n {
            "default" => "Mermaid's lavender, what GitHub shows",
            "neutral" => "greys; prints well",
            "dark" => "Mermaid's dark",
            "forest" => "greens",
            "base" => "the one themeVariables tune",
            "fenix" => "the editor theme's colours",
            _ => "yours",
        };
        let candidates = themes
            .names()
            .into_iter()
            .map(|n| {
                let mark = if n == current { "●" } else { " " };
                fenix_picker::Candidate::new(format!("{mark} {n:<10} {}", about(&n)), NotebookPick::Theme { buffer: source, name: n })
            })
            .collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "THEME", picker: fenix_picker::PickerState::new(candidates) });
    }

    /// `SPC m t` in a diagram.
    pub(crate) fn diagram_pick_theme_here(&mut self) {
        let source = match self.pages.get(&self.focused_buffer_id()).map(|s| &s.model) {
            Some(PageModel::Diagram(p)) => p.source,
            _ => self.in_mermaid().then(|| self.focused_buffer_id()),
        };
        match source {
            Some(s) => self.diagram_pick_theme_for(s),
            None => self.set_error("SPC m t sets a diagram's theme"),
        }
    }

    /// Writes theme `name` into the diagram in buffer `source`.
    pub(super) fn diagram_set_theme(&mut self, source: BufferId, name: &str) {
        let Some(ob) = self.buffers.get_mut(source) else { return };
        let text = ob.buffer.text();
        let new = fenix_diagram::set_theme(&text, name);
        if new != text {
            let end = ob.buffer.len_chars();
            let mut scratch = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch, 0, end, &new);
        }
        for state in self.pages.values_mut() {
            if let PageModel::Diagram(p) = &mut state.model {
                if p.source == Some(source) {
                    p.theme_over = None;
                    p.seen = None;
                    state.stale = true;
                }
            }
        }
        self.set_message(format!("theme: {name}"));
    }

    /// A notebook diagram's theme, from the notebook page.
    pub(super) fn diagram_pick_theme(&mut self, id: Option<String>) {
        let Some(id) = id else { return };
        let Some(path) = self.notebook.book.as_ref().and_then(|nb| nb.get(&id)).map(|e| e.path.clone()) else { return };
        let buffer = match self.buffers.id_for_path(&path) {
            Some(b) => b,
            None => {
                let b = self.buffers.open_path(&path);
                self.note_disk_state(b);
                b
            }
        };
        self.diagram_pick_theme_for(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(app: &App) -> (BufferId, fenix_window::WindowId) {
        let id = app.find_page(|m| matches!(m, PageModel::Diagram(_))).expect("the preview");
        let pane = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&id)).unwrap();
        (id, pane)
    }

    fn page(app: &App, id: BufferId) -> &DiagramPage {
        match app.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Diagram(p)) => p,
            _ => panic!("a diagram page"),
        }
    }

    #[test]
    fn a_new_diagram_draws_beside_its_source_and_keeps_the_last_good_drawing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.notebook_folder = Some(dir.path().join("nb"));
        app.notebook_create_diagram("TC flow", "flowchart TD\n    op[Operator] --> val[Validate]\n");
        let source = app.focused_buffer_id();
        assert!(app.open().buffer.path().unwrap().ends_with("diagrams/tc-flow.mmd"));
        let (id, pane) = preview(&app);
        // Laid out, drawn (synchronously in tests), laid out again.
        app.ensure_page_layout(id, pane, 80);
        app.ensure_page_layout(id, pane, 80);
        let p = page(&app, id);
        assert!(p.key.is_some() && p.error.is_none(), "{:?}", p.error);
        assert_eq!(p.drawn.nodes.len(), 2);
        assert_eq!(p.drawn.theme, "fenix");
        let key = p.key.clone().unwrap();
        assert!(app.notebook.textures.get(&key).is_some_and(|t| t.w > 0));
        // Break it: the error shows, on its line, and the drawing stays.
        let ob = app.buffers.get_mut(source).unwrap();
        let end = ob.buffer.len_chars();
        let mut c = Cursor::at_start();
        ob.buffer.replace_range(&mut c, 0, end, "flowchart TD\n    op[Operator] --> val[Validate]\n    val -->\n");
        app.ensure_page_layout(id, pane, 80);
        app.ensure_page_layout(id, pane, 80);
        let p = page(&app, id);
        assert_eq!(p.error.as_ref().map(|e| e.0), Some(2));
        assert_eq!(p.key.as_ref(), Some(&key), "the last good drawing");
        let path = app.buffers.get(source).unwrap().buffer.path().unwrap().to_path_buf();
        let path = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap());
        assert_eq!(app.notebook.diagrams.diagnostics.get(&path).map(|d| d[0].range.start.line), Some(2));
        // A theme written into the source.
        app.diagram_set_theme(source, "forest");
        assert!(app.buffers.get(source).unwrap().buffer.text().starts_with("---\nconfig:\n  theme: forest\n---\n"));
    }
}
