//! The Fenix identity as code: the Forged F mark, the wordmark, and the
//! app icon, all drawn from one vector definition by a small rasterizer
//! -- so the window icon, the `.ico` embedded in every Windows build, the
//! committed `fenix.ico`, and the Home dashboard's logo can't drift apart.
//!
//! Construction (see the identity's "01 · Mark" board): three blades on
//! a 48-unit grid, each 8 units wide, every cut rising 1 : 2, 6 units
//! between blades; the stem is Cinder, the long arm Ember, the short arm
//! Flame. The wordmark is "fenix" in Martian Mono SemiBold at −4 %
//! tracking, kept as outlines (`wordmark.rs`) so no font ships.

mod raster;
mod wordmark;

pub use raster::Image;
use raster::{rounded_rect, Canvas};

pub const CINDER: [u8; 3] = [0xD6, 0x3A, 0x2F];
pub const EMBER: [u8; 3] = [0xFF, 0x6A, 0x3D];
pub const FLAME: [u8; 3] = [0xFF, 0xB5, 0x47];
/// The app icon's tile and its hairline edge (graphite 800/700).
const TILE: [u8; 3] = [0x1D, 0x1E, 0x22];
const TILE_EDGE: [u8; 3] = [0x2F, 0x30, 0x36];

/// The mark's three blades on its 48-unit grid, already shifted down one
/// unit so the drawing sits centred in the box (it spans y 3..45).
/// One blade: its colour and its four corners.
type Blade = ([u8; 3], [(f32, f32); 4]);

const MARK: [Blade; 3] = [
    (CINDER, [(8.0, 45.0), (16.0, 45.0), (16.0, 7.0), (8.0, 11.0)]),
    (EMBER, [(16.0, 15.0), (40.0, 3.0), (40.0, 11.0), (16.0, 23.0)]),
    (FLAME, [(16.0, 29.0), (32.0, 21.0), (32.0, 29.0), (16.0, 37.0)]),
];
const GRID: f32 = 48.0;
/// Where the mark's blades stand within the grid: its visible top and
/// foot, and the long arm's tip.
const MARK_TOP: f32 = 3.0;
const MARK_FOOT: f32 = 45.0;
const MARK_RIGHT: f32 = 40.0;
const BLADE: f32 = 8.0;

fn draw_mark(canvas: &mut Canvas, x: f32, y: f32, box_size: f32) {
    let unit = box_size / GRID;
    for (color, points) in MARK {
        let contour: Vec<(f32, f32)> = points.iter().map(|&(px, py)| (x + px * unit, y + py * unit)).collect();
        canvas.fill(&[contour], color, 1.0);
    }
}

/// The bare mark, `size` pixels square, on transparency.
pub fn mark(size: u32) -> Image {
    let mut canvas = Canvas::new(size, size);
    draw_mark(&mut canvas, 0.0, 0.0, size as f32);
    canvas.into_image()
}

/// One blade of the mark -- 0 the stem, 1 the long arm, 2 the short arm
/// -- drawn where it sits in the whole mark, `size` pixels square, so the
/// three can be moved and faded separately and still line up.
pub fn blade(index: usize, size: u32) -> Image {
    let mut canvas = Canvas::new(size, size);
    let unit = size as f32 / GRID;
    if let Some((color, points)) = MARK.get(index) {
        let contour: Vec<(f32, f32)> = points.iter().map(|&(px, py)| (px * unit, py * unit)).collect();
        canvas.fill(&[contour], *color, 1.0);
    }
    canvas.into_image()
}

/// "fenix" alone in `text`, its capitals `cap_height` pixels tall, with
/// a pixel of room on every side.
pub fn wordmark(cap_height: u32, text: [u8; 3]) -> Image {
    let em = cap_height as f32 / wordmark::CAP_HEIGHT;
    let pad = 1.0;
    let width = ((wordmark::MAX_X - wordmark::MIN_X) * em + pad * 2.0).ceil() as u32;
    let height = ((wordmark::MAX_Y - wordmark::MIN_Y) * em + pad * 2.0).ceil() as u32;
    let origin_x = pad - wordmark::MIN_X * em;
    let baseline = pad - wordmark::MIN_Y * em;
    let mut canvas = Canvas::new(width.max(1), height.max(1));
    let contours: Vec<Vec<(f32, f32)>> =
        wordmark::CONTOURS.iter().map(|c| c.iter().map(|&(x, y)| (origin_x + x * em, baseline + y * em)).collect()).collect();
    canvas.fill(&contours, text, 1.0);
    canvas.into_image()
}

/// The app icon: the mark at 60 % on a graphite tile with a 22.5 %
/// corner radius and a hairline edge. At 16 px and below the tile goes --
/// there are too few pixels for both, and the mark is what has to read.
pub fn app_icon(size: u32) -> Image {
    if size <= 16 {
        return mark(size);
    }
    let s = size as f32;
    let mut canvas = Canvas::new(size, size);
    let radius = s * 0.225;
    let edge = (s / 128.0).max(1.0);
    canvas.fill(&[rounded_rect(0.0, 0.0, s, s, radius)], TILE_EDGE, 1.0);
    canvas.fill(&[rounded_rect(edge, edge, s - 2.0 * edge, s - 2.0 * edge, radius - edge)], TILE, 1.0);
    let box_size = s * 0.6;
    draw_mark(&mut canvas, (s - box_size) / 2.0, (s - box_size) / 2.0, box_size);
    canvas.into_image()
}

/// The horizontal lockup -- mark, then "fenix" in `text` -- `height`
/// pixels tall (the mark's full 48-unit box). Proportions from the
/// "02 · Wordmark & lockups" board: mark height is 1.4 × the wordmark's
/// cap height, one blade width between them, and the mark's foot sits on
/// the wordmark's baseline.
pub fn lockup(height: u32, text: [u8; 3]) -> Image {
    let h = height as f32;
    let unit = h / GRID;
    let em = ((MARK_FOOT - MARK_TOP) * unit / 1.4) / wordmark::CAP_HEIGHT;
    let origin_x = (MARK_RIGHT + BLADE) * unit - wordmark::MIN_X * em;
    let baseline = MARK_FOOT * unit;
    let width = (origin_x + wordmark::MAX_X * em + BLADE * unit / 2.0).ceil() as u32;

    let mut canvas = Canvas::new(width, height);
    draw_mark(&mut canvas, 0.0, 0.0, h);
    let contours: Vec<Vec<(f32, f32)>> = wordmark::CONTOURS
        .iter()
        .map(|c| c.iter().map(|&(x, y)| (origin_x + x * em, baseline + y * em)).collect())
        .collect();
    canvas.fill(&contours, text, 1.0);
    canvas.into_image()
}

/// Width of `lockup(height, _)` without drawing it -- for laying out
/// what goes beside it.
pub fn lockup_width(height: u32) -> u32 {
    let h = height as f32;
    let unit = h / GRID;
    let em = ((MARK_FOOT - MARK_TOP) * unit / 1.4) / wordmark::CAP_HEIGHT;
    let origin_x = (MARK_RIGHT + BLADE) * unit - wordmark::MIN_X * em;
    (origin_x + wordmark::MAX_X * em + BLADE * unit / 2.0).ceil() as u32
}

/// The sizes packed into `fenix.ico`: what Windows picks from for the
/// taskbar, Alt-Tab, Explorer's views and the title bar.
pub const ICO_SIZES: [u32; 6] = [256, 64, 48, 32, 24, 16];

/// `fenix.ico`: every size in `ICO_SIZES`, PNG-compressed.
#[cfg(feature = "ico")]
pub fn ico_bytes() -> Vec<u8> {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    let frames: Vec<IcoFrame> = ICO_SIZES
        .iter()
        .map(|&size| {
            let icon = app_icon(size);
            IcoFrame::as_png(&icon.rgba, size, size, image::ExtendedColorType::Rgba8).expect("an RGBA frame of a valid icon size encodes")
        })
        .collect();
    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames).expect("encoding into memory cannot fail");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        image.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_mark_puts_each_blade_where_the_grid_says() {
        let m = mark(48);
        assert_eq!(&pixel(&m, 12, 40)[..3], &CINDER, "stem");
        assert_eq!(&pixel(&m, 28, 12)[..3], &EMBER, "long arm");
        assert_eq!(&pixel(&m, 22, 29)[..3], &FLAME, "short arm");
        assert_eq!(pixel(&m, 2, 2)[3], 0, "outside the blades is transparent");
        assert_eq!(pixel(&m, 36, 30)[3], 0, "between the arms is transparent");
    }

    #[test]
    fn edges_are_antialiased() {
        let m = mark(96);
        let alphas: Vec<u8> = (0..96).map(|x| pixel(&m, x, 30)[3]).collect();
        assert!(alphas.iter().any(|&a| a > 0 && a < 255), "no partial coverage along a row crossing the arms");
    }

    #[test]
    fn the_icon_tile_is_opaque_graphite_and_rounded() {
        let icon = app_icon(256);
        assert_eq!(pixel(&icon, 0, 0)[3], 0, "the corner is cut away");
        assert_eq!(pixel(&icon, 128, 250), [TILE[0], TILE[1], TILE[2], 255]);
        assert_eq!(&pixel(&icon, 128, 99)[..3], &EMBER, "the mark sits in the middle");
    }

    #[test]
    fn tiny_icons_drop_the_tile() {
        assert_eq!(app_icon(16).rgba, mark(16).rgba);
    }

    #[test]
    fn the_lockup_is_the_mark_then_the_word() {
        let l = lockup(96, [0xF2, 0xF2, 0xF2]);
        assert_eq!(l.width, lockup_width(96));
        assert!(l.width > 96 * 3, "the word is wider than the mark: {}", l.width);
        let word_ink = (100..l.width).flat_map(|x| (0..96).map(move |y| (x, y))).filter(|&(x, y)| pixel(&l, x, y)[3] == 255).count();
        assert!(word_ink > 1000, "the wordmark was drawn: {word_ink} solid pixels");
        let above_mark_top = (0..l.width).filter(|&x| pixel(&l, x, 3)[3] > 0).count();
        assert_eq!(above_mark_top, 0, "nothing reaches above the mark's top edge");
    }

    #[cfg(feature = "ico")]
    #[test]
    fn the_ico_carries_every_size() {
        let bytes = ico_bytes();
        let count = u16::from_le_bytes([bytes[4], bytes[5]]);
        assert_eq!(count as usize, ICO_SIZES.len());
    }

    /// The committed `fenix.ico` -- what's in the repository for anyone
    /// using the icon outside a build -- is the one this crate draws.
    /// Compared as pixels, not bytes, so a PNG encoder update can't fail
    /// it. Regenerate with `cargo run -p fenix-brand --example write-icon
    /// --features ico`.
    #[cfg(feature = "ico")]
    #[test]
    fn the_committed_fenix_ico_is_the_current_icon() {
        let committed = image::load_from_memory_with_format(include_bytes!("../../../fenix.ico"), image::ImageFormat::Ico)
            .expect("fenix.ico decodes")
            .to_rgba8();
        let expected = app_icon(committed.width());
        let worst = committed.as_raw().iter().zip(&expected.rgba).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(worst <= 2, "fenix.ico differs from fenix-brand's icon (worst channel diff {worst}); run the write-icon example");
    }
}
