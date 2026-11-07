//! Code screenshots (`SPC c s`): lines of coloured text drawn on the CPU
//! into a picture -- a window with rounded corners and a soft shadow, an
//! optional title bar with three dots and the file's name, on a
//! background of its own -- the way Carbon and ray.so draw them.
//!
//! Pure: lines of `(text, colour)` spans, the theme's colours, a font
//! system and the options in; RGBA pixels out. The host (`app/
//! screenshot_host.rs`) reads the buffer, and copies and saves what this
//! draws.

use glyphon::cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, Wrap};
use tiny_skia::{FillRule, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point, Rect, SpreadMode, Stroke, Transform};
use unicode_width::UnicodeWidthChar;

use crate::tabstops::{self, TabStops};

/// Red, green, blue and alpha.
pub type Rgba = [u8; 4];

/// One line of text, as runs of one colour each.
pub type Line = Vec<(String, Rgba)>;

/// What's behind the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// A gradient of the theme's accent.
    Accent,
    /// The theme's accent, flat.
    Solid,
    Sunset,
    Ocean,
    Candy,
    Forest,
    /// Nothing: the window and its shadow on a see-through picture.
    None,
}

impl Style {
    pub fn parse(name: &str) -> Option<Style> {
        Some(match name {
            "accent" => Style::Accent,
            "solid" => Style::Solid,
            "sunset" => Style::Sunset,
            "ocean" => Style::Ocean,
            "candy" => Style::Candy,
            "forest" => Style::Forest,
            "none" => Style::None,
            _ => return None,
        })
    }

    /// Its two ends, top left to bottom right; `None` for no background.
    fn gradient(self, accent: Rgba) -> Option<(Rgba, Rgba)> {
        match self {
            // From the accent to a neighbouring hue, a little deeper.
            Style::Accent => {
                let (h, s, l) = hsl(accent);
                let s = s.max(0.45);
                Some((from_hsl(h, s, (l + 0.06).min(0.78)), from_hsl(h - 50.0, (s * 1.1).min(1.0), (l - 0.12).max(0.28))))
            }
            Style::Solid => Some((accent, accent)),
            Style::Sunset => Some(([0xff, 0x7e, 0x5f, 255], [0xa8, 0x3f, 0x8f, 255])),
            Style::Ocean => Some(([0x36, 0xd1, 0xdc, 255], [0x3a, 0x47, 0xb8, 255])),
            Style::Candy => Some(([0xff, 0x8a, 0xc6, 255], [0x78, 0x73, 0xf5, 255])),
            Style::Forest => Some(([0x8c, 0xc8, 0x7a, 255], [0x13, 0x4e, 0x5e, 255])),
            Style::None => None,
        }
    }
}

/// What a line too long for `Options::max_columns` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongLines {
    /// It goes on the next row, under its own text.
    Wrap,
    /// It stops at the edge, with an ellipsis.
    Cut,
}

/// The theme's colours the picture is drawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// The window.
    pub bg: Rgba,
    /// Text without a colour of its own.
    pub fg: Rgba,
    /// Line numbers, the title and a cut line's ellipsis.
    pub gutter: Rgba,
    /// The accent background and highlighted lines.
    pub accent: Rgba,
}

/// How it's drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub style: Style,
    /// The title bar's text; `None` draws no title bar at all.
    pub title: Option<String>,
    pub line_numbers: bool,
    /// The number the first line shows.
    pub first_number: usize,
    /// Indexes (into the lines) of the lines to highlight.
    pub highlight: Vec<usize>,
    /// Pixels per logical pixel: 2 is retina quality.
    pub scale: f32,
    /// Logical pixels of background around the window.
    pub padding: f32,
    /// Logical points.
    pub font_size: f32,
    /// A font family installed on this machine; `None` is the system's
    /// monospace font.
    pub family: Option<String>,
    /// Columns of text, at most, before a line wraps or is cut.
    pub max_columns: usize,
    pub long_lines: LongLines,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            style: Style::Accent,
            title: None,
            line_numbers: false,
            first_number: 1,
            highlight: Vec::new(),
            scale: 2.0,
            padding: 56.0,
            font_size: 15.0,
            family: None,
            max_columns: 100,
            long_lines: LongLines::Wrap,
        }
    }
}

/// A drawn picture: straight (not premultiplied) RGBA, row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    #[cfg(test)]
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        let i = ((y * self.width + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    pub fn png(&self) -> Option<Vec<u8>> {
        let img = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())?;
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some(out.into_inner())
    }
}

// Logical pixels.
const WINDOW_PAD_X: f32 = 28.0;
const WINDOW_PAD_Y: f32 = 24.0;
const TITLE_HEIGHT: f32 = 44.0;
const RADIUS: f32 = 12.0;
const DOT_RADIUS: f32 = 6.5;
const DOT_GAP: f32 = 21.0;
const SHADOW_BLUR: f32 = 26.0;
const SHADOW_OFFSET: f32 = 16.0;
const SHADOW_ALPHA: f32 = 0.55;
/// The narrowest a window is drawn, so a one-word picture still looks
/// like a window.
const MIN_WINDOW_WIDTH: f32 = 340.0;
/// Rows are this many times the font size apart.
const LINE_SPACING: f32 = 1.55;
/// The longest side a picture has, in pixels: a bigger one is drawn at a
/// smaller scale instead.
pub const MAX_SIDE: f32 = 16_384.0;
/// The most rows a picture has; a longer one stops there.
pub const MAX_ROWS: usize = 600;

/// The system's fonts, for drawing with no window open (the control
/// channel, tests) -- read the first time, since reading them is slow.
pub fn offscreen_fonts() -> std::sync::MutexGuard<'static, FontSystem> {
    static FONTS: std::sync::OnceLock<std::sync::Mutex<FontSystem>> = std::sync::OnceLock::new();
    FONTS.get_or_init(|| std::sync::Mutex::new(FontSystem::new())).lock().unwrap_or_else(|p| p.into_inner())
}

// -- Text --------------------------------------------------------------

/// What the lines read, ready to draw: tabs expanded (by `tab_width`),
/// trailing whitespace gone, the indentation every line shares removed,
/// and blank lines at either end dropped. `dropped_above` is how many
/// went from the top, for the line numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub lines: Vec<Line>,
    pub dropped_above: usize,
}

pub fn prepare(lines: Vec<Line>, tab_width: usize) -> Prepared {
    let mut lines: Vec<Line> = lines.into_iter().map(|l| trim_end(expand_tabs(l, tab_width))).collect();
    let blank = |l: &Line| l.iter().all(|(t, _)| t.is_empty());
    let dropped_above = lines.iter().take_while(|l| blank(l)).count();
    lines.drain(..dropped_above);
    while lines.last().is_some_and(blank) {
        lines.pop();
    }
    let indent = lines.iter().filter(|l| !blank(l)).map(|l| text_of(l).chars().take_while(|c| *c == ' ').count()).min().unwrap_or(0);
    if indent > 0 {
        for line in &mut lines {
            *line = skip_columns(line, indent);
        }
    }
    Prepared { lines, dropped_above }
}

fn text_of(line: &Line) -> String {
    line.iter().map(|(t, _)| t.as_str()).collect()
}

/// `line` with its tabs as spaces, each space the colour of its tab.
fn expand_tabs(line: Line, tab_width: usize) -> Line {
    let text = text_of(&line);
    if !text.contains('\t') {
        return line;
    }
    let (display, col_map) = tabstops::expand_line(&text, &TabStops::Fixed(tab_width.max(1)));
    let colours: Vec<Rgba> = line.iter().flat_map(|(t, c)| t.chars().map(move |_| *c)).collect();
    let display: Vec<char> = display.chars().collect();
    let mut out: Line = Vec::new();
    for (i, colour) in colours.iter().enumerate() {
        let piece: String = display[col_map[i]..col_map[i + 1]].iter().collect();
        push(&mut out, &piece, *colour);
    }
    out
}

/// Appends `text` in `colour`, onto the last run when it's the same colour.
fn push(line: &mut Line, text: &str, colour: Rgba) {
    match line.last_mut() {
        Some((t, c)) if *c == colour => t.push_str(text),
        _ if text.is_empty() => {}
        _ => line.push((text.to_string(), colour)),
    }
}

fn trim_end(mut line: Line) -> Line {
    while let Some((t, _)) = line.last_mut() {
        let kept = t.trim_end().len();
        t.truncate(kept);
        if !t.is_empty() {
            break;
        }
        line.pop();
    }
    line
}

/// `line` without its first `n` chars.
fn skip_columns(line: &Line, mut n: usize) -> Line {
    let mut out = Vec::new();
    for (t, c) in line {
        let count = t.chars().count();
        if n >= count {
            n -= count;
            continue;
        }
        out.push((t.chars().skip(n).collect(), *c));
        n = 0;
    }
    out
}

/// One row of the picture: a line, or the part of one that wrapped.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// Its line's number; `None` on a wrapped line's later rows.
    pub number: Option<usize>,
    pub spans: Line,
    pub highlighted: bool,
    /// The line went on past the edge (`LongLines::Cut`).
    pub cut: bool,
}

/// The rows `lines` take at `options.max_columns`, wrapped or cut.
pub fn rows(lines: &[Line], options: &Options) -> Vec<Row> {
    let max = options.max_columns.max(8);
    let mut rows = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let highlighted = options.highlight.contains(&i);
        let number = Some(options.first_number + i);
        let mut pieces = split_columns(line, max);
        if options.long_lines == LongLines::Cut && pieces.len() > 1 {
            let first = split_columns(line, max - 1).swap_remove(0);
            rows.push(Row { number, spans: first, highlighted, cut: true });
            continue;
        }
        if pieces.is_empty() {
            pieces.push(Vec::new());
        }
        for (k, spans) in pieces.into_iter().enumerate() {
            rows.push(Row { number: if k == 0 { number } else { None }, spans, highlighted, cut: false });
        }
    }
    rows.truncate(MAX_ROWS);
    rows
}

/// `line` in pieces of at most `max` columns, a wide character counting
/// two.
fn split_columns(line: &Line, max: usize) -> Vec<Line> {
    let mut pieces: Vec<Line> = Vec::new();
    let mut current: Line = Vec::new();
    let mut width = 0;
    for (t, c) in line {
        for ch in t.chars() {
            let w = ch.width().unwrap_or(0);
            if width + w > max && width > 0 {
                pieces.push(std::mem::take(&mut current));
                width = 0;
            }
            let mut buf = [0u8; 4];
            push(&mut current, ch.encode_utf8(&mut buf), *c);
            width += w;
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

// -- Layout ------------------------------------------------------------

/// Where everything goes, in pixels (already scaled).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// The window: left, top, width, height.
    pub window: (f32, f32, f32, f32),
    pub title_height: f32,
    /// Where the first row's text starts.
    pub text_x: f32,
    pub text_y: f32,
    pub line_height: f32,
}

/// The layout for `rows` rows whose widest is `text_width` logical pixels
/// across. A picture that would be bigger than `MAX_SIDE` is drawn at a
/// smaller scale.
pub fn layout(rows: usize, text_width: f32, options: &Options) -> Layout {
    let logical = |scale: f32| {
        let title_height = if options.title.is_some() { TITLE_HEIGHT } else { 0.0 };
        let top_pad = if options.title.is_some() { WINDOW_PAD_Y * 0.5 } else { WINDOW_PAD_Y };
        let line_height = (options.font_size * LINE_SPACING).round();
        let window_w = (text_width + 2.0 * WINDOW_PAD_X).max(MIN_WINDOW_WIDTH);
        let window_h = title_height + top_pad + rows.max(1) as f32 * line_height + WINDOW_PAD_Y;
        let pad = options.padding.max(0.0);
        let width = ((window_w + 2.0 * pad) * scale).ceil();
        let height = ((window_h + 2.0 * pad) * scale).ceil();
        Layout {
            width: width as u32,
            height: height as u32,
            scale,
            window: (pad * scale, pad * scale, window_w * scale, window_h * scale),
            title_height: title_height * scale,
            text_x: (pad + WINDOW_PAD_X) * scale,
            text_y: (pad + title_height + top_pad) * scale,
            line_height: line_height * scale,
        }
    };
    let wanted = options.scale.clamp(0.5, 4.0);
    let at_one = logical(1.0);
    let biggest = at_one.width.max(at_one.height) as f32;
    let scale = if biggest * wanted > MAX_SIDE { (MAX_SIDE / biggest).max(0.25) } else { wanted };
    logical(scale)
}

/// How wide the line numbers are, in columns: the widest number and two
/// spaces.
pub fn gutter_columns(rows: &[Row]) -> usize {
    rows.iter().filter_map(|r| r.number).max().map(|n| n.to_string().len() + 2).unwrap_or(0)
}

// -- Drawing -----------------------------------------------------------

/// Draws `lines` (as `prepare` left them) as a framed picture.
pub fn render(lines: &[Line], palette: &Palette, font_system: &mut FontSystem, options: &Options) -> Image {
    let rows = rows(lines, options);
    let gutter = if options.line_numbers { gutter_columns(&rows) } else { 0 };
    let family_name = options.family.clone();
    let family = match &family_name {
        Some(name) => Family::Name(name.as_str()),
        None => Family::Monospace,
    };

    // Shaped once at 1x to know how wide the text is, then again at the
    // scale it's drawn at.
    let texts: Vec<Line> = rows.iter().map(|r| row_spans(r, gutter, palette)).collect();
    let probe = Metrics::new(options.font_size, options.font_size * LINE_SPACING);
    let text_width = texts.iter().map(|spans| shape(font_system, spans, probe, family).1).fold(0.0, f32::max).ceil();
    let layout = layout(rows.len(), text_width, options);
    let s = layout.scale;

    let mut pixmap = Pixmap::new(layout.width.max(1), layout.height.max(1)).expect("a picture of some size");
    background(&mut pixmap, options.style, palette.accent);
    let (wx, wy, ww, wh) = layout.window;
    let window = rounded_rect(wx, wy, ww, wh, RADIUS * s);
    shadow(&mut pixmap, &window, s);
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(skia(palette.bg));
    pixmap.fill_path(&window, &paint, FillRule::Winding, Transform::identity(), None);

    // A hairline edge, so a dark window reads on a dark background.
    let edge = if luminance(palette.bg) < 0.5 { [255, 255, 255, 26] } else { [0, 0, 0, 30] };
    paint.set_color(skia(edge));
    let inset = rounded_rect(wx + 0.5 * s, wy + 0.5 * s, ww - s, wh - s, (RADIUS - 0.5) * s);
    pixmap.stroke_path(&inset, &paint, &Stroke { width: s, ..Stroke::default() }, Transform::identity(), None);

    // Highlighted lines: a band of the accent across the window, and a
    // bar of it at the window's left.
    for (i, row) in rows.iter().enumerate().filter(|(_, r)| r.highlighted) {
        let y = layout.text_y + i as f32 * layout.line_height;
        if let Some(rect) = Rect::from_xywh(wx + s, y, ww - 2.0 * s, layout.line_height) {
            paint.set_color(skia(with_alpha(palette.accent, 38)));
            pixmap.fill_rect(rect, &paint, Transform::identity(), None);
        }
        if let Some(bar) = Rect::from_xywh(wx + s, y, 3.0 * s, layout.line_height) {
            paint.set_color(skia(palette.accent));
            pixmap.fill_rect(bar, &paint, Transform::identity(), None);
        }
        let _ = row;
    }

    let mut cache = SwashCache::new();
    if let Some(title) = &options.title {
        let centre_y = wy + layout.title_height / 2.0;
        for (k, colour) in [[0xff, 0x5f, 0x57, 255], [0xfe, 0xbc, 0x2e, 255], [0x28, 0xc8, 0x40, 255]].into_iter().enumerate() {
            if let Some(dot) = PathBuilder::from_circle(wx + (WINDOW_PAD_X - 6.0 + DOT_GAP * k as f32) * s, centre_y, DOT_RADIUS * s) {
                paint.set_color(skia(colour));
                pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
        let size = options.font_size * 0.85 * s;
        let metrics = Metrics::new(size, size * 1.3);
        let spans = vec![(title.clone(), palette.gutter)];
        let (buffer, w) = shape(font_system, &spans, metrics, family);
        // Centred, unless that would run into the dots.
        let dots_end = wx + (WINDOW_PAD_X + DOT_GAP * 3.0) * s;
        let x = (wx + (ww - w) / 2.0).max(dots_end);
        draw(&mut pixmap, &mut cache, font_system, &buffer, x, centre_y - metrics.line_height / 2.0, palette.fg);
    }

    let metrics = Metrics::new(options.font_size * s, layout.line_height);
    for (i, spans) in texts.iter().enumerate() {
        let (buffer, _) = shape(font_system, spans, metrics, family);
        draw(&mut pixmap, &mut cache, font_system, &buffer, layout.text_x, layout.text_y + i as f32 * layout.line_height, palette.fg);
    }

    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Image { width: pixmap.width(), height: pixmap.height(), rgba }
}

/// A row's runs as drawn: its line number first when there are any, and
/// an ellipsis when it was cut.
fn row_spans(row: &Row, gutter: usize, palette: &Palette) -> Line {
    let mut spans = Vec::new();
    if gutter > 0 {
        let number = row.number.map(|n| n.to_string()).unwrap_or_default();
        let colour = if row.highlighted { palette.fg } else { palette.gutter };
        spans.push((format!("{number:>width$}  ", width = gutter - 2), colour));
    }
    spans.extend(row.spans.iter().cloned());
    if row.cut {
        spans.push(("…".to_string(), palette.gutter));
    }
    spans
}

/// `spans` shaped on one line, and how wide they came out.
fn shape(font_system: &mut FontSystem, spans: &Line, metrics: Metrics, family: Family) -> (Buffer, f32) {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_wrap(Wrap::None);
    buffer.set_size(None, Some(metrics.line_height * 2.0));
    let attrs = Attrs::new().family(family);
    let rich = spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(Color::rgba(c[0], c[1], c[2], c[3]))));
    buffer.set_rich_text(rich, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let width = buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
    (buffer, width)
}

/// Draws `buffer`'s glyphs with its top left at `x`, `y`.
fn draw(pixmap: &mut Pixmap, cache: &mut SwashCache, font_system: &mut FontSystem, buffer: &Buffer, x: f32, y: f32, default: Rgba) {
    let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
    let data = pixmap.data_mut();
    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical = glyph.physical((x, y + run.line_y), 1.0);
            let colour = glyph.color_opt.map(|c| [c.r(), c.g(), c.b(), c.a()]).unwrap_or(default);
            let Some(image) = cache.get_image(font_system, physical.cache_key) else { continue };
            let left = physical.x + image.placement.left;
            let top = physical.y - image.placement.top;
            let (w, h) = (image.placement.width as i32, image.placement.height as i32);
            for gy in 0..h {
                for gx in 0..w {
                    let (px, py) = (left + gx, top + gy);
                    if px < 0 || py < 0 || px >= width || py >= height {
                        continue;
                    }
                    let k = (gy * w + gx) as usize;
                    let src = match image.content {
                        SwashContent::Mask => with_alpha(colour, mul(colour[3], image.data[k])),
                        SwashContent::SubpixelMask => {
                            let m = &image.data[k * 4..k * 4 + 3];
                            let cover = ((m[0] as u16 + m[1] as u16 + m[2] as u16) / 3) as u8;
                            with_alpha(colour, mul(colour[3], cover))
                        }
                        SwashContent::Color => [image.data[k * 4], image.data[k * 4 + 1], image.data[k * 4 + 2], image.data[k * 4 + 3]],
                    };
                    blend(&mut data[((py * width + px) * 4) as usize..][..4], src);
                }
            }
        }
    }
}

/// `src` (straight alpha) over a premultiplied pixel.
fn blend(dst: &mut [u8], src: Rgba) {
    let a = src[3] as f32 / 255.0;
    if a <= 0.0 {
        return;
    }
    for i in 0..3 {
        dst[i] = (src[i] as f32 * a + dst[i] as f32 * (1.0 - a)).round().min(255.0) as u8;
    }
    dst[3] = (src[3] as f32 + dst[3] as f32 * (1.0 - a)).round().min(255.0) as u8;
}

fn mul(a: u8, b: u8) -> u8 {
    ((a as u16 * b as u16 + 127) / 255) as u8
}

fn background(pixmap: &mut Pixmap, style: Style, accent: Rgba) {
    let Some((from, to)) = style.gradient(accent) else { return };
    let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
    let rect = Rect::from_xywh(0.0, 0.0, w, h).expect("a picture of some size");
    let mut paint = Paint::default();
    match LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(w, h),
        vec![GradientStop::new(0.0, skia(from)), GradientStop::new(1.0, skia(to))],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(shader) => paint.shader = shader,
        // Both ends the same colour.
        None => paint.set_color(skia(from)),
    }
    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
}

/// A soft shadow under `window`: its shape, lowered and blurred.
fn shadow(pixmap: &mut Pixmap, window: &Path, scale: f32) {
    let (w, h) = (pixmap.width(), pixmap.height());
    let Some(mut mask) = Pixmap::new(w, h) else { return };
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
    mask.fill_path(window, &paint, FillRule::Winding, Transform::from_translate(0.0, SHADOW_OFFSET * scale), None);
    let mut alpha: Vec<f32> = mask.pixels().iter().map(|p| p.alpha() as f32).collect();
    // Three box blurs come close to a Gaussian.
    let radius = ((SHADOW_BLUR * scale) / 3.0).round().max(1.0) as usize;
    for _ in 0..3 {
        box_blur(&mut alpha, w as usize, h as usize, radius);
    }
    for (p, a) in mask.pixels_mut().iter_mut().zip(alpha) {
        let a = (a * SHADOW_ALPHA).round().clamp(0.0, 255.0) as u8;
        *p = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, a).expect("black is premultiplied");
    }
    pixmap.draw_pixmap(0, 0, mask.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

/// One horizontal and one vertical pass of a running-sum box blur.
fn box_blur(values: &mut [f32], w: usize, h: usize, r: usize) {
    let mut line = vec![0.0; w.max(h)];
    let pass = |get: &dyn Fn(usize) -> f32, n: usize, out: &mut [f32]| {
        let span = (2 * r + 1) as f32;
        let at = |i: isize| get(i.clamp(0, n as isize - 1) as usize);
        let mut sum: f32 = (-(r as isize)..=r as isize).map(at).sum();
        for (i, o) in out.iter_mut().enumerate().take(n) {
            *o = sum / span;
            sum += at(i as isize + r as isize + 1) - at(i as isize - r as isize);
        }
    };
    for y in 0..h {
        let row = &values[y * w..(y + 1) * w];
        pass(&|x| row[x], w, &mut line);
        values[y * w..(y + 1) * w].copy_from_slice(&line[..w]);
    }
    for x in 0..w {
        {
            let column = |y: usize| values[y * w + x];
            pass(&column, h, &mut line);
        }
        for y in 0..h {
            values[y * w + x] = line[y];
        }
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let r = r.min(w / 2.0).min(h / 2.0);
    // How far along a corner's tangents its curve's handles sit.
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().expect("a window of some size")
}

fn skia(c: Rgba) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c[0], c[1], c[2], c[3])
}

fn with_alpha(c: Rgba, a: u8) -> Rgba {
    [c[0], c[1], c[2], a]
}

/// Hue (degrees), saturation and lightness (0 to 1).
fn hsl(c: Rgba) -> (f32, f32, f32) {
    let [r, g, b] = [c[0], c[1], c[2]].map(|v| v as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Rgba {
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let v = |n: f32| ((n + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [v(r), v(g), v(b), 255]
}

fn luminance(c: Rgba) -> f32 {
    (0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> std::sync::MutexGuard<'static, FontSystem> {
        offscreen_fonts()
    }

    const BG: Rgba = [0x1e, 0x1e, 0x2e, 255];
    const FG: Rgba = [0xcd, 0xd6, 0xf4, 255];
    const KEYWORD: Rgba = [0xff, 0x40, 0x40, 255];

    fn palette() -> Palette {
        Palette { bg: BG, fg: FG, gutter: [0x6c, 0x70, 0x86, 255], accent: [0x89, 0xb4, 0xfa, 255] }
    }

    fn plain(text: &str) -> Line {
        vec![(text.to_string(), FG)]
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(text_of).collect()
    }

    #[test]
    fn prepare_expands_tabs_trims_and_removes_the_shared_indentation() {
        let lines = vec![plain(""), plain("\tfn a() {   "), vec![("\t\t".into(), FG), ("x".into(), KEYWORD)], plain(""), plain("\t}"), plain("  ")];
        let p = prepare(lines, 4);
        assert_eq!(p.dropped_above, 1);
        assert_eq!(texts(&p.lines), ["fn a() {", "    x", "", "}"]);
        assert_eq!(p.lines[1].last(), Some(&("x".to_string(), KEYWORD)), "colours survive");
    }

    #[test]
    fn a_tab_takes_its_colour_into_its_spaces() {
        let line = vec![("a".into(), KEYWORD), ("\tb".into(), FG)];
        assert_eq!(expand_tabs(line, 4), vec![("a".to_string(), KEYWORD), ("   b".to_string(), FG)]);
    }

    #[test]
    fn long_lines_wrap_under_themselves_or_are_cut() {
        let lines = vec![plain(&"x".repeat(25)), plain("short")];
        let mut options = Options { max_columns: 10, first_number: 7, ..Options::default() };
        let wrapped = rows(&lines, &options);
        assert_eq!(wrapped.iter().map(|r| r.number).collect::<Vec<_>>(), [Some(7), None, None, Some(8)]);
        assert_eq!(text_of(&wrapped[2].spans), "xxxxx");
        options.long_lines = LongLines::Cut;
        let cut = rows(&lines, &options);
        assert_eq!(cut.len(), 2);
        assert!(cut[0].cut && !cut[1].cut);
        assert_eq!(text_of(&cut[0].spans).len(), 9, "room for the ellipsis");
    }

    #[test]
    fn a_picture_has_at_most_max_rows() {
        let lines = vec![plain(&"x".repeat(50)); MAX_ROWS];
        assert_eq!(rows(&lines, &Options { max_columns: 10, ..Options::default() }).len(), MAX_ROWS);
    }

    #[test]
    fn a_wide_character_counts_two_columns() {
        let pieces = split_columns(&plain("日本語x"), 4);
        assert_eq!(texts(&pieces), ["日本", "語x"]);
    }

    #[test]
    fn the_layout_adds_padding_title_and_rows_up() {
        let options = Options { scale: 2.0, padding: 50.0, font_size: 10.0, title: Some("a.rs".into()), ..Options::default() };
        let l = layout(4, 400.0, &options);
        let window_w = 400.0 + 2.0 * WINDOW_PAD_X;
        let window_h = TITLE_HEIGHT + WINDOW_PAD_Y * 0.5 + 4.0 * 16.0 + WINDOW_PAD_Y;
        assert_eq!((l.width, l.height), (((window_w + 100.0) * 2.0) as u32, ((window_h + 100.0) * 2.0) as u32));
        assert_eq!(l.window, (100.0, 100.0, window_w * 2.0, window_h * 2.0));
        assert_eq!(l.text_y, (50.0 + TITLE_HEIGHT + WINDOW_PAD_Y * 0.5) * 2.0);
        assert_eq!(l.line_height, 32.0);
        let untitled = layout(4, 400.0, &Options { title: None, ..options.clone() });
        assert_eq!(untitled.text_y, (50.0 + WINDOW_PAD_Y) * 2.0);
        let narrow = layout(1, 10.0, &options);
        assert_eq!(narrow.window.2, MIN_WINDOW_WIDTH * 2.0);
    }

    #[test]
    fn a_picture_too_tall_is_drawn_smaller() {
        let l = layout(MAX_ROWS, 400.0, &Options::default());
        assert!(l.height as f32 <= MAX_SIDE + 1.0, "{}", l.height);
        assert!(l.scale < 2.0);
    }

    #[test]
    fn the_gutter_fits_the_biggest_number() {
        let options = Options { first_number: 98, ..Options::default() };
        let r = rows(&[plain("a"), plain("b"), plain("c")], &options);
        assert_eq!(gutter_columns(&r), 5);
        assert_eq!(text_of(&row_spans(&r[0], 5, &palette())), " 98  a");
    }

    #[test]
    fn the_picture_has_its_size_background_window_and_text() {
        let lines = vec![vec![("fn".to_string(), KEYWORD), (" main() {}".to_string(), FG)]];
        let options = Options { scale: 1.0, padding: 40.0, font_size: 16.0, title: Some("main.rs".into()), line_numbers: true, ..Options::default() };
        let image = render(&lines, &palette(), &mut fonts(), &options);
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
        let rows = rows(&lines, &options);
        // As wide as the window it lays out, whatever the font's advance.
        let l = layout(rows.len(), 0.0, &options);
        assert_eq!(image.height, l.height);
        assert!(image.width >= l.width);

        // The background's top left: the accent gradient's lighter end.
        let corner = image.pixel(0, 0);
        assert_eq!(corner, Style::Accent.gradient(palette().accent).unwrap().0);
        // The window, inside its edge.
        let (wx, wy, ww, _) = l.window;
        assert_eq!(image.pixel((wx + ww / 2.0) as u32, (wy + l.title_height + 4.0) as u32), BG);
        // Text in the keyword's colour, where the first row is.
        let row_band = (l.text_y as u32)..((l.text_y + l.line_height) as u32);
        let red = row_band.flat_map(|y| (0..image.width).map(move |x| (x, y))).filter(|&(x, y)| image.pixel(x, y) == KEYWORD).count();
        assert!(red > 10, "the keyword's pixels: {red}");
    }

    #[test]
    fn the_window_casts_a_shadow_below_it() {
        let options = Options { scale: 1.0, style: Style::Solid, ..Options::default() };
        let image = render(&[plain("x")], &palette(), &mut fonts(), &options);
        let l = layout(1, 0.0, &options);
        let (wx, wy, ww, wh) = l.window;
        assert_eq!(image.pixel(0, 0), palette().accent);
        let below = image.pixel((wx + ww / 2.0) as u32, (wy + wh + 8.0) as u32);
        assert!(luminance(below) < luminance(palette().accent) - 0.1, "{below:?}");
        let beside = image.pixel(2, (wy + wh / 2.0) as u32);
        assert_eq!(beside, palette().accent, "it fades out before the edge");
    }

    #[test]
    fn no_background_leaves_the_corners_see_through() {
        let options = Options { scale: 1.0, style: Style::None, ..Options::default() };
        let image = render(&[plain("x")], &palette(), &mut fonts(), &options);
        assert_eq!(image.pixel(0, 0)[3], 0);
        assert!(image.png().is_some_and(|png| png.starts_with(b"\x89PNG")));
    }

    #[test]
    fn a_highlighted_line_is_drawn_on_the_accent() {
        let options = Options { scale: 1.0, highlight: vec![1], ..Options::default() };
        let image = render(&[plain("a"), plain("b")], &palette(), &mut fonts(), &options);
        let l = layout(2, 0.0, &options);
        let (wx, ..) = l.window;
        let bar = image.pixel((wx + 2.0) as u32, (l.text_y + l.line_height * 1.5) as u32);
        assert_eq!(bar, palette().accent);
        let not = image.pixel((wx + 2.0) as u32, (l.text_y + l.line_height * 0.5) as u32);
        assert_eq!(not, BG);
    }

    #[test]
    fn hsl_goes_there_and_back() {
        for c in [[0x89, 0xb4, 0xfa, 255], [255, 0, 0, 255], [10, 200, 90, 255], [128, 128, 128, 255]] {
            let (h, s, l) = hsl(c);
            let back = from_hsl(h, s, l);
            assert!(back.iter().zip(c).all(|(a, b)| a.abs_diff(b) <= 1), "{c:?} -> {back:?}");
        }
    }

    #[test]
    fn every_style_the_setting_offers_is_one() {
        let setting = fenix_config::schema::setting("screenshot.style").expect("the setting");
        let fenix_config::Kind::Choice(names) = setting.kind else { panic!("a choice") };
        for name in names {
            assert!(Style::parse(name).is_some(), "{name}");
        }
    }
}
