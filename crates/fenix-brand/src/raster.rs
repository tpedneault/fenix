//! Just enough of a rasterizer for the logo: filled polygons (nonzero
//! winding, so letters keep their counters), antialiased with exact
//! horizontal coverage over a handful of sub-scanlines per pixel row,
//! composited source-over onto a straight-alpha RGBA canvas.

/// Straight (not premultiplied) RGBA, row-major, 4 bytes per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// The same pixels as BGRA -- what the GPU textures the editor
    /// draws with expect.
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut out = self.rgba.clone();
        for px in out.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        out
    }
}

/// Sub-scanlines sampled per pixel row.
const SUBSAMPLES: usize = 5;

pub(crate) struct Canvas {
    width: u32,
    height: u32,
    /// Straight-alpha colour and alpha, kept as floats until the end.
    color: Vec<[f32; 3]>,
    alpha: Vec<f32>,
    coverage: Vec<f32>,
}

impl Canvas {
    pub(crate) fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        Self { width, height, color: vec![[0.0; 3]; n], alpha: vec![0.0; n], coverage: vec![0.0; n] }
    }

    /// Fills the shape the closed `contours` enclose, in pixel
    /// coordinates, with `rgb` at `opacity`.
    pub(crate) fn fill(&mut self, contours: &[Vec<(f32, f32)>], rgb: [u8; 3], opacity: f32) {
        self.coverage.iter_mut().for_each(|c| *c = 0.0);
        let edges: Vec<((f32, f32), (f32, f32))> = contours
            .iter()
            .flat_map(|c| (0..c.len()).map(move |i| (c[i], c[(i + 1) % c.len()])))
            .filter(|(a, b)| a.1 != b.1)
            .collect();
        let (w, h) = (self.width as usize, self.height as usize);
        let mut crossings: Vec<(f32, i32)> = Vec::new();
        for row in 0..h {
            for sub in 0..SUBSAMPLES {
                let y = row as f32 + (sub as f32 + 0.5) / SUBSAMPLES as f32;
                crossings.clear();
                for &((x0, y0), (x1, y1)) in &edges {
                    let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                    if y < lo || y >= hi {
                        continue;
                    }
                    let t = (y - y0) / (y1 - y0);
                    crossings.push((x0 + t * (x1 - x0), if y1 > y0 { 1 } else { -1 }));
                }
                crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut winding = 0;
                for pair in crossings.windows(2) {
                    winding += pair[0].1;
                    if winding != 0 {
                        self.cover_span(row, pair[0].0, pair[1].0, w);
                    }
                }
            }
        }
        let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
        for i in 0..w * h {
            let a = (self.coverage[i] / SUBSAMPLES as f32).min(1.0) * opacity;
            if a <= 0.0 {
                continue;
            }
            let dst_a = self.alpha[i];
            let out_a = a + dst_a * (1.0 - a);
            let blend = |src: f32, dst: f32| (src * a + dst * dst_a * (1.0 - a)) / out_a;
            let d = self.color[i];
            self.color[i] = [blend(r, d[0]), blend(g, d[1]), blend(b, d[2])];
            self.alpha[i] = out_a;
        }
    }

    /// Adds `[x0, x1)` on one sub-scanline of `row` to the coverage
    /// buffer, with fractional coverage at both ends.
    fn cover_span(&mut self, row: usize, x0: f32, x1: f32, w: usize) {
        let x0 = x0.max(0.0);
        let x1 = x1.min(w as f32);
        if x1 <= x0 {
            return;
        }
        let first = x0.floor() as usize;
        let last = (x1.ceil() as usize).min(w);
        for px in first..last {
            let lo = x0.max(px as f32);
            let hi = x1.min(px as f32 + 1.0);
            if hi > lo {
                self.coverage[row * w + px] += hi - lo;
            }
        }
    }

    pub(crate) fn into_image(self) -> Image {
        let mut rgba = Vec::with_capacity(self.alpha.len() * 4);
        for (c, a) in self.color.iter().zip(&self.alpha) {
            let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            rgba.extend_from_slice(&[to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), to_u8(*a)]);
        }
        Image { width: self.width, height: self.height, rgba }
    }
}

/// A rounded rectangle as one closed contour (quarter circles of 16
/// segments at each corner).
pub(crate) fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<(f32, f32)> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let corners = [(x + w - r, y + r, -90.0f32), (x + w - r, y + h - r, 0.0), (x + r, y + h - r, 90.0), (x + r, y + r, 180.0)];
    let mut points = Vec::with_capacity(4 * 17);
    for (cx, cy, start) in corners {
        for i in 0..=16 {
            let angle = (start + 90.0 * i as f32 / 16.0).to_radians();
            points.push((cx + r * angle.cos(), cy + r * angle.sin()));
        }
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_aligned_square_is_fully_covered_and_crisp() {
        let mut canvas = Canvas::new(8, 8);
        canvas.fill(&[vec![(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)]], [255, 0, 0], 1.0);
        let image = canvas.into_image();
        let alpha = |x: u32, y: u32| image.rgba[((y * 8 + x) * 4 + 3) as usize];
        assert_eq!(alpha(3, 3), 255);
        assert_eq!(alpha(1, 3), 0);
        assert_eq!(alpha(6, 3), 0);
    }

    #[test]
    fn a_hole_stays_empty_under_nonzero_winding() {
        // Outer square clockwise, inner square counter-clockwise: the
        // counter of an "o".
        let outer = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let inner = vec![(3.0, 3.0), (3.0, 7.0), (7.0, 7.0), (7.0, 3.0)];
        let mut canvas = Canvas::new(10, 10);
        canvas.fill(&[outer, inner], [0, 0, 0], 1.0);
        let image = canvas.into_image();
        assert_eq!(image.rgba[((5 * 10 + 5) * 4 + 3) as usize], 0);
        assert_eq!(image.rgba[((10 + 1) * 4 + 3) as usize], 255);
    }

    #[test]
    fn bgra_swaps_red_and_blue() {
        let image = Image { width: 1, height: 1, rgba: vec![1, 2, 3, 4] };
        assert_eq!(image.to_bgra(), vec![3, 2, 1, 4]);
    }
}
