/// A named, swappable color palette (plus a couple of non-color style
/// knobs -- font family, window border). Every field is consumed
/// generically by rendering code (`syntax_color`, `git_status_color`,
/// `App::mode_colors`, every `theme.SOMETHING` read in `App::redraw`) --
/// adding a second `Theme` value and pointing `App` at it is the entire
/// integration surface, no per-theme special-casing elsewhere.
/// How a pane's tab strip is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStyle {
    /// Filled tabs on the chrome shade, the active one in the editor's
    /// background with an accent along its top -- Visual Studio's.
    Block,
    /// Flat on the editor background; the active tab is bright text over
    /// an underline in the mode colour.
    Underline,
}

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
    /// The line drawn along a window split's boundary, between two
    /// adjacent panes.
    pub divider: [f32; 4],
    /// How this theme draws each pane's tab strip when the
    /// `appearance.tabs` setting leaves it to the theme: Visual Studio's
    /// filled blocks, or the flat underline every other theme uses.
    pub tabs: TabStyle,

    pub bg: [f32; 4],
    /// The persistent file-explorer sidebar's own background -- real VS
    /// paints its Solution Explorer/tool windows a shade lighter than the
    /// editor canvas (`#252526` vs `#1E1E1E`), not the identical color.
    /// Every existing theme sets this equal to `bg`, matching their
    /// current on-screen appearance exactly (the sidebar's own icon/git
    /// colors were tuned assuming that equality, per the "unreadable
    /// blue-background/black-text" fix `App::redraw`'s sidebar-background
    /// push documents) -- only a theme that actually wants the two
    /// surfaces to read as visually distinct sets this to something else.
    pub sidebar_bg: [f32; 4],
    pub bg_modeline: [f32; 4],
    pub fg: glyphon::Color,
    pub fg_modeline: glyphon::Color,
    pub caret: [f32; 4],
    /// Same hue as `caret`, in the glyph-text `Color` representation
    /// (`caret` is `[f32; 4]`, rect-fill shaped) -- used wherever this
    /// accent needs to color actual *text*, not just the caret rect
    /// itself (e.g. the which-key popup's key column, which needs a
    /// second accent color that's guaranteed high-contrast against
    /// `bg_modeline`, the popup's own background).
    pub caret_text: glyphon::Color,
    /// Current-line highlight -- orbit-dark's own "bg-hl", the shade its
    /// palette table names for exactly this purpose.
    pub hl_line: [f32; 4],
    /// Visual-mode selection, and the "this row is the one selected"
    /// highlight for the sidebar/explorer/picker lists. Needs real
    /// contrast against `bg` on its own -- unlike `hl_line`, nothing else
    /// (no caret) backs it up in the sidebar, so a hue too close to `bg`
    /// reads as barely-there regardless of alpha (a light hue like yellow
    /// blended over a light `bg` moves least in the channels that already
    /// matched, which is what made TempleOS's original choice here hard to
    /// see -- picking a hue further from `bg` fixes that, not just a
    /// bigger alpha).
    pub selection: [f32; 4],
    /// Backdrop drawn behind a bracket and its match when the cursor
    /// sits on one of `(){}[]` -- a distinct color from `selection`/
    /// `hl_line` so it reads as its own thing even when it happens to
    /// land on the current line.
    pub bracket_match: [f32; 4],
    /// `hlsearch`'s persistent match-highlight backdrop, drawn behind
    /// every occurrence of the last confirmed search pattern -- a plain,
    /// unremarkable overlay distinct from `selection`/`bracket_match` so
    /// it doesn't compete with a real Visual selection or the bracket
    /// highlight when they happen to coincide.
    pub search_match: [f32; 4],

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
    /// Badge foreground for Normal, Insert, Replace, Command, Explorer and Picker.
    pub mode_text_dark: glyphon::Color,
    /// Visual-mode badge foreground. The legacy name does not require a light
    /// color: dark themes use dark lettering against their bright accents.
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
        // Markdown's inline/heading captures share one top-level "text"
        // segment (`text.title`/`text.strong`/`text.emphasis`/`text.
        // literal`/`text.uri`/`text.reference`) but need to read as
        // visibly different things -- checked by exact name before the
        // generic top-segment fallback below, which stays untouched for
        // every capture that doesn't need this. No new `syntax_*`
        // fields for them: reusing the closest existing slot's own
        // per-theme color keeps every theme's Markdown rendering
        // consistent with its own palette for free, matching this
        // function's whole existing "map onto a representative slot,
        // not an entry per exact capture" design.
        match capture_name {
            "text.title" => return self.syntax_function, // headings: the same prominent accent a function name gets
            "text.strong" => return self.syntax_keyword,  // **bold**: keywords are usually the strongest, most saturated accent
            "text.emphasis" => return self.syntax_type,   // *italic*: a secondary accent, less emphatic than strong
            "text.literal" => return self.syntax_string,  // `code spans`: read as a literal value, same as a real string
            "text.uri" | "text.reference" => return self.syntax_constant, // links
            // XML: CDATA contents read as a literal, its `<![CDATA[`/`]]>`
            // brackets as punctuation, a link (a DOCTYPE's system ID) as
            // any other link; element text itself stays body text.
            "markup.raw" => return self.syntax_string,
            "markup.heading" => return self.syntax_punctuation,
            "markup.link" => return self.syntax_constant,
            _ => {}
        }
        if let Some(kind) = fenix_syntax::TodoKind::from_capture_name(capture_name) {
            return self.todo_color(kind);
        }
        let top = capture_name.split('.').next().unwrap_or(capture_name);
        match top {
            // "repeat"/"conditional" are Tcl's own bare (non-dotted) names
            // for loop/if-family keywords -- its highlights.scm pairs them
            // with a plain "keyword" capture on the same node too, but
            // it's not guaranteed which of the two wins the overlap
            // resolution, so both need a real mapping. Same for "spell",
            // which it pairs with "comment".
            "keyword" | "repeat" | "conditional" | "storageclass" | "include" => self.syntax_keyword,
            // Preprocessor lines (C#'s `#region`, `#if`) read as
            // annotations, like an attribute.
            "preproc" => self.syntax_attribute,
            "string" => self.syntax_string,
            "escape" => self.syntax_string,
            "comment" | "spell" => self.syntax_comment,
            "function" | "method" => self.syntax_function,
            // A namespace or module (C#'s `using`, Lua's `require`d
            // names) reads as a type does.
            "type" | "namespace" | "module" => self.syntax_type,
            "number" | "float" => self.syntax_number,
            "constant" | "boolean" => self.syntax_constant,
            // `field`/`parameter`: Go's, Java's and Lua's names for
            // struct fields and function parameters.
            "variable" | "property" | "label" | "field" | "parameter" => self.syntax_variable,
            "operator" => self.syntax_operator,
            "punctuation" => self.syntax_punctuation,
            "attribute" | "constructor" => self.syntax_attribute,
            // XML element names read as the language's keywords do in
            // every other grammar -- the structural words -- and a
            // processing instruction as an annotation, like an attribute.
            "tag" => self.syntax_keyword,
            "embedded" => self.syntax_attribute,
            _ => self.fg,
        }
    }

    /// The color a TODO-style comment keyword is drawn in (and, faded, the
    /// tint behind it) -- mapped onto colors every theme already defines
    /// with a fixed meaning, so each kind reads the same way across
    /// themes: broken is the conflict red, a hack/warning the modified
    /// amber, a note the staged green.
    pub fn todo_color(&self, kind: fenix_syntax::TodoKind) -> glyphon::Color {
        use fenix_syntax::TodoKind;
        match kind {
            TodoKind::Fix => self.git_conflicted,
            TodoKind::Todo => self.syntax_function,
            TodoKind::Hack | TodoKind::Warn => self.git_modified,
            TodoKind::Perf => self.syntax_type,
            TodoKind::Note => self.git_staged,
            TodoKind::Test => self.syntax_constant,
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

/// The themes Fenix ships, as data -- the same format your own are
/// written in (`themes/` in the settings folder), in cycling order.
const BUILT_IN: &[(&str, &str)] = &[
    ("orbit-dark.toml", include_str!("../themes/orbit-dark.toml")),
    ("templeos.toml", include_str!("../themes/templeos.toml")),
    ("gruvbox-dark.toml", include_str!("../themes/gruvbox-dark.toml")),
    ("nord.toml", include_str!("../themes/nord.toml")),
    ("dracula.toml", include_str!("../themes/dracula.toml")),
    ("solarized-dark.toml", include_str!("../themes/solarized-dark.toml")),
    ("one-dark.toml", include_str!("../themes/one-dark.toml")),
    ("visual-studio-dark.toml", include_str!("../themes/visual-studio-dark.toml")),
    ("catppuccin-latte.toml", include_str!("../themes/catppuccin-latte.toml")),
    ("catppuccin-frappe.toml", include_str!("../themes/catppuccin-frappe.toml")),
    ("catppuccin-macchiato.toml", include_str!("../themes/catppuccin-macchiato.toml")),
    ("catppuccin-mocha.toml", include_str!("../themes/catppuccin-mocha.toml")),
    ("monokai.toml", include_str!("../themes/monokai.toml")),
    ("github-dark.toml", include_str!("../themes/github-dark.toml")),
    ("tokyo-night-storm.toml", include_str!("../themes/tokyo-night-storm.toml")),
    ("kanagawa.toml", include_str!("../themes/kanagawa.toml")),
    ("rose-pine.toml", include_str!("../themes/rose-pine.toml")),
    ("everforest-dark.toml", include_str!("../themes/everforest-dark.toml")),
    ("ayu-mirage.toml", include_str!("../themes/ayu-mirage.toml")),
    ("night-owl.toml", include_str!("../themes/night-owl.toml")),
    ("material-palenight.toml", include_str!("../themes/material-palenight.toml")),
    ("zenburn.toml", include_str!("../themes/zenburn.toml")),
    ("github-light.toml", include_str!("../themes/github-light.toml")),
    ("one-light.toml", include_str!("../themes/one-light.toml")),
    ("solarized-light.toml", include_str!("../themes/solarized-light.toml")),
    ("gruvbox-light.toml", include_str!("../themes/gruvbox-light.toml")),
    ("rose-pine-dawn.toml", include_str!("../themes/rose-pine-dawn.toml")),
    ("tokyo-night-day.toml", include_str!("../themes/tokyo-night-day.toml")),
    ("visual-studio-light.toml", include_str!("../themes/visual-studio-light.toml")),
];

/// The theme used when none is set, or the one set isn't there.
pub const DEFAULT: &str = "Orbit Dark";

/// A color as a theme file writes it: `"#rrggbb"`, `"#rrggbbaa"`, or
/// `{ color = "#rrggbb", alpha = 0.12 }` for an overlay.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ColorSpec {
    Hex(String),
    Alpha { color: String, alpha: f32 },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    #[serde(default)]
    tabs: Option<String>,
    #[serde(default)]
    font_family: Option<String>,
    #[serde(default)]
    border: Option<ColorSpec>,
    surfaces: Surfaces,
    text: Text,
    modes: Modes,
    syntax: Syntax,
    explorer: Explorer,
    git: Git,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Surfaces {
    bg: ColorSpec,
    /// The explorer sidebar's; the editor's `bg` when left out.
    #[serde(default)]
    sidebar_bg: Option<ColorSpec>,
    bg_modeline: ColorSpec,
    divider: ColorSpec,
    caret: ColorSpec,
    hl_line: ColorSpec,
    selection: ColorSpec,
    bracket_match: ColorSpec,
    search_match: ColorSpec,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    fg: ColorSpec,
    /// The modeline's text; `fg` when left out.
    #[serde(default)]
    fg_modeline: Option<ColorSpec>,
    /// The caret's color as text (the which-key keys); the caret's own
    /// when left out.
    #[serde(default)]
    caret_text: Option<ColorSpec>,
    gutter_fg: ColorSpec,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Modes {
    normal: ColorSpec,
    insert: ColorSpec,
    visual: ColorSpec,
    replace: ColorSpec,
    command: ColorSpec,
    explorer: ColorSpec,
    picker: ColorSpec,
    text_dark: ColorSpec,
    /// The Visual badge's lettering; `text_dark` when left out.
    #[serde(default)]
    text_light: Option<ColorSpec>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Syntax {
    keyword: ColorSpec,
    string: ColorSpec,
    comment: ColorSpec,
    function: ColorSpec,
    #[serde(rename = "type")]
    ty: ColorSpec,
    number: ColorSpec,
    constant: ColorSpec,
    variable: ColorSpec,
    operator: ColorSpec,
    punctuation: ColorSpec,
    attribute: ColorSpec,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Explorer {
    folder: ColorSpec,
    file: ColorSpec,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Git {
    modified: ColorSpec,
    staged: ColorSpec,
    untracked: ColorSpec,
    ignored: ColorSpec,
    conflicted: ColorSpec,
}

impl ColorSpec {
    /// As a fill: red, green, blue and alpha, each 0..=1.
    fn rgba(&self, key: &str) -> Result<[f32; 4], String> {
        let (hex, alpha) = match self {
            ColorSpec::Hex(hex) => (hex.as_str(), None),
            ColorSpec::Alpha { color, alpha } => (color.as_str(), Some(*alpha)),
        };
        let digits = hex.strip_prefix('#').filter(|d| (d.len() == 6 || d.len() == 8) && d.chars().all(|c| c.is_ascii_hexdigit()));
        let Some(digits) = digits else { return Err(format!("{key}: {hex} isn't a color -- write #rrggbb")) };
        let byte = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).expect("checked hex") as f32 / 255.0;
        let alpha = match (alpha, digits.len()) {
            (Some(a), 6) if (0.0..=1.0).contains(&a) => a,
            (Some(_), 6) => return Err(format!("{key}: alpha is from 0 to 1")),
            (Some(_), _) => return Err(format!("{key}: an alpha of its own, or #rrggbbaa -- not both")),
            (None, 8) => byte(6),
            (None, _) => 1.0,
        };
        Ok([byte(0), byte(2), byte(4), alpha])
    }

    /// As text: opaque.
    fn text(&self, key: &str) -> Result<glyphon::Color, String> {
        let rgba = self.rgba(key)?;
        if rgba[3] < 1.0 {
            return Err(format!("{key}: text can't be see-through -- write #rrggbb"));
        }
        Ok(opaque(rgba))
    }
}

/// A fill's color as text.
fn opaque([r, g, b, _]: [f32; 4]) -> glyphon::Color {
    let byte = |v: f32| (v * 255.0).round() as u8;
    glyphon::Color::rgb(byte(r), byte(g), byte(b))
}

/// Kept for the life of the editor, like the themes that were consts.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// A theme file read: the theme, or what's wrong with it.
pub fn parse(text: &str) -> Result<Theme, String> {
    let f: ThemeFile = toml::from_str(text).map_err(|e| e.message().to_string())?;
    if f.name.trim().is_empty() {
        return Err("name: a theme needs one".into());
    }
    let tabs = match f.tabs.as_deref().unwrap_or("underline") {
        "underline" => TabStyle::Underline,
        "block" => TabStyle::Block,
        other => return Err(format!("tabs: {other} -- underline or block")),
    };
    let s = &f.surfaces;
    let bg = s.bg.rgba("surfaces.bg")?;
    let caret = s.caret.rgba("surfaces.caret")?;
    let fg = f.text.fg.text("text.fg")?;
    let m = &f.modes;
    let text_dark = m.text_dark.text("modes.text_dark")?;
    Ok(Theme {
        name: leak(f.name.trim().to_string()),
        font_family: f.font_family.filter(|s| !s.trim().is_empty()).map(leak),
        border: f.border.as_ref().map(|c| c.rgba("border")).transpose()?,
        divider: s.divider.rgba("surfaces.divider")?,
        tabs,
        bg,
        sidebar_bg: s.sidebar_bg.as_ref().map(|c| c.rgba("surfaces.sidebar_bg")).transpose()?.unwrap_or(bg),
        bg_modeline: s.bg_modeline.rgba("surfaces.bg_modeline")?,
        fg,
        fg_modeline: f.text.fg_modeline.as_ref().map(|c| c.text("text.fg_modeline")).transpose()?.unwrap_or(fg),
        caret,
        caret_text: f.text.caret_text.as_ref().map(|c| c.text("text.caret_text")).transpose()?.unwrap_or(opaque(caret)),
        hl_line: s.hl_line.rgba("surfaces.hl_line")?,
        selection: s.selection.rgba("surfaces.selection")?,
        bracket_match: s.bracket_match.rgba("surfaces.bracket_match")?,
        search_match: s.search_match.rgba("surfaces.search_match")?,
        mode_normal: m.normal.rgba("modes.normal")?,
        mode_insert: m.insert.rgba("modes.insert")?,
        mode_visual: m.visual.rgba("modes.visual")?,
        mode_replace: m.replace.rgba("modes.replace")?,
        mode_command: m.command.rgba("modes.command")?,
        mode_explorer: m.explorer.rgba("modes.explorer")?,
        mode_picker: m.picker.rgba("modes.picker")?,
        mode_text_dark: text_dark,
        mode_text_light: m.text_light.as_ref().map(|c| c.text("modes.text_light")).transpose()?.unwrap_or(text_dark),
        gutter_fg: f.text.gutter_fg.text("text.gutter_fg")?,
        syntax_keyword: f.syntax.keyword.text("syntax.keyword")?,
        syntax_string: f.syntax.string.text("syntax.string")?,
        syntax_comment: f.syntax.comment.text("syntax.comment")?,
        syntax_function: f.syntax.function.text("syntax.function")?,
        syntax_type: f.syntax.ty.text("syntax.type")?,
        syntax_number: f.syntax.number.text("syntax.number")?,
        syntax_constant: f.syntax.constant.text("syntax.constant")?,
        syntax_variable: f.syntax.variable.text("syntax.variable")?,
        syntax_operator: f.syntax.operator.text("syntax.operator")?,
        syntax_punctuation: f.syntax.punctuation.text("syntax.punctuation")?,
        syntax_attribute: f.syntax.attribute.text("syntax.attribute")?,
        icon_folder: f.explorer.folder.text("explorer.folder")?,
        icon_file: f.explorer.file.text("explorer.file")?,
        git_modified: f.git.modified.text("git.modified")?,
        git_staged: f.git.staged.text("git.staged")?,
        git_untracked: f.git.untracked.text("git.untracked")?,
        git_ignored: f.git.ignored.text("git.ignored")?,
        git_conflicted: f.git.conflicted.text("git.conflicted")?,
    })
}

/// The themes there are, and what was wrong reading any of yours.
struct Library {
    themes: Vec<&'static Theme>,
    problems: Vec<String>,
}

fn library() -> &'static Library {
    static LIBRARY: std::sync::OnceLock<Library> = std::sync::OnceLock::new();
    LIBRARY.get_or_init(|| {
        let mut lib = Library { themes: Vec::new(), problems: Vec::new() };
        for (file, text) in BUILT_IN {
            match parse(text) {
                Ok(theme) => lib.themes.push(Box::leak(Box::new(theme))),
                // A shipped theme that doesn't read is a bug the tests
                // catch; a release just goes without it.
                Err(e) => lib.problems.push(format!("built-in {file}: {e}")),
            }
        }
        // The installed extensions' (`installed::data_dirs`), then yours,
        // from `themes/` in the settings folder: one with the same name as
        // a theme before it takes its place. Tests use the shipped ones
        // only.
        let mut dirs: Vec<(String, std::path::PathBuf)> = crate::installed::data_dirs("themes").into_iter().map(|d| (format!("{}/", d.display()), d)).collect();
        if !cfg!(test) {
            dirs.extend(fenix_storage::paths::themes_dir().map(|d| ("themes/".to_string(), d)));
        }
        for (shown, dir) in dirs {
            for (file, result) in read_dir_themes(&dir) {
                match result {
                    Ok(theme) => match lib.themes.iter().position(|t| t.name.eq_ignore_ascii_case(theme.name)) {
                        Some(i) => lib.themes[i] = Box::leak(Box::new(theme)),
                        None => lib.themes.push(Box::leak(Box::new(theme))),
                    },
                    Err(e) => lib.problems.push(format!("{shown}{file}: {e}")),
                }
            }
        }
        lib
    })
}

/// Every `.toml` in `dir`, by file name, read.
fn read_dir_themes(dir: &std::path::Path) -> Vec<(String, Result<Theme, String>)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<std::path::PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml"))).collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let result = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| parse(&text));
            (file, result)
        })
        .collect()
}

/// Every theme there is: the shipped ones, then yours. `by_name` and the
/// theme picker work off this.
pub fn all() -> &'static [&'static Theme] {
    &library().themes
}

/// What's wrong with your theme files, if anything, to be said once.
pub fn problems() -> &'static [String] {
    &library().problems
}

/// Case-insensitive lookup by `Theme::name` -- the saved setting just
/// holds a name.
pub fn by_name(name: &str) -> Option<&'static Theme> {
    all().iter().find(|t| t.name.eq_ignore_ascii_case(name)).copied()
}

/// `DEFAULT`, the theme there always is.
pub fn default_theme() -> &'static Theme {
    by_name(DEFAULT).expect("Orbit Dark ships with Fenix")
}

/// The theme `name` names, else the default.
pub fn named_or_default(name: Option<&str>) -> &'static Theme {
    name.and_then(by_name).unwrap_or_else(default_theme)
}

#[cfg(test)]
mod tests {

    fn composite(over: [f32; 4], under: [f32; 4]) -> [f32; 4] {
        let mut result = [0.0; 4];
        for i in 0..3 {
            result[i] = over[i] * over[3] + under[i] * (1.0 - over[3]);
        }
        result[3] = 1.0;
        result
    }

    fn contrast(fg: glyphon::Color, bg: [f32; 4]) -> f32 {
        let back = glyphon::Color::rgb(
            (bg[0] * 255.0).round() as u8,
            (bg[1] * 255.0).round() as u8,
            (bg[2] * 255.0).round() as u8,
        );
        let (a, b) = (luminance(fg), luminance(back));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Whether `theme` is a light one: its page brighter than mid-grey.
    fn is_light(theme: &Theme) -> bool {
        bg_luminance(theme) > 0.5
    }

    /// What's read word by word -- body text, identifiers, comments, line
    /// numbers -- keeps 4.5:1 on every surface it's drawn on, in every
    /// theme. Accents (syntax colours, folders, Git) do too on a dark
    /// page; on a light one 3:1, the bar for text that's also told apart
    /// by its colour, so a light palette's accents stay vivid instead of
    /// all darkening to the same near-black. Through a highlight,
    /// everything keeps 3:1 and body text 4.5:1.
    #[test]
    fn every_theme_keeps_text_readable_on_editor_and_popup_surfaces() {
        for theme in all().iter().filter(|theme| theme.name != "TempleOS") {
            let reading = [theme.fg, theme.gutter_fg, theme.syntax_comment, theme.syntax_variable, theme.icon_file, theme.caret_text];
            let accents = [
                theme.syntax_keyword,
                theme.syntax_string,
                theme.syntax_function,
                theme.syntax_type,
                theme.syntax_number,
                theme.syntax_constant,
                theme.syntax_operator,
                theme.syntax_punctuation,
                theme.syntax_attribute,
                theme.icon_folder,
                theme.git_modified,
                theme.git_staged,
                theme.git_untracked,
                theme.git_ignored,
                theme.git_conflicted,
            ];
            let accent_need = if is_light(theme) { 3.0 } else { 4.5 };
            let line = composite(theme.hl_line, theme.bg);
            for surface in [theme.bg, line, theme.bg_modeline, theme.sidebar_bg] {
                for (colors, need) in [(&reading[..], 4.5), (&accents[..], accent_need)] {
                    for &color in colors {
                        let ratio = contrast(color, surface);
                        assert!(ratio >= need, "{}: {color:?} on {surface:?}: {ratio:.2}:1, wants {need}", theme.name);
                    }
                }
            }
            for overlay in [theme.selection, theme.bracket_match, theme.search_match] {
                let surface = composite(overlay, line);
                assert!(contrast(theme.fg, surface) >= 4.5, "{}: highlighted body text", theme.name);
                for color in reading.iter().chain(&accents) {
                    assert!(contrast(*color, surface) >= 3.0, "{}: highlighted {color:?}", theme.name);
                }
            }
            assert!(contrast(theme.fg_modeline, theme.bg_modeline) >= 4.5);
        }
    }

    /// A light theme's main accents -- keywords, strings, functions, types
    /// -- stand apart from its body text (OKLab distance), so code reads in
    /// colour rather than as shades of the same ink.
    #[test]
    fn light_themes_keep_their_main_accents_apart_from_body_text() {
        for theme in all().iter().filter(|theme| is_light(theme) && theme.name != "TempleOS") {
            for (role, color) in [("keyword", theme.syntax_keyword), ("string", theme.syntax_string), ("function", theme.syntax_function), ("type", theme.syntax_type)] {
                let apart = oklab_distance(color, theme.fg);
                assert!(apart >= 0.10, "{}: {role} is {apart:.3} from body text", theme.name);
            }
        }
    }

    fn oklab(c: glyphon::Color) -> [f32; 3] {
        let lin = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }

    fn oklab_distance(a: glyphon::Color, b: glyphon::Color) -> f32 {
        let (a, b) = (oklab(a), oklab(b));
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn dark_theme_mode_badges_have_readable_labels() {
        for theme in all().iter().filter(|theme| theme.name != "TempleOS") {
            for bg in [
                theme.mode_normal,
                theme.mode_insert,
                theme.mode_replace,
                theme.mode_command,
                theme.mode_explorer,
                theme.mode_picker,
            ] {
                assert!(contrast(theme.mode_text_dark, bg) >= 4.5, "{}: badge {bg:?}", theme.name);
            }
            assert!(contrast(theme.mode_text_light, theme.mode_visual) >= 4.5, "{}: visual badge", theme.name);
        }
    }

    /// Relative luminance, the sRGB formula WCAG uses -- enough to ask
    /// "is this closer to the background than the body text is", which
    /// is what "dimmer" means for a color that has to work on both a
    /// white page and a dark terminal.
    fn luminance(c: glyphon::Color) -> f32 {
        let channel = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn bg_luminance(theme: &Theme) -> f32 {
        let to_u8 = |v: f32| (v * 255.0).round() as u8;
        luminance(glyphon::Color::rgb(to_u8(theme.bg[0]), to_u8(theme.bg[1]), to_u8(theme.bg[2])))
    }

    /// Comments must read as *quieter* than ordinary code in every
    /// theme -- closer to the background than the body text is, whether
    /// that means a lighter grey on a white page or a darker one on a
    /// dark terminal. A comment the same weight as the code it explains
    /// is noise.
    #[test]
    fn every_theme_dims_its_comments_below_its_body_text() {
        for theme in all() {
            let (bg, fg, comment) = (bg_luminance(theme), luminance(theme.fg), luminance(theme.syntax_comment));
            assert!(
                (comment - bg).abs() < (fg - bg).abs(),
                "{}: comments ({comment:.3}) must sit closer to the background ({bg:.3}) than body text ({fg:.3}) does",
                theme.name
            );
        }
    }

    /// Operators and punctuation carry the structure of a line -- the
    /// braces and brackets of a Tcl script are most of what there is to
    /// read. They were the same color as body text in several themes
    /// (invisible as a distinct thing) and the same as comments in
    /// others (actively de-emphasized), so both are pinned here: a
    /// symbol is neither ordinary text nor a comment.
    #[test]
    fn every_theme_makes_its_symbols_stand_out_from_text_and_comments() {
        for theme in all() {
            for (role, color) in [("operator", theme.syntax_operator), ("punctuation", theme.syntax_punctuation)] {
                assert_ne!(rgb_of(color), rgb_of(theme.fg), "{}: {role} is the same color as body text", theme.name);
                assert_ne!(rgb_of(color), rgb_of(theme.syntax_comment), "{}: {role} is as dim as a comment", theme.name);
            }
        }
    }

    fn rgb_of(c: glyphon::Color) -> (u8, u8, u8) {
        (c.r(), c.g(), c.b())
    }
    use super::*;

    fn named(name: &str) -> &'static Theme {
        by_name(name).unwrap_or_else(|| panic!("{name} ships"))
    }

    #[test]
    fn exact_top_level_names_resolve_directly() {
        assert_eq!(named("Orbit Dark").syntax_color("keyword"), named("Orbit Dark").syntax_keyword);
        assert_eq!(named("Orbit Dark").syntax_color("string"), named("Orbit Dark").syntax_string);
    }

    #[test]
    fn dotted_sub_captures_fall_back_to_their_top_level_color() {
        assert_eq!(named("Orbit Dark").syntax_color("function.method"), named("Orbit Dark").syntax_function);
        assert_eq!(named("Orbit Dark").syntax_color("constant.builtin"), named("Orbit Dark").syntax_constant);
        assert_eq!(named("Orbit Dark").syntax_color("punctuation.bracket"), named("Orbit Dark").syntax_punctuation);
    }

    #[test]
    fn the_newer_grammars_capture_names_get_real_colors() {
        let t = named("Orbit Dark");
        assert_eq!(t.syntax_color("field"), t.syntax_variable);
        assert_eq!(t.syntax_color("parameter"), t.syntax_variable);
        assert_eq!(t.syntax_color("method.call"), t.syntax_function);
        assert_eq!(t.syntax_color("float"), t.syntax_number);
        assert_eq!(t.syntax_color("namespace"), t.syntax_type);
        assert_eq!(t.syntax_color("storageclass"), t.syntax_keyword);
        assert_eq!(t.syntax_color("preproc"), t.syntax_attribute);
    }

    #[test]
    fn unrecognized_capture_falls_back_to_plain_fg() {
        assert_eq!(named("Orbit Dark").syntax_color("some.unknown.capture"), named("Orbit Dark").fg);
    }

    #[test]
    fn markdown_text_captures_resolve_by_their_full_name_not_the_shared_text_prefix() {
        // All six share the same "text" top-level segment -- proves
        // they're actually distinguished by their exact name, not all
        // silently landing on one fallback color together.
        assert_eq!(named("Orbit Dark").syntax_color("text.title"), named("Orbit Dark").syntax_function);
        assert_eq!(named("Orbit Dark").syntax_color("text.strong"), named("Orbit Dark").syntax_keyword);
        assert_eq!(named("Orbit Dark").syntax_color("text.emphasis"), named("Orbit Dark").syntax_type);
        assert_eq!(named("Orbit Dark").syntax_color("text.literal"), named("Orbit Dark").syntax_string);
        assert_eq!(named("Orbit Dark").syntax_color("text.uri"), named("Orbit Dark").syntax_constant);
        assert_eq!(named("Orbit Dark").syntax_color("text.reference"), named("Orbit Dark").syntax_constant);

        let distinct: std::collections::HashSet<_> = ["text.title", "text.strong", "text.emphasis", "text.literal", "text.uri"]
            .map(|name| format!("{:?}", named("Orbit Dark").syntax_color(name)))
            .into_iter()
            .collect();
        assert_eq!(distinct.len(), 5, "each of these should read as visibly different from the others");
    }

    #[test]
    fn an_unrelated_dotted_text_capture_still_falls_back_to_the_generic_top_segment_rule() {
        // "text" itself isn't one of the generic top-level slots
        // `syntax_color` knows about -- a capture named "text.*" that
        // isn't one of the five specific ones above should fall all
        // the way through to plain `fg`, not silently match one of
        // them by accident.
        assert_eq!(named("Orbit Dark").syntax_color("text.something_else"), named("Orbit Dark").fg);
    }

    #[test]
    fn tcls_bare_control_flow_and_spell_captures_resolve() {
        assert_eq!(named("Orbit Dark").syntax_color("repeat"), named("Orbit Dark").syntax_keyword);
        assert_eq!(named("Orbit Dark").syntax_color("conditional"), named("Orbit Dark").syntax_keyword);
        assert_eq!(named("Orbit Dark").syntax_color("spell"), named("Orbit Dark").syntax_comment);
    }

    #[test]
    fn git_status_color_covers_every_status() {
        use fenix_explorer::GitStatus;
        assert_eq!(named("Orbit Dark").git_status_color(GitStatus::Modified), named("Orbit Dark").git_modified);
        assert_eq!(named("Orbit Dark").git_status_color(GitStatus::Staged), named("Orbit Dark").git_staged);
        assert_eq!(named("Orbit Dark").git_status_color(GitStatus::Untracked), named("Orbit Dark").git_untracked);
        assert_eq!(named("Orbit Dark").git_status_color(GitStatus::Ignored), named("Orbit Dark").git_ignored);
        assert_eq!(named("Orbit Dark").git_status_color(GitStatus::Conflicted), named("Orbit Dark").git_conflicted);
    }

    #[test]
    fn templeos_syntax_and_git_colors_resolve_the_same_generic_way() {
        // named("TempleOS") is a plain data value like named("Orbit Dark") -- no special
        // casing anywhere in `syntax_color`/`git_status_color`, so this
        // is really a check that the const itself is well-formed.
        assert_eq!(named("TempleOS").syntax_color("keyword"), named("TempleOS").syntax_keyword);
        assert_eq!(named("TempleOS").syntax_color("function.method"), named("TempleOS").syntax_function);
        assert_eq!(named("TempleOS").syntax_color("some.unknown.capture"), named("TempleOS").fg);
        use fenix_explorer::GitStatus;
        assert_eq!(named("TempleOS").git_status_color(GitStatus::Staged), named("TempleOS").git_staged);
    }

    #[test]
    fn all_contains_every_shipped_theme_by_name_in_cycling_order() {
        let names: Vec<&str> = all().iter().map(|t| t.name).collect();
        assert_eq!(
            names,
            vec![
                "Orbit Dark",
                "TempleOS",
                "Gruvbox Dark",
                "Nord",
                "Dracula",
                "Solarized Dark",
                "One Dark",
                "Visual Studio Dark",
                "Catppuccin Latte",
                "Catppuccin Frappé",
                "Catppuccin Macchiato",
                "Catppuccin Mocha",
                "Monokai",
                "GitHub Dark",
                "Tokyo Night Storm",
                "Kanagawa",
                "Rosé Pine",
                "Everforest Dark",
                "Ayu Mirage",
                "Night Owl",
                "Material Palenight",
                "Zenburn",
                "GitHub Light",
                "One Light",
                "Solarized Light",
                "Gruvbox Light",
                "Rosé Pine Dawn",
                "Tokyo Night Day",
                "Visual Studio Light"
            ]
        );
    }

    #[test]
    fn every_new_theme_resolves_syntax_and_git_colors_the_same_generic_way() {
        // Mirrors `templeos_syntax_and_git_colors_resolve_the_same_generic_way`
        // -- confirms each new const is a well-formed plain data value,
        // no special-casing needed anywhere `syntax_color`/`git_status_
        // color` are called.
        use fenix_explorer::GitStatus;
        for theme in all().iter().copied() {
            assert_eq!(theme.syntax_color("keyword"), theme.syntax_keyword, "{}", theme.name);
            assert_eq!(theme.syntax_color("function.method"), theme.syntax_function, "{}", theme.name);
            assert_eq!(theme.syntax_color("some.unknown.capture"), theme.fg, "{}", theme.name);
            assert_eq!(theme.syntax_color("text.title"), theme.syntax_function, "{}", theme.name);
            assert_eq!(theme.syntax_color("text.strong"), theme.syntax_keyword, "{}", theme.name);
            assert_eq!(theme.git_status_color(GitStatus::Staged), theme.git_staged, "{}", theme.name);
        }
    }

    #[test]
    fn by_name_resolves_every_new_theme_case_insensitively() {
        for name in all().iter().map(|t| t.name) {
            assert_eq!(by_name(&name.to_lowercase()).map(|t| t.name), Some(name));
        }
    }

    /// Every shipped theme reads: none is left out for being wrong.
    #[test]
    fn every_shipped_theme_file_reads() {
        assert!(problems().is_empty(), "{:?}", problems());
        assert_eq!(all().len(), BUILT_IN.len());
        assert_eq!(default_theme().name, DEFAULT);
    }

    /// A theme of yours: what it can leave out is filled in, what's wrong
    /// with it is said, by key.
    #[test]
    fn a_theme_file_fills_its_defaults_and_says_what_is_wrong() {
        let mine = BUILT_IN[0].1.replace("name = \"Orbit Dark\"", "name = \"Mine\"").replace("sidebar_bg = \"#1a1b26\"\n", "").replace("fg_modeline = \"#c0caf5\"\n", "").replace("caret_text = \"#e0af68\"\n", "");
        let theme = parse(&mine).expect("reads");
        assert_eq!(theme.name, "Mine");
        assert_eq!(theme.sidebar_bg, theme.bg, "the sidebar is the editor's background");
        assert_eq!(theme.fg_modeline, theme.fg);
        assert_eq!(theme.caret_text, opaque(theme.caret));
        assert_eq!(theme.tabs, TabStyle::Underline);

        let alpha = mine.replace("hl_line = \"#222534\"", "hl_line = \"#22253480\"");
        assert!((parse(&alpha).unwrap().hl_line[3] - 128.0 / 255.0).abs() < 1e-6, "#rrggbbaa");
        let bad = mine.replace("keyword = \"#bb9af7\"", "keyword = \"purple\"");
        assert_eq!(parse(&bad).err().as_deref(), Some("syntax.keyword: purple isn't a color -- write #rrggbb"));
        let see_through = mine.replace("keyword = \"#bb9af7\"", "keyword = { color = \"#bb9af7\", alpha = 0.5 }");
        assert!(parse(&see_through).err().unwrap().starts_with("syntax.keyword: text can't be see-through"));
        let tabs = mine.replace("tabs = \"underline\"", "tabs = \"round\"");
        assert!(parse(&tabs).err().unwrap().contains("underline or block"));
        assert!(parse(&mine.replace("[git]", "[gits]")).is_err(), "an unknown section is an error, not ignored");
    }

    /// Yours are read from a folder, sorted; a broken one is said, not
    /// fatal.
    #[test]
    fn your_themes_are_read_from_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.toml"), BUILT_IN[0].1.replace("Orbit Dark", "Mine")).unwrap();
        std::fs::write(dir.path().join("a.toml"), "name = \"Broken\"").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a theme").unwrap();
        let read = read_dir_themes(dir.path());
        assert_eq!(read.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(), ["a.toml", "b.toml"]);
        assert!(read[0].1.is_err() && read[1].1.as_ref().is_ok_and(|t| t.name == "Mine"));
    }

    #[test]
    fn by_name_is_case_insensitive_and_none_for_unknown() {
        assert_eq!(by_name("templeos").map(|t| t.name), Some("TempleOS"));
        assert_eq!(by_name("TEMPLEOS").map(|t| t.name), Some("TempleOS"));
        assert_eq!(by_name("orbit dark").map(|t| t.name), Some("Orbit Dark"));
        assert!(by_name("nonexistent-theme").is_none());
    }

}
