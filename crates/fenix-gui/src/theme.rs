use std::io;
use std::path::{Path, PathBuf};

/// A named, swappable color palette (plus a couple of non-color style
/// knobs -- font family, window border). Every field is consumed
/// generically by rendering code (`syntax_color`, `git_status_color`,
/// `App::mode_colors`, every `theme.SOMETHING` read in `App::redraw`) --
/// adding a second `Theme` value and pointing `App` at it is the entire
/// integration surface, no per-theme special-casing elsewhere.
pub struct Theme {
    /// Display/persistence identity -- also used for `by_name` lookup
    /// and cycling, since two separate `&SOME_CONST` expressions aren't
    /// guaranteed by Rust to share an address (no pointer-identity
    /// bookkeeping here).
    pub name: &'static str,
    /// Body-text font family. `None` falls back to the generic
    /// `Family::Monospace` (today's only behavior); `Some(family)` asks
    /// `cosmic-text`'s system font database for that family by name,
    /// same mechanism already proven for the file explorer's Nerd Font
    /// icons (`icon::ICON_FONT_FAMILY`). Not a guarantee the font is
    /// actually installed -- an unresolvable name falls back to
    /// `fontdb`'s own default substitution.
    pub font_family: Option<&'static str>,
    /// Thin colored frame drawn around the whole window when `Some`.
    /// `None` (today's only behavior) draws nothing.
    pub border: Option<[f32; 4]>,

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
    /// Badge accent for the file explorer, not a real Vim mode -- reuses
    /// the same blue as `icon_folder`/`git_untracked`, just in the rect-
    /// fill `[f32; 4]` representation those need.
    pub mode_explorer: [f32; 4],
    /// Badge accent for the project picker (find-file/grep/switch-project),
    /// not a real Vim mode either -- purple, distinct from the explorer's
    /// blue so the two full-buffer-takeover views read differently at a
    /// glance.
    pub mode_picker: [f32; 4],
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

    /// File explorer: icon tint. Two-tone for v1 -- folders get their own
    /// accent since they're usually the most useful thing to spot at a
    /// glance in a listing; every file icon (regardless of which glyph)
    /// shares one color. Per-language icon tinting (Rust's real brand
    /// orange, Python's blue, etc.) is a natural next increment, not done
    /// here -- the icon *shape* already carries the file-type signal.
    pub icon_folder: glyphon::Color,
    pub icon_file: glyphon::Color,
    /// Git status badges in the explorer listing.
    pub git_modified: glyphon::Color,
    pub git_staged: glyphon::Color,
    pub git_untracked: glyphon::Color,
    pub git_ignored: glyphon::Color,
    pub git_conflicted: glyphon::Color,
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
            // "repeat"/"conditional" are Tcl's own bare (non-dotted) names
            // for loop/if-family keywords -- its highlights.scm pairs them
            // with a plain "keyword" capture on the same node too, but
            // it's not guaranteed which of the two wins the overlap
            // resolution, so both need a real mapping. Same for "spell",
            // which it pairs with "comment".
            "keyword" | "repeat" | "conditional" => self.syntax_keyword,
            "string" => self.syntax_string,
            "escape" => self.syntax_string,
            "comment" | "spell" => self.syntax_comment,
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

    pub fn git_status_color(&self, status: fenix_explorer::GitStatus) -> glyphon::Color {
        match status {
            fenix_explorer::GitStatus::Modified => self.git_modified,
            fenix_explorer::GitStatus::Staged => self.git_staged,
            fenix_explorer::GitStatus::Untracked => self.git_untracked,
            fenix_explorer::GitStatus::Ignored => self.git_ignored,
            fenix_explorer::GitStatus::Conflicted => self.git_conflicted,
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
    name: "Orbit Dark",
    font_family: None,
    border: None,

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
    mode_explorer: rgba(0x7aa2f7),
    mode_picker: rgba(0xbb9af7),
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

    icon_folder: text_color(0x7aa2f7),
    icon_file: text_color(0xc0caf5),
    git_modified: text_color(0xff9e64),
    git_staged: text_color(0x9ece6a),
    git_untracked: text_color(0x7aa2f7),
    git_ignored: text_color(0x565f89),
    git_conflicted: text_color(0xf7768e),
};

/// A recreation of TempleOS's look, built entirely from the standard
/// 16-color CGA/EGA palette (the same fixed, well-established set
/// TempleOS itself is restricted to). TempleOS's actual desktop/DolDoc
/// look (confirmed against real screenshots, not the classic-Borland-
/// IDE "blue screen" look this theme originally, incorrectly, assumed)
/// is a **white** page background with black body text, a solid blue
/// window-chrome/status-bar color, and red/purple/green accent text for
/// headings, links, and directives -- much closer to a print/document
/// editor than a dark-background terminal IDE.
///
/// `font_family` names the real 8x8 TempleOS bitmap font, embedded into
/// the binary (`text::TEMPLEOS_FONT_BYTES`, a community TTF conversion
/// of the original) rather than depending on it being installed --
/// works on any machine.
///
/// One remaining disclosed simplification: TempleOS's own HolyC IDE
/// colors identifiers quasi-randomly per token, which isn't replicated
/// here (`syntax_color` resolves a fixed color per capture name
/// everywhere in Fenix, same as every other theme).
pub const TEMPLEOS: Theme = Theme {
    name: "TempleOS",
    font_family: Some("TempleOS"),
    border: Some(rgba(0x0000aa)),

    bg: rgba(0xffffff),
    bg_modeline: rgba(0x0000aa),
    fg: text_color(0x000000),
    fg_modeline: text_color(0xffffff),
    caret: rgba(0xffff55),
    hl_line: rgba_alpha(0x5555ff, 0.15),
    selection: rgba_alpha(0xffff55, 0.45),

    mode_normal: rgba(0x55ff55),
    mode_insert: rgba(0x55ffff),
    mode_visual: rgba(0xff5555),
    mode_replace: rgba(0xaa5500),
    mode_command: rgba(0x5555ff),
    mode_explorer: rgba(0x00aaaa),
    mode_picker: rgba(0xff55ff),
    mode_text_dark: text_color(0x000000),
    mode_text_light: text_color(0xffffff),

    gutter_fg: text_color(0x555555),

    // Dark/saturated (not the pastel "Light*") variants throughout --
    // body text sits on the white page background now, where the pastel
    // half of the 16-color set reads as barely-there instead of a
    // bright accent.
    syntax_keyword: text_color(0x0000aa),
    syntax_string: text_color(0x00aa00),
    syntax_comment: text_color(0x555555),
    syntax_function: text_color(0xaa00aa),
    syntax_type: text_color(0x00aaaa),
    syntax_number: text_color(0xaa0000),
    syntax_constant: text_color(0xaa5500),
    syntax_variable: text_color(0x000000),
    syntax_operator: text_color(0x000000),
    syntax_punctuation: text_color(0x555555),
    syntax_attribute: text_color(0x00aaaa),

    icon_folder: text_color(0x0000aa),
    icon_file: text_color(0x000000),
    git_modified: text_color(0xaa5500),
    git_staged: text_color(0x00aa00),
    git_untracked: text_color(0x00aaaa),
    git_ignored: text_color(0xaaaaaa),
    git_conflicted: text_color(0xaa0000),
};

/// Every theme Fenix ships, in cycling order. `App::cycle_theme` and
/// `by_name` both work off this.
pub const ALL: &[&Theme] = &[&ORBIT_DARK, &TEMPLEOS];

/// Case-insensitive lookup by `Theme::name` -- used for persistence
/// (the saved file just holds a name) and is the reason `name` exists
/// as a field at all rather than relying on pointer identity, which
/// Rust doesn't guarantee is stable across separate `&SOME_CONST`
/// expressions.
pub fn by_name(name: &str) -> Option<&'static Theme> {
    ALL.iter().find(|t| t.name.eq_ignore_ascii_case(name)).copied()
}

/// `dirs::config_dir()/fenix/theme.txt` -- same location convention
/// `fenix_project::KnownProjects::default_path` already established,
/// just a different file. `None` only on a platform with no notion of
/// a config directory at all.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("fenix").join("theme.txt"))
}

/// Loads the persisted theme choice from `path`, falling back to
/// `ORBIT_DARK` on any failure -- missing file, unreadable file, or a
/// name that doesn't match any known theme. This is a convenience
/// preference, not critical data, so it never fails outright -- same
/// posture as `fenix_project::KnownProjects::load_or_default`.
pub fn load_from(path: &Path) -> &'static Theme {
    match std::fs::read_to_string(path) {
        Ok(contents) => by_name(contents.trim()).unwrap_or(&ORBIT_DARK),
        Err(_) => &ORBIT_DARK,
    }
}

/// Persists `theme`'s name to `path`, creating parent directories as
/// needed -- mirrors `KnownProjects::save`'s shape exactly.
pub fn save_to(theme: &Theme, path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, theme.name)
}

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

    #[test]
    fn tcls_bare_control_flow_and_spell_captures_resolve() {
        assert_eq!(ORBIT_DARK.syntax_color("repeat"), ORBIT_DARK.syntax_keyword);
        assert_eq!(ORBIT_DARK.syntax_color("conditional"), ORBIT_DARK.syntax_keyword);
        assert_eq!(ORBIT_DARK.syntax_color("spell"), ORBIT_DARK.syntax_comment);
    }

    #[test]
    fn git_status_color_covers_every_status() {
        use fenix_explorer::GitStatus;
        assert_eq!(ORBIT_DARK.git_status_color(GitStatus::Modified), ORBIT_DARK.git_modified);
        assert_eq!(ORBIT_DARK.git_status_color(GitStatus::Staged), ORBIT_DARK.git_staged);
        assert_eq!(ORBIT_DARK.git_status_color(GitStatus::Untracked), ORBIT_DARK.git_untracked);
        assert_eq!(ORBIT_DARK.git_status_color(GitStatus::Ignored), ORBIT_DARK.git_ignored);
        assert_eq!(ORBIT_DARK.git_status_color(GitStatus::Conflicted), ORBIT_DARK.git_conflicted);
    }

    #[test]
    fn templeos_syntax_and_git_colors_resolve_the_same_generic_way() {
        // TEMPLEOS is a plain data value like ORBIT_DARK -- no special
        // casing anywhere in `syntax_color`/`git_status_color`, so this
        // is really a check that the const itself is well-formed.
        assert_eq!(TEMPLEOS.syntax_color("keyword"), TEMPLEOS.syntax_keyword);
        assert_eq!(TEMPLEOS.syntax_color("function.method"), TEMPLEOS.syntax_function);
        assert_eq!(TEMPLEOS.syntax_color("some.unknown.capture"), TEMPLEOS.fg);
        use fenix_explorer::GitStatus;
        assert_eq!(TEMPLEOS.git_status_color(GitStatus::Staged), TEMPLEOS.git_staged);
    }

    #[test]
    fn all_contains_exactly_orbit_dark_and_templeos_by_name() {
        let names: Vec<&str> = ALL.iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["Orbit Dark", "TempleOS"]);
    }

    #[test]
    fn by_name_is_case_insensitive_and_none_for_unknown() {
        assert_eq!(by_name("templeos").map(|t| t.name), Some("TempleOS"));
        assert_eq!(by_name("TEMPLEOS").map(|t| t.name), Some("TempleOS"));
        assert_eq!(by_name("orbit dark").map(|t| t.name), Some("Orbit Dark"));
        assert!(by_name("nonexistent-theme").is_none());
    }

    /// A real, uniquely-named temp directory, removed on drop -- same
    /// reasoning as every other crate's own `TempDir`: persistence here
    /// is real filesystem I/O, tested against a real filesystem.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-gui-theme-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_from_a_missing_file_falls_back_to_orbit_dark() {
        let dir = TempDir::new("load_missing");
        let theme = load_from(&dir.path().join("does-not-exist.txt"));
        assert_eq!(theme.name, "Orbit Dark");
    }

    #[test]
    fn load_from_an_unrecognized_name_falls_back_to_orbit_dark() {
        let dir = TempDir::new("load_unrecognized");
        let path = dir.path().join("theme.txt");
        std::fs::write(&path, "not-a-real-theme").unwrap();
        let theme = load_from(&path);
        assert_eq!(theme.name, "Orbit Dark");
    }

    #[test]
    fn save_to_then_load_from_round_trips() {
        let dir = TempDir::new("save_round_trip");
        let path = dir.path().join("theme.txt");
        save_to(&TEMPLEOS, &path).unwrap();
        assert_eq!(load_from(&path).name, "TempleOS");
    }

    #[test]
    fn save_to_creates_missing_parent_directories() {
        let dir = TempDir::new("save_creates_parents");
        let path = dir.path().join("nested").join("config").join("theme.txt");
        save_to(&ORBIT_DARK, &path).unwrap();
        assert!(path.exists());
    }
}
