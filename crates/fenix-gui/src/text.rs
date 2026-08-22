use glyphon::{
    Attrs, Buffer as GlyphBuffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};

use crate::gpu::GpuState;

pub const FONT_SIZE: f32 = 16.0;
pub const LINE_HEIGHT: f32 = 20.0;
/// Rough monospace advance width as a fraction of font size; good enough
/// until the caret is positioned from real glyph metrics instead.
pub const CHAR_WIDTH: f32 = FONT_SIZE * 0.6;
pub const PAD_LEFT: f32 = 8.0;
pub const PAD_TOP: f32 = 4.0;

/// Shapes and rasterizes buffer text into the wgpu glyph atlas via glyphon.
pub struct TextPipeline {
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffer: GlyphBuffer,
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
        buffer.set_size(Some(gpu.size.width as f32), Some(gpu.size.height as f32));

        Self { font_system, swash_cache, viewport, atlas, renderer, buffer }
    }

    pub fn set_text(&mut self, text: &str) {
        self.buffer.set_text(text, &Attrs::new().family(Family::Monospace), Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.buffer.set_size(Some(width), Some(height));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    pub fn prepare(&mut self, gpu: &GpuState) {
        self.viewport.update(
            &gpu.queue,
            Resolution { width: gpu.config.width, height: gpu.config.height },
        );
        let text_area = TextArea {
            buffer: &self.buffer,
            left: PAD_LEFT,
            top: PAD_TOP,
            scale: 1.0,
            bounds: TextBounds {
                left: 0,
                top: 0,
                right: gpu.config.width as i32,
                bottom: gpu.config.height as i32,
            },
            default_color: Color::rgb(220, 220, 220),
            custom_glyphs: &[],
        };
        self.renderer
            .prepare(
                &gpu.device,
                &gpu.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [text_area],
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
