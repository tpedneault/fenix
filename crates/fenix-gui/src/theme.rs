/// Hardcoded color palette, borrowed from `orbit-emacs`'s `orbit-dark` theme
/// for visual continuity. A generalized/swappable theme system is Phase 5
/// work; for now this is a single fixed set of colors.
pub struct Theme {
    pub bg: [f32; 4],
    pub bg_modeline: [f32; 4],
    pub fg: glyphon::Color,
    pub fg_modeline: glyphon::Color,
    pub caret: [f32; 4],
    /// Current-line highlight -- orbit-dark's own "bg-hl", the shade its
    /// palette table names for exactly this purpose.
    pub hl_line: [f32; 4],
    /// Visual-mode selection: the caret's amber hue at low alpha, drawn
    /// over text. A solid dark shade close to `bg` (the original choice
    /// here, before `hl_line` existed to actually use that color) wasn't
    /// visibly distinguishable from the background.
    pub selection: [f32; 4],

    /// Per-mode accent, matching orbit-emacs's own evil-state modeline
    /// colors (see its docs/design.org "Evil state colors" table) so the
    /// modeline badge and caret tint reuse an already-established scheme
    /// rather than inventing a new one.
    pub mode_normal: [f32; 4],
    pub mode_insert: [f32; 4],
    pub mode_visual: [f32; 4],
    pub mode_replace: [f32; 4],
    pub mode_command: [f32; 4],
    /// Badge text color for the light-background modes (amber/cyan/orange/blue).
    pub mode_text_dark: glyphon::Color,
    /// Badge text color for the one mode whose accent is too dark for that (red).
    pub mode_text_light: glyphon::Color,
}

const fn rgba(hex: u32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    [r, g, b, 1.0]
}

const fn rgba_alpha(hex: u32, alpha: f32) -> [f32; 4] {
    let r = ((hex >> 16) & 0xff) as f32 / 255.0;
    let g = ((hex >> 8) & 0xff) as f32 / 255.0;
    let b = (hex & 0xff) as f32 / 255.0;
    [r, g, b, alpha]
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
    hl_line: rgba(0x292e42),
    selection: rgba_alpha(0xe0af68, 0.25),

    mode_normal: rgba(0xe0af68),
    mode_insert: rgba(0x7dcfff),
    mode_visual: rgba(0xf7768e),
    mode_replace: rgba(0xff9e64),
    mode_command: rgba(0x7aa2f7),
    mode_text_dark: text_color(0x1a1b26),
    mode_text_light: text_color(0xffffff),
};
