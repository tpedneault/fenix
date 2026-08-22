use glyphon::{
    Attrs, Buffer as GlyphBuffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};

use crate::gpu::GpuState;
use crate::theme::Theme;

pub const FONT_SIZE: f32 = 16.0;
pub const LINE_HEIGHT: f32 = 20.0;
/// Rough monospace advance width as a fraction of font size; good enough
/// until the caret is positioned from real glyph metrics instead.
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

/// Shapes and rasterizes buffer text into the wgpu glyph atlas via glyphon.
///
/// Holds three independent glyph buffers sharing one atlas/renderer:
/// `buffer` for the windowed slice of editor content currently on screen,
/// `modeline` for the single-line status bar, and `which_key` for the
/// popup listing a pending key sequence's continuations (only shown while
/// one is in progress).
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffer: GlyphBuffer,
    modeline: GlyphBuffer,
    which_key: GlyphBuffer,
}

impl TextPipeline {
    pub fn new(gpu: &GpuState) -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&gpu.device);
        let viewport = Viewport::new(&gpu.device, &cache);
        let mut atlas = TextAtlas::new(&gpu.device, &gpu.queue, &cache, gpu.config.format);
        let renderer =
            TextRenderer::new(&mut atlas, &gpu.device, wgpu::MultisampleState::default(), None);

        let mut buffer = GlyphBuffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(Some(gpu.size.width as f32), Some(content_height(gpu.size.height as f32)));

        let mut modeline = GlyphBuffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        modeline.set_wrap(Wrap::None);
        modeline.set_size(Some(gpu.size.width as f32), Some(MODELINE_HEIGHT));

        let mut which_key = GlyphBuffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        which_key.set_wrap(Wrap::None);
        which_key.set_size(Some(WHICH_KEY_WIDTH), None);

        Self { font_system, swash_cache, viewport, atlas, renderer, buffer, modeline, which_key }
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer.set_text(text, &Attrs::new().family(Family::Monospace), Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Sets the buffer-content text as differently-colored spans -- the
    /// line-number gutter prefix (or `~` for past-end-of-buffer rows) in
    /// one color per span, the actual line content in another. Same
    /// rich-text mechanism as `set_modeline_text`, applied per source line
    /// instead of once for the whole area.
    pub fn set_content_rich(&mut self, segments: &[(&str, Color)]) {
        let default_attrs = Attrs::new().family(Family::Monospace);
        let spans: Vec<(&str, Attrs)> = segments
            .iter()
            .map(|(text, color)| (*text, Attrs::new().family(Family::Monospace).color(*color)))
            .collect();
        self.buffer.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Sets the modeline text as a sequence of differently-colored spans
    /// (e.g. the mode badge in its own accent, the rest in the normal
    /// modeline color) -- a single flat color can't do that, so this
    /// takes rich text instead of a plain string like `set_text`/
    /// `set_which_key_text`.
    pub fn set_modeline_text(&mut self, segments: &[(&str, Color)]) {
        let default_attrs = Attrs::new().family(Family::Monospace);
        let spans: Vec<(&str, Attrs)> = segments
            .iter()
            .map(|(text, color)| (*text, Attrs::new().family(Family::Monospace).color(*color)))
            .collect();
        self.modeline.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.modeline.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn set_which_key_text(&mut self, text: &str) {
        self.which_key.set_text(
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.which_key.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.buffer.set_size(Some(width), Some(content_height(height)));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
        self.modeline.set_size(Some(width), Some(MODELINE_HEIGHT));
        self.modeline.shape_until_scroll(&mut self.font_system, false);
    }

    /// `content_top_offset` shifts the buffer-content text up by that many
    /// pixels (0..LINE_HEIGHT) -- the sub-line-height part of a smooth
    /// scroll transition; the caller fetches text starting from the
    /// integer line the transition has reached and shifts it up by the
    /// fractional remainder so it pans instead of jumping.
    ///
    /// `which_key_panel`, when present, is the (left, top, height) box to
    /// render the pending-sequence popup in -- top-right corner of the
    /// window, clear of both the content being edited and the modeline.
    /// The caller (App) already knows this from the hint count it built
    /// the text from, and draws the panel's background rect itself.
    pub fn prepare(
        &mut self,
        gpu: &GpuState,
        theme: &Theme,
        content_top_offset: f32,
        which_key_panel: Option<(f32, f32, f32)>,
    ) {
        self.viewport.update(
            &gpu.queue,
            Resolution { width: gpu.config.width, height: gpu.config.height },
        );

        let content_bounds = TextBounds {
            left: 0,
            top: 0,
            right: gpu.config.width as i32,
            bottom: (gpu.size.height as f32 - MODELINE_HEIGHT) as i32,
        };
        let content_area = TextArea {
            buffer: &self.buffer,
            left: PAD_LEFT,
            top: PAD_TOP - content_top_offset,
            scale: 1.0,
            bounds: content_bounds,
            default_color: theme.fg,
            custom_glyphs: &[],
        };

        let modeline_top = gpu.size.height as f32 - MODELINE_HEIGHT;
        let modeline_bounds = TextBounds {
            left: 0,
            top: modeline_top as i32,
            right: gpu.config.width as i32,
            bottom: gpu.config.height as i32,
        };
        let modeline_area = TextArea {
            buffer: &self.modeline,
            left: PAD_LEFT,
            top: modeline_top + 4.0,
            scale: 1.0,
            bounds: modeline_bounds,
            default_color: theme.fg_modeline,
            custom_glyphs: &[],
        };

        let mut areas = vec![content_area, modeline_area];
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

/// How many full text lines fit in the content area above the modeline.
pub fn visible_line_count(window_height: f32) -> usize {
    (content_height(window_height) / LINE_HEIGHT).floor().max(1.0) as usize
}
