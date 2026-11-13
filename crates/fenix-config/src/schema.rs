//! Every setting, declared once: its key in `settings.toml`, where it
//! goes on the settings page, what kind of value it takes and what it's
//! for. Loading, checking, saving, the settings page and the settings
//! table in docs/CONFIGURATION.md all read this list, so they can't disagree.
//!
//! Core's settings are built in, each kept in a field of `Config`. A
//! feature (an extension, later) registers its own at startup
//! (`register`); those are kept by key (`Config::value`), with the same
//! keys in `settings.toml`.

use std::path::PathBuf;
use std::sync::LazyLock;

use toml_edit::{value, Array, ArrayOfTables, InlineTable, Item, Table};

use crate::{Config, Secret};

/// A setting's value, independent of how it's stored.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    List(Vec<String>),
    /// Named entries, in order: `[lsp.servers] python = "..."`.
    Map(Vec<(String, String)>),
    /// Rows of fields, as the `Kind::Records` names them.
    Records(Vec<Vec<String>>),
}

/// One field of a `Kind::Records` row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Field {
    pub name: &'static str,
    pub label: &'static str,
    /// Whole numbers in this range, for a field that's a number.
    pub range: Option<(i64, i64)>,
    /// Used when the field is left out.
    pub default: Option<&'static str>,
    /// The only values it takes, picked rather than typed; empty for
    /// free text.
    pub choices: &'static [&'static str],
}

/// What a setting takes, and so how it's edited and checked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Bool,
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    /// A number of minutes; `5`, `5m` and `1h` are all accepted.
    Minutes,
    Text,
    Path,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A theme's name; the choices are the themes the editor has.
    Theme,
    /// A font's name; the choices are the fonts installed.
    Font,
    /// A list of words (`["alex", "sam"]`).
    List,
    /// Named entries; `key` and `value` label the two halves.
    Map { key: &'static str, value: &'static str, paths: bool },
    Records(&'static [Field]),
    /// A token: typed masked on the page, and an environment variable
    /// overrides it.
    Secret(Secret),
}

/// Where a setting sits on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Editor,
    Appearance,
    Motion,
    Files,
    Completion,
    Git,
    Forges,
    Jira,
    Documents,
    Session,
    Extensions,
    /// The coding agents (`SPC a`).
    Agents,
    /// Toasts, their history and do not disturb (`SPC o n`).
    Notifications,
    /// An extension's own: the section its settings name, its name when
    /// they name none. Each is a section of the page, as core's are.
    Extension(&'static str),
}

impl Category {
    /// Core's, in page order. Extensions' sections (`Extension`) follow
    /// them on the page.
    pub const ALL: [Category; 13] = [
        Category::Editor,
        Category::Appearance,
        Category::Motion,
        Category::Files,
        Category::Completion,
        Category::Git,
        Category::Forges,
        Category::Jira,
        Category::Documents,
        Category::Session,
        Category::Extensions,
        Category::Agents,
        Category::Notifications,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Editor => "Editor",
            Category::Appearance => "Appearance",
            Category::Motion => "Motion",

            Category::Files => "Files & explorer",
            Category::Completion => "Completion & LSP",
            Category::Git => "Git",
            Category::Forges => "Forges",
            Category::Jira => "Jira & agenda",
            Category::Documents => "Workspaces",
            Category::Session => "Windows & session",
            Category::Extensions => "Extensions",
            Category::Agents => "Agents",
            Category::Notifications => "Notifications",
            Category::Extension(section) => section,
        }
    }
}

type Get = fn(&Config) -> Option<Value>;
type Set = fn(&mut Config, Option<Value>) -> Result<(), String>;
type Check = fn(&Value) -> Result<(), String>;

/// Where a setting's value lives.
#[derive(Clone, Copy)]
enum Access {
    /// In a field of `Config` (core's settings).
    Field(Get, Set),
    /// By its key, in `Config`'s registered values (a feature's).
    Stored,
}

/// One setting.
pub struct Setting {
    /// Its key in `settings.toml`: `editor.font_size`.
    pub key: &'static str,
    pub category: Category,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// What applies when it isn't set, as the page shows it.
    pub default: &'static str,
    /// Whether a project's own settings may set it.
    pub project: bool,
    /// Whether it only takes effect after a restart.
    pub restart: bool,
    access: Access,
    /// What a value has to be beyond its kind (`checked`).
    check: Option<Check>,
}

impl std::fmt::Debug for Setting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setting").field("key", &self.key).finish()
    }
}

impl Setting {
    /// Its value in `config`; `None` when it isn't set.
    pub fn get(&self, config: &Config) -> Option<Value> {
        match self.access {
            Access::Field(get, _) => get(config),
            Access::Stored => config.values.get(self.key).cloned(),
        }
    }

    /// Sets it in `config`, after checking it; `None` unsets it.
    pub fn set(&self, config: &mut Config, value: Option<Value>) -> Result<(), String> {
        if let Some(v) = &value {
            self.kind.check(v)?;
            if let Some(check) = self.check {
                check(v)?;
            }
        }
        match self.access {
            Access::Field(_, set) => set(config, value),
            Access::Stored => {
                match value {
                    Some(v) => config.values.insert(self.key.to_string(), v),
                    None => config.values.remove(self.key),
                };
                Ok(())
            }
        }
    }

    /// A feature's setting, kept by its key: what a feature registers
    /// (`register`).
    pub const fn stored(key: &'static str, category: Category, label: &'static str, kind: Kind, help: &'static str) -> Setting {
        Setting { key, category, label, help, kind, default: "", project: false, restart: false, access: Access::Stored, check: None }
    }

    /// Checks a value beyond its kind (a Jira project's Blocked meaning):
    /// a value that fails is refused on the page and reported when the
    /// file is loaded.
    pub const fn checked(mut self, check: Check) -> Self {
        self.check = Some(check);
        self
    }

    /// What applies when it isn't set, as the page shows it.
    pub const fn with_default(mut self, default: &'static str) -> Self {
        self.default = default;
        self
    }

    /// A project can set it too (`.fenix/settings.toml`).
    pub const fn project(mut self) -> Self {
        self.project = true;
        self
    }

    /// It only takes effect after a restart.
    pub const fn restart(mut self) -> Self {
        self.restart = true;
        self
    }

    fn default(mut self, default: &'static str) -> Self {
        self.default = default;
        self
    }
}

fn s(key: &'static str, category: Category, label: &'static str, kind: Kind, help: &'static str, (get, set): (Get, Set)) -> Setting {
    Setting { key, category, label, help, kind, default: "", project: false, restart: false, access: Access::Field(get, set), check: None }
}

// -- Conversions between a field's type and a `Value` ---------------------

fn text_get(v: &Option<String>) -> Option<Value> {
    v.clone().map(Value::Text)
}
fn text_set(v: Option<Value>) -> Result<Option<String>, String> {
    match v {
        None => Ok(None),
        Some(Value::Text(t)) => Ok(Some(t)),
        Some(other) => Err(format!("expected text, got {}", other.describe())),
    }
}
fn path_get(v: &Option<PathBuf>) -> Option<Value> {
    v.as_ref().map(|p| Value::Text(p.display().to_string()))
}
fn path_set(v: Option<Value>) -> Result<Option<PathBuf>, String> {
    text_set(v).map(|t| t.map(PathBuf::from))
}
fn bool_get(v: &Option<bool>) -> Option<Value> {
    v.map(Value::Bool)
}
fn bool_set(v: Option<Value>) -> Result<Option<bool>, String> {
    match v {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(b)),
        Some(other) => Err(format!("expected true or false, got {}", other.describe())),
    }
}
fn int_set<T: TryFrom<i64>>(v: Option<Value>) -> Result<Option<T>, String> {
    match v {
        None => Ok(None),
        Some(Value::Int(n)) => T::try_from(n).map(Some).map_err(|_| format!("{n} is out of range")),
        Some(other) => Err(format!("expected a whole number, got {}", other.describe())),
    }
}
fn usize_get(v: &Option<usize>) -> Option<Value> {
    v.map(|n| Value::Int(n as i64))
}
fn usize_set(v: Option<Value>) -> Result<Option<usize>, String> {
    int_set::<i64>(v)?.map(|n| usize::try_from(n).map_err(|_| format!("{n} is out of range"))).transpose()
}
fn u64_get(v: &Option<u64>) -> Option<Value> {
    v.map(|n| Value::Int(n as i64))
}
fn u64_set(v: Option<Value>) -> Result<Option<u64>, String> {
    int_set::<i64>(v)?.map(|n| u64::try_from(n).map_err(|_| format!("{n} is out of range"))).transpose()
}
fn f32_get(v: &Option<f32>) -> Option<Value> {
    v.map(|n| Value::Float(n as f64))
}
fn f32_set(v: Option<Value>) -> Result<Option<f32>, String> {
    match v {
        None => Ok(None),
        Some(Value::Float(n)) => Ok(Some(n as f32)),
        Some(Value::Int(n)) => Ok(Some(n as f32)),
        Some(other) => Err(format!("expected a number, got {}", other.describe())),
    }
}
fn list_get(v: &[String]) -> Option<Value> {
    (!v.is_empty()).then(|| Value::List(v.to_vec()))
}
fn list_set(v: Option<Value>) -> Result<Vec<String>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::List(l)) => Ok(l),
        Some(Value::Text(t)) => Ok(crate::names(&t)),
        Some(other) => Err(format!("expected a list, got {}", other.describe())),
    }
}
fn map_get(v: &[(String, String)]) -> Option<Value> {
    (!v.is_empty()).then(|| Value::Map(v.to_vec()))
}
fn map_set(v: Option<Value>) -> Result<Vec<(String, String)>, String> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Map(m)) => Ok(m),
        Some(other) => Err(format!("expected named entries, got {}", other.describe())),
    }
}


macro_rules! field {
    ($f:ident, $get:expr, $set:expr) => {
        (|c: &Config| $get(&c.$f), |c: &mut Config, v: Option<Value>| {
            c.$f = $set(v)?;
            Ok(())
        })
    };
}

/// A field of one of `Config`'s grouped settings: `sub!(motion.beacon, ...)`.
macro_rules! sub {
    ($group:ident . $f:ident, $get:expr, $set:expr) => {
        (|c: &Config| $get(&c.$group.$f), |c: &mut Config, v: Option<Value>| {
            c.$group.$f = $set(v)?;
            Ok(())
        })
    };
}

static SETTINGS: LazyLock<Vec<Setting>> = LazyLock::new(|| {
    use Category::*;
    vec![
        // Editor
        s("editor.indent_width", Editor, "Indent width", Kind::Int { min: 1, max: 16 }, "Spaces a Tab or >> inserts. A file's .editorconfig comes before it (indent_size, and indent_style = tab for tabs); the project's own setting before both.", field!(indent_width, usize_get, usize_set)).default("4").project(),
        s("editor.tab_width", Editor, "Tab width", Kind::Int { min: 1, max: 16 }, "Columns a tab character takes up. A file's .editorconfig (tab_width) comes before it; the project's own setting before both.", field!(tab_width, usize_get, usize_set)).default("8").project(),
Setting::stored("editor.wrap", Editor, "Wrap long lines", Kind::Bool, "A line wider than its pane goes on under itself, at the pane's width; SPC t w (Alt+Z with standard keys) turns it on or off for one buffer until it's closed or Fenix restarts. Not tables, diffs, Git pages, terminals or a side by side.").with_default("off"),
        Setting::stored("editor.wrap_languages", Editor, "Languages that wrap", Kind::List, "Languages whose lines wrap whatever Wrap long lines says (markdown; text for .txt and .text files).").with_default("markdown, text"),
        Setting::stored("editor.nowrap_languages", Editor, "Languages that don't wrap", Kind::List, "Languages whose lines never wrap unless SPC t w says so: over Wrap long lines and the list of those that do.").with_default("none"),
        Setting::stored("editor.line_numbers", Editor, "Line numbers", Kind::Choice(&["absolute", "relative", "off"]), "The gutter's numbers: each line's own, how far each is from the cursor's line, or none. SPC t n cycles them until Fenix restarts.").with_default("absolute"),
        Setting::stored("editor.fold_markers", Editor, "Fold markers", Kind::Bool, "A column in the gutter of a file's lines: ▾ where a fold starts, ▸ where it's closed; a click opens or closes it, as za does.").with_default("on"),
        Setting::stored("editor.mouse_hover", Editor, "Hover with the mouse", Kind::Bool, "The pointer resting on a symbol shows what K would -- its problems first -- under it; moving away or typing closes it.").with_default("on"),
        Setting::stored("editor.hover_delay_ms", Editor, "Mouse hover delay", Kind::Int { min: 0, max: 5000 }, "Milliseconds the pointer rests on a symbol before its hover shows; 0 never shows it.").with_default("500"),
                Setting::stored("editor.keys", Editor, "Keys", Kind::Choice(&["vim", "standard"]), "How you edit. Vim: Vim's modes and keys, and SPC for everything else. Standard: the keys of a traditional IDE -- typing at once, Shift+arrows to select, Ctrl+C/X/V, Alt+click for more cursors -- and Ctrl+K for the leader's menu. SPC t k switches.").with_default("vim"),
        Setting::stored("keys.leader_chord", Editor, "Leader chord", Kind::Text, "With standard keys: the chord that opens the leader's menu, as SPC does with Vim keys (C-k, C-;, F1...).").with_default("C-k"),
        Setting::stored("keys.leader_hint", Editor, "Leader hint", Kind::Bool, "With standard keys: the modeline says which chord opens the leader's menu, until you've used it a few times.").with_default("on"),
        Setting::stored("editor.multi_cursor_modifier", Editor, "More cursors with", Kind::Choice(&["alt", "ctrl"]), "With standard keys: the key held while clicking to add a cursor. Ctrl, for desktops that move windows with Alt+drag; go to definition then moves to Alt+click.").with_default("alt"),
        Setting::stored("editor.undo_pause_ms", Editor, "Undo pause", Kind::Int { min: 0, max: 10000 }, "With standard keys: a pause this long, in milliseconds, between keys you type starts a new undo step; 0 never does.").with_default("1000"),
        s("editor.iskeyword_extra", Editor, "Word characters", Kind::Text, "Characters besides letters, digits and _ that count as part of a word, for w, * and completion.", field!(iskeyword_extra, text_get, text_set)).default("none extra").project(),
        // Appearance
        s("editor.theme", Appearance, "Theme", Kind::Theme, "The colour theme; h and l preview each one.", field!(theme, text_get, text_set)).default("Orbit Dark"),
        s("editor.font_family", Appearance, "Font", Kind::Font, "A monospace font installed on this machine; Enter lists them to pick from, h and l step through them.", field!(font_family, text_get, text_set)).default("the system's monospace font"),
        Setting::stored("accessibility.screen_reader", Appearance, "Screen readers", Kind::Bool, "Fenix tells a screen reader (Narrator, NVDA, JAWS, Orca, VoiceOver) the text you're in with its cursor and selection, the modeline, prompts, and each change of mode and message. Nothing is worked out until one asks. Takes effect the next time Fenix starts.").with_default("on"),
        s("editor.font_size", Appearance, "Font size", Kind::Float { min: 6.0, max: 48.0 }, "Text size, in points.", field!(font_size, f32_get, f32_set)).default("16"),
        s("editor.animations", Motion, "Animations", Kind::Bool, "Off stops every animation, whatever the settings below say.", field!(animations, bool_get, bool_set)).default("on"),
        s("motion.level", Motion, "Level", Kind::Choice(&["off", "subtle", "full"]), "How much moves. Subtle: short animations that show what changed. Full adds the caret gliding, folds opening and panes moving. Each animation below can be turned on or off on its own. SPC t a cycles it.", sub!(motion.level, text_get, text_set)).default("subtle"),
        s("motion.speed", Motion, "Duration scale", Kind::Float { min: 0.25, max: 4.0 }, "Multiplies every animation's length: 2 is half as fast, 0.5 twice as fast.", sub!(motion.speed, f32_get, f32_set)).default("1"),
        s("motion.caret_fade", Motion, "Caret blink fade", Kind::Bool, "The caret fades in and out when it blinks instead of switching.", sub!(motion.caret_fade, bool_get, bool_set)).default("with subtle"),
        s("motion.smooth_scroll", Motion, "Smooth scrolling", Kind::Bool, "The view eases to where it scrolls: up and down, sideways, and from one mouse-wheel notch to the next.", sub!(motion.smooth_scroll, bool_get, bool_set)).default("with subtle"),
        s("motion.scroll_ms", Motion, "Scroll length", Kind::Int { min: 20, max: 1000 }, "Milliseconds a scroll takes.", sub!(motion.scroll_ms, u64_get, u64_set)).default("150"),
        s("motion.yank_pulse", Motion, "Yank and paste pulse", Kind::Bool, "What you yank or paste flashes once.", sub!(motion.yank_pulse, bool_get, bool_set)).default("with subtle"),
        s("motion.beacon", Motion, "Jump beacon", Kind::Bool, "After a jump (gd, a search, Ctrl-O, a picker, a mark) the line you land on flashes once.", sub!(motion.beacon, bool_get, bool_set)).default("with subtle"),
        s("motion.beacon_ms", Motion, "Beacon length", Kind::Int { min: 50, max: 2000 }, "Milliseconds the jump beacon takes to fade.", sub!(motion.beacon_ms, u64_get, u64_set)).default("300"),
        s("motion.beacon_on_focus", Motion, "Beacon on pane switch", Kind::Bool, "Moving to another pane with the keyboard flashes its cursor line.", sub!(motion.beacon_on_focus, bool_get, bool_set)).default("with subtle"),
        s("motion.change_pulse", Motion, "Change pulse", Kind::Bool, "Undo, redo, . and :s flash what they changed; a deletion flashes a bar where the text was.", sub!(motion.change_pulse, bool_get, bool_set)).default("with subtle"),
        s("motion.popups", Motion, "Popups fade", Kind::Bool, "Which-key, completion, hover, prompts and menus fade in rising from where they open.", sub!(motion.popups, bool_get, bool_set)).default("with subtle"),
        s("motion.messages", Motion, "Messages fade", Kind::Bool, "Messages in the modeline fade in and out instead of appearing and cutting off.", sub!(motion.messages, bool_get, bool_set)).default("with subtle"),
        s("motion.error_flash", Motion, "Error flash", Kind::Bool, "An error flashes the mode rail red once.", sub!(motion.error_flash, bool_get, bool_set)).default("with subtle"),
        s("motion.progress", Motion, "Progress spinner", Kind::Bool, "Work in the background (a fetch, a sync, a language server starting) shows in the modeline with the Fenix mark turning.", sub!(motion.progress, bool_get, bool_set)).default("with subtle"),
        s("motion.mode_fade", Motion, "Mode colour fade", Kind::Bool, "Changing mode blends the rail and the caret to the new mode's colour.", sub!(motion.mode_fade, bool_get, bool_set)).default("with subtle"),
        s("motion.tabs", Motion, "Tabs slide", Kind::Bool, "The active tab's accent slides to the tab you move to.", sub!(motion.tabs, bool_get, bool_set)).default("with subtle"),
        s("motion.caret_glide", Motion, "Caret glide", Kind::Bool, "The caret slides to where a motion takes it instead of jumping. Never while typing.", sub!(motion.caret_glide, bool_get, bool_set)).default("with full"),
        s("motion.glide_ms", Motion, "Glide length", Kind::Int { min: 20, max: 500 }, "Milliseconds the caret takes to glide.", sub!(motion.glide_ms, u64_get, u64_set)).default("45"),
        s("motion.folds", Motion, "Folds open", Kind::Bool, "Rows revealed by unfolding (a folder, a code fold) open downwards.", sub!(motion.folds, bool_get, bool_set)).default("with full"),
        s("motion.layout", Motion, "Panes move", Kind::Bool, "Splitting, closing and resizing panes move their edges instead of jumping.", sub!(motion.layout, bool_get, bool_set)).default("with full"),
        s("motion.theme_fade", Motion, "Theme cross-fade", Kind::Bool, "Changing theme fades from the old colours to the new.", sub!(motion.theme_fade, bool_get, bool_set)).default("with subtle"),
        s("motion.splash", Motion, "Launch splash", Kind::Bool, "While Fenix starts, the mark and a line that fills as it loads show until the editor is ready. It moves unless animations are off; only this setting hides it.", sub!(motion.splash, bool_get, bool_set)).default("on"),
        s("appearance.corner_radius", Appearance, "Corner radius", Kind::Int { min: 0, max: 16 }, "Pixels of rounding on popups and floating boxes. 0 keeps them square.", sub!(polish.corner_radius, usize_get, usize_set)).default("the theme's"),
        s("appearance.shadows", Appearance, "Shadows", Kind::Bool, "Popups cast a soft shadow.", sub!(polish.shadows, bool_get, bool_set)).default("on"),
        s("appearance.indent_guides", Appearance, "Indent guides", Kind::Bool, "A thin line for each indent level.", sub!(polish.indent_guides, bool_get, bool_set)).default("on"),
        s("appearance.active_indent_guide", Appearance, "Active indent guide", Kind::Bool, "The guide of the block the cursor is in is brighter.", sub!(polish.active_indent_guide, bool_get, bool_set)).default("on"),
        s("appearance.overview_ruler", Appearance, "Overview ruler", Kind::Bool, "A track on each code pane's right edge showing the visible part, search matches, problems by severity, Git changes, the cursor and any extra cursors. A click on a mark goes to it; elsewhere, to that part of the file.", sub!(polish.overview_ruler, bool_get, bool_set)).default("on"),
        s("appearance.sticky_scroll", Appearance, "Sticky scroll", Kind::Bool, "The first lines of the function and class you're inside stay at the top of the pane.", sub!(polish.sticky_scroll, bool_get, bool_set)).default("on"),
        s("appearance.sticky_lines", Appearance, "Sticky lines", Kind::Int { min: 1, max: 6 }, "How many enclosing lines sticky scroll keeps at most.", sub!(polish.sticky_lines, usize_get, usize_set)).default("3"),
        s("appearance.rounded_selection", Appearance, "Rounded selection", Kind::Bool, "A Visual selection's outer corners are rounded, so it reads as one shape.", sub!(polish.rounded_selection, bool_get, bool_set)).default("on"),
        s("appearance.selection_whitespace", Appearance, "Whitespace in selection", Kind::Bool, "Spaces show as dots and tabs as arrows inside a Visual selection.", sub!(polish.selection_whitespace, bool_get, bool_set)).default("on"),
        s("appearance.dim_unfocused", Appearance, "Dim unfocused panes", Kind::Bool, "Panes that don't have the keyboard are drawn dimmer.", sub!(polish.dim_unfocused, bool_get, bool_set)).default("on"),
        s("appearance.dim_amount", Appearance, "Dim amount", Kind::Int { min: 5, max: 80 }, "How much dimmer, in percent.", sub!(polish.dim_amount, usize_get, usize_set)).default("30"),
        s("which_key.delay_ms", Appearance, "Key menu delay", Kind::Int { min: 0, max: 2000 }, "Milliseconds a leader key (SPC, g, ...) waits before the menu of what comes next opens. Once open, deeper levels show at once.", sub!(polish.which_key_delay_ms, u64_get, u64_set)).default("250"),
        s("which_key.order", Appearance, "Key menu order", Kind::Choice(&["key", "label"]), "The key menu's order: by key, so a key never moves, or by what it does.", sub!(polish.which_key_order, text_get, text_set)).default("key"),
        s("appearance.rainbow_brackets", Appearance, "Rainbow brackets", Kind::Bool, "Brackets are coloured by how deeply they're nested: the code's own, from its syntax tree, not those in strings and comments. The colours are the theme's [brackets] colors, or its function, keyword and type colours.", sub!(polish.rainbow_brackets, bool_get, bool_set)).default("off"),
        s("appearance.tabs", Appearance, "Tabs", Kind::Choice(&["auto", "block", "underline", "off"]), "How each pane's tabs are drawn: the theme's own look (Visual Studio Dark uses blocks, the rest an underline), always blocks, always an underline, or no tab strip at all -- gt and gT still move between tabs then.", sub!(polish.tabs, text_get, text_set)).default("auto"),
        Setting::stored("appearance.panels", Appearance, "Panels", Kind::Choice(&["framed", "flat"]), "How the window's panes, sidebar, terminal and docks are drawn: rounded panels on a darker tray a gap apart, with an edge on the one that has the keyboard, or flat and edge to edge with lines between them, as before 1.4.").with_default("framed"),
        s("editor.preview_tab", Appearance, "Preview tabs", Kind::Bool, "A jump (gd, a search result, a symbol) opens in one reusable tab, in italics, until you edit it or keep it with SPC b P.", field!(preview_tab, bool_get, bool_set)).default("on"),
        // Screenshots (`SPC c s`)
        Setting::stored("screenshot.style", Appearance, "Screenshot background", Kind::Choice(&["accent", "solid", "sunset", "ocean", "candy", "forest", "none"]), "What's behind a code screenshot's window: a gradient of the theme's accent, the accent flat, one of four gradients, or nothing (see-through).").with_default("accent"),
        Setting::stored("screenshot.title", Appearance, "Screenshot title bar", Kind::Bool, "A code screenshot's window has a title bar with three dots and the file's name.").with_default("on"),
        Setting::stored("screenshot.line_numbers", Appearance, "Screenshot line numbers", Kind::Bool, "A code screenshot shows each line's number in the file.").with_default("off"),
        Setting::stored("screenshot.scale", Appearance, "Screenshot scale", Kind::Float { min: 1.0, max: 4.0 }, "Pixels per point a code screenshot is drawn at; 2 is sharp on a retina screen.").with_default("2"),
        Setting::stored("screenshot.padding", Appearance, "Screenshot padding", Kind::Int { min: 0, max: 200 }, "Points of background around a code screenshot's window.").with_default("56"),
        Setting::stored("screenshot.max_width", Appearance, "Screenshot width", Kind::Int { min: 20, max: 400 }, "Columns a code screenshot's lines take at most; a longer one wraps or is cut.").with_default("100"),
        Setting::stored("screenshot.long_lines", Appearance, "Screenshot long lines", Kind::Choice(&["wrap", "cut"]), "A line wider than the screenshot's width goes on under itself, or stops with an ellipsis.").with_default("wrap"),
        Setting::stored("screenshot.context", Appearance, "Screenshot context", Kind::Int { min: 0, max: 50 }, "Lines either side of the highlighted ones that SPC c S takes too.").with_default("3"),
        // Terminals (`SPC o t`, `SPC o T`)
        Setting::stored("terminal.shell", Editor, "Terminal shell", Kind::Text, "The shell a terminal starts, with its arguments: pwsh -NoLogo, bash -l; a path with spaces in double quotes. Empty: PowerShell 7 on Windows when it's installed, else Windows PowerShell; $SHELL elsewhere. A remote project's terminal is your own shell on its machine.").with_default("PowerShell 7, else Windows PowerShell; $SHELL"),
        Setting::stored("screenshot.folder", Appearance, "Screenshot folder", Kind::Path, "Where code screenshots are saved, besides the clipboard.").with_default("the Pictures folder"),
        // Files
        s("editor.watch_files", Files, "Watch files on disk", Kind::Bool, "Notice when an open file changes on disk, and reload it when you haven't edited it.", field!(watch_files, bool_get, bool_set)).default("on"),
        Setting::stored("editor.auto_save", Files, "Auto-save", Kind::Choice(&["off", "after_delay", "on_focus_change"]), "Save a file's edits for you: after a pause in editing (the delay below), or when you leave it for another buffer, pane or application. Never a file with no name yet or not yet on disk, one shown read-only, a remote project's, or one that changed on disk since it was read -- that, or a save that fails, waits until you save it yourself; it doesn't format.").with_default("off"),
        Setting::stored("editor.auto_save_delay_ms", Files, "Auto-save delay", Kind::Int { min: 100, max: 600000 }, "With auto-save after a delay: milliseconds without an edit before the file is saved.").with_default("1000"),
        s("editor.format_on_save", Files, "Format on save", Kind::Bool, "Saving runs the formatter an extension gives for the file's language (a language pack's) first.", field!(format_on_save, bool_get, bool_set)).default("off"),
        s("explorer.follow", Files, "Sidebar follows the file", Kind::Bool, "The explorer's sidebar shows the file being edited: its project, the folders down to it, the file selected. Off pins it where it is; SPC e r still shows the file.", field!(explorer_follow, bool_get, bool_set)).default("on"),
        s("explorer.width", Files, "Sidebar width", Kind::Int { min: 120, max: 2000 }, "The explorer sidebar's width in pixels. Dragging its edge sets it.", field!(explorer_width, usize_get, usize_set)).default("240"),
        s("explorer.hide", Files, "Hidden from the explorer", Kind::List, "Names the explorer doesn't list, as globs: * is any run of characters, ? any one (\"*.pyc\", \"node_modules\"). Hidden files (. shows them) are separate.", (|c: &Config| list_get(&c.explorer_hide), |c: &mut Config, v| {
            c.explorer_hide = list_set(v)?;
            Ok(())
        })),
        s("explorer.compact", Files, "Compact folders", Kind::Bool, "A folder holding only a folder shares its row with it (src/main/java), and opens down to where the tree branches.", field!(explorer_compact, bool_get, bool_set)).default("on"),
        s("explorer.show_hidden", Files, "Hidden files", Kind::Bool, "List hidden files and folders: those whose name starts with a dot (.git), and on Windows those marked hidden or system. . in the explorer, or SPC e h, switches it.", field!(explorer_show_hidden, bool_get, bool_set)).default("off"),
        s("search.smart_case", Editor, "Smart-case search", Kind::Bool, "A search in lower case matches any case; a capital letter makes it exact. For / and the Search page; Alt-c on the page, \\C in /, match case anyway.", field!(search_smart_case, bool_get, bool_set)).default("on"),
        s("search.regex", Editor, "Search page reads regexes", Kind::Bool, "Whether the Search page (SPC /) starts reading the query as a regex rather than literal text. Alt-r flips it there; / and :s are always regexes.", field!(search_regex, bool_get, bool_set)).default("off"),
        s("search.exclude", Editor, "Left out of searches", Kind::List, "Globs the Search page leaves out to begin with (\"target\", \"*.lock\"). Ignored files are left out anyway; f on the page edits the globs.", (|c: &Config| list_get(&c.search_exclude), |c: &mut Config, v| {
            c.search_exclude = list_set(v)?;
            Ok(())
        })),
        s("search.context", Editor, "Lines around a match", Kind::Int { min: 0, max: 9 }, "Lines the Search page shows around each match; + and - change it there.", field!(search_context, usize_get, usize_set)).default("0"),
        s("search.max_results", Editor, "Where a search stops", Kind::Int { min: 100, max: 1000000 }, "A search stops after this many matches and says so, rather than filling memory.", field!(search_max_results, usize_get, usize_set)).default("10000"),
        // Completion & LSP
        s("completion.symbols_file", Completion, "Extra words file", Kind::Path, "A text file of words, one per line, offered by completion everywhere.", field!(completion_symbols_file, path_get, path_set)),
        s("snippets.builtin", Completion, "Built-in snippets", Kind::Bool, "Offer the snippets that come with Fenix; yours and a project's always are. SPC i S manages them.", field!(snippets_builtin, bool_get, bool_set)).default("on"),
        s("diagnostics.inline", Completion, "Inline problems", Kind::Choice(&["all", "errors", "off"]), "Problems from language servers are underlined in the text, with a dot in the gutter. SPC t d cycles it.", sub!(polish.diagnostics_inline, text_get, text_set)).default("all"),
        s("diagnostics.message", Completion, "Problem messages", Kind::Choice(&["cursor", "all", "off"]), "Which lines show their problem's message at the end: the cursor's line, every line, or none.", sub!(polish.diagnostics_message, text_get, text_set)).default("cursor"),
        s("diagnostics.delay_ms", Completion, "Problems while typing", Kind::Int { min: 0, max: 5000 }, "Milliseconds after you stop typing in Insert before new problems are drawn.", sub!(polish.diagnostics_delay_ms, u64_get, u64_set)).default("400"),
        Setting::stored("lsp.semantic_highlighting", Completion, "Colours from language servers", Kind::Bool, "Code is coloured by what its language server knows (a parameter apart from a local, a macro apart from a function), over the grammar's colours, where the server offers it. Applies to servers started after it's changed (SPC q r).").with_default("on"),
        Setting::stored("lsp.semantic_highlighting_off", Completion, "Languages without server colours", Kind::List, "Languages whose files keep the grammar's colours alone, whatever Colours from language servers says.").with_default("none"),
        Setting::stored("lsp.code_lens", Completion, "Code lens", Kind::Bool, "What a language server offers for a function or test, on a row of its own above it (Run, Debug, 3 references), dimmed. Off for now: while a file has lenses its pane snaps rather than easing, and sticky scroll is off. Turned off, every lens goes in the next frame. Turned on, each file is asked at its next sync; a server started while it was off learns that Fenix takes its refreshes only once it's restarted (SPC q r).").with_default("off"),
        Setting::stored("lsp.code_lens_off", Completion, "Languages without code lens", Kind::List, "Languages whose files show no code lenses, whatever Code lens says.").with_default("none"),
        Setting::stored("lsp.code_lens_tests", Completion, "Run test lenses", Kind::Bool, "With Code lens on: a ▶ Run test lens over each test the file's test provider found (with the test explorer on: ▶ Run test | Debug), or Fenix finds (a Rust #[test], a pytest test, a Go TestXxx), where the language server offers none, with or without a language server; it runs only in a trusted project. So with Code lens on, a file of tests goes by rows (it snaps, and sticky scroll is off) though no server offers lenses.").with_default("on"),
        Setting::stored("lsp.inlay_hints", Completion, "Inlay hints", Kind::Bool, "What a language server would write into the code without it being there -- a variable's type after its name, a parameter's name before an argument -- drawn dimmed in the line, where the server offers it. SPC t i turns them on and off. A server started while they were off learns that Fenix takes its refreshes only once it's restarted (SPC q r).").with_default("on"),
        Setting::stored("lsp.inlay_hints_off", Completion, "Languages without inlay hints", Kind::List, "Languages whose files show no inlay hints, whatever Inlay hints says.").with_default("none"),
        Setting::stored("lsp.offer_install", Completion, "Offer to install language servers", Kind::Bool, "When a file's built-in language server (rust-analyzer, pyright, clangd, gopls...) isn't installed, a notice says so and how to get it; SPC l i shows the command and runs it once you say y. Off, no notice: SPC l i still works.").with_default("on"),
        s("lsp.servers", Completion, "Language servers", Kind::Map { key: "Language", value: "Command", paths: false }, "A language server to run for a language, as the command line that starts it.", field!(lsp_servers, map_get, map_set)),
        // Git
        s("git.base_branch", Git, "Base branch", Kind::Text, "The branch pull requests and comparisons start from.", field!(git_base_branch, text_get, text_set)).default("main or master").project(),
        s("git.reviewers", Git, "Reviewers", Kind::List, "Usernames asked to review a new pull request.", (|c: &Config| list_get(&c.git_reviewers), |c: &mut Config, v| {
            c.git_reviewers = list_set(v)?;
            Ok(())
        })).project(),
        s("git.auto_fetch", Git, "Fetch every", Kind::Minutes, "Fetch the focused repository in the background this often, in minutes.", field!(git_auto_fetch_minutes, u64_get, u64_set)).default("never"),
        s("git.layout", Git, "SPC g g opens", Kind::Choice(&["page", "panes"]), "The Git status page, or the older seven-pane panel.", field!(git_layout, text_get, text_set)).default("page"),
        s("git.graph_style", Git, "Graph lines", Kind::Choice(&["ascii", "unicode"]), "Characters the commit graph is drawn with; unicode needs a font with box-drawing glyphs.", field!(git_graph_style, text_get, text_set)).default("ascii"),
        s("git.graph_limit", Git, "Graph commits", Kind::Int { min: 10, max: 100_000 }, "How many commits the graph view reads.", field!(git_graph_limit, usize_get, usize_set)).default("200"),
        // Forges
        s("gitlab.base_url", Forges, "GitLab server", Kind::Text, "The GitLab instance's address, like https://gitlab.example.com.", field!(gitlab_base_url, text_get, text_set)),
        s("gitlab.token", Forges, "GitLab token", Kind::Secret(Secret::GitLab), "A personal access token with the api scope.", field!(gitlab_token, text_get, text_set)),
        s("github.token", Forges, "GitHub token", Kind::Secret(Secret::GitHub), "Used when the GitHub CLI isn't signed in (gh auth login).", field!(github_token, text_get, text_set)),
        // Jira & agenda
        s("jira.base_url", Jira, "Jira server", Kind::Text, "Your Jira Server or Data Center's address.", field!(jira_base_url, text_get, text_set)),
        s("jira.token", Jira, "Jira token", Kind::Secret(Secret::Jira), "A personal access token for the Jira server.", field!(jira_token, text_get, text_set)),
        // Workspaces

        // Debugging
        Setting::stored("debug.trail_stops", Category::Completion, "Ember Trail stops", Kind::Int { min: 1, max: 10_000 }, "How many of a debug session's stops the Ember Trail keeps, oldest first to go. Kept in memory only.").with_default("200"),
        // The test explorer (docs/plans/test-explorer.md), being built
        // behind `tests.enabled`.
        Setting::stored("tests.enabled", Category::Completion, "Test explorer (being built)", Kind::Bool, "Finds and runs tests through the test providers of extensions that have them (extension API 1.6), their output in Test output (tests.output). Being built: its page, the gutter, results after a test's line, failures as problems, debugging a test (SPC T d), and SPC p r, SPC p T, the lenses and the agents' run_test through it where a provider serves the file are in. Off: nothing is scanned, listed or run for it, none of it is offered, and every key and lens is as before (keys of yours under SPC T stay yours).").with_default("off"),
        Setting::stored("tests.list_automatically", Category::Completion, "List tests automatically", Kind::Bool, "With the test explorer on, in a trusted project: list a project's tests with its runner after a run that built what the listing needs, and when a provider says it changed. A listing runs project code (it builds or imports the tests). Off: only when asked.").with_default("on"),
        Setting::stored("tests.on_save", Category::Completion, "Run tests on save", Kind::Choice(&["off", "file", "failed"]), "With the test explorer on, in a trusted project: a second after a save, run the saved file's tests, or those that failed again.").with_default("off"),
        Setting::stored("tests.problems", Category::Completion, "Test failures as problems", Kind::Bool, "With the test explorer on: a failed test's failure is a problem (an error) where it failed, else at the test's line, beside the language servers' and extensions' own. Cleared when the test runs again or its file is saved.").with_default("on"),
        Setting::stored("tests.inline", Category::Completion, "Test results after the line", Kind::Choice(&["all", "failed", "off"]), "With the test explorer on: after a test's first line, a failed or errored one's message (failed), and a passed one's time too (all).").with_default("failed"),
        Setting::stored("tests.gutter", Category::Completion, "Test marks in the gutter", Kind::Bool, "With the test explorer on: in a file a test provider found tests in, a cell left of the line numbers with each test's state on its first line; a click runs it. Off: no cell.").with_default("on"),
        Setting::stored("tests.timeout_minutes", Category::Completion, "Stop a test run after", Kind::Int { min: 0, max: 1440 }, "With the test explorer on: a test run that has gone on this many minutes is stopped, its unfinished tests errored. 0: never.").with_default("0"),
        Setting::stored("tests.keep_results", Category::Completion, "Keep test results", Kind::Bool, "With the test explorer on: each project's last test results are kept across restarts, in Fenix's state folder, and shown stale until the tests run again.").with_default("on"),
        s("workspaces", Documents, "Workspace shelf", Kind::Map { key: "Name", value: "Opens", paths: false }, "SPC TAB f: git, jira, containers, vnc:HOST, project:PATH, or nothing.", field!(workspaces, map_get, map_set)),
        // Windows & session
        s("session.restore_windows", Session, "Reopen windows", Kind::Bool, "Put Fenix's windows back where they were, on the monitors they were on.", field!(restore_windows, bool_get, bool_set)).default("on").restart(),
        s("session.restore_session", Session, "Reopen workspaces", Kind::Bool, "Reopen the workspaces and files you had open.", field!(restore_session, bool_get, bool_set)).default("on").restart(),
        Setting::stored("updates.check", Session, "Look for a newer Fenix", Kind::Bool, "Once a day, ask GitHub whether there's a newer release of Fenix, and say so on Home. Nothing is downloaded. app.check_for_updates looks whenever you ask, this on or off.").with_default("off"),
        Setting::stored("updates.channel", Session, "Update channel", Kind::Choice(&["stable", "nightly"]), "Which builds the update check tells you about. Stable: releases alone. Nightly: also the nightly build of master (1.6.0-nightly.YYYYMMDD), for trying what's next before it's released; a release newer than your nightly is still offered.").with_default("stable"),
        s("session.workspace_per_project", Session, "A workspace per project", Kind::Bool, "Opening a project gives it a workspace of its own.", field!(workspace_per_project, bool_get, bool_set)).default("on"),
        // Extensions
        Setting::stored("extensions.store", Category::Extensions, "The store", Kind::Bool, "Whether Fenix may look up and download extensions from the registry. Off on a machine that installs only from files (fenix --install-extension x.fenixext).").with_default("on"),
        // Agents
        Setting::stored("agents.default", Category::Agents, "Default agent", Kind::Text, "The agent a new thread talks to: claude, codex, devin, gemini, or one of yours. Empty: the first one found.").project(),
        Setting::stored("agents.servers", Category::Agents, "Agents", Kind::Map { key: "Name", value: "Command", paths: false }, "Agents to start, as the command line that runs each over ACP. A built-in's name (claude, codex, devin, gemini) replaces it."),
        Setting::stored("agents.env", Category::Agents, "Environment", Kind::Map { key: "Variable", value: "Value", paths: false }, "Environment variables every agent gets: a proxy, a region."),
        Setting::stored("agents.secrets", Category::Agents, "Secrets", Kind::List, "Environment variables filled from the keychain for every agent (ANTHROPIC_API_KEY, OPENAI_API_KEY, WINDSURF_API_KEY): SPC a s sets one."),
        Setting::stored("agents.write", Category::Agents, "Edits through Fenix", Kind::Choice(&["save", "buffer"]), "An agent's edits made through Fenix are saved as they're made (save), so its builds see them, or left unsaved for you (buffer).").with_default("save"),
        Setting::stored("agents.save_before_prompt", Category::Agents, "Save before a prompt", Kind::Choice(&["ask", "always", "never"]), "Agents read files from disk: save the project's changed files before a prompt goes out.").with_default("ask"),
        Setting::stored("agents.follow", Category::Agents, "Follow the agent", Kind::Bool, "The pane beside the agent's thread shows each file it works on.").with_default("off"),
        Setting::stored("agents.panel_width", Category::Agents, "Panel width", Kind::Int { min: 20, max: 80 }, "How much of the width the agent's thread takes, in percent.").with_default("38"),
        Setting::stored("agents.idle_minutes", Category::Agents, "Stop idle agents after", Kind::Minutes, "An agent with no thread open is stopped after this long.").with_default("10"),
        Setting::stored("agents.notify", Category::Agents, "Ask for attention", Kind::Bool, "When Fenix isn't in front: a permission question, or a turn ended, asks for your attention.").with_default("on"),
        Setting::stored("agents.usage.modeline", Category::Agents, "Usage on the modeline", Kind::Choice(&["off", "tokens", "cost", "all"]), "What the modeline says each agent used this session: its tokens, its cost (where the agent says), or both with its turns and time. An agent that reports neither shows its turns and time. SPC a U has the days.").with_default("tokens"),
        Setting::stored("agents.usage.daily_budget", Category::Agents, "Daily budget", Kind::Text, "What the agents may use in a day before Fenix says so, at 80% and again at 100%: a cost ($5) or tokens (2M, 500k). Empty: no budget."),
        Setting::stored("agents.mcp.fenix", Category::Agents, "Offer Fenix's tools", Kind::Bool, "Agents get Fenix as an MCP server (fenix --mcp): diagnostics, language servers' answers, open files as they stand, the debugger's stop.").with_default("on"),
        Setting::stored("agents.mcp.fenix_brief", Category::Agents, "Tell agents to prefer Fenix's tools", Kind::Bool, "Agents Fenix starts are told to prefer its tools, in a note before a new session's first prompt, which isn't shown in the thread.").with_default("on"),
        Setting::stored("agents.mcp.allow", Category::Agents, "What Fenix's tools may do", Kind::Map { key: "Class, toolset:name or tool", value: "allow, ask or deny", paths: false }, "Rules for the tools agents call through Fenix: a class (read, show, edit, run, external), toolset:<name>, or a tool's name, each allow, ask or deny; the most specific wins. By default reading and showing are allowed, edits and running are asked about, external is refused. client:<rule> is for MCP clients Fenix didn't start. A project's rules can only be stricter.").project(),
        Setting::stored("agents.mcp.toolsets", Category::Agents, "Toolsets offered", Kind::List, "Fenix's toolsets agents are shown, beyond those that fit the project: +name adds one (+commands, +debug, an extension's), -name takes one away. Agents can turn on the others themselves (fenix_toolsets).").project(),
        Setting::stored("agents.mcp.http", Category::Agents, "MCP over HTTP", Kind::Text, "An address on this machine (127.0.0.1:7357) where Fenix serves its MCP tools over HTTP, for MCP clients that don't start fenix --mcp themselves. They present the token fenix mcp token prints, and follow the client: rules. Empty: off.").restart(),
        Setting::stored("agents.mcp.max_answer_kib", Category::Agents, "Largest tool answer", Kind::Int { min: 4, max: 1024 }, "The most a Fenix tool answers an agent, in KiB: a longer diff, log or output is cut, saying how to get the rest.").with_default("64"),
        Setting::stored("agents.mcp.fenix_auto_read", Category::Agents, "Let agents read through Fenix without asking (older)", Kind::Bool, "Fenix 1.0's switch, still read: off makes agents ask to read (read = ask in agents.mcp.allow). Use agents.mcp.allow instead.").with_default("on"),
        Setting::stored("agents.mcp.fenix_write", Category::Agents, "Let agents run Fenix's commands (older)", Kind::Bool, "Fenix 1.0's switch, still read: on lets agents edit and run without asking (edit = allow, run = allow in agents.mcp.allow), and lets other control clients run tasks and commands. Use agents.mcp.allow instead.").with_default("off"),
        Setting::stored("agents.mcp.servers", Category::Agents, "MCP servers", Kind::Map { key: "Name", value: "Command", paths: false }, "Your own MCP servers, handed to every agent session: the command line that starts each, or the https:// address of one that's running (sse+https:// for SSE), with any headers after it as Name=value."),
        Setting::stored("agents.terminal.output_limit", Category::Agents, "Terminal output kept", Kind::Int { min: 16, max: 65536 }, "The most of a command's output an agent's terminal keeps, in KiB.").with_default("1024"),
        Setting::stored("agents.handoff_kib", Category::Agents, "Hand-off size", Kind::Int { min: 4, max: 1024 }, "How much of a thread's transcript goes with a hand-off to another agent, in KiB.").with_default("64"),
        Setting::stored("agents.ask.agent", Category::Agents, "Agent for extensions' questions", Kind::Text, "The agent that answers an extension's question (agents.ask) when it doesn't name one: a commit message, a pull request's description. Empty: the default agent.").project(),
        Setting::stored("agents.ask.model", Category::Agents, "Model for extensions' questions", Kind::Text, "The model those questions go to, when the agent offers a choice (haiku, sonnet): a quick, cheap one for drafts. Empty: the agent's own.").project(),
        Setting::stored("agents.ask.show", Category::Agents, "Show extensions' questions", Kind::Choice(&["thread", "hidden"]), "Whether an extension's question to an agent is kept as a thread on SPC a t (thread), or gone once it's answered (hidden), when the extension doesn't say.").with_default("thread"),
        Setting::stored("agents.registry", Category::Agents, "Agent registry", Kind::Text, "The ACP registry agents are installed from. Empty: none.").with_default("https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json"),
        // Notifications
        Setting::stored("notifications.enabled", Category::Notifications, "Notifications", Kind::Bool, "Toasts in a corner of the window for what finished or wants you: an agent's turn, a task, an extension's news. Off, they're only kept in the history (SPC o n).").with_default("on"),
        Setting::stored("notifications.position", Category::Notifications, "Where they show", Kind::Choice(&["top-right", "bottom-right", "top-left", "bottom-left"]), "The corner the toasts stack in, the newest nearest it.").with_default("top-right"),
        Setting::stored("notifications.max_visible", Category::Notifications, "At once", Kind::Int { min: 1, max: 8 }, "How many toasts show at once; the rest wait their turn.").with_default("4"),
        Setting::stored("notifications.width", Category::Notifications, "Width", Kind::Int { min: 24, max: 120 }, "A toast's width, in characters.").with_default("46"),
        Setting::stored("notifications.timeout_low", Category::Notifications, "Low ones show for", Kind::Int { min: 1, max: 600 }, "Seconds a low-urgency toast shows, counted only while Fenix is in front.").with_default("4"),
        Setting::stored("notifications.timeout_normal", Category::Notifications, "Normal ones show for", Kind::Int { min: 1, max: 600 }, "Seconds a normal toast shows, counted only while Fenix is in front.").with_default("8"),
        Setting::stored("notifications.timeout_critical", Category::Notifications, "Critical ones show for", Kind::Int { min: 0, max: 3600 }, "Seconds a critical toast shows; 0 keeps it until you dismiss it (SPC o x).").with_default("0"),
        Setting::stored("notifications.system", Category::Notifications, "The system's notifications", Kind::Choice(&["unfocused", "always", "never"]), "Also show each one as the system's own notification (notify-send on Linux, a Windows toast, macOS's): only while Fenix isn't in front, always, or never.").with_default("unfocused"),
        Setting::stored("notifications.rules", Category::Notifications, "Rules", Kind::Map { key: "Source (agents, tasks, tests, an extension's id, fenix.*)", value: "mute, history, low, normal or critical", paths: false }, "What to do with a source's notifications: mute them, keep them to the history, or show them at another urgency. A source ending in * matches every one it begins."),
        Setting::stored("notifications.agents", Category::Notifications, "Agents", Kind::Choice(&["all", "unfocused", "never"]), "Notify when an agent's turn ends or it asks you something: always, only when its thread isn't in front of you, or never.").with_default("unfocused"),
        Setting::stored("notifications.tasks", Category::Notifications, "Tasks", Kind::Choice(&["all", "long", "failed", "never"]), "Notify when a task or a test run ends: every one, those that took 10 seconds or more, only those that failed, or never.").with_default("long"),
        Setting::stored("extensions.registry", Category::Extensions, "Registry", Kind::Text, "Where the registry's index.json is: Fenix's own, or a mirror of it. The index must be signed with Fenix's registry key either way.").with_default("https://raw.githubusercontent.com/tpedneault/fenix-extensions/registry/index.json"),
    ]
});

/// Settings features registered (`register`), after core's.
static REGISTERED: std::sync::RwLock<Vec<&'static Setting>> = std::sync::RwLock::new(Vec::new());

/// Adds a feature's settings to the schema, so they're loaded, checked,
/// saved, shown on the settings page and documented like core's. Done at
/// startup, before the settings are read; a key already declared is left
/// as it was.
pub fn register(settings: &'static [Setting]) {
    let mut registered = REGISTERED.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    for setting in settings {
        if !SETTINGS.iter().any(|s| s.key == setting.key) && !registered.iter().any(|s| s.key == setting.key) {
            registered.push(setting);
        }
    }
}

/// Every setting: core's in page order, then the features'.
pub fn settings() -> Vec<&'static Setting> {
    let registered = REGISTERED.read().unwrap_or_else(|poisoned| poisoned.into_inner());
    SETTINGS.iter().chain(registered.iter().copied()).collect()
}

/// The setting at `key`.
pub fn setting(key: &str) -> Option<&'static Setting> {
    settings().into_iter().find(|s| s.key == key)
}

/// Every setting as a Markdown table, by category -- the copy in docs/CONFIGURATION.md,
/// which a test keeps in step with this list.
pub fn markdown_table() -> String {
    let mut out = String::from("| Setting | Takes | Default | What it does |
|---|---|---|---|
");
    for category in Category::ALL {
        out.push_str(&format!("| **{}** | | | |
", category.label()));
        for s in settings().iter().filter(|s| s.category == category) {
            let takes = match s.kind {
                Kind::Bool => "true / false".to_string(),
                Kind::Int { min, max } => format!("{min}–{max}"),
                Kind::Float { min, max } => format!("{min}–{max}"),
                Kind::Minutes => "minutes".to_string(),
                Kind::Text | Kind::Font | Kind::Theme => "text".to_string(),
                Kind::Path => "a path".to_string(),
                Kind::Choice(c) => c.join(" / "),
                Kind::List => "a list".to_string(),
                Kind::Map { key, value, .. } => format!("{} = {}", key.to_lowercase(), value.to_lowercase()),
                Kind::Records(fields) => format!("[[tables]] of {}", fields.iter().map(|f| f.name).collect::<Vec<_>>().join(", ")),
                Kind::Secret(_) => "text (a token)".to_string(),
            };
            let mut help = s.help.replace('|', r"\|");
            if s.project {
                help.push_str(" *A project can set it.*");
            }
            if s.restart {
                help.push_str(" *Needs a restart.*");
            }
            let key = format!("`{}`", s.key);
            out.push_str(&format!("| {key} | {takes} | {} | {help} |
", if s.default.is_empty() { "–" } else { s.default }));
        }
    }
    out
}

// -- Values ----------------------------------------------------------------

impl Value {
    fn describe(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) => n.to_string(),
            Value::Text(t) => format!("\"{t}\""),
            Value::List(_) => "a list".into(),
            Value::Map(_) => "named entries".into(),
            Value::Records(_) => "a table".into(),
        }
    }

    /// As the page shows it, on one line.
    pub fn show(&self) -> String {
        match self {
            Value::Bool(true) => "on".into(),
            Value::Bool(false) => "off".into(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) if n.fract() == 0.0 => format!("{n:.0}"),
            Value::Float(n) => n.to_string(),
            Value::Text(t) if t.is_empty() => "(empty)".into(),
            Value::Text(t) => t.clone(),
            Value::List(l) => l.join(", "),
            Value::Map(m) => match m.len() {
                1 => "1 entry".into(),
                n => format!("{n} entries"),
            },
            Value::Records(r) => match r.len() {
                1 => "1 entry".into(),
                n => format!("{n} entries"),
            },
        }
    }
}

fn minutes(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Some(h) = text.strip_suffix('h') {
        return h.trim().parse::<i64>().ok().map(|h| h * 60);
    }
    text.strip_suffix('m').unwrap_or(text).trim().parse().ok()
}

impl Kind {
    /// Whether `value` is one this setting accepts, and why not.
    pub fn check(&self, value: &Value) -> Result<(), String> {
        match (self, value) {
            (Kind::Int { min, max }, Value::Int(n)) if n < min || n > max => Err(format!("a whole number from {min} to {max}")),
            (Kind::Float { min, max }, Value::Float(n)) if n < min || n > max => Err(format!("a number from {min} to {max}")),
            (Kind::Float { min, max }, Value::Int(n)) if (*n as f64) < *min || (*n as f64) > *max => Err(format!("a number from {min} to {max}")),
            (Kind::Minutes, Value::Int(n)) if *n < 1 => Err("at least 1 minute".into()),
            (Kind::Choice(choices), Value::Text(t)) if !choices.contains(&t.as_str()) => Err(format!("one of {}", choices.join(", "))),
            (Kind::Records(fields), Value::Records(rows)) => {
                for row in rows {
                    if row.len() != fields.len() {
                        return Err(format!("each entry needs {}", fields.iter().map(|f| f.label).collect::<Vec<_>>().join(", ")));
                    }
                    for (f, v) in fields.iter().zip(row) {
                        if v.trim().is_empty() && f.default.is_none() {
                            return Err(format!("{} can't be empty", f.label));
                        }
                        if !f.choices.is_empty() && !v.trim().is_empty() && !f.choices.contains(&v.trim()) {
                            return Err(format!("{}: one of {}", f.label, f.choices.join(", ")));
                        }
                        if let Some((min, max)) = f.range {
                            match v.trim().parse::<i64>() {
                                Ok(n) if n >= min && n <= max => {}
                                _ => return Err(format!("{}: a whole number from {min} to {max}", f.label)),
                            }
                        }
                    }
                }
                Ok(())
            }
            (Kind::Map { .. }, Value::Map(entries)) => {
                let mut seen = std::collections::HashSet::new();
                for (k, _) in entries {
                    if k.trim().is_empty() {
                        return Err("an entry needs a name".into());
                    }
                    if !seen.insert(k) {
                        return Err(format!("\"{k}\" is there twice"));
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// A value typed on the page, for a setting that takes one line.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let t = text.trim();
        let value = match self {
            Kind::Bool => match t {
                "on" | "true" | "yes" => Value::Bool(true),
                "off" | "false" | "no" => Value::Bool(false),
                _ => return Err("on or off".into()),
            },
            Kind::Int { min, max } => Value::Int(t.parse().map_err(|_| format!("a whole number from {min} to {max}"))?),
            Kind::Float { min, max } => Value::Float(t.parse().map_err(|_| format!("a number from {min} to {max}"))?),
            Kind::Minutes => Value::Int(minutes(t).ok_or("a number of minutes, like 5, 5m or 1h")?),
            Kind::List => Value::List(crate::names(t)),
            _ => Value::Text(text.to_string()),
        };
        self.check(&value)?;
        Ok(value)
    }

    /// Reads it from the file.
    pub(crate) fn read(&self, item: &Item) -> Result<Value, String> {
        let got = |what: &str| format!("expected {what}, got {}", item.type_name());
        match self {
            Kind::Bool => item.as_bool().map(Value::Bool).ok_or_else(|| got("true or false")),
            Kind::Int { .. } => item.as_integer().map(Value::Int).ok_or_else(|| got("a whole number")),
            Kind::Float { .. } => item.as_float().or_else(|| item.as_integer().map(|n| n as f64)).map(Value::Float).ok_or_else(|| got("a number")),
            Kind::Minutes => item.as_integer().or_else(|| item.as_str().and_then(minutes)).map(Value::Int).ok_or_else(|| got("a number of minutes")),
            Kind::Text | Kind::Path | Kind::Choice(_) | Kind::Theme | Kind::Font | Kind::Secret(_) => item.as_str().map(|s| Value::Text(s.to_string())).ok_or_else(|| got("text in quotes")),
            Kind::List => match item.as_array() {
                Some(a) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| "every entry should be text in quotes".to_string())).collect::<Result<_, _>>().map(Value::List),
                None => item.as_str().map(|s| Value::List(crate::names(s))).ok_or_else(|| got("a list, like [\"a\", \"b\"]")),
            },
            Kind::Map { .. } => {
                let table = item.as_table_like().ok_or_else(|| got("a table of names"))?;
                table
                    .iter()
                    .map(|(k, v)| match v.as_str() {
                        Some(s) => Ok((k.to_string(), s.to_string())),
                        None => Err(format!("{k}: expected text in quotes")),
                    })
                    .collect::<Result<_, _>>()
                    .map(Value::Map)
            }
            Kind::Records(fields) => {
                let rows = item.as_array_of_tables().ok_or_else(|| got("a list of [[tables]]"))?;
                rows.iter()
                    .map(|row| {
                        fields
                            .iter()
                            .map(|f| match row.get(f.name) {
                                Some(v) => v.as_str().map(str::to_string).or_else(|| v.as_integer().map(|n| n.to_string())).ok_or_else(|| format!("{}: expected text or a number", f.name)),
                                None => f.default.map(str::to_string).ok_or_else(|| format!("an entry is missing {}", f.name)),
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<_, _>>()
                    .map(Value::Records)
            }
        }
    }

    /// Writes it for the file.
    pub(crate) fn write(self, v: &Value) -> Item {
        match (self, v) {
            (_, Value::Bool(b)) => value(*b),
            (_, Value::Int(n)) => value(*n),
            (_, Value::Float(n)) if n.fract() == 0.0 && n.abs() < 1e15 => value(*n as i64),
            (_, Value::Float(n)) => value(*n),
            (_, Value::Text(t)) => value(t.as_str()),
            (_, Value::List(l)) => value(l.iter().map(String::as_str).collect::<Array>()),
            (_, Value::Map(m)) => {
                let mut table = Table::new();
                for (k, v) in m {
                    table.insert(k, value(v.as_str()));
                }
                Item::Table(table)
            }
            (Kind::Records(fields), Value::Records(rows)) => {
                let mut array = ArrayOfTables::new();
                for row in rows {
                    let mut table = Table::new();
                    for (f, v) in fields.iter().zip(row) {
                        if f.default == Some(v.as_str()) {
                            continue;
                        }
                        match (f.range, v.parse::<i64>()) {
                            (Some(_), Ok(n)) => table.insert(f.name, value(n)),
                            _ => table.insert(f.name, value(v.as_str())),
                        };
                    }
                    array.push(table);
                }
                Item::ArrayOfTables(array)
            }
            (_, Value::Records(rows)) => value(rows.iter().map(|r| InlineTable::from_iter(r.iter().enumerate().map(|(i, v)| (i.to_string(), v.as_str())))).collect::<Array>()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every category having a setting is checked where the features'
    /// are registered too (fenix-gui's `app::feature` tests).
    #[test]
    fn every_key_is_unique() {
        let mut keys: Vec<&str> = settings().iter().map(|s| s.key).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "a key declared twice");
    }

    #[test]
    fn typed_values_are_checked_against_the_setting() {
        let size = setting("editor.font_size").unwrap();
        assert_eq!(size.kind.parse("18"), Ok(Value::Float(18.0)));
        assert!(size.kind.parse("90").unwrap_err().contains("6 to 48"));
        assert!(size.kind.parse("16px").is_err());
        let fetch = setting("git.auto_fetch").unwrap();
        assert_eq!(fetch.kind.parse("1h"), Ok(Value::Int(60)));
        assert!(fetch.kind.parse("five minutes").unwrap_err().contains("minutes"));
        let layout = setting("git.layout").unwrap();
        assert!(layout.kind.parse("tabs").unwrap_err().contains("page, panes"));
    }
}

#[cfg(test)]
mod registered {
    use super::*;

    static PORTS: [Setting; 1] = [Setting::stored("test.ports", Category::Extensions, "Ports", Kind::Records(&[Field { name: "port", label: "Port", range: Some((1, 65535)), default: Some("5900"), choices: &[] }]), "Test ports.")];

    #[test]
    fn a_registered_setting_is_loaded_checked_saved_and_kept_by_key() {
        register(&PORTS);
        register(&PORTS);
        assert_eq!(settings().iter().filter(|s| s.key == "test.ports").count(), 1, "registered once");
        let ports = setting("test.ports").unwrap();
        assert!(ports.set(&mut Config::empty("s.toml".into(), "state".into()), Some(Value::Records(vec![vec!["70000".into()]]))).unwrap_err().contains("Port"));
        let dir = std::env::temp_dir().join(format!("fenix-config-registered-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.toml");
        let mut config = Config::load(path.clone()).unwrap();
        ports.set(&mut config, Some(Value::Records(vec![vec!["5901".into()]]))).unwrap();
        assert_eq!(config.value("test.ports"), Some(&Value::Records(vec![vec!["5901".into()]])));
        config.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[test.ports]]") && text.contains("port = 5901"), "{text}");
        let reloaded = Config::load(path).unwrap();
        assert_eq!(reloaded.value("test.ports"), Some(&Value::Records(vec![vec!["5901".into()]])));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
