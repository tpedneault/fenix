use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use glyphon::{
    Attrs, Buffer as GlyphBuffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};

use fenix_window::{Rect, WindowId as PaneId};

use crate::gpu::GpuState;
use crate::icon::ICON_FONT_FAMILY;
use crate::popup::PopupId;
use crate::theme::Theme;

/// Default/starting font size -- also the fallback used wherever a real
/// `TextPipeline` isn't available (headless tests, a `redraw()` called
/// before the GPU is ready). The actual live size the user sees is
/// `TextPipeline::font_size()`, adjustable at runtime via `SPC t =`/`SPC
/// t -`/`SPC t 0` -- see `TextPipeline::set_font_size`.
pub const FONT_SIZE: f32 = 16.0;
/// Default/starting line height, and the ratio (`LINE_HEIGHT /
/// FONT_SIZE` = 1.25) every runtime font-size change preserves.
pub const LINE_HEIGHT: f32 = 20.0;
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 40.0;
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
/// Smallest and largest a popup like which-key is allowed to size itself
/// to its own content (see `App::which_key_popup`) -- always at least
/// this wide even for a short list, never wider than this even for a
/// long label, regardless of the active font's real measured width.
pub const WHICH_KEY_MIN_WIDTH: f32 = 220.0;
pub const WHICH_KEY_MAX_WIDTH: f32 = 480.0;
/// Gap between the which-key panel and the window's top/right edges.
pub const WHICH_KEY_MARGIN: f32 = 12.0;
/// Width (in chars) of the modeline's mode badge, centered within it.
/// Fits the longest label ("V-BLOCK"/"REPLACE"/"COMMAND", 7 chars) with a
/// little breathing room.
pub const MODE_BADGE_CHARS: usize = 8;
/// Width of the file-explorer sidebar panel.
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// Height, in rows, of the terminal panel -- fixed (not user-resizable
/// in v1), same posture as `SIDEBAR_WIDTH`. Kept as a row count rather
/// than a raw pixel height since the real PTY underneath needs an exact
/// row count to stay in sync with what's rendered.
pub const TERMINAL_ROWS: usize = 12;

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
/// since splits/closes churn pane ids), one per floating popup
/// (`popups`, keyed by `popup::PopupId` -- same lazy-create/retain
/// lifecycle as `content_buffers`, since which-key already comes and
/// goes with what's pending and a future completion popup will too),
/// plus three fixed panels: `modeline` for the single-line status bar,
/// `clock` for its right-aligned date/time (its own buffer rather than
/// text embedded in `modeline`, see `clock`'s own doc comment), and
/// `sidebar` for the file explorer's persistent side panel. All share
/// one atlas/renderer/font system -- glyphon's `TextRenderer::prepare`
/// already takes an arbitrary-length `Vec<TextArea>`, so N panes or
/// popups is just N more entries, not a different rendering pipeline.
/// The font machinery every frame shares: the font database and its
/// shaping caches, the rasterized-glyph cache, the GPU glyph atlas, and
/// glyphon's own pipeline cache.
///
/// Shared rather than built per frame because cosmic-text's own docs are
/// blunt about the cost -- building a `FontSystem` scans the system font
/// database and "can take up to a second" in a release build, ten times
/// that in debug, and it "should only be called once" -- and because a
/// second window's glyph atlas would be a duplicate GPU texture holding
/// the same glyphs as the first.
///
/// Reached through `Rc<RefCell<..>>` rather than passed as an argument
/// to each of `TextPipeline`'s sixteen text-setting methods. Every
/// borrow here begins and ends inside a single method call -- including
/// in `render`, because glyphon takes the atlas by a reference
/// independent of the render pass's lifetime rather than one tied to it
/// -- and nothing inside re-enters `TextPipeline`, so there is no path
/// to a borrow panic. Threading a `&mut` down through every signature
/// instead would have churned all of `redraw`'s call sites for no
/// behavioural difference.
pub struct FontContext {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: TextAtlas,
    cache: Cache,
    /// See `TextPipeline::default_family` -- resolved here now, once
    /// for the whole process rather than once per window.
    default_family: &'static str,
}

impl FontContext {
    pub fn new(gpu: &GpuState) -> Self {
        let mut font_system = FontSystem::new();
        font_system.db_mut().load_font_data(TEMPLEOS_FONT_BYTES.to_vec());
        let default_family: &'static str =
            Box::leak(font_system.db().family_name(&Family::Monospace).to_string().into_boxed_str());
        let cache = Cache::new(&gpu.device);
        let atlas = TextAtlas::new(&gpu.device, &gpu.queue, &cache, gpu.config.format);
        Self { font_system, swash_cache: SwashCache::new(), atlas, cache, default_family }
    }
}

pub struct TextPipeline {
    /// The font database, glyph caches and atlas -- shared with every
    /// other frame's pipeline. See `FontContext`.
    fonts: Rc<RefCell<FontContext>>,
    viewport: Viewport,
    renderer: TextRenderer,
    /// A second, independent renderer for the popup overlay pass, sharing
    /// `atlas`/`viewport` but owning its own vertex buffer. Necessary
    /// because glyphon's `TextRenderer::prepare` calls `vertex_buffer.
    /// destroy()` before replacing it whenever new content needs more
    /// capacity than currently allocated (see `text_render.rs` in the
    /// vendored glyphon source) -- an *immediate* GPU-side destroy, not a
    /// deferred one. With one shared renderer, `prepare()` (base layer) +
    /// `render()` (pass one) + `prepare_popups()` (popups) + `render()`
    /// (pass two) all in the same frame meant `prepare_popups` could
    /// destroy the very vertex buffer pass one's draw call had already
    /// been recorded against, in the same not-yet-submitted command
    /// encoder -- `queue.submit` then failed validation with "Buffer ...
    /// has been destroyed". Two renderers means a popup-triggered resize
    /// only ever touches `popup_renderer`'s own buffer.
    popup_renderer: TextRenderer,
    content_buffers: HashMap<PaneId, GlyphBuffer>,
    /// One single-line `GlyphBuffer` per pane that currently has a title
    /// bar (`App::pane_titles`) -- same lazy-create/retain lifecycle as
    /// `content_buffers`, just for the reserved strip at a pane's top
    /// edge instead of its main content. Rendered in the same base-layer
    /// `prepare`/`render` pass as pane content (not a separate pass like
    /// popups need -- see `popup_renderer`'s own doc comment for why
    /// popups specifically need that; titles have no such conflict since
    /// nothing else destroys/resizes a title buffer mid-frame).
    titles: HashMap<PaneId, GlyphBuffer>,
    popups: HashMap<PopupId, GlyphBuffer>,
    modeline: GlyphBuffer,
    /// The modeline's right-aligned date/time clock -- a *separate*
    /// buffer from `modeline` (rather than embedding it via literal
    /// space-padding in that shared string, which a first attempt at
    /// this did) so its position comes from cosmic-text's own real
    /// `Align::Right` layout against a sized box (`set_clock_rich`),
    /// not from approximating the text's pixel width as `char_count *
    /// char_width` -- that approximation was consistently a little off
    /// in practice (real glyph shaping isn't perfectly uniform even in
    /// a "monospace" font), which is exactly what two real, reported
    /// bugs looked like: the clock drifting off the right edge, first
    /// after going fullscreen (space-padding accumulated the error over
    /// a very long run) and then even at ordinary widths after that fix
    /// (the *width estimate itself* was still off, just by less).
    /// Letting the shaping engine do the real measurement sidesteps the
    /// whole class of "my estimate doesn't match the real glyph
    /// advances" bugs instead of chasing another one.
    clock: GlyphBuffer,
    sidebar: GlyphBuffer,
    terminal: GlyphBuffer,
    /// Effective body-text font family: a `config.ini` `font_family`
    /// override, or else the active theme's own `Theme::font_family` --
    /// whichever `App::redraw` resolved and passed to `set_font_family`.
    /// `None` falls back to `default_family` (see its own doc comment).
    /// Icon spans in `rich_spans` are unaffected, always `ICON_FONT_
    /// FAMILY` regardless of this.
    ///
    /// `&'static str`, not `&str`/`String`: an incoming family name is
    /// leaked once via `Box::leak` in `set_font_family` the first time
    /// it's seen, the standard idiom for turning a small, bounded set
    /// of long-lived runtime strings into `'static` ones. Bounded here
    /// by "however many distinct fonts a user names in one session"
    /// (a config value plus however many themes get cycled through,
    /// each already `&'static str` itself) -- negligible, and it keeps
    /// `content_family()` returning `Family<'static>`, so none of its
    /// six call sites need touching just because this field's *source*
    /// changed from theme-only to theme-or-config.
    content_family: Option<&'static str>,
    /// The system's real default monospace font, resolved *once* here
    /// (via `fontdb::Database::family_name(&Family::Monospace)`, the
    /// same generic-family-alias resolution `Family::Monospace` itself
    /// would trigger) rather than re-resolved on every shape. This is
    /// the fix for a real, measured performance bug: shaping text with
    /// the generic `Family::Monospace` enum variant directly costs
    /// `cosmic-text` extra per-shape fallback-substitution work it only
    /// does for that variant (confirmed via a standalone headless
    /// benchmark against this crate's own font database: ~37ms/shape
    /// for `Family::Monospace` vs. ~6ms for a concrete `Family::Name`
    /// on identical text) -- paid on every keystroke, since content is
    /// reshaped once per `redraw()`. Resolving the *name* once and
    /// always shaping via `Family::Name(_)` from then on keeps the
    /// same visual font (it's the same name the generic variant would
    /// have resolved to) without paying that tax continuously. Used as
    /// the fallback whenever no theme/config font is set -- previously
    /// every theme without an explicit `font_family` (i.e. every theme
    /// except TempleOS) paid this cost on every frame, which is why
    /// typing felt slower on any theme but TempleOS.
    default_family: &'static str,
    /// The active font's real, measured monospace advance width in
    /// pixels at the current `font_size` -- recomputed whenever
    /// `content_family` (`set_theme`) or `font_size` (`set_font_size`)
    /// actually changes, not every frame. Different fonts have very
    /// different advance-to-em ratios (the bundled TempleOS bitmap font
    /// is a full square cell, ~1.0x its em size, vastly wider than a
    /// typical outline monospace font's ~0.6x), so a single hardcoded
    /// ratio (`CHAR_WIDTH`) breaks caret/column alignment the moment a
    /// second font enters the mix -- callers needing per-column pixel
    /// math should use `char_width()`, not the `CHAR_WIDTH` constant.
    char_width: f32,
    /// Live font size/line height -- start at `FONT_SIZE`/`LINE_HEIGHT`,
    /// adjustable at runtime via `set_font_size` (`SPC t =`/`-`/`0`).
    /// `line_height` always preserves the original `LINE_HEIGHT /
    /// FONT_SIZE` ratio; see `set_font_size`'s own doc comment.
    font_size: f32,
    line_height: f32,
}

impl TextPipeline {
    /// `fonts` is shared with every other frame -- see `FontContext`.
    pub fn new(gpu: &GpuState, fonts: Rc<RefCell<FontContext>>) -> Self {
        // Bound to a named guard rather than `&mut *fonts.borrow_mut()`:
        // that temporary would be dropped at the end of its own `let`,
        // leaving `ctx` borrowing nothing.
        let mut guard = fonts.borrow_mut();
        let ctx = &mut *guard;
        let default_family = ctx.default_family;
        let viewport = Viewport::new(&gpu.device, &ctx.cache);
        let renderer =
            TextRenderer::new(&mut ctx.atlas, &gpu.device, wgpu::MultisampleState::default(), None);
        let popup_renderer =
            TextRenderer::new(&mut ctx.atlas, &gpu.device, wgpu::MultisampleState::default(), None);
        let font_system = &mut ctx.font_system;

        let mut modeline = GlyphBuffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        modeline.set_wrap(Wrap::None);
        modeline.set_size(Some(gpu.size.width as f32), Some(LINE_HEIGHT + 8.0));

        let mut clock = GlyphBuffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        clock.set_wrap(Wrap::None);
        clock.set_size(Some(gpu.size.width as f32), Some(LINE_HEIGHT + 8.0));

        let mut sidebar = GlyphBuffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        sidebar.set_wrap(Wrap::None);
        sidebar.set_size(Some(SIDEBAR_WIDTH), Some(content_height(gpu.size.height as f32, LINE_HEIGHT + 8.0)));

        let mut terminal = GlyphBuffer::new(font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        terminal.set_wrap(Wrap::None);
        terminal.set_size(Some(gpu.size.width as f32), Some(terminal_height(LINE_HEIGHT)));

        let char_width = Self::measure_char_width(font_system, Family::Name(default_family), FONT_SIZE, LINE_HEIGHT);
        // The borrow has to end before `fonts` is moved into `Self`.
        drop(guard);

        Self {
            fonts,
            viewport,
            renderer,
            popup_renderer,
            content_buffers: HashMap::new(),
            titles: HashMap::new(),
            popups: HashMap::new(),
            modeline,
            clock,
            sidebar,
            terminal,
            content_family: None,
            default_family,
            char_width,
            font_size: FONT_SIZE,
            line_height: LINE_HEIGHT,
        }
    }

    /// Resolves the active body-text font: `Family::Name(_)` for whatever
    /// `set_font_family` was last given, else `default_family` (see its
    /// own doc comment -- never the slow generic `Family::Monospace`).
    fn content_family(&self) -> Family<'static> {
        Family::Name(self.content_family.unwrap_or(self.default_family))
    }

    /// Sets the effective body-text font for all subsequent `set_*`
    /// calls -- called once per `redraw()`, before them, with whatever
    /// `App` resolved as the effective family (a `config.ini`
    /// `font_family` override, else the active theme's own `Theme::
    /// font_family`, else `None`). A no-op beyond the equality check
    /// when the family hasn't actually changed since last frame, so
    /// re-measuring `char_width` (a real shaping pass) and leaking a
    /// new `'static` copy of `family` only happen on an actual switch,
    /// not every redraw.
    pub fn set_font_family(&mut self, family: Option<&str>) {
        if self.content_family == family {
            return;
        }
        self.content_family = family.map(|f| -> &'static str { Box::leak(f.to_string().into_boxed_str()) });
        let family = self.content_family();
        self.char_width = Self::measure_char_width(&mut self.fonts.borrow_mut().font_system, family, self.font_size, self.line_height);
    }

    /// The active font's real monospace advance width in pixels --
    /// callers doing per-column pixel math (caret, selection, badge
    /// sizing) should use this instead of the `CHAR_WIDTH` constant.
    pub fn char_width(&self) -> f32 {
        self.char_width
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    /// The modeline bar's own height -- `line_height` plus a little
    /// breathing room, tracking font size the same way every other
    /// line-height-derived measurement now does.
    pub fn modeline_height(&self) -> f32 {
        self.line_height + 8.0
    }

    /// `SPC t =`/`SPC t -`/`SPC t 0`: grows, shrinks, or resets the body
    /// text size at runtime, clamped to `[MIN_FONT_SIZE, MAX_FONT_SIZE]`.
    /// `line_height` always scales with it, preserving the original
    /// `LINE_HEIGHT / FONT_SIZE` ratio (1.25) rather than staying fixed
    /// -- a bigger font with unchanged line spacing would visually
    /// crowd lines together instead of scaling proportionally. Updates
    /// every glyph buffer that currently exists immediately (not just
    /// ones re-rendered this frame): panes and the modeline get a fresh
    /// `set_*_rich`/`set_modeline_text` call every redraw regardless and
    /// would self-correct anyway, but the sidebar and any popup only get
    /// re-rendered while actually visible, so without this a
    /// currently-hidden one could stay stale until its next content
    /// change.
    /// `scale` is the window's DPI scale factor, applied on top of the
    /// requested size, so the same configured size comes out the same
    /// *physical* size on a 100% monitor and a 150% one instead of
    /// shrinking on the denser screen. Each frame passes its own
    /// window's factor, which is what makes a two-monitor setup with
    /// mismatched scaling look right in both windows.
    ///
    /// The clamp is on the requested size, not on the scaled result:
    /// 24pt on a 200% monitor is a legitimate 48 physical pixels, not
    /// something to pull back to `MAX_FONT_SIZE`.
    pub fn set_font_size(&mut self, size: f32, scale: f32) {
        let (font_size, line_height) = scaled_font_metrics(size, scale);
        if font_size == self.font_size {
            return;
        }
        self.font_size = font_size;
        self.line_height = line_height;
        let metrics = Metrics::new(self.font_size, self.line_height);

        self.modeline.set_metrics(metrics);
        self.modeline.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        self.clock.set_metrics(metrics);
        self.clock.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        self.sidebar.set_metrics(metrics);
        self.sidebar.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        for buf in self.content_buffers.values_mut() {
            buf.set_metrics(metrics);
            buf.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        }
        for buf in self.popups.values_mut() {
            buf.set_metrics(metrics);
            buf.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        }

        let family = self.content_family();
        self.char_width = Self::measure_char_width(&mut self.fonts.borrow_mut().font_system, family, self.font_size, self.line_height);
    }

    /// Measures `family`'s real advance width at `font_size`/`line_height`
    /// by shaping two characters and taking the pixel distance between
    /// them, rather than assuming a fixed ratio of `font_size` -- see the
    /// `char_width` field's doc comment for why a single hardcoded ratio
    /// doesn't hold across fonts. Falls back to the old assumed `0.6`
    /// ratio only if shaping somehow produces fewer than two glyphs
    /// (should not happen for two ordinary ASCII letters, but a shaping
    /// failure shouldn't be able to divide-by-zero or panic downstream).
    fn measure_char_width(font_system: &mut FontSystem, family: Family<'static>, font_size: f32, line_height: f32) -> f32 {
        let mut probe = GlyphBuffer::new(font_system, Metrics::new(font_size, line_height));
        probe.set_wrap(Wrap::None);
        probe.set_size(Some(1000.0), Some(line_height));
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
            .unwrap_or(font_size * 0.6)
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
    /// capturing `&mut self.fonts.borrow_mut().font_system` to construct a fresh buffer,
    /// which can't then *also* be reborrowed for `shape_until_scroll`
    /// afterward (the closure would move it) -- a plain contains-key
    /// check plus a fresh `get_mut` avoids the conflict.
    pub fn set_pane_rich(&mut self, pane: PaneId, w: f32, h: f32, segments: &[(&str, Color, bool)]) {
        let spans = self.rich_spans(segments);
        let default_attrs = Attrs::new().family(self.content_family());

        if !self.content_buffers.contains_key(&pane) {
            let mut buf = GlyphBuffer::new(&mut self.fonts.borrow_mut().font_system, Metrics::new(self.font_size, self.line_height));
            buf.set_wrap(Wrap::None);
            self.content_buffers.insert(pane, buf);
        }
        let buf = self.content_buffers.get_mut(&pane).expect("just inserted if missing");
        buf.set_size(Some(w), Some(h));
        buf.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// Drops every pane `GlyphBuffer` not in `keep` -- splits/closes churn
    /// `PaneId`s, so without this, closed panes' buffers (and their glyph
    /// atlas entries) would accumulate forever.
    pub fn retain_panes(&mut self, keep: &[PaneId]) {
        self.content_buffers.retain(|id, _| keep.contains(id));
    }

    /// Sets one pane's title-bar text -- same rich-span/lazy-create
    /// mechanism as `set_pane_rich`, but always exactly one line tall
    /// (`h` isn't a parameter: a title strip is always `line_height`,
    /// unlike a pane's own content area).
    pub fn set_pane_title_rich(&mut self, pane: PaneId, w: f32, segments: &[(&str, Color, bool)]) {
        let spans = self.rich_spans(segments);
        let default_attrs = Attrs::new().family(self.content_family());

        if !self.titles.contains_key(&pane) {
            let mut buf = GlyphBuffer::new(&mut self.fonts.borrow_mut().font_system, Metrics::new(self.font_size, self.line_height));
            buf.set_wrap(Wrap::None);
            self.titles.insert(pane, buf);
        }
        let buf = self.titles.get_mut(&pane).expect("just inserted if missing");
        buf.set_size(Some(w), Some(self.line_height));
        buf.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// Drops every title `GlyphBuffer` not in `keep` -- same reasoning as
    /// `retain_panes`.
    pub fn retain_titles(&mut self, keep: &[PaneId]) {
        self.titles.retain(|id, _| keep.contains(id));
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
        self.modeline.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// Sets the modeline clock's text, right-aligned within a `box_width`-
    /// wide box whose left edge is the window's own left edge (`0`) --
    /// its *right* edge (`box_width`) is therefore where the clock's own
    /// right edge lands, computed by cosmic-text's real `Align::Right`
    /// layout against the glyphs' actual shaped widths, not by this
    /// crate guessing a pixel width from a char count (`clock`'s own doc
    /// comment explains why that guess wasn't reliable enough). The
    /// caller (`App::redraw`) passes `window_width` minus its own right
    /// margin as `box_width`. `segments` empty means "don't show the
    /// clock this frame" (not enough room); `box_width` is irrelevant
    /// in that case.
    pub fn set_clock_rich(&mut self, box_width: f32, segments: &[(&str, Color)]) {
        self.clock.set_size(Some(box_width.max(0.0)), Some(self.line_height));
        let body_family = self.content_family();
        let default_attrs = Attrs::new().family(body_family);
        let spans: Vec<(&str, Attrs)> =
            segments.iter().map(|(text, color)| (*text, Attrs::new().family(body_family).color(*color))).collect();
        self.clock.set_rich_text(spans, &default_attrs, Shaping::Advanced, Some(glyphon::cosmic_text::Align::Right));
        self.clock.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// Sets one popup's content (rich, icon-aware spans, same shape as
    /// `set_pane_rich`) and pixel width -- creating its `GlyphBuffer`
    /// lazily the first time it's rendered, same pattern and same
    /// `entry`/`or_insert_with` borrow conflict avoided the same way.
    /// Height is left unconstrained (`None`): popups size themselves to
    /// their own content, clamped by `popup::resolve`'s position math
    /// and (for how many rows are offered in the first place)
    /// `popup::max_rows` -- not by wrapping/clipping inside the buffer.
    pub fn set_popup_rich(&mut self, id: PopupId, width: f32, segments: &[(&str, Color, bool)]) {
        let spans = self.rich_spans(segments);
        let default_attrs = Attrs::new().family(self.content_family());

        if !self.popups.contains_key(&id) {
            let mut buf = GlyphBuffer::new(&mut self.fonts.borrow_mut().font_system, Metrics::new(self.font_size, self.line_height));
            buf.set_wrap(Wrap::None);
            self.popups.insert(id, buf);
        }
        let buf = self.popups.get_mut(&id).expect("just inserted if missing");
        buf.set_size(Some(width), None);
        buf.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        buf.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// Drops every popup `GlyphBuffer` not in `keep` -- popups come and
    /// go with what's pending (which-key) or what's being typed
    /// (completion), so without this, closed ones would accumulate
    /// forever, same reasoning as `retain_panes`.
    pub fn retain_popups(&mut self, keep: &[PopupId]) {
        self.popups.retain(|id, _| keep.contains(id));
    }

    /// Sets the sidebar's rows -- same icon/color rich-text mechanism as
    /// `set_pane_rich`, into its own independent glyph buffer since the
    /// sidebar renders alongside the editor content, not interleaved
    /// with it.
    pub fn set_sidebar_rich(&mut self, segments: &[(&str, Color, bool)]) {
        let default_attrs = Attrs::new().family(self.content_family());
        let spans = self.rich_spans(segments);
        self.sidebar.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.sidebar.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// Sets the terminal panel's rows -- same mechanism as `set_sidebar_
    /// rich`, into its own independent glyph buffer.
    pub fn set_terminal_rich(&mut self, segments: &[(&str, Color, bool)]) {
        let default_attrs = Attrs::new().family(self.content_family());
        let spans = self.rich_spans(segments);
        self.terminal.set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
        self.terminal.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        // Pane buffers are resized every frame in `set_pane_rich` (their
        // rect comes from `WindowTree::layout`, recomputed each redraw
        // regardless of whether the window itself just resized), and
        // `clock` gets its width fresh every frame from `set_clock_rich`
        // (it needs to be current every single redraw, not just after a
        // resize, since it also depends on how much of the modeline's
        // left side is occupied) -- so only the two remaining fixed
        // panels need handling here.
        self.modeline.set_size(Some(width), Some(self.modeline_height()));
        self.modeline.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        self.sidebar.set_size(Some(SIDEBAR_WIDTH), Some(content_height(height, self.modeline_height())));
        self.sidebar.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
        // The width-vs-height mirror of the sidebar's own resize just
        // above: fixed height (`TERMINAL_ROWS`), full width instead of
        // fixed width/remaining height.
        self.terminal.set_size(Some(width), Some(terminal_height(self.line_height)));
        self.terminal.shape_until_scroll(&mut self.fonts.borrow_mut().font_system, false);
    }

    /// One pane's render info for `prepare`: its id (to look up the right
    /// `GlyphBuffer`), its current pixel rect, and the sub-line-height
    /// fractional offset to shift its content up by (0.0 for panes that
    /// don't animate scroll, i.e. everything but the focused pane's own
    /// smooth-scroll transition). `popups` is the analogous list for
    /// floating popups: id plus its already-`popup::resolve`d on-screen
    /// rect -- an empty slice when nothing (no pending which-key
    /// sequence, no completion popup) is showing right now.
    /// Prepares every *base-layer* text area -- pane content, the
    /// modeline, and the sidebar -- for the next `render()` call. Popups
    /// are deliberately not included here: they're prepared and drawn in
    /// a later, separate pass (`prepare_popups`) so they visually cover
    /// this layer regardless of how far pane content extends underneath
    /// them, rather than relying on `TextBounds` clipping (a single
    /// rectangle per pane can't carve a popup-shaped hole out of it) --
    /// see `App::redraw`'s two-pass render sequence for the full picture.
    pub fn prepare(
        &mut self,
        gpu: &GpuState,
        theme: &Theme,
        panes: &[(PaneId, Rect, f32)],
        titles: &[(PaneId, Rect)],
        sidebar_open: bool,
        terminal_open: bool,
    ) {
        self.viewport.update(
            &gpu.queue,
            Resolution { width: gpu.config.width, height: gpu.config.height },
        );

        let modeline_top = gpu.size.height as f32 - self.modeline_height();

        let mut areas = Vec::with_capacity(panes.len() + titles.len() + 2);
        for &(pane, rect, content_frac) in panes {
            let Some(buffer) = self.content_buffers.get(&pane) else { continue };
            areas.push(TextArea {
                buffer,
                left: rect.x + PAD_LEFT,
                top: rect.y + PAD_TOP - content_frac * self.line_height,
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

        // A pane's title strip -- `rect` here is the *strip itself*
        // (its own top-left/size, computed by the caller as `line_
        // height`-tall and sitting just above that pane's now-shrunk
        // content rect), not the pane's full original rect.
        for &(pane, rect) in titles {
            let Some(buffer) = self.titles.get(&pane) else { continue };
            areas.push(TextArea {
                buffer,
                left: rect.x + PAD_LEFT,
                top: rect.y,
                scale: 1.0,
                bounds: TextBounds {
                    left: rect.x as i32,
                    top: rect.y as i32,
                    right: (rect.x + rect.w) as i32,
                    bottom: (rect.y + rect.h) as i32,
                },
                default_color: theme.fg_modeline,
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
        areas.push(TextArea {
            buffer: &self.clock,
            // Always `0` -- `set_clock_rich`'s box starts at the window's
            // left edge; its own `Align::Right` layout is what actually
            // pushes the glyphs to sit at the box's *right* edge.
            left: 0.0,
            top: modeline_top + 4.0,
            scale: 1.0,
            bounds: modeline_bounds,
            default_color: theme.fg_modeline,
            custom_glyphs: &[],
        });

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

        if terminal_open {
            let top = modeline_top - terminal_height(self.line_height);
            areas.push(TextArea {
                buffer: &self.terminal,
                left: PAD_LEFT,
                top: top + PAD_TOP,
                scale: 1.0,
                bounds: TextBounds { left: 0, top: top as i32, right: gpu.config.width as i32, bottom: modeline_top as i32 },
                default_color: theme.fg,
                custom_glyphs: &[],
            });
        }

        // One guard, not three `borrow_mut()` calls in one
        // expression -- that would be a double mutable borrow and
        // panic at runtime.
        let ctx = &mut *self.fonts.borrow_mut();
        self.renderer
            .prepare(
                &gpu.device,
                &gpu.queue,
                &mut ctx.font_system,
                &mut ctx.atlas,
                &self.viewport,
                areas,
                &mut ctx.swash_cache,
            )
            .expect("glyphon prepare failed");
    }

    /// The overlay layer: just popup text, prepared for a second
    /// `render()` call in a second render pass (`LoadOp::Load`) drawn
    /// after the base layer -- see `prepare`'s own doc comment for why.
    pub fn prepare_popups(&mut self, gpu: &GpuState, theme: &Theme, popups: &[(PopupId, Rect)]) {
        self.viewport.update(
            &gpu.queue,
            Resolution { width: gpu.config.width, height: gpu.config.height },
        );

        let mut areas = Vec::with_capacity(popups.len());
        for &(id, rect) in popups {
            let Some(buffer) = self.popups.get(&id) else { continue };
            areas.push(TextArea {
                buffer,
                left: rect.x + PAD_LEFT,
                top: rect.y + 4.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: rect.x as i32,
                    top: rect.y as i32,
                    right: (rect.x + rect.w) as i32,
                    bottom: (rect.y + rect.h) as i32,
                },
                default_color: theme.fg_modeline,
                custom_glyphs: &[],
            });
        }

        // One guard, not three `borrow_mut()` calls in one
        // expression -- that would be a double mutable borrow and
        // panic at runtime.
        let ctx = &mut *self.fonts.borrow_mut();
        self.popup_renderer
            .prepare(
                &gpu.device,
                &gpu.queue,
                &mut ctx.font_system,
                &mut ctx.atlas,
                &self.viewport,
                areas,
                &mut ctx.swash_cache,
            )
            .expect("glyphon prepare failed");
    }

    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        self.renderer.render(&self.fonts.borrow().atlas, &self.viewport, pass).expect("glyphon render failed");
    }

    /// Draws whatever `prepare_popups` most recently prepared -- the
    /// second pass's counterpart to `render`, using the separate
    /// `popup_renderer` (see its field doc comment for why this can't
    /// just be another call to `render`).
    pub fn render_popups<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) {
        self.popup_renderer.render(&self.fonts.borrow().atlas, &self.viewport, pass).expect("glyphon render failed");
    }

    pub fn trim(&mut self) {
        self.fonts.borrow_mut().atlas.trim();
    }
}

/// The physical font size and line height for a requested size on a
/// window with DPI scale factor `scale`.
///
/// The clamp applies to what was *requested*, before scaling -- 24pt on
/// a 200% monitor is a legitimate 48 physical pixels, and pulling that
/// back to `MAX_FONT_SIZE` would make the same setting render smaller
/// on the denser screen, which is the whole thing this exists to stop.
fn scaled_font_metrics(requested: f32, scale: f32) -> (f32, f32) {
    let (font_size, line_height) = resolve_font_size(requested);
    (font_size * scale, line_height * scale)
}

/// Clamps a requested font size to `[MIN_FONT_SIZE, MAX_FONT_SIZE]` and
/// derives the paired line height that preserves the original
/// `LINE_HEIGHT / FONT_SIZE` ratio -- pulled out of `TextPipeline::
/// set_font_size` as a pure function so it's directly unit-testable
/// without needing a real GPU-backed `TextPipeline` to construct.
fn resolve_font_size(requested: f32) -> (f32, f32) {
    let font_size = requested.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    let line_height = font_size * (LINE_HEIGHT / FONT_SIZE);
    (font_size, line_height)
}

fn content_height(window_height: f32, modeline_height: f32) -> f32 {
    (window_height - modeline_height).max(0.0)
}

/// Pixel height of the terminal panel's `TERMINAL_ROWS` rows, at the
/// given `line_height` -- mirrors `content_height`'s role for the
/// sidebar, just for a row count instead of "whatever's left."
pub fn terminal_height(line_height: f32) -> f32 {
    TERMINAL_ROWS as f32 * line_height + PAD_TOP * 2.0
}

/// How many full text lines fit in a content area of the given pixel
/// height (already excluding the modeline, e.g. a pane's own rect
/// height) at the given `line_height` -- used both for the whole-window
/// case (`content_height` applied first) and per-pane. Takes
/// `line_height` explicitly (rather than being a `TextPipeline` method)
/// so it stays a pure, headlessly-testable function -- callers pass
/// `TextPipeline::line_height()` (or `text::LINE_HEIGHT` as a fallback
/// when no pipeline exists yet, e.g. a `redraw()` before the GPU is
/// ready).
///
/// Reserves `PAD_TOP` off the top before dividing -- every caller of
/// this (pane content, the sidebar, the explorer/picker overlay) draws
/// its first row starting `PAD_TOP` down from the container's own top
/// (`row_y`/`sidebar_row_y`/`caret_pixel_pos` all add it), so the *last*
/// row this claims fits needs `PAD_TOP` counted against the available
/// height too, or its bottom edge lands `PAD_TOP` past the container's
/// real bottom. Without this, whenever `height` happened to be an exact
/// or near-exact multiple of `line_height` (common at ordinary window
/// sizes, not a rare fraction), one row too many was reported as
/// fitting -- harmless for a lightly-colored highlight lost among real
/// text above it, but glaring for the current line's own highlight/
/// caret specifically (drawn as solid, undamped rects, not clipped text)
/// when that line was also the *last* visible row: reported bug was the
/// caret visibly bleeding a few pixels into the modeline while sitting
/// on a file's last line, which is exactly the "current line = last
/// reported-fitting row" case this always could have hit.
pub fn lines_that_fit(height: f32, line_height: f32) -> usize {
    ((height - PAD_TOP) / line_height).floor().max(1.0) as usize
}

/// Horizontal analog of `lines_that_fit` -- how many full character
/// columns fit in `width`, reserving `PAD_LEFT` on *both* sides (a
/// pane's own left inset, and matching breathing room on the right so
/// the last visible column isn't drawn flush against the pane's own
/// edge or a neighboring split's divider).
pub fn cols_that_fit(width: f32, char_width: f32) -> usize {
    ((width - PAD_LEFT * 2.0) / char_width).floor().max(1.0) as usize
}

/// How many full text lines fit in the content area above the modeline,
/// for the single-pane/whole-window case.
pub fn visible_line_count(window_height: f32, modeline_height: f32, line_height: f32) -> usize {
    lines_that_fit(content_height(window_height, modeline_height), line_height)
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
        let width = TextPipeline::measure_char_width(&mut font_system, Family::Name("TempleOS"), FONT_SIZE, LINE_HEIGHT);
        assert!((width - FONT_SIZE).abs() < 0.5, "expected ~{FONT_SIZE}px (1:1 ratio), got {width}px");
    }

    #[test]
    fn measured_char_width_for_the_default_family_is_narrower_than_the_bitmap_font() {
        let mut font_system = FontSystem::new();
        let default_width = TextPipeline::measure_char_width(&mut font_system, Family::Monospace, FONT_SIZE, LINE_HEIGHT);
        font_system.db_mut().load_font_data(TEMPLEOS_FONT_BYTES.to_vec());
        let templeos_width =
            TextPipeline::measure_char_width(&mut font_system, Family::Name("TempleOS"), FONT_SIZE, LINE_HEIGHT);
        assert!(
            default_width < templeos_width,
            "expected the system default monospace font ({default_width}px) to be narrower \
             than the bundled 1:1-ratio bitmap font ({templeos_width}px)"
        );
    }

    #[test]
    fn resolving_the_monospace_generic_family_yields_a_usable_concrete_name() {
        // The precondition `default_family` (in `TextPipeline::new`)
        // depends on: `content_family()`'s fallback must always be a
        // resolved concrete `Family::Name`, never the generic `Family::
        // Monospace` variant itself, because shaping via the generic
        // variant is measurably slower (see `default_family`'s own doc
        // comment -- ~37ms/shape vs. ~6ms for a concrete name on this
        // crate's own font database in a standalone benchmark). This
        // guards the one fact that fix relies on: `fontdb` actually
        // resolves `Family::Monospace` to some real, non-empty family
        // name on a real font database, so leaking it once at startup
        // and shaping via `Family::Name(_)` from then on can't silently
        // fall back to shaping against an empty/bogus family.
        let font_system = FontSystem::new();
        let resolved = font_system.db().family_name(&Family::Monospace);
        assert!(!resolved.is_empty(), "expected fontdb to resolve Family::Monospace to a real family name");
    }

    #[test]
    fn lines_that_fit_reserves_pad_top_so_the_last_rows_bottom_edge_never_exceeds_height() {
        // A height that's an exact multiple of line_height is exactly
        // the case that used to overreport by one row (see this
        // function's own doc comment) -- confirmed here by checking the
        // actual geometric claim, not just a specific returned number:
        // PAD_TOP + n * line_height must never exceed height. Excludes
        // heights small enough that the pre-existing "always at least 1
        // row" floor (below) is the binding constraint instead -- that
        // floor deliberately trades a bit of overflow for never
        // reporting zero rows, a different, unrelated tradeoff.
        for height in [400.0, 400.0 + PAD_TOP, 800.0] {
            let n = lines_that_fit(height, LINE_HEIGHT);
            assert!(
                PAD_TOP + n as f32 * LINE_HEIGHT <= height,
                "height {height}: {n} rows claims to fit, but PAD_TOP + {n} * {LINE_HEIGHT} = {} > {height}",
                PAD_TOP + n as f32 * LINE_HEIGHT
            );
        }
    }

    #[test]
    fn cols_that_fit_reserves_pad_left_on_both_sides() {
        for width in [400.0, 400.0 + PAD_LEFT, 800.0] {
            let n = cols_that_fit(width, CHAR_WIDTH);
            assert!(
                PAD_LEFT * 2.0 + n as f32 * CHAR_WIDTH <= width,
                "width {width}: {n} cols claims to fit, but PAD_LEFT*2 + {n} * {CHAR_WIDTH} = {} > {width}",
                PAD_LEFT * 2.0 + n as f32 * CHAR_WIDTH
            );
        }
    }

    #[test]
    fn cols_that_fit_never_reports_zero_even_for_a_too_narrow_width() {
        assert_eq!(cols_that_fit(1.0, CHAR_WIDTH), 1);
    }

    #[test]
    fn lines_that_fit_matches_visible_line_count_when_modeline_already_excluded() {
        let window_height = 500.0;
        let modeline_height = LINE_HEIGHT + 8.0;
        assert_eq!(
            lines_that_fit(content_height(window_height, modeline_height), LINE_HEIGHT),
            visible_line_count(window_height, modeline_height, LINE_HEIGHT)
        );
    }

    // `set_font_size` itself needs a real `TextPipeline`, which needs a
    // real GPU device to construct (`GpuState::new` is async and opens
    // an actual window) -- not something a headless unit test can do,
    // the same reason no other GPU-backed rendering in this file has
    // direct test coverage. `resolve_font_size` pulls out its one
    // genuinely pure piece (clamping + ratio-preserving line-height
    // derivation) so that much stays testable the same way everything
    // else here is; the buffer-reshaping side of `set_font_size` is
    // covered by code review instead, same posture as `prepare`/`render`.
    #[test]
    fn resolve_font_size_clamps_to_the_allowed_range() {
        assert_eq!(resolve_font_size(1000.0).0, MAX_FONT_SIZE);
        assert_eq!(resolve_font_size(0.0).0, MIN_FONT_SIZE);
    }

    #[test]
    fn a_scale_factor_of_one_leaves_the_resolved_metrics_alone() {
        assert_eq!(scaled_font_metrics(16.0, 1.0), resolve_font_size(16.0));
    }

    #[test]
    fn a_denser_monitor_gets_proportionally_more_physical_pixels() {
        let (base_size, base_height) = resolve_font_size(16.0);
        let (size, height) = scaled_font_metrics(16.0, 1.5);
        assert_eq!(size, base_size * 1.5);
        assert_eq!(height, base_height * 1.5);
        // The ratio the whole layout depends on has to survive scaling.
        assert!((height / size - base_height / base_size).abs() < 1e-6);
    }

    #[test]
    fn scaling_happens_after_the_clamp_not_before_it() {
        // The largest size you can ask for, on a 200% monitor, is 2x
        // MAX_FONT_SIZE of real pixels -- not MAX_FONT_SIZE, which
        // would render smaller on the denser screen than on a normal
        // one and defeat the point.
        assert_eq!(scaled_font_metrics(1000.0, 2.0).0, MAX_FONT_SIZE * 2.0);
        assert_eq!(scaled_font_metrics(1.0, 2.0).0, MIN_FONT_SIZE * 2.0);
    }

    #[test]
    fn resolve_font_size_preserves_the_line_height_ratio() {
        let (font_size, line_height) = resolve_font_size(24.0);
        assert_eq!(font_size, 24.0);
        assert_eq!(line_height, 24.0 * (LINE_HEIGHT / FONT_SIZE));
    }

    #[test]
    fn resolve_font_size_at_the_default_matches_the_default_constants() {
        assert_eq!(resolve_font_size(FONT_SIZE), (FONT_SIZE, LINE_HEIGHT));
    }

}
