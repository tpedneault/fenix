use glyphon::{
    Attrs, Buffer as GlyphBuffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping,
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

/// Shapes and rasterizes buffer text into the wgpu glyph atlas via glyphon.
///
/// Holds two independent glyph buffers sharing one atlas/renderer: `buffer`
/// for the windowed slice of editor content currently on screen, and
/// `modeline` for the single-line status bar.
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffer: GlyphBuffer,
    modeline: GlyphBuffer,
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

        Self { font_system, swash_cache, viewport, atlas, renderer, buffer, modeline }
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer.set_text(text, &Attrs::new().family(Family::Monospace), Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn set_modeline_text(&mut self, text: &str) {
        self.modeline.set_text(
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        self.modeline.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.buffer.set_size(Some(width), Some(content_height(height)));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
        self.modeline.set_size(Some(width), Some(MODELINE_HEIGHT));
        self.modeline.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn prepare(&mut self, gpu: &GpuState, theme: &Theme) {
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
            top: PAD_TOP,
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

        self.renderer
            .prepare(
                &gpu.device,
                &gpu.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [content_area, modeline_area],
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
