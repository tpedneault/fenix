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

    /// Line-number gutter: muted so it recedes behind actual content, same
    /// role as most editors' "line number" / vim's `LineNr` highlight group.
    /// The current line's number uses `fg` instead (see `App::mode_colors`'
    /// sibling gutter logic), so it stands out without needing its own color.
    pub gutter_fg: glyphon::Color,

    /// Syntax-highlight token colors. A representative top-level set --
    /// tree-sitter capture names are often more specific than this (e.g.
    /// `function.method`, `constant.builtin`), so `syntax_color` resolves
    /// down to whichever of these a capture's first `.`-segment names,
    /// rather than needing an exact entry per possible capture name.
    pub syntax_keyword: glyphon::Color,
    pub syntax_string: glyphon::Color,
    pub syntax_comment: glyphon::Color,
    pub syntax_function: glyphon::Color,
    pub syntax_type: glyphon::Color,
    pub syntax_number: glyphon::Color,
    pub syntax_constant: glyphon::Color,
    pub syntax_variable: glyphon::Color,
    pub syntax_operator: glyphon::Color,
    pub syntax_punctuation: glyphon::Color,
    pub syntax_attribute: glyphon::Color,
}

impl Theme {
    /// Resolves a tree-sitter capture name (e.g. `"function.method"`,
    /// `"constant.builtin"`) to a color, falling back to whichever
    /// `syntax_*` field its first `.`-segment names -- the same
    /// hierarchical-fallback convention every real `highlights.scm`
    /// relies on (a theme need not enumerate every sub-capture a grammar
    /// might produce). Anything unrecognized falls back to plain `fg`,
    /// so an unfamiliar capture name reads as ordinary text instead of
    /// vanishing or breaking.
    pub fn syntax_color(&self, capture_name: &str) -> glyphon::Color {
        let top = capture_name.split('.').next().unwrap_or(capture_name);
        match top {
            "keyword" => self.syntax_keyword,
            "string" => self.syntax_string,
            "escape" => self.syntax_string,
            "comment" => self.syntax_comment,
            "function" => self.syntax_function,
            "type" => self.syntax_type,
            "number" => self.syntax_number,
            "constant" | "boolean" => self.syntax_constant,
            "variable" | "property" | "label" => self.syntax_variable,
            "operator" => self.syntax_operator,
            "punctuation" => self.syntax_punctuation,
            "attribute" | "constructor" => self.syntax_attribute,
            _ => self.fg,
        }
    }
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

    gutter_fg: text_color(0x565f89),

    syntax_keyword: text_color(0xbb9af7),
    syntax_string: text_color(0x9ece6a),
    syntax_comment: text_color(0x565f89),
    syntax_function: text_color(0x7aa2f7),
    syntax_type: text_color(0x2ac3de),
    syntax_number: text_color(0xff9e64),
    syntax_constant: text_color(0xff9e64),
    syntax_variable: text_color(0xc0caf5),
    syntax_operator: text_color(0x89ddff),
    syntax_punctuation: text_color(0xc0caf5),
    syntax_attribute: text_color(0xe0af68),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_top_level_names_resolve_directly() {
        assert_eq!(ORBIT_DARK.syntax_color("keyword"), ORBIT_DARK.syntax_keyword);
        assert_eq!(ORBIT_DARK.syntax_color("string"), ORBIT_DARK.syntax_string);
    }

    #[test]
    fn dotted_sub_captures_fall_back_to_their_top_level_color() {
        assert_eq!(ORBIT_DARK.syntax_color("function.method"), ORBIT_DARK.syntax_function);
        assert_eq!(ORBIT_DARK.syntax_color("constant.builtin"), ORBIT_DARK.syntax_constant);
        assert_eq!(ORBIT_DARK.syntax_color("punctuation.bracket"), ORBIT_DARK.syntax_punctuation);
    }

    #[test]
    fn unrecognized_capture_falls_back_to_plain_fg() {
        assert_eq!(ORBIT_DARK.syntax_color("some.unknown.capture"), ORBIT_DARK.fg);
    }
}
