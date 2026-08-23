use std::collections::HashMap;

use glyphon::{
    Attrs, Buffer as GlyphBuffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};

use fenix_window::{Rect, WindowId as PaneId};

use crate::gpu::GpuState;
use crate::icon::ICON_FONT_FAMILY;
use crate::theme::Theme;

pub const FONT_SIZE: f32 = 16.0;
pub const LINE_HEIGHT: f32 = 20.0;
/// A rough monospace advance-width fallback -- used only by
/// `TextPipeline::measure_char_width` if shaping ever somehow fails to
/// produce usable glyphs. Real per-column pixel math (caret, selection,
/// badge sizing) should use `TextPipeline::char_width()` instead, which
/// measures the *actual* active font's advance width rather than
/// assuming this ratio holds -- it doesn't across fonts (the bundled
/// TempleOS bitmap font is ~1.0x its em size, not ~0.6x).
pub const CHAR_WIDTH: f32 = FONT_SIZE * 0.6;
pub const PAD_LEFT: f32 = 8.0;
pub const PAD_TOP: f32 = 4.0;
pub const MODELINE_HEIGHT: f32 = LINE_HEIGHT + 8.0;
/// Width of the which-key popup panel.
pub const WHICH_KEY_WIDTH: f32 = 260.0;
/// Gap between the which-key panel and the window's top/right edges.
pub const WHICH_KEY_MARGIN: f32 = 12.0;
/// Width (in chars) of the modeline's mode badge, centered within it.
/// Fits the longest label ("V-BLOCK"/"REPLACE"/"COMMAND", 7 chars) with a
/// little breathing room.
pub const MODE_BADGE_CHARS: usize = 8;
/// Width of the file-explorer sidebar panel.
pub const SIDEBAR_WIDTH: f32 = 240.0;

/// A community TTF conversion (github.com/rendello/templeos_font) of
/// TempleOS's actual 8x8 bitmap font, embedded so the TempleOS theme
/// looks right on any machine without needing the font installed --
/// same reasoning as bundling any other asset the app depends on.
/// Registered into `FontSystem`'s font database at `TextPipeline::new`
/// time via `load_font_data`; its family name (confirmed with
/// `fc-scan`, not guessed) is `"TempleOS"`, matching `Theme::
/// font_family` on the `TEMPLEOS` theme.
static TEMPLEOS_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/templeos_font.ttf");

/// Shapes and rasterizes buffer text into the wgpu glyph atlas via glyphon.
///
/// Holds one independent glyph buffer per visible window pane
/// (`content_buffers`, keyed by `PaneId` -- created lazily the first time
/// a pane renders, dropped once it stops appearing in a `prepare()` call
/// since splits/closes churn pane ids) plus three fixed panels:
/// `modeline` for the single-line status bar, `which_key` for the popup
/// listing a pending key sequence's continuations, and `sidebar` for the
/// file explorer's persistent side panel. All share one atlas/renderer/
/// font system -- glyphon's `TextRenderer::prepare` already takes an
/// arbitrary-length `Vec<TextArea>`, so N panes is just N more entries,
/// not a different rendering pipeline.
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    content_buffers: HashMap<PaneId, GlyphBuffer>,
    modeline: GlyphBuffer,
    which_key: GlyphBuffer,
    sidebar: GlyphBuffer,
    /// Body-text font family for the active theme -- `None` resolves to
    /// `Family::Monospace` (see `content_family`), `Some(name)` to
    /// `Family::Name(name)`. Icon spans in `rich_spans` are unaffected,
    /// always `ICON_FONT_FAMILY` regardless of this.
    content_family: Option<&'static str>,
    /// The active font's real, measured monospace advance width in
    /// pixels at `FONT_SIZE` -- recomputed only when `content_family`
    /// actually changes (see `set_theme`), not every frame. Different
    /// fonts have very different advance-to-em ratios (the bundled
    /// TempleOS bitmap font is a full square cell, ~1.0x its em size,
    /// vastly wider than a typical outline monospace font's ~0.6x), so
    /// a single hardcoded ratio (`CHAR_WIDTH`) breaks caret/column
    /// alignment the moment a second font enters the mix -- callers
    /// needing per-column pixel math should use `char_width()`, not
    /// the `CHAR_WIDTH` constant.
    char_width: f32,
}

impl TextPipeline {
    pub fn new(gpu: &GpuState) -> Self {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(TEMPLEOS_FONT_BYTES.to_vec());
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&gpu.device);
        let viewport = Viewport::new(&gpu.device, &cache);
        let mut atlas = TextAtlas::new(&gpu.device, &gpu.queue, &cache, gpu.config.format);
        let renderer =
            TextRenderer::new(&mut atlas, &gpu.device, wgpu::MultisampleState::default(), None);

        let mut modeline = GlyphBuffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        modeline.set_wrap(Wrap::None);
        modeline.set_size(Some(gpu.size.width as f32), Some(MODELINE_HEIGHT));

        let mut which_key = GlyphBuffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        which_key.set_wrap(Wrap::None);
        which_key.set_size(Some(WHICH_KEY_WIDTH), None);

        let mut sidebar = GlyphBuffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        sidebar.set_wrap(Wrap::None);
        sidebar.set_size(Some(SIDEBAR_WIDTH), Some(content_height(gpu.size.height as f32)));

        let char_width = Self::measure_char_width(&mut font_system, Family::Monospace);

        Self {
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            content_buffers: HashMap::new(),
            modeline,
            which_key,
            sidebar,
            content_family: None,
            char_width,
        }
    }

    /// Resolves the active theme's body-text font: `Family::Name(_)` when
    /// `Theme::font_family` names one, else the generic `Family::Monospace`
    /// fallback every theme used before this existed.
    fn content_family(&self) -> Family<'static> {
        self.content_family.map(Family::Name).unwrap_or(Family::Monospace)
    }

    /// Adopts `theme`'s font choice for all subsequent `set_*` calls --
    /// called once per `redraw()`, before them. A no-op beyond the
    /// equality check when the family hasn't actually changed since
    /// last frame, so re-measuring `char_width` (a real shaping pass)
    /// only happens on an actual theme/font switch, not every redraw.
    pub fn set_theme(&mut self, theme: &Theme) {
        if self.content_family == theme.font_family {
            return;
        }
        self.content_family = theme.font_family;
        let family = self.content_family();
        self.char_width = Self::measure_char_width(&mut self.font_system, family);
    }

    /// The active font's real monospace advance width in pixels --
    /// callers doing per-column pixel math (caret, selection, badge
    /// sizing) should use this instead of the `CHAR_WIDTH` constant.
    pub fn char_width(&self) -> f32 {
        self.char_width
    }

    /// Measures `family`'s real advance width by shaping two characters
    /// and taking the pixel distance between them, rather than assuming
    /// a fixed ratio of `FONT_SIZE` -- see the `char_width` field's doc
    /// comment for why a single hardcoded ratio doesn't hold across
    /// fonts. Falls back to the old assumed `FONT_SIZE * 0.6` ratio only
    /// if shaping somehow produces fewer than two glyphs (should not
    /// happen for two ordinary ASCII letters, but a shaping failure
    /// shouldn't be able to divide-by-zero or panic downstream).
    fn measure_char_width(font_system: &mut FontSystem, family: Family<'static>) -> f32 {
        let mut probe = GlyphBuffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        probe.set_wrap(Wrap::None);
        probe.set_size(Some(1000.0), Some(LINE_HEIGHT));
        probe.set_text("MM", &Attrs::new().family(family), Shaping::Advanced, None);
        probe.shape_until_scroll(font_system, false);
        probe
            .layout_runs()
            .next()
            .and_then(|run| {
                let mut glyphs = run.glyphs.iter();
                let a = glyphs.next()?;
                let b = glyphs.next()?;
                Some(b.x - a.x)
            })
            .filter(|w| *w > 0.0)
            .unwrap_or(FONT_SIZE * 0.6)
    }

    /// Builds rich-text spans from `(text, color, use_icon_font)` triples --
    /// the icon flag switches that one span to `ICON_FONT_FAMILY` instead
    /// of the body font, letting an icon glyph and ordinary text sit in
    /// the same row (same mechanism `content_spans`/explorer row building
    /// already mix gutter numbers and line text with).
    fn rich_spans<'a>(&self, segments: &'a [(&'a str, Color, bool)]) -> Vec<(&'a str, Attrs<'a>)> {
        let body_family = self.content_family();
        segments
            .iter()
            .map(|(text, color, is_icon)| {
                let family = if *is_icon { Family::Name(ICON_FONT_FAMILY) } else { body_family };
                (*text, Attrs::new().family(family).color(*color))
            })
            .collect()
    }

    /// Sets one pane's content text (rich, icon-aware spans same as
    /// `content_spans`/explorer row building produce) and its pixel size
    /// (that pane's current layout rect) in one call -- creating the
    /// pane's `GlyphBuffer` lazily the first time it's rendered. Doesn't
    /// use `HashMap::entry`/`or_insert_with`: that would need a closure
    /// capturing `&mut self.font_system` to construct a fresh buffer,
    /// which can't then *also* be reborrowed for `shape_until_scroll`
    /// afterward (the closure would move it) -- a plain contains-key
    /// check plus a fresh `get_mut` avoids the conflict.
    pub fn set_pane_rich(&mut self, pane: PaneId, w: f32, h: f32, segments: &[(&str, Color, bool)]) {
        let spans = self.rich_spans(segments);
        let default_attrs = Attrs::new().family(self.content_family());

        if !self.content_buffers.contains_key(&pane) {
            let mut buf = GlyphBuffer::new(&mut self.font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
            buf.set_wrap(Wrap::None);
            self.content_buffers.insert(pane, buf);
        }
        let buf = self.content_buffers.get_mut(&pane).expect("just inserted if missing");
        buf.set_size(Some(w), Some(h));
        buf.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.font_system, false);
    }

    /// Drops every pane `GlyphBuffer` not in `keep` -- splits/closes churn
    /// `PaneId`s, so without this, closed panes' buffers (and their glyph
    /// atlas entries) would accumulate forever.
    pub fn retain_panes(&mut self, keep: &[PaneId]) {
        self.content_buffers.retain(|id, _| keep.contains(id));
    }

    /// Sets the modeline text as a sequence of differently-colored spans
    /// (e.g. the mode badge in its own accent, the rest in the normal
    /// modeline color) -- a single flat color can't do that, so this
    /// takes rich text instead of a plain string like `set_which_key_text`.
    pub fn set_modeline_text(&mut self, segments: &[(&str, Color)]) {
        let body_family = self.content_family();
        let default_attrs = Attrs::new().family(body_family);
        let spans: Vec<(&str, Attrs)> =
            segments.iter().map(|(text, color)| (*text, Attrs::new().family(body_family).color(*color))).collect();
        self.modeline.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.modeline.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn set_which_key_text(&mut self, text: &str) {
        self.which_key.set_text(text, &Attrs::new().family(self.content_family()), Shaping::Advanced, None);
        self.which_key.shape_until_scroll(&mut self.font_system, false);
    }

    /// Sets the sidebar's rows -- same icon/color rich-text mechanism as
    /// `set_pane_rich`, into its own independent glyph buffer since the
    /// sidebar renders alongside the editor content, not interleaved
    /// with it.
    pub fn set_sidebar_rich(&mut self, segments: &[(&str, Color, bool)]) {
        let default_attrs = Attrs::new().family(self.content_family());
        let spans = self.rich_spans(segments);
        self.sidebar.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.sidebar.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        // Pane buffers are resized every frame in `set_pane_rich` (their
        // rect comes from `WindowTree::layout`, recomputed each redraw
        // regardless of whether the window itself just resized), so only
        // the three fixed panels need handling here.
        self.modeline.set_size(Some(width), Some(MODELINE_HEIGHT));
        self.modeline.shape_until_scroll(&mut self.font_system, false);
        self.sidebar.set_size(Some(SIDEBAR_WIDTH), Some(content_height(height)));
        self.sidebar.shape_until_scroll(&mut self.font_system, false);
    }

    /// One pane's render info for `prepare`: its id (to look up the right
    /// `GlyphBuffer`), its current pixel rect, and the sub-line-height
    /// fractional offset to shift its content up by (0.0 for panes that
    /// don't animate scroll, i.e. everything but the focused pane's own
    /// smooth-scroll transition).
    pub fn prepare(
        &mut self,
        gpu: &GpuState,
        theme: &Theme,
        panes: &[(PaneId, Rect, f32)],
        which_key_panel: Option<(f32, f32, f32)>,
        sidebar_open: bool,
    ) {
        self.viewport.update(
            &gpu.queue,
            Resolution { width: gpu.config.width, height: gpu.config.height },
        );

        let modeline_top = gpu.size.height as f32 - MODELINE_HEIGHT;

        let mut areas = Vec::with_capacity(panes.len() + 3);
        for &(pane, rect, content_frac) in panes {
            let Some(buffer) = self.content_buffers.get(&pane) else { continue };
            areas.push(TextArea {
                buffer,
                left: rect.x + PAD_LEFT,
                top: rect.y + PAD_TOP - content_frac * LINE_HEIGHT,
                scale: 1.0,
                bounds: TextBounds {
                    left: rect.x as i32,
                    top: rect.y as i32,
                    right: (rect.x + rect.w) as i32,
                    bottom: (rect.y + rect.h) as i32,
                },
                default_color: theme.fg,
                custom_glyphs: &[],
            });
        }

        let modeline_bounds = TextBounds {
            left: 0,
            top: modeline_top as i32,
            right: gpu.config.width as i32,
            bottom: gpu.config.height as i32,
        };
        areas.push(TextArea {
            buffer: &self.modeline,
            left: PAD_LEFT,
            top: modeline_top + 4.0,
            scale: 1.0,
            bounds: modeline_bounds,
            default_color: theme.fg_modeline,
            custom_glyphs: &[],
        });

        if let Some((left, top, height)) = which_key_panel {
            areas.push(TextArea {
                buffer: &self.which_key,
                left: left + PAD_LEFT,
                top: top + 4.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: left as i32,
                    top: top as i32,
                    right: (left + WHICH_KEY_WIDTH) as i32,
                    bottom: (top + height) as i32,
                },
                default_color: theme.fg_modeline,
                custom_glyphs: &[],
            });
        }
        if sidebar_open {
            areas.push(TextArea {
                buffer: &self.sidebar,
                left: PAD_LEFT,
                top: PAD_TOP,
                scale: 1.0,
                bounds: TextBounds { left: 0, top: 0, right: SIDEBAR_WIDTH as i32, bottom: modeline_top as i32 },
                default_color: theme.fg,
                custom_glyphs: &[],
            });
        }

        self.renderer
            .prepare(
                &gpu.device,
                &gpu.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .expect("glyphon prepare failed");
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        self.renderer.render(&self.atlas, &self.viewport, pass).expect("glyphon render failed");
    }

    pub fn trim(&mut self) {
        self.atlas.trim();
    }
}

fn content_height(window_height: f32) -> f32 {
    (window_height - MODELINE_HEIGHT).max(0.0)
}

/// How many full text lines fit in a content area of the given pixel
/// height (already excluding the modeline, e.g. a pane's own rect
/// height) -- used both for the whole-window case (`content_height`
/// applied first) and per-pane.
pub fn lines_that_fit(height: f32) -> usize {
    (height / LINE_HEIGHT).floor().max(1.0) as usize
}

/// How many full text lines fit in the content area above the modeline,
/// for the single-pane/whole-window case.
pub fn visible_line_count(window_height: f32) -> usize {
    lines_that_fit(content_height(window_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_templeos_font_loads_and_resolves_by_the_expected_family_name() {
        // No GPU needed for this -- `FontSystem`/`fontdb` are pure font
        // data structures; only `TextPipeline` itself needs a `GpuState`.
        // This guards against the embedded bytes going stale/corrupt, or
        // the family name (`Theme::font_family` on `TEMPLEOS`, confirmed
        // with `fc-scan` at the time this was bundled) silently drifting.
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(TEMPLEOS_FONT_BYTES.to_vec());
        let found = font_system
            .db_mut()
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == "TempleOS"));
        assert!(found, "expected the embedded font to register under the family name \"TempleOS\"");
    }

    #[test]
    fn measured_char_width_reflects_the_bundled_fonts_real_1_to_1_advance_ratio() {
        // Confirmed via fontTools against the actual font file: every
        // glyph's advance is exactly 1000/1000 units (a full square
        // cell) -- i.e. at FONT_SIZE=16 the real advance is ~16px, not
        // the ~9.6px a typical outline monospace font's ~0.6 ratio
        // would give. This is the bug the caret/column misalignment
        // traced back to: a single hardcoded CHAR_WIDTH assumed every
        // font shared that ~0.6 ratio.
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(TEMPLEOS_FONT_BYTES.to_vec());
        let width = TextPipeline::measure_char_width(&mut font_system, Family::Name("TempleOS"));
        assert!((width - FONT_SIZE).abs() < 0.5, "expected ~{FONT_SIZE}px (1:1 ratio), got {width}px");
    }

    #[test]
    fn measured_char_width_for_the_default_family_is_narrower_than_the_bitmap_font() {
        let mut font_system = FontSystem::new();
        let default_width = TextPipeline::measure_char_width(&mut font_system, Family::Monospace);
        font_system.db_mut().load_font_data(TEMPLEOS_FONT_BYTES.to_vec());
        let templeos_width = TextPipeline::measure_char_width(&mut font_system, Family::Name("TempleOS"));
        assert!(
            default_width < templeos_width,
            "expected the system default monospace font ({default_width}px) to be narrower \
             than the bundled 1:1-ratio bitmap font ({templeos_width}px)"
        );
    }

    #[test]
    fn lines_that_fit_matches_visible_line_count_when_modeline_already_excluded() {
        let window_height = 500.0;
        assert_eq!(lines_that_fit(content_height(window_height)), visible_line_count(window_height));
    }
}
