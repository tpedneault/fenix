//! Surfaces: panes of pixels it streams -- a remote desktop, rendered
//! pages -- with the keyboard, the pointer and the wheel sent back to it
//! ([`crate::Extension::surface_input`]).
//!
//! ```no_run
//! use fenix_extension_api::{surface, Event, Extension, SurfaceInput, SurfaceOptions};
//!
//! #[derive(Default)]
//! struct Screen { shown: Option<u64> }
//!
//! impl Extension for Screen {
//!     fn run_command(&mut self, _id: &str) -> Result<(), String> {
//!         self.shown = Some(surface::open("screen", "Screen", &SurfaceOptions::new().capture())?);
//!         Ok(())
//!     }
//!
//!     fn handle(&mut self, event: Event) {
//!         if let Event::SurfaceResized(size) = event {
//!             // Draw at the pane's size: everything, then the frame shown.
//!             let mut frame = surface::Frame::new(size.width, size.height);
//!             frame.fill(0, 0, size.width, size.height, (30, 30, 46));
//!             let _ = frame.show(size.surface);
//!         }
//!     }
//!
//!     fn surface_input(&mut self, _surface: u64, input: SurfaceInput) {
//!         if let SurfaceInput::Key(key) = input {
//!             fenix_extension_api::info!("{:?} {}", key.key, if key.down { "down" } else { "up" });
//!         }
//!     }
//! }
//! ```
//!
//! A surface is one of its pages in all else: its tab and title
//! ([`retitle`]), [`crate::page::close`], `page_state` and `page_restore`,
//! [`crate::Event::PageShown`]. Its pixels are BGRA, row after row;
//! what it writes is shown at the next [`present`], all at once.

use crate::{host, CursorImage, KeyEvent, Modifiers, NamedKey, Role, SurfaceKey, SurfaceMark, SurfaceOptions, SurfaceScale};

/// Opens a surface; its pane's size comes as
/// [`crate::Event::SurfaceResized`].
pub fn open(kind: &str, title: &str, options: &SurfaceOptions) -> Result<u64, String> {
    host::surface_open(kind, title, *options)
}

/// Its pixels made `width` x `height`, black.
pub fn resize(surface: u64, width: u32, height: u32) -> Result<(), String> {
    host::surface_resize(surface, width, height)
}

/// A rectangle of its pixels, BGRA, row after row.
pub fn write(surface: u64, x: u32, y: u32, width: u32, height: u32, bgra: &[u8]) -> Result<(), String> {
    host::surface_write(surface, x, y, width, height, bgra)
}

/// A rectangle in one colour, (r, g, b).
pub fn fill(surface: u64, x: u32, y: u32, width: u32, height: u32, rgb: (u8, u8, u8)) -> Result<(), String> {
    host::surface_fill(surface, x, y, width, height, rgb)
}

/// A rectangle moved within it, to (`to_x`, `to_y`): a scroll.
pub fn copy(surface: u64, x: u32, y: u32, width: u32, height: u32, to_x: u32, to_y: u32) -> Result<(), String> {
    host::surface_copy(surface, x, y, width, height, to_x, to_y)
}

/// What it wrote since the last, shown, as one frame.
pub fn present(surface: u64) {
    host::surface_present(surface)
}

/// The pointer's picture over it; `None` draws nothing there.
pub fn cursor(surface: u64, cursor: Option<&CursorImage>) {
    host::surface_cursor(surface, cursor)
}

/// Boxes tinted over it, in its pixels, in place of the last.
pub fn marks(surface: u64, marks: &[SurfaceMark]) {
    host::surface_marks(surface, marks)
}

/// It, or any of its pages, called `title` now.
pub fn retitle(page: u64, title: &str) {
    host::page_retitle(page, title)
}

impl SurfaceOptions {
    /// In the focused pane, fitted, a page's keys.
    pub fn new() -> SurfaceOptions {
        SurfaceOptions { capture: false, scale: SurfaceScale::Fit, beside: None }
    }

    /// Takes every key while focused, pressed and let go, as a remote
    /// desktop does; Ctrl+\ gives them back.
    pub fn capture(mut self) -> SurfaceOptions {
        self.capture = true;
        self
    }

    pub fn scale(mut self, scale: SurfaceScale) -> SurfaceOptions {
        self.scale = scale;
        self
    }

    /// In a new pane beside the focused one, `width` of it.
    pub fn beside(mut self, width: f32) -> SurfaceOptions {
        self.beside = Some(width);
        self
    }
}

impl Default for SurfaceOptions {
    fn default() -> SurfaceOptions {
        SurfaceOptions::new()
    }
}

impl SurfaceMark {
    /// A box at (x, y), `width` x `height`, tinted in `role`'s colour.
    pub fn new(x: i32, y: i32, width: u32, height: u32, role: Role) -> SurfaceMark {
        SurfaceMark { x, y, width, height, role }
    }
}

impl KeyEvent {
    /// The character it types, if it's one.
    pub fn char(&self) -> Option<char> {
        match self.key {
            SurfaceKey::Char(c) => Some(c),
            SurfaceKey::Named(_) => None,
        }
    }

    /// Whether it's `named`.
    pub fn is(&self, named: NamedKey) -> bool {
        self.key == SurfaceKey::Named(named)
    }
}

impl Modifiers {
    /// Whether none is held.
    pub fn none(&self) -> bool {
        !self.ctrl && !self.alt && !self.meta
    }
}

/// A picture drawn in its own memory, then shown on a surface whole --
/// for a frame drawn at once (a page rendered). A stream of changes (a
/// remote desktop) writes its rectangles with [`write`] instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// BGRA, row after row.
    pub bgra: Vec<u8>,
}

impl Frame {
    /// `width` x `height`, black.
    pub fn new(width: u32, height: u32) -> Frame {
        let mut bgra = vec![0; width as usize * height as usize * 4];
        for px in bgra.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Frame { width, height, bgra }
    }

    /// RGBA pixels (a decoded PNG's) as a frame.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Frame {
        let mut bgra = rgba.to_vec();
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        Frame { width, height, bgra }
    }

    /// Pixel (x, y) in (r, g, b); outside it, nothing.
    pub fn set(&mut self, x: u32, y: u32, (r, g, b): (u8, u8, u8)) {
        if x < self.width && y < self.height {
            let at = (y as usize * self.width as usize + x as usize) * 4;
            self.bgra[at..at + 4].copy_from_slice(&[b, g, r, 255]);
        }
    }

    /// A rectangle in one colour, cut to the frame.
    pub fn fill(&mut self, x: u32, y: u32, width: u32, height: u32, rgb: (u8, u8, u8)) {
        for row in y..(y + height).min(self.height) {
            for col in x..(x + width).min(self.width) {
                self.set(col, row, rgb);
            }
        }
    }

    /// Shown on `surface`: sized to it if need be, written whole, presented.
    pub fn show(&self, surface: u64) -> Result<(), String> {
        resize(surface, self.width, self.height)?;
        write(surface, 0, 0, self.width, self.height, &self.bgra)?;
        present(surface);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_bgra() {
        let mut f = Frame::new(2, 2);
        f.set(1, 0, (10, 20, 30));
        assert_eq!(&f.bgra[4..8], &[30, 20, 10, 255]);
        f.fill(0, 1, 9, 9, (1, 2, 3));
        assert_eq!(&f.bgra[12..16], &[3, 2, 1, 255], "cut to the frame");
        assert_eq!(Frame::from_rgba(1, 1, &[1, 2, 3, 4]).bgra, [3, 2, 1, 4]);
        let key = KeyEvent { key: SurfaceKey::Char('a'), down: true, mods: Modifiers { ctrl: false, alt: false, meta: false } };
        assert_eq!(key.char(), Some('a'));
        assert!(key.mods.none() && !key.is(NamedKey::Enter));
        assert!(SurfaceOptions::new().capture().beside(0.4).capture);
    }
}
