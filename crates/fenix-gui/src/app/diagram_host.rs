//! The host half of diagrams (`diagram_page`, `fenix-diagram`): a
//! diagram's source is drawn off the UI thread, keyed by the source and
//! the theme, so the same picture is drawn once however many places
//! show it (the preview, the notebook page, a note's reading view). The
//! last drawing that worked stays up while you type through an error,
//! and the error is a diagnostic on its line. Also: new diagrams (type,
//! starter, name), the preview beside a `.mmd`, the viewer, and themes.

use std::cell::RefCell;

use super::feature::page_as;
use super::feature::ServiceCall::{Mut, Read};
use super::pages::{PageEvent, PageModel};
use super::reading_host::PageTex;
use super::*;
use crate::diagram_page::{Action as DgAction, DiagramPage, Drawn};
use crate::page::ImageKey;

/// What drawing a diagram sends back (as the diagrams feature's page
/// event).
#[derive(Debug)]
pub(crate) enum DiagramJob {
    /// A diagram, drawn (or why it couldn't be).
    DiagramDrawn { key: u64, result: Result<(crate::diagram_page::Drawn, u32, u32, Vec<u8>), (usize, String)> },
}

/// The scale diagrams are drawn at, over their natural size: sharp on
/// high-DPI screens and when zoomed a little.
const DIAGRAM_SCALE: f32 = 2.0;

/// What a diagram's drawing came to.
pub(super) type DrawResult = Result<Drawn, (usize, String)>;

/// The diagrams' commands and their `SPC m` menu in a Mermaid file or
/// its preview.
pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::Bind;
    super::feature::FeatureSpec {
        id: "fenix.diagrams",
        commands: &[
            C { id: "diagram.preview", description: "The diagram drawn beside its source (again closes it)", external: false, run: |app| app.toggle_diagram_preview() },
            C { id: "diagram.viewer", description: "The diagram on its own: zoom, pan, walk its nodes", external: false, run: |app| app.open_diagram_viewer() },
            C { id: "diagram.theme", description: "Set the diagram's theme", external: false, run: |app| app.diagram_pick_theme_here() },
            C { id: "diagram.export", description: "Export the diagram", external: false, run: |app| app.diagram_export_here() },
        ],
        leader: &[],
        local_menu: &[
            Bind(&['p'], "preview beside", "diagram.preview"),
            Bind(&['v'], "view it on its own", "diagram.viewer"),
            Bind(&['t'], "set its theme", "diagram.theme"),
            Bind(&['e'], "export", "diagram.export"),
        ],
        // A Mermaid file, or a diagram page: the Mermaid menu.
        local_context: Some(|app| {
            (app.in_mermaid() || app.feature_page::<DiagramPage>(app.focused_buffer_id()).is_some()).then_some(crate::keymap::LocalContext::Mermaid)
        }),
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        // A choice in one of its pickers (a new diagram's type or starter,
        // a theme).
        picker_confirm: Some(|app, pick, _marked, _query| {
            if let Some(pick) = pick.downcast_ref::<DiagramPick>() {
                app.diagram_picked(pick.clone());
            }
        }),
        picker_cancel: None,
        modeline_clock: None,
        page_action: Some(|app, id, action| {
            if let Ok(action) = action.downcast::<DgAction>() {
                app.diagram_action(id, *action);
            }
        }),
        // What the page should show now -- worked out every frame, since
        // a drawing finishing or the source's cursor moving changes it.
        page_context: Some(|app, id, need| match need {
            super::feature::PageNeed::Layout { .. } => Box::new(app.diagram_precompute(id)),
            super::feature::PageNeed::Key => Box::new(()),
        }),
        page_title: None,
        page_after_layout: Some(|app, id| app.diagram_after_layout(id)),
        on_event: Some(|app, event| {
            if let Ok(job) = event.downcast::<DiagramJob>() {
                let DiagramJob::DiagramDrawn { key, result } = *job;
                app.apply_diagram_drawn(key, result);
            }
        }),
        settings: &SETTINGS,
        services: &SERVICES,
        contributes: &[],
        session_documents: None,
        session_reopen: None,
        home: None,
        home_activate: None,
        doctor: None,
    }
};

/// The diagrams' state. Like the notebook's, reached by type
/// (`Features::diagram_state`) by code that's still `App` methods; its
/// only instance hook, `diagnostics`, only reads.
#[derive(Default)]
pub(crate) struct DiagramsHost {
    pub(super) state: DiagramState,
}

impl super::feature::Feature for DiagramsHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    /// A diagram's drawing error, as a problem on its line.
    fn diagnostics(&self, path: &Path) -> Vec<lsp_types::Diagnostic> {
        self.state.diagnostics.get(path).cloned().unwrap_or_default()
    }
}

impl super::feature::Features {
    pub(super) fn diagram_state(&self) -> &DiagramState {
        &self.get::<DiagramsHost>().expect("the diagrams have no instance hooks that run code, so they're always in place").state
    }

    pub(super) fn diagram_state_mut(&mut self) -> &mut DiagramState {
        &mut self.get_mut::<DiagramsHost>().expect("the diagrams have no instance hooks that run code, so they're always in place").state
    }
}

/// A diagram, beside its source or on its own (`SPC m p`, `SPC m v`).
impl super::feature::FeaturePage for DiagramPage {
    fn feature(&self) -> &'static str {
        SPEC.id
    }

    fn title(&self) -> String {
        format!("*{}: {}*", if self.viewer { "view" } else { "diagram" }, self.title)
    }

    fn typing(&self) -> bool {
        DiagramPage::typing(self)
    }

    fn paste(&mut self, text: &str) {
        DiagramPage::paste(self, text);
    }

    fn changed(&self, ctx: &dyn std::any::Any) -> bool {
        matches!(ctx.downcast_ref::<Option<DiagramPre>>(), Some(Some(pre)) if App::diagram_changed(self, pre))
    }

    /// Takes in what `page_context` found -- the drawing, the node under
    /// the source's cursor -- and lays the page out.
    fn layout(&mut self, ctx: Box<dyn std::any::Any>, cols: usize) -> crate::page::Page {
        match ctx.downcast::<Option<DiagramPre>>().ok().and_then(|pre| *pre) {
            Some(pre) => App::diagram_apply(self, pre, cols),
            None => crate::page::Page::default(),
        }
    }

    fn key(&mut self, key: crate::page::Key, _ctx: Box<dyn std::any::Any>) -> Box<dyn std::any::Any> {
        Box::new(DiagramPage::key(self, key))
    }

    /// The selected node and the source cursor's, outlined.
    fn image_marks(&self) -> Vec<(usize, (f32, f32, f32, f32), bool)> {
        let (w, h) = self.drawn.size;
        if w <= 0.0 || h <= 0.0 {
            return Vec::new();
        }
        [(self.cursor_node, false), (self.selected, true)]
            .into_iter()
            .filter_map(|(i, strong)| i.and_then(|i| self.drawn.nodes.get(i)).map(|n| (0, (n.x / w, n.y / h, n.w / w, n.h / h), strong)))
            .collect()
    }
}

/// What the diagrams keep: drawings by key, problems by file.
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

/// What the diagrams feature offers others (the notebook's export): a
/// diagram's source in, drawn with `diagrams.export_theme` and
/// `diagrams.background`.
static SERVICES: [super::feature::Service; 9] = [
    // `{path}` -> the diagram in that file, in the viewer.
    super::feature::Service {
        name: "diagrams.view",
        call: Mut(|app, payload| {
            let call: PathCall = serde_json::from_str(payload).map_err(|e| format!("a diagrams call: {e}"))?;
            app.diagram_view_at(&call.path);
            Ok(String::new())
        }),
    },
    // `{}` -> a new diagram: its type, then a starter, then the notebook
    // (`notebook.new_diagram`) asks its name and keeps it.
    super::feature::Service {
        name: "diagrams.new",
        call: Mut(|app, _| {
            app.diagram_new();
            Ok(String::new())
        }),
    },
    // `{path}` -> a theme picked for the diagram in that file.
    super::feature::Service {
        name: "diagrams.pick_theme",
        call: Mut(|app, payload| {
            let call: PathCall = serde_json::from_str(payload).map_err(|e| format!("a diagrams call: {e}"))?;
            app.diagram_pick_theme_at(&call.path);
            Ok(String::new())
        }),
    },
    // `{source}` -> the source with Fenix's theme written into it, so it
    // renders the same anywhere Mermaid does.
    super::feature::Service { name: "diagrams.portable", call: Read(|app, payload| Ok(app.diagram_portable(&DiagramCall::from(payload)?.source))) },
    // `{source}` -> an SVG.
    super::feature::Service { name: "diagrams.svg", call: Read(|app, payload| app.diagram_svg(&DiagramCall::from(payload)?.source)) },
    // `{source}` -> the picture a page shows for it (`ImageKey::Diagram`),
    // drawn once it's asked for and kept for every page that shows it.
    super::feature::Service { name: "diagrams.picture", call: Read(|app, payload| Ok(app.diagram_key(&DiagramCall::from(payload)?.source, None).to_string())) },
    // `{source}` -> what's wrong with it once it's been drawn ("line 3:
    // ..."), else nothing.
    super::feature::Service { name: "diagrams.problem", call: Read(|app, payload| Ok(app.diagram_problem(&DiagramCall::from(payload)?.source).unwrap_or_default())) },
    // `{source, format: "svg" | "png", path}` -> the file written (by
    // mermaid-cli under `diagrams.export_with = "mmdc"`); the theme it
    // was drawn with back.
    super::feature::Service {
        name: "diagrams.export",
        call: Read(|app, payload| {
            let call = DiagramCall::from(payload)?;
            let path = call.path.ok_or("no path")?;
            app.diagram_export_file(&call.source, call.format.as_deref().unwrap_or("svg"), &path)
        }),
    },
    // `{source}` -> the picture (PNG, 2x) on the clipboard.
    super::feature::Service { name: "diagrams.copy_picture", call: Mut(|app, payload| app.diagram_copy_picture(&DiagramCall::from(payload)?.source).map(|()| String::new())) },
];

/// A theme of your own's fields in `diagrams.themes`.
const DIAGRAM_THEME: &[fenix_config::Field] = {
    use fenix_config::Field;
    &[
        Field { name: "name", label: "Name", range: None, default: None, choices: &[] },
        Field { name: "base", label: "Starts from", range: None, default: Some("base"), choices: &["base", "default", "neutral", "dark", "forest", "fenix"] },
        Field { name: "primaryColor", label: "Node fill", range: None, default: Some(""), choices: &[] },
        Field { name: "primaryBorderColor", label: "Node border", range: None, default: Some(""), choices: &[] },
        Field { name: "primaryTextColor", label: "Text", range: None, default: Some(""), choices: &[] },
        Field { name: "lineColor", label: "Lines", range: None, default: Some(""), choices: &[] },
        Field { name: "secondaryColor", label: "Second fill", range: None, default: Some(""), choices: &[] },
        Field { name: "background", label: "Background", range: None, default: Some(""), choices: &[] },
    ]
};

/// The diagrams feature's settings: `[diagrams]` in `settings.toml`.
static SETTINGS: [fenix_config::Setting; 6] = {
    use fenix_config::{Category::Notebook, Kind, Setting};
    [
        Setting::stored("diagrams.theme", Notebook, "Diagram theme", Kind::Text, "The theme a diagram is drawn in when it doesn't name one: fenix (follows the editor theme), default, neutral, dark, forest, base, or one of yours below.").with_default("fenix"),
        Setting::stored("diagrams.export_theme", Notebook, "Export theme", Kind::Text, "The theme exports are drawn in: same (as shown), or a theme's name.").with_default("same"),
        Setting::stored("diagrams.background", Notebook, "Export background", Kind::Choice(&["theme", "transparent", "white"]), "What's behind an exported diagram.").with_default("theme"),
        Setting::stored("diagrams.themes", Notebook, "Your themes", Kind::Records(DIAGRAM_THEME), "Themes of your own: Mermaid theme variables over a theme to start from. Colours as #rrggbb; empty ones keep the starting theme's."),
        Setting::stored("diagrams.export_with", Notebook, "Export with", Kind::Choice(&["fenix", "mmdc"]), "What draws exported files: Fenix, or mermaid-cli (mmdc, when it's installed) for output identical to mermaid.js.").with_default("fenix"),
        Setting::stored("diagrams.font", Notebook, "Diagram font", Kind::Font, "The font diagrams are drawn with.").with_default("the renderer's"),
    ]
};

/// A choice in one of the diagrams feature's pickers.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DiagramPick {
    /// A new diagram of `fenix_diagram::KINDS[i]`: its starter next.
    Kind(usize),
    /// A new diagram from this text: its name next.
    Starter(String),
    /// Write theme `name` into the diagram in `buffer`.
    Theme { buffer: BufferId, name: String },
}

/// One of the diagrams feature's pickers.
fn diagram_picker(title: &'static str, candidates: Vec<fenix_picker::Candidate<DiagramPick>>) -> ActivePicker {
    let candidates = candidates.into_iter().map(|c| super::feature::FeaturePick::candidate(c.label, c.payload)).collect();
    ActivePicker::Feature { feature: SPEC.id, badge: title, state: fenix_picker::PickerState::new(candidates) }
}

/// A diagrams service's payload naming a file.
#[derive(serde::Deserialize)]
struct PathCall {
    path: PathBuf,
}

/// A diagrams service's payload.
#[derive(serde::Deserialize)]
struct DiagramCall {
    source: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
}

impl DiagramCall {
    fn from(payload: &str) -> Result<Self, String> {
        serde_json::from_str(payload).map_err(|e| format!("a diagrams call: {e}"))
    }
}

impl App {
    /// `diagrams.export_theme`, unless it's "same" (the one it's drawn with).
    fn diagram_export_theme(&self) -> Option<String> {
        self.config.text_value("diagrams.export_theme").filter(|t| !t.trim().is_empty() && !t.eq_ignore_ascii_case("same")).map(str::to_string)
    }

    fn diagram_export_background(&self) -> fenix_diagram::Background {
        match self.config.text_value("diagrams.background") {
            Some("transparent") => fenix_diagram::Background::Transparent,
            Some("white") => fenix_diagram::Background::White,
            _ => fenix_diagram::Background::Theme,
        }
    }

    fn diagram_portable(&self, source: &str) -> String {
        fenix_diagram::portable(source, &self.diagram_themes(), self.diagram_export_theme().as_deref())
    }

    fn diagram_svg(&self, source: &str) -> Result<String, String> {
        let r = fenix_diagram::render(source, &self.diagram_themes(), self.diagram_export_theme().as_deref()).map_err(|d| format!("line {}: {}", d.line + 1, d.message))?;
        Ok(fenix_diagram::svg_with(&r.svg, self.diagram_export_background()))
    }

    fn diagram_export_file(&self, source: &str, format: &str, path: &Path) -> Result<String, String> {
        if self.config.text_value("diagrams.export_with") == Some("mmdc") {
            let bg = match self.diagram_export_background() {
                fenix_diagram::Background::Transparent => "transparent",
                _ => "white",
            };
            fenix_diagram::export_with_mmdc(&self.diagram_portable(source), path, bg, 2.0)?;
        } else {
            let svg = self.diagram_svg(source)?;
            let bytes = match format {
                "png" => fenix_diagram::png(&svg, 2.0, fenix_diagram::Background::Theme)?,
                _ => svg.into_bytes(),
            };
            fenix_storage::write(path, &bytes).map_err(|e| e.to_string())?;
        }
        Ok(self.diagram_export_theme().unwrap_or_default())
    }

    fn diagram_copy_picture(&mut self, source: &str) -> Result<(), String> {
        let svg = self.diagram_svg(source)?;
        let (w, h, px) = fenix_diagram::rasterize(&svg, 2.0, fenix_diagram::Background::Theme)?;
        let clipboard = self.clipboard.as_mut().ok_or("no clipboard")?;
        clipboard.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: px.into() }).map_err(|e| e.to_string())
    }

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
            .records_value("diagrams.themes")
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
            default: self.config.text_value("diagrams.theme").filter(|s| !s.trim().is_empty()).unwrap_or("fenix").to_string(),
            custom,
            editor,
            font: self.config.text_value("diagrams.font").filter(|s| !s.trim().is_empty()).map(str::to_string),
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
        self.config.text_value("diagrams.theme").hash(&mut h);
        self.config.records_value("diagrams.themes").hash(&mut h);
        self.config.text_value("diagrams.font").hash(&mut h);
        let key = h.finish();
        self.features.diagram_state().sources.borrow_mut().entry(key).or_insert_with(|| (source.to_string(), over.map(str::to_string)));
        key
    }

    /// Starts drawing a diagram picture, when it isn't drawn or drawing.
    pub(super) fn request_diagram_image(&mut self, key: ImageKey) {
        let ImageKey::Diagram(h) = key else { return };
        if self.features.diagram_state().results.contains_key(&h) || self.features.diagram_state().pending.contains(&h) {
            return;
        }
        let Some((source, over)) = self.features.diagram_state().sources.borrow().get(&h).cloned() else { return };
        self.features.diagram_state_mut().pending.insert(h);
        let themes = self.diagram_themes();
        self.page_spawn(move |send| {
            let result = fenix_diagram::render(&source, &themes, over.as_deref()).map_err(|d| (d.line, d.message)).and_then(|r| {
                let (w, h, rgba) = fenix_diagram::rasterize(&r.svg, DIAGRAM_SCALE, fenix_diagram::Background::Theme).map_err(|e| (0, e))?;
                Ok((Drawn { size: (r.width, r.height), nodes: r.nodes, theme: r.theme }, w, h, super::reading_host::rgba_to_bgra(rgba)))
            });
            send(PageEvent::feature(SPEC.id, "DiagramDrawn", DiagramJob::DiagramDrawn { key: h, result }));
        });
    }

    /// A drawing arrived: its picture kept for upload, pages showing it
    /// laid out again, its problem (or none) on its file.
    pub(super) fn apply_diagram_drawn(&mut self, h: u64, result: Result<(Drawn, u32, u32, Vec<u8>), (usize, String)>) {
        self.features.diagram_state_mut().pending.remove(&h);
        let key = ImageKey::Diagram(h);
        let stored = match result {
            Ok((drawn, w, hh, bgra)) => {
                self.page_images.image_sizes.insert(key.clone(), (drawn.size.0.round() as u32, drawn.size.1.round() as u32));
                self.page_images.textures.insert(key.clone(), PageTex { w, h: hh, bgra: Some(bgra), texture: None });
                Ok(drawn)
            }
            Err(e) => Err(e),
        };
        self.features.diagram_state_mut().results.insert(h, stored);
        self.evict_pictures();
        for state in self.pages.values_mut() {
            match &mut state.model {
                PageModel::Reading(p) => {
                    p.seen = None;
                    state.stale = true;
                }
                PageModel::Feature(page) if page.shows_pictures() || (&**page as &dyn std::any::Any).is::<DiagramPage>() => state.stale = true,
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
        if self.page_images.textures.len() < 48 {
            return;
        }
        let mut keep: std::collections::HashSet<ImageKey> = self.pages.values().flat_map(|s| s.page.images.iter().map(|i| i.key.clone())).collect();
        for s in self.pages.values() {
            if let Some(p) = page_as::<DiagramPage>(&s.model) {
                keep.extend(p.key.clone());
                keep.extend(p.want.clone());
            }
        }
        self.page_images.textures.retain(|k, _| keep.contains(k));
        self.page_images.image_sizes.retain(|k, _| keep.contains(k));
        let hashes: std::collections::HashSet<u64> = keep.iter().filter_map(|k| if let ImageKey::Diagram(h) = k { Some(*h) } else { None }).collect();
        self.features.diagram_state_mut().results.retain(|h, _| hashes.contains(h));
        self.features.diagram_state().sources.borrow_mut().retain(|h, _| hashes.contains(h));
    }

    /// Why `source` doesn't draw, once it's been tried.
    pub(super) fn diagram_problem(&self, source: &str) -> Option<String> {
        let h = self.diagram_key(source, None);
        match self.features.diagram_state().results.get(&h) {
            Some(Err((line, message))) => Some(format!("line {}: {message}", line + 1)),
            _ => None,
        }
    }

    // ------------------------------------------------------------------
    // The page
    // ------------------------------------------------------------------

    /// Before a diagram page is borrowed: what it should show now.
    pub(super) fn diagram_precompute(&self, id: BufferId) -> Option<DiagramPre> {
        let p = self.feature_page::<DiagramPage>(id)?;
        let rows = self.features.diagram_state().rows.get(&id).copied().unwrap_or(30);
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
        let ready = target.and_then(|h| self.features.diagram_state().results.get(&h).cloned());
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
        let Some(p) = self.feature_page::<DiagramPage>(id) else { return };
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
                self.features.diagram_state_mut().diagnostics.insert(path, vec![d]);
            }
            None => {
                self.features.diagram_state_mut().diagnostics.remove(&path);
            }
        }
    }

    /// Records how tall a page's pane is; a page that fills it is laid
    /// out again when that changes.
    pub(super) fn note_page_rows(&mut self, id: BufferId, rows: usize) {
        if self.features.diagram_state_mut().rows.insert(id, rows) != Some(rows) {
            if let Some(state) = self.pages.get_mut(&id) {
                if page_as::<DiagramPage>(&state.model).is_some() {
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
        if self.feature_page::<DiagramPage>(source).is_some() {
            // On the preview: close it.
            self.diagram_action(source, DgAction::Close);
            return;
        }
        if let Some(id) = self.find_page(|m| page_as::<DiagramPage>(m).is_some_and(|p| p.source == Some(source) && !p.viewer)) {
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
        if self.find_page(|m| page_as::<DiagramPage>(m).is_some_and(|p| p.source == Some(source) && !p.viewer)).is_some() {
            return;
        }
        let title = self.diagram_title(source);
        let origin = self.focused_pane_id();
        self.open_page_beside(PageModel::Feature(Box::new(DiagramPage::new(Some(source), title, false))), 0.5);
        self.windows_mut().focus(origin);
    }

    /// A diagram's name: the notebook's (`notebook.entry_name`), else its
    /// file's.
    fn diagram_title(&self, source: BufferId) -> String {
        let path = self.buffers.get(source).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
        let kept = path.as_ref().and_then(|p| self.query_service("notebook.entry_name", &serde_json::json!({ "path": p })).ok()).filter(|n| !n.is_empty());
        match kept {
            Some(name) => name,
            None => path.and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_else(|| "diagram".into()),
        }
    }

    /// `v` / `SPC m v`: the diagram on its own, in this pane.
    pub(crate) fn open_diagram_viewer(&mut self) {
        let source = match self.feature_page::<DiagramPage>(self.focused_buffer_id()) {
            Some(p) => p.source,
            None => Some(self.focused_buffer_id()),
        };
        let Some(source) = source else { return };
        let title = self.diagram_title(source);
        let mut page = DiagramPage::new(Some(source), title, true);
        page.zoom = 1.0;
        self.open_page(PageModel::Feature(Box::new(page)));
    }

    /// `diagrams.view`: the viewer for the diagram in `path` (a notebook
    /// diagram, from the notebook page).
    fn diagram_view_at(&mut self, path: &Path) {
        let buffer = match self.buffers.id_for_path(path) {
            Some(b) => b,
            None => {
                let b = self.buffers.open_path(path);
                self.note_disk_state(b);
                b
            }
        };
        let title = self.diagram_title(buffer);
        self.open_page(PageModel::Feature(Box::new(DiagramPage::new(Some(buffer), title, true))));
    }

    pub(super) fn diagram_action(&mut self, page_id: BufferId, action: DgAction) {
        let Some(p) = self.feature_page::<DiagramPage>(page_id) else { return };
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
                    self.place_cursor(line, 0);
                }
            }
            DgAction::CycleTheme => {
                let names = self.diagram_themes().names();
                if let Some(p) = self.feature_page_mut::<DiagramPage>(page_id) {
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

    /// A choice in one of its pickers.
    fn diagram_picked(&mut self, pick: DiagramPick) {
        match pick {
            DiagramPick::Kind(i) => self.diagram_kind_picked(i),
            DiagramPick::Starter(text) => self.diagram_starter_picked(text),
            DiagramPick::Theme { buffer, name } => self.diagram_set_theme(buffer, &name),
        }
    }

    /// `diagrams.new` (`SPC n d`): pick a type, a starter, then a name.
    fn diagram_new(&mut self) {
        let candidates = fenix_diagram::KINDS
            .iter()
            .enumerate()
            .map(|(i, k)| fenix_picker::Candidate::new(format!("{:<7} {:<19} {}", k.tag, k.header, k.about), DiagramPick::Kind(i)))
            .collect();
        self.completion = None;
        self.enter_picker(diagram_picker("DIAGRAM", candidates));
    }

    pub(super) fn diagram_kind_picked(&mut self, i: usize) {
        let Some(kind) = fenix_diagram::KINDS.get(i) else { return };
        let mut candidates: Vec<fenix_picker::Candidate<DiagramPick>> = fenix_diagram::starters(kind)
            .into_iter()
            .map(|(name, text)| fenix_picker::Candidate::new(format!("{name:<10} {}", text.lines().nth(1).unwrap_or("").trim()), DiagramPick::Starter(text.to_string())))
            .collect();
        // Yours: Mermaid snippets whose trigger starts with the type.
        let catalog = self.snippet_layers(self.project_root.as_ref().map(|r| r.join(".fenix").join("snippets")).as_deref());
        for s in &catalog.snippets {
            if s.scopes.iter().any(|x| x == "mermaid" || x == "mmd") {
                let body = s.template.render(&Default::default(), &fenix_snippets::Context::current(None), "").text;
                if fenix_diagram::kind(&body).map(|k| k.header) == Some(kind.header) {
                    candidates.push(fenix_picker::Candidate::new(format!("{:<10} yours ({})", s.name, s.trigger), DiagramPick::Starter(body)));
                }
            }
        }
        self.enter_picker(diagram_picker("STARTER", candidates));
    }

    /// The starter's text to the notebook, which asks its name and keeps it.
    fn diagram_starter_picked(&mut self, text: String) {
        if let Err(e) = self.call_service("notebook.new_diagram", &serde_json::json!({ "text": text })) {
            self.set_error(format!("couldn't make the diagram: {e}"));
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
                fenix_picker::Candidate::new(format!("{mark} {n:<10} {}", about(&n)), DiagramPick::Theme { buffer: source, name: n })
            })
            .collect();
        self.completion = None;
        self.enter_picker(diagram_picker("THEME", candidates));
    }

    /// `SPC m t` in a diagram.
    pub(crate) fn diagram_pick_theme_here(&mut self) {
        let source = match self.feature_page::<DiagramPage>(self.focused_buffer_id()) {
            Some(p) => p.source,
            None => self.in_mermaid().then(|| self.focused_buffer_id()),
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
            if let PageModel::Feature(p) = &mut state.model {
                if let Some(p) = (&mut **p as &mut dyn std::any::Any).downcast_mut::<DiagramPage>() {
                    if p.source == Some(source) {
                        p.theme_over = None;
                        p.seen = None;
                        state.stale = true;
                    }
                }
            }
        }
        self.set_message(format!("theme: {name}"));
    }

    /// `diagrams.pick_theme`: a theme for the diagram in `path` (from the
    /// notebook page).
    fn diagram_pick_theme_at(&mut self, path: &Path) {
        let buffer = match self.buffers.id_for_path(path) {
            Some(b) => b,
            None => {
                let b = self.buffers.open_path(path);
                self.note_disk_state(b);
                b
            }
        };
        self.diagram_pick_theme_for(buffer);
    }
}

#[cfg(test)]
mod tests {
    use super::super::notebook_host::{NotebookPrompt, NotebookPromptKind};
    use super::*;

    /// What the notebook's export gets from the diagrams services.
    #[test]
    fn the_diagrams_services_draw_export_and_refuse_a_bad_call() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        let flow = "flowchart TD\n    a --> b\n";
        let source = serde_json::json!({ "source": flow });
        assert!(app.call_service("diagrams.svg", &source).unwrap().starts_with("<svg"));
        assert!(app.call_service("diagrams.portable", &source).unwrap().starts_with("---\nconfig:\n  theme: base\n"));
        let png = dir.path().join("flow.png");
        let call = serde_json::json!({ "source": flow, "format": "png", "path": png });
        assert_eq!(app.call_service("diagrams.export", &call), Ok(String::new()), "drawn with the theme it's shown in");
        assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
        let bad = app.call_service("diagrams.svg", &serde_json::json!({ "source": "flowchart TD\n    a -->\n" })).unwrap_err();
        assert!(bad.starts_with("line "), "{bad}");
        assert!(app.call_service("diagrams.svg", &serde_json::json!({ "text": "x" })).unwrap_err().contains("source"));
        // What a page shows for it: the picture's key, and its problem once drawn.
        let key: u64 = app.query_service("diagrams.picture", &source).unwrap().parse().unwrap();
        assert_eq!(key, app.diagram_key(flow, None));
        assert_eq!(app.query_service("diagrams.problem", &source), Ok(String::new()));
    }

    fn pick_first(app: &mut App, badge: &str) {
        match &app.active_picker {
            Some(ActivePicker::Feature { feature: "fenix.diagrams", badge: b, .. }) => assert_eq!(*b, badge),
            _ => panic!("the diagrams feature's {badge} picker"),
        }
        app.picker_confirm();
    }

    /// `SPC n d`: the notebook asks the diagrams feature for a new one;
    /// its type and starter are the diagrams feature's pickers; the
    /// notebook asks the name and keeps it.
    #[test]
    fn a_new_diagram_goes_from_the_notebook_to_the_diagrams_pickers_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.set_path_value("notebook.folder", Some(&dir.path().join("nb")));
        app.notebook_new_diagram();
        pick_first(&mut app, "DIAGRAM");
        pick_first(&mut app, "STARTER");
        let Some(NotebookPrompt { kind: NotebookPromptKind::DiagramName { text }, .. }) = app.features.notebook_state().prompt.clone() else { panic!("the name prompt") };
        assert!(text.starts_with("flowchart"), "{text}");
        app.features.notebook_state_mut().prompt = None;
        app.notebook_create_diagram("Fresh", &text);
        let entry = app.features.notebook_state().book.as_ref().unwrap().by_name("Fresh").cloned().unwrap();
        // Its theme, from the notebook page, through `diagrams.pick_theme`.
        app.call_service("diagrams.pick_theme", &serde_json::json!({ "path": entry.path })).unwrap();
        pick_first(&mut app, "THEME");
        assert!(app.buffers.get(app.buffers.id_for_path(&entry.path).unwrap()).unwrap().buffer.text().contains("theme:"));
        // Its name, the notebook's (`notebook.entry_name`); a file the
        // notebook doesn't keep has none.
        assert_eq!(app.query_service("notebook.entry_name", &serde_json::json!({ "path": entry.path })), Ok("Fresh".to_string()));
        assert_eq!(app.query_service("notebook.entry_name", &serde_json::json!({ "path": dir.path().join("elsewhere.mmd") })), Ok(String::new()));
        // `v` on the notebook page: the viewer, through `diagrams.view`,
        // titled with the notebook's name.
        app.call_service("diagrams.view", &serde_json::json!({ "path": entry.path })).unwrap();
        let viewer = app.feature_page::<DiagramPage>(app.focused_buffer_id()).expect("the viewer");
        assert!(viewer.viewer && viewer.title == "Fresh", "{}", viewer.title);
    }

    fn preview(app: &App) -> (BufferId, fenix_window::WindowId) {
        let id = app.find_page(|m| page_as::<DiagramPage>(m).is_some()).expect("the preview");
        let pane = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&id)).unwrap();
        (id, pane)
    }

    fn page(app: &App, id: BufferId) -> &DiagramPage {
        app.feature_page::<DiagramPage>(id).expect("a diagram page")
    }

    #[test]
    fn a_new_diagram_draws_beside_its_source_and_keeps_the_last_good_drawing() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.set_path_value("notebook.folder", Some(&dir.path().join("nb")));
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
        assert!(app.page_images.textures.get(&key).is_some_and(|t| t.w > 0));
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
        assert_eq!(app.features.diagram_state().diagnostics.get(&path).map(|d| d[0].range.start.line), Some(2));
        // A theme written into the source.
        app.diagram_set_theme(source, "forest");
        assert!(app.buffers.get(source).unwrap().buffer.text().starts_with("---\nconfig:\n  theme: forest\n---\n"));
    }

    // Through its spec and hooks, the way `App` reaches the diagrams now.

    #[test]
    fn the_diagrams_contribute_their_commands_and_mermaid_menu_from_their_spec() {
        let registry = CommandRegistry::with_builtins();
        let ids: Vec<&str> = registry.iter().filter(|c| c.id.starts_with("diagram.")).map(|c| c.id.as_ref()).collect();
        assert_eq!(ids, ["diagram.preview", "diagram.viewer", "diagram.theme", "diagram.export"]);
        let menu = keymap::local_trie(&[crate::keymap::LocalContext::Mermaid]);
        assert!(matches!(menu.matcher().feed(KeyPress::char('v')), fenix_keymap::Step::Matched(&"diagram.viewer")));
    }

    #[test]
    fn a_drawing_error_reaches_the_files_diagnostics_through_the_hook() {
        let mut app = App::with_file(None);
        let path = PathBuf::from("flow.mmd");
        let problem = lsp_types::Diagnostic { message: "unknown or missing Mermaid diagram header".into(), ..Default::default() };
        app.features.diagram_state_mut().diagnostics.insert(path.clone(), vec![problem]);
        let found: Vec<_> = app.features.iter().flat_map(|f| f.diagnostics(&path)).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(app.all_diagnostics(&path).len(), 1);
    }

    #[test]
    fn a_diagram_page_is_the_diagrams_with_their_mermaid_menu_and_node_outlines() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.set_path_value("notebook.folder", Some(&dir.path().join("nb")));
        app.notebook_create_diagram("Outline", "flowchart TD
    a[One] --> b[Two]
");
        let (id, pane) = preview(&app);
        app.ensure_page_layout(id, pane, 80);
        app.ensure_page_layout(id, pane, 80);
        let Some(PageModel::Feature(p)) = app.pages.get(&id).map(|s| &s.model) else { panic!("a feature page") };
        assert_eq!(p.feature(), "fenix.diagrams");
        assert!(app.page_title(id).starts_with("*diagram: "), "{}", app.page_title(id));
        if let Some(page) = app.feature_page_mut::<DiagramPage>(id) {
            page.selected = Some(1);
        }
        let Some(PageModel::Feature(p)) = app.pages.get(&id).map(|s| &s.model) else { unreachable!() };
        assert!(p.image_marks().iter().any(|&(_, _, strong)| strong), "the selected node, outlined");
        app.windows_mut().focus(pane);
        assert!(app.local_contexts().contains(&crate::keymap::LocalContext::Mermaid), "the Mermaid menu, from the diagrams' spec");
    }

    /// `SPC m v` from a focused preview: the page leaves the menu's key
    /// to the menu, which opens the viewer.
    #[test]
    fn the_mermaid_menu_gets_its_key_from_a_focused_preview() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.set_path_value("notebook.folder", Some(&dir.path().join("nb")));
        app.notebook_create_diagram("Menu", "flowchart TD
    a --> b
");
        let (id, pane) = preview(&app);
        app.windows_mut().focus(pane);
        assert_eq!(app.focused_buffer_id(), id);
        app.start_local_leader();
        assert!(app.local_matcher.is_some());
        // The order the window dispatches in: the page first, then the menu.
        assert!(!app.page_key(KeyPress::char('v')), "the page leaves the menu's key alone");
        assert_eq!(app.local_leader_key(KeyPress::char('v')), Some(Some("diagram.viewer")));
        assert!(app.local_matcher.is_none());
        app.open_diagram_viewer();
        assert!(page(&app, app.focused_buffer_id()).viewer, "the viewer opens");
        // With the menu closed, the page has its keys back.
        assert!(app.page_key(KeyPress::char('j')));
    }
}
