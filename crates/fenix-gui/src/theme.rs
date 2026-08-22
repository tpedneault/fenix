/// Hardcoded color palette, borrowed from `orbit-emacs`'s `orbit-dark` theme
/// for visual continuity. A generalized/swappable theme system is Phase 5
/// work; for now this is a single fixed set of colors.
pub struct Theme {
    pub bg: [f32; 4],
    pub bg_modeline: [f32; 4],
    pub fg: glyphon::Color,
    pub fg_modeline: glyphon::Color,
    pub caret: [f32; 4],
}

const fn rgba(hex: u32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}

const fn text_color(hex: u32) -> glyphon::Color {
    let r = ((hex >> 16) & 0xff) as u8;
    let g = ((hex >> 8) & 0xff) as u8;
    let b = (hex & 0xff) as u8;
    glyphon::Color::rgb(r, g, b)
}

pub const ORBIT_DARK: Theme = Theme {
    bg: rgba(0x1a1b26),
    bg_modeline: rgba(0x24283b),
    fg: text_color(0xc0caf5),
    fg_modeline: text_color(0xc0caf5),
    caret: rgba(0xe0af68),
};
