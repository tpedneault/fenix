//! Fenix's settings: `settings.toml` (`%AppData%\fenix\settings.toml` on
//! Windows, `~/.config/fenix/settings.toml` on Linux), API tokens
//! included, with window placement and explorer bookmarks in state files
//! beside the other things Fenix remembers about this machine (see
//! `fenix_storage::paths`).
//!
//! Every field is `Option<T>` (or an empty list): "set in the file" vs
//! not. What a default *is* lives with the code that uses the setting,
//! in crates this one can't depend on, so each consumer supplies its own
//! with `.unwrap_or(...)`; `schema` only says how the default reads on
//! the settings page. A bad value costs only that setting, and says why
//! in `problems`.
//!
//! `schema` declares every setting once; loading, saving, the settings
//! page and the table in docs/CONFIGURATION.md all come from it. Saving
//! changes only the lines of the settings that changed (`toml_edit`), so
//! comments and hand edits survive it.

mod ini;
mod legacy;
mod project;
pub mod schema;
pub mod secrets;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod keybindings;
pub use schema::{setting, settings, Category, Field, Kind, Setting, Value};
pub use project::ProjectSettings;
pub use secrets::Secret;

/// `[motion]`. Each feature unset follows `level`: `subtle` turns on
/// the short, informative ones, `full` adds the rest. `editor.animations
/// = false` still turns every one of them off.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MotionSettings {
    /// `off`, `subtle` or `full`.
    pub level: Option<String>,
    /// Multiplies every duration: 2.0 is half as fast.
    pub speed: Option<f32>,
    pub caret_fade: Option<bool>,
    pub smooth_scroll: Option<bool>,
    pub scroll_ms: Option<u64>,
    pub yank_pulse: Option<bool>,
    pub beacon: Option<bool>,
    pub beacon_ms: Option<u64>,
    pub beacon_on_focus: Option<bool>,
    pub change_pulse: Option<bool>,
    pub popups: Option<bool>,
    pub messages: Option<bool>,
    pub error_flash: Option<bool>,
    pub progress: Option<bool>,
    pub mode_fade: Option<bool>,
    pub tabs: Option<bool>,
    pub caret_glide: Option<bool>,
    pub glide_ms: Option<u64>,
    pub folds: Option<bool>,
    pub layout: Option<bool>,
    pub theme_fade: Option<bool>,
    pub splash: Option<bool>,
}

/// The static polish, under `[appearance]` and `[diagnostics]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Polish {
    pub corner_radius: Option<usize>,
    pub shadows: Option<bool>,
    pub overview_ruler: Option<bool>,
    pub sticky_scroll: Option<bool>,
    pub sticky_lines: Option<usize>,
    pub indent_guides: Option<bool>,
    pub active_indent_guide: Option<bool>,
    pub rounded_selection: Option<bool>,
    pub selection_whitespace: Option<bool>,
    pub dim_unfocused: Option<bool>,
    /// How much an unfocused pane is dimmed, in percent.
    pub dim_amount: Option<usize>,
    pub rainbow_brackets: Option<bool>,
    /// `all`, `errors` or `off`.
    pub diagnostics_inline: Option<String>,
    /// `cursor`, `all` or `off`: which lines show their message.
    pub diagnostics_message: Option<String>,
    pub diagnostics_delay_ms: Option<u64>,
    /// `auto`, `block`, `underline` or `off`: how pane tabs are drawn.
    pub tabs: Option<String>,
    /// How long a leader key waits before the which-key menu shows.
    pub which_key_delay_ms: Option<u64>,
    /// `key` or `label`: how the menu orders its keys.
    pub which_key_order: Option<String>,
}

pub struct Config {
    path: PathBuf,
    /// Where window placement and explorer bookmarks are kept.
    state_dir: PathBuf,
    /// The file as last read or written, comments and all.
    doc: RefCell<toml_edit::DocumentMut>,
    /// Each setting's value when the file was last read or written, so a
    /// save writes only what Fenix changed.
    baseline: RefCell<HashMap<&'static str, Option<schema::Value>>>,
    /// The file couldn't be parsed, so it mustn't be written over.
    broken: bool,
    /// What's wrong in the file, if anything.
    pub problems: Vec<Problem>,
    pub theme: Option<String>,
    pub font_size: Option<f32>,
    pub font_family: Option<String>,
    pub indent_width: Option<usize>,
    /// How many visual columns a literal `\t` character expands to when
    /// rendered (real Vim's own `:set tabstop`) -- distinct from
    /// `indent_width`, which governs what typing Tab in Insert mode or
    /// `>>`/`<<` actually *inserts* (always spaces; Fenix never inserts
    /// a real tab character itself). Consulted by `fenix-gui`'s
    /// `tabstops` module.
    /// The extra (beyond alphanumeric) word-class characters real Vim's
    /// `'iskeyword'` option holds, exactly as `:set iskeyword=...` would
    /// list them -- literal characters concatenated with no delimiter
    /// (`"_"` is the default fenix-vim itself falls back to when this is
    /// `None`, `""` is the user's *explicit* choice of no extras at
    /// all). Runtime-changed via `:set iskeyword=...`/`+=`/`-=`
    /// (`fenix_vim::VimEvent::IsKeywordChanged`) and persisted the same
    /// way `indent_width` already is.
    pub iskeyword_extra: Option<String>,
    pub tab_width: Option<usize>,
    /// Whether caret-fade, scroll-ease, and yank/paste-pulse animations
    /// play at all -- `None`/unset means "on" (the default look); `false`
    /// snaps every one of them straight to its end state instead, for a
    /// user who wants to rule animation cost in/out of a responsiveness
    /// complaint, or who just prefers snappier motion.
    pub animations: Option<bool>,
    /// `[motion]`: how much moves, feature by feature.
    pub motion: MotionSettings,
    /// The static polish: corners, the overview ruler, sticky scroll,
    /// dimming, inline diagnostics and the rest.
    pub polish: Polish,
    /// Whether a jump (`gd`, a search result, a symbol) opens in one
    /// reusable preview tab until it's edited or kept. `None` means on.
    pub preview_tab: Option<bool>,
    pub completion_symbols_file: Option<PathBuf>,
    /// Whether the snippets that come with Fenix are offered; yours and
    /// a project's always are.
    pub snippets_builtin: Option<bool>,
    /// Configured language server commands, `(language, command_line)`
    /// -- `[lsp]`'s `serverN = LANGUAGE|COMMAND_LINE`, same numbered-key
    /// list convention `mib_roots`/`jira_projects` already established.
    /// `LANGUAGE` matches `fenix_syntax::LanguageId`'s own name (e.g.
    /// `python`, `rust`); `COMMAND_LINE` is a plain space-separated
    /// program-plus-arguments string (e.g. `pyright-langserver
    /// --stdio`), split on whitespace at the point of use -- no shell
    /// quoting support, since every server this actually needs to
    /// launch takes simple flag-only arguments. A language with no
    /// entry here falls back to `fenix_lsp`'s own built-in default
    /// command for it, so the common case (the obvious server already
    /// on `PATH`) needs no configuration at all; this section exists
    /// for anyone who wants a different server, extra flags, or a
    /// language `fenix-lsp` has no built-in default for yet. Hand-
    /// authored by the user -- same re-read-fresh-before-writing
    /// protection as `vnc_hosts`/`documents`/`workspaces` (see `save`'s
    /// own doc comment), since nothing in this app writes to it itself.
    pub lsp_servers: Vec<(String, String)>,
    /// Whether to notice files changing on disk while they're open and
    /// re-read them. Unset means on, which is what anyone expects; the
    /// setting exists for a working copy on a network share, where a
    /// `stat` every couple of seconds per open file is not free.
    pub watch_files: Option<bool>,
    /// The self-hosted Jira instance's own REST API root (e.g.
    /// `https://jira.mycompany.com`), and a personal access token for
    /// it -- plaintext, same as every other setting in this file (a
    /// deliberate choice, not an oversight: `fenix-jira`'s own design
    /// notes cover the tradeoff against an OS credential store, which is
    /// planned).
    pub jira_base_url: Option<String>,
    pub jira_token: Option<String>,
    /// The GitLab instance's own root URL (e.g.
    /// `https://gitlab.mycompany.com` -- the instance, *not* `/api/v4`,
    /// which `fenix-gitlab` appends itself) and a personal access token
    /// with `api` scope. Plaintext, same tradeoff `jira_token` already
    /// documents.
    ///
    /// There is deliberately no project setting: which project a repo
    /// belongs to is read from its own `origin` remote, so one pair of
    /// values covers every repo on the instance.
    pub gitlab_base_url: Option<String>,
    pub gitlab_token: Option<String>,
    /// `[github] token`: used when the GitHub CLI isn't signed in.
    pub github_token: Option<String>,
    /// Frequently-read documents, `(display name, path)`, in the order
    /// they appear in `config.ini`'s `[documents]` section -- what the
    /// reader's `SPC r f` index picks from. Same numbered-key `docN =
    /// NAME|PATH` convention (and same `Vec`-not-`Option` reasoning) as
    /// `mib_roots`; the path can be any file Fenix can open, not just a
    /// PDF, though a reference shelf is mostly PDFs in practice.
    /// Hand-authored by the user, never written by the app itself --
    /// `save` re-reads this section fresh from disk right before
    /// writing rather than trusting this struct's own (possibly
    /// session-old) copy, so a hand-edit made after this was loaded
    /// survives the next save instead of being silently erased by it.
    pub documents: Vec<(String, PathBuf)>,
    /// How many commits the History view's graph loads (`SPC g l`) --
    /// unset means 200, enough to cover recent work without making
    /// `git log --all` on a large repo feel slow.
    /// Directories worth a key rather than a walk -- `[explorer]`'s
    /// `bookmarkN = NAME|PATH`, the same numbered-pair convention
    /// `mib_roots`/`documents` already use.
    ///
    /// Driven by `self` on save (not re-read fresh like `vnc_hosts`),
    /// because unlike those this one genuinely has an in-app add flow:
    /// `SPC e m` bookmarks wherever you are. Hand-editing the file is
    /// still fine; it just has to happen between sessions, the same
    /// deal `mib_roots` has.
    pub explorer_bookmarks: Vec<(String, PathBuf)>,
    pub git_graph_limit: Option<usize>,
    /// The branch ref-comparison defaults its base to (`SPC g c`), e.g.
    /// `develop` -- unset means `main`. What "how does my branch differ
    /// from the mainline" means depends on the project's own convention,
    /// and there's no way to infer it reliably.
    pub git_base_branch: Option<String>,
    /// How the History view draws its commit graph: `ascii` (default)
    /// or `unicode`. Unicode looks better *if* the configured font has
    /// the box-drawing glyphs; when it doesn't, the fallback font's
    /// different advance width knocks every row out of alignment, which
    /// is why it isn't the default (see `graph_view::GraphStyle`).
    pub git_graph_style: Option<String>,
    /// `SPC g g`: `page` (default) opens the Git status page; `panes`
    /// keeps the older seven-pane panel.
    pub git_layout: Option<String>,
    /// `[git] auto_fetch = 5m`: fetch the focused repository in the
    /// background when its last fetch is older than this many minutes.
    /// Off unless set.
    pub git_auto_fetch_minutes: Option<u64>,
    /// `[git] reviewers = alex, sam`: who a new pull request asks for a
    /// review, prefilled on its page (`SPC g P`). A project's own
    /// `.fenix/project.ini` `[git] reviewers` takes its place.
    pub git_reviewers: Vec<String>,
    /// Settings features registered (`schema::register`), by key -- see
    /// `value`.
    values: std::collections::BTreeMap<String, Value>,
    /// Where each OS window sat when Fenix last exited, in the order
    /// they were open -- restored on the next launch when
    /// `restore_windows` is on. Unlike every other list here this one
    /// is written by the app, not hand-authored, so a two-monitor
    /// setup comes back the way it was left without arranging it
    /// again each morning.
    pub windows: Vec<WindowLayout>,
    /// Whether to reopen the windows recorded in `windows` at startup.
    /// `None` means on -- restoring what you had is the behaviour
    /// worth defaulting to; `restore_windows = false` opts out and
    /// always starts with a single window.
    pub restore_windows: Option<bool>,
    /// Restore documents and layouts; enabled unless explicitly disabled.
    pub restore_session: Option<bool>,
    /// Whether opening a project from the hub gives it a workspace of
    /// its own (and returns to it next time). `None` means on.
    pub workspace_per_project: Option<bool>,
    /// Built-in extensions switched off, by id (`fenix.jira`). Read at
    /// startup (`disabled_extensions_in`), so a change takes a restart.
    pub extensions_disabled: Vec<String>,
    /// Named workspace launchers, `(display name, action)`, in the
    /// order they appear in `config.ini`'s `[workspaces]` section --
    /// what `SPC TAB f` picks from. `action` is one of `git`, `jira`,
    /// `docker` (opens the matching built-in panel; `docker` also
    /// covers Podman, since that panel autodetects the engine),
    /// `vnc:HOST` (a name from `[vnc]`), or anything else (including
    /// empty) for a plain workspace with no live session behind it --
    /// what "editor" would be. The actual parsing
    /// (`App::parse_workspace_action`) lives in `fenix-gui`, not here,
    /// since it dispatches to session state this crate has no business
    /// knowing about. Hand-authored by the user, same as `documents`
    /// -- including its own re-read-fresh-before-writing protection.
    pub workspaces: Vec<(String, String)>,
}

/// One OS window's remembered geometry: where its *outer* frame sat on
/// the desktop, how big its *inner* (client) area was, and whether it
/// was maximized.
///
/// That pairing isn't arbitrary -- it's the pair a window can actually
/// be restored from. `set_outer_position` and `with_inner_size` are
/// what winit offers, so recording anything else would mean converting
/// by the border and title-bar thickness on the way back, and getting
/// that wrong makes a window creep across the screen a little further
/// on every save-and-restore cycle.
///
/// Deliberately a plain rectangle rather than a monitor name plus an
/// offset into it. Monitor identifiers are long, platform-specific and
/// not stable across driver or dock changes, whereas a rectangle
/// degrades gracefully all by itself: a window whose saved position no
/// longer lands on any connected monitor just gets placed by the
/// window manager instead of opening off-screen (see
/// `App::restore_windows`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowLayout {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

/// Something wrong in `settings.toml`, as the settings page and the
/// modeline report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The setting it's about, when it's about one.
    pub key: Option<String>,
    /// 1-based.
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, &self.key) {
            (Some(line), Some(key)) => write!(f, "settings.toml:{line} -- {key}: {}", self.message),
            (Some(line), None) => write!(f, "settings.toml:{line} -- {}", self.message),
            (None, Some(key)) => write!(f, "{key}: {}", self.message),
            (None, None) => f.write_str(&self.message),
        }
    }
}

/// Walks `key`'s dotted path in `doc`.
/// `extensions.disabled` in the settings file at `path`, read on its own:
/// what's switched off has to be known before any extension's settings
/// are registered and the file is read properly. Empty when the file or
/// the key isn't there.
pub fn disabled_extensions_in(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return Vec::new() };
    lookup(&doc, "extensions.disabled").and_then(|item| item.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

pub(crate) fn lookup<'a>(doc: &'a toml_edit::DocumentMut, key: &str) -> Option<&'a toml_edit::Item> {
    let mut item = doc.as_item();
    for part in key.split('.') {
        item = item.as_table_like()?.get(part)?;
    }
    Some(item)
}

/// Sets (or, with `None`, removes) `key`'s dotted path in `doc`, making
/// the tables on the way as it goes.
pub(crate) fn store(doc: &mut toml_edit::DocumentMut, key: &str, item: Option<toml_edit::Item>) {
    let parts: Vec<&str> = key.split('.').collect();
    let (last, parents) = parts.split_last().expect("a key");
    let mut table: &mut dyn toml_edit::TableLike = doc.as_table_mut();
    for part in parents {
        if table.get(part).and_then(|i| i.as_table_like()).is_none() {
            if item.is_none() {
                return;
            }
            // Implicit: a table that only holds tables (`[lsp]` above
            // `[lsp.servers]`) gets no header of its own.
            let mut new = toml_edit::Table::new();
            new.set_implicit(true);
            table.insert(part, toml_edit::Item::Table(new));
        }
        table = table.get_mut(part).and_then(|i| i.as_table_like_mut()).expect("just made");
    }
    match item {
        Some(item) => {
            table.insert(last, item);
        }
        None => {
            table.remove(last);
        }
    }
}

/// The 1-based line `offset` falls on in `text`.
pub(crate) fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

const HEADER: &str = "# Fenix settings. SPC , edits them; so can you -- Fenix keeps your\n# comments and only ever changes the lines of the settings it changes.\n# Only what differs from the defaults needs to be here.\n\n";

impl Config {
    /// `settings.toml`.
    pub fn default_path() -> Option<PathBuf> {
        fenix_storage::paths::settings_file()
    }

    /// Nothing set, reading from and saving to `path`, with state files
    /// in `state_dir`.
    pub fn empty(path: PathBuf, state_dir: PathBuf) -> Self {
        Config {
            path,
            state_dir,
            doc: RefCell::new(toml_edit::DocumentMut::new()),
            baseline: RefCell::new(HashMap::new()),
            broken: false,
            problems: Vec::new(),
            theme: None,
            font_size: None,
            font_family: None,
            indent_width: None,
            iskeyword_extra: None,
            tab_width: None,
            animations: None,
            motion: MotionSettings::default(),
            polish: Polish::default(),
            preview_tab: None,
            completion_symbols_file: None,
            snippets_builtin: None,
            lsp_servers: Vec::new(),
            explorer_bookmarks: Vec::new(),
            watch_files: None,
            jira_base_url: None,
            jira_token: None,
            git_graph_limit: None,
            git_base_branch: None,
            gitlab_base_url: None,
            gitlab_token: None,
            github_token: None,
            git_graph_style: None,
            git_layout: None,
            git_auto_fetch_minutes: None,
            git_reviewers: Vec::new(),
            values: std::collections::BTreeMap::new(),
            documents: Vec::new(),
            windows: Vec::new(),
            restore_windows: None,
            restore_session: None,
            workspace_per_project: None,
            extensions_disabled: Vec::new(),
            workspaces: Vec::new(),
        }
    }

    /// Reads `path`, with the state files beside it in `state/`. A file
    /// that isn't there is every setting at its default; a file that
    /// can't be parsed is the same, with the reason in `problems` -- and
    /// is never written over until it's fixed.
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let state_dir = path.parent().map(|p| p.join("state")).unwrap_or_else(|| PathBuf::from("state"));
        Self::load_at(path, state_dir)
    }

    /// Reads `path`, with the state files in `state_dir`.
    pub fn load_at(path: PathBuf, state_dir: PathBuf) -> io::Result<Self> {
        let mut config = Self::empty(path, state_dir);
        match std::fs::read_to_string(&config.path) {
            Ok(text) => config.read_toml(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        config.read_state();
        config.remember();
        Ok(config)
    }

    /// Same as `load`, but never fails: a file that can't be read at all
    /// is every setting at its default.
    pub fn load_or_default(path: PathBuf) -> Self {
        Self::load(path.clone()).unwrap_or_else(|e| {
            let state_dir = path.parent().map(|p| p.join("state")).unwrap_or_default();
            let mut config = Self::empty(path, state_dir);
            config.broken = true;
            config.problems.push(Problem { key: None, line: None, message: e.to_string() });
            config
        })
    }

    /// `load_at`, never failing.
    pub fn load_at_or_default(path: PathBuf, state_dir: PathBuf) -> Self {
        Self::load_at(path.clone(), state_dir.clone()).unwrap_or_else(|e| {
            let mut config = Self::empty(path, state_dir);
            config.broken = true;
            config.problems.push(Problem { key: None, line: None, message: e.to_string() });
            config
        })
    }

    fn read_toml(&mut self, text: &str) {
        let parsed = match text.parse::<toml_edit::ImDocument<String>>() {
            Ok(doc) => doc,
            Err(e) => {
                self.broken = true;
                let line = e.span().map(|s| line_of(text, s.start));
                self.problems.push(Problem { key: None, line, message: e.message().trim().to_string() });
                return;
            }
        };
        // Spans are only on the parsed document, so each setting's line
        // is found before it becomes the editable one.
        let lines: HashMap<&str, usize> = schema::settings()
            .iter()
            .filter_map(|s| {
                let mut item = parsed.as_item();
                for part in s.key.split('.') {
                    item = item.as_table_like()?.get(part)?;
                }
                item.span().map(|span| (s.key, line_of(text, span.start)))
            })
            .collect();
        let doc = parsed.into_mut();
        for setting in schema::settings() {
            let Some(item) = lookup(&doc, setting.key) else { continue };
            let result = setting.kind.read(item).and_then(|value| setting.set(self, Some(value)));
            if let Err(message) = result {
                self.problems.push(Problem { key: Some(setting.key.into()), line: lines.get(setting.key).copied(), message });
            }
        }
        // In the order they're in the file.
        self.problems.sort_by_key(|p| p.line.unwrap_or(usize::MAX));
        *self.doc.borrow_mut() = doc;
    }

    fn read_state(&mut self) {
        let windows = self.state_dir.join("windows.json");
        self.windows = fenix_storage::state::read(&windows, "windows").ok().flatten().unwrap_or_default();
        let bookmarks = self.state_dir.join("bookmarks.json");
        self.explorer_bookmarks = fenix_storage::state::read(&bookmarks, "explorer").ok().flatten().unwrap_or_default();
    }

    /// Notes every setting's value as the file has it, so a save knows
    /// which ones Fenix itself changed.
    fn remember(&self) {
        let mut baseline = self.baseline.borrow_mut();
        baseline.clear();
        for setting in schema::settings() {
            baseline.insert(setting.key, setting.get(self));
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Where the state files are.
    pub fn state_dir(&self) -> &std::path::Path {
        &self.state_dir
    }

    /// Whether the file couldn't be parsed; it isn't written until it is.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Saves what changed. The file is read again first and only the
    /// settings Fenix changed since it was last read are written into it
    /// -- so a hand edit made in the meantime, a comment, the order of
    /// things and any key Fenix doesn't know all stay as they are. A
    /// setting back at its default loses its line. The state files
    /// (window placement, bookmarks) are written beside it.
    pub fn save(&self) -> io::Result<()> {
        self.save_state()?;
        if self.broken {
            let why = self.problems.first().map(|p| p.to_string()).unwrap_or_default();
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("settings.toml can't be read, so it wasn't written over -- fix it first ({why})")));
        }
        let on_disk = match std::fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        // Fresh from disk when it parses, else what was loaded.
        let mut doc = match on_disk.as_deref().map(str::parse::<toml_edit::DocumentMut>) {
            Some(Ok(doc)) => doc,
            Some(Err(_)) => return Err(io::Error::new(io::ErrorKind::InvalidData, "settings.toml was changed into something that can't be read, so it wasn't written over")),
            None => {
                let mut doc = toml_edit::DocumentMut::new();
                doc.decor_mut().set_prefix(HEADER);
                doc
            }
        };
        let mut baseline = self.baseline.borrow_mut();
        for setting in schema::settings() {
            let now = setting.get(self);
            if baseline.get(setting.key) == Some(&now) {
                continue;
            }
            let same_on_disk = lookup(&doc, setting.key).and_then(|i| setting.kind.read(i).ok()) == now;
            if !same_on_disk {
                store(&mut doc, setting.key, now.as_ref().map(|v| setting.kind.write(v)));
            }
            baseline.insert(setting.key, now);
        }
        let text = doc.to_string();
        if on_disk.as_deref() != Some(text.as_str()) {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            fenix_storage::write(&self.path, text.as_bytes())?;
        }
        *self.doc.borrow_mut() = doc;
        Ok(())
    }

    fn save_state(&self) -> io::Result<()> {
        let windows = self.state_dir.join("windows.json");
        if !self.windows.is_empty() || windows.exists() {
            fenix_storage::state::write(&windows, "windows", &self.windows)?;
        }
        let bookmarks = self.state_dir.join("bookmarks.json");
        if !self.explorer_bookmarks.is_empty() || bookmarks.exists() {
            fenix_storage::state::write(&bookmarks, "explorer", &self.explorer_bookmarks)?;
        }
        Ok(())
    }

    /// Reads the file again after it changed on disk. When it parses, its
    /// settings replace these; when it doesn't, these stay in use and
    /// `problems` says why. State is left alone.
    pub fn reload(&mut self) {
        let mut fresh = Self::empty(self.path.clone(), self.state_dir.clone());
        match std::fs::read_to_string(&self.path) {
            Ok(text) => fresh.read_toml(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                self.problems = vec![Problem { key: None, line: None, message: e.to_string() }];
                return;
            }
        }
        if fresh.broken {
            self.problems = fresh.problems;
            return;
        }
        for setting in schema::settings() {
            // Checked already, so this can't fail.
            let _ = setting.set(self, setting.get(&fresh));
        }
        self.problems = fresh.problems;
        self.broken = false;
        *self.doc.borrow_mut() = fresh.doc.into_inner();
        self.remember();
    }

    /// A registered setting's value (`schema::register`); `None` when it
    /// isn't set.
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// A registered setting's text (`Kind::Text`, `Kind::Choice`).
    pub fn text_value(&self, key: &str) -> Option<&str> {
        match self.value(key) {
            Some(Value::Text(t)) => Some(t),
            _ => None,
        }
    }

    /// A registered `Kind::Path` setting.
    pub fn path_value(&self, key: &str) -> Option<PathBuf> {
        self.text_value(key).map(PathBuf::from)
    }

    /// A registered `Kind::Bool` setting.
    pub fn bool_value(&self, key: &str) -> Option<bool> {
        match self.value(key) {
            Some(Value::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// A registered whole-number setting (`Kind::Int`, `Kind::Minutes`).
    pub fn int_value(&self, key: &str) -> Option<i64> {
        match self.value(key) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    /// A registered `Kind::List` setting; empty when unset.
    pub fn list_value(&self, key: &str) -> &[String] {
        match self.value(key) {
            Some(Value::List(items)) => items,
            _ => &[],
        }
    }

    /// A registered `Kind::Map` setting, in order; empty when unset.
    pub fn map_value(&self, key: &str) -> &[(String, String)] {
        match self.value(key) {
            Some(Value::Map(entries)) => entries,
            _ => &[],
        }
    }

    /// A registered `Kind::Records` setting's rows; empty when unset.
    pub fn records_value(&self, key: &str) -> &[Vec<String>] {
        match self.value(key) {
            Some(Value::Records(rows)) => rows,
            _ => &[],
        }
    }

    /// Sets a registered setting by key, unchecked -- `Setting::set`
    /// checks it first. For code that edits one directly (tests, a
    /// migration); `None` unsets it.
    pub fn set_value(&mut self, key: &str, value: Option<Value>) {
        match value {
            Some(v) => self.values.insert(key.to_string(), v),
            None => self.values.remove(key),
        };
    }

    /// Sets a registered `Kind::Path` setting by key (`None` unsets it).
    pub fn set_path_value(&mut self, key: &str, path: Option<&std::path::Path>) {
        self.set_value(key, path.map(|p| Value::Text(p.display().to_string())));
    }

    /// A token: from the environment when it's set there, else from the
    /// file.
    pub fn token(&self, secret: Secret) -> Option<String> {
        secret.from_env().or_else(|| {
            match secret {
                Secret::GitLab => &self.gitlab_token,
                Secret::Jira => &self.jira_token,
                Secret::GitHub => &self.github_token,
            }
            .clone()
            .filter(|t| !t.trim().is_empty())
        })
    }

    /// Moves `config.ini` at `ini` into this file: every setting it had,
    /// tokens included, is written here, its window placement and
    /// bookmarks to the state files. `ini` itself isn't touched.
    pub fn migrate_ini(ini: &std::path::Path, path: PathBuf, state_dir: PathBuf) -> io::Result<Self> {
        let old = legacy::load(ini, path.clone(), state_dir.clone())?;
        let mut config = Self::load_at(path, state_dir)?;
        for setting in schema::settings() {
            // A value the old file had that the new one doesn't take (a
            // theme name with a typo, say) is dropped, not fatal.
            if let Some(value) = setting.get(&old) {
                if let Err(message) = setting.set(&mut config, Some(value)) {
                    config.problems.push(Problem { key: Some(setting.key.into()), line: None, message: format!("from config.ini: {message}") });
                }
            }
        }
        config.windows = old.windows.clone();
        config.explorer_bookmarks = old.explorer_bookmarks.clone();
        config.save()?;
        Ok(config)
    }
}

/// Usernames written with commas or spaces between them, `@` or not.
pub fn names(text: &str) -> Vec<String> {
    text.split([',', ' ']).map(|n| n.trim().trim_start_matches('@')).filter(|n| !n.is_empty()).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// `settings.toml` in a folder of its own, so its `state/` is too.
    fn temp_path(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-config-test-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("settings.toml")
    }

    #[test]
    fn documents_round_trip_through_save_and_load() {
        let path = temp_path("documents_round_trip");
        let mut config = Config::load(path.clone()).unwrap();
        config.documents = vec![
            ("Space Packet Protocol".to_string(), PathBuf::from("C:/refs/133x0b2e2.pdf")),
            ("Notes".to_string(), PathBuf::from("/refs/notes.md")),
        ];

        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();

        assert_eq!(reloaded.documents, config.documents);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn windows_round_trip_through_save_and_load() {
        let path = temp_path("windows_round_trip");
        let mut config = Config::load(path.clone()).unwrap();
        config.windows = vec![
            WindowLayout { x: 0, y: 0, width: 2560, height: 1440, maximized: true },
            // A monitor to the left of the primary one sits at a
            // negative x on Windows.
            WindowLayout { x: -1920, y: -120, width: 1920, height: 1080, maximized: false },
        ];
        config.restore_windows = Some(false);
        config.restore_session = Some(false);
        config.workspace_per_project = Some(false);

        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();

        assert_eq!(reloaded.windows, config.windows);
        assert_eq!(reloaded.restore_windows, Some(false));
        assert_eq!(reloaded.restore_session, Some(false));
        assert_eq!(reloaded.workspace_per_project, Some(false));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lsp_servers_round_trip_through_save_and_load() {
        let path = temp_path("lsp_servers_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.lsp_servers = vec![("python".to_string(), "pyright-langserver --stdio".to_string()), ("rust".to_string(), "rust-analyzer".to_string())];
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(
            reloaded.lsp_servers,
            vec![("python".to_string(), "pyright-langserver --stdio".to_string()), ("rust".to_string(), "rust-analyzer".to_string())]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn no_windows_section_means_nothing_recorded_and_no_opt_out() {
        let config = Config::load(temp_path("windows_absent")).unwrap();
        assert!(config.windows.is_empty());
        assert_eq!(config.restore_windows, None, "unset means restore, which is the default the app applies");
    }

    #[test]
    fn workspaces_round_trip_through_save_and_load() {
        let path = temp_path("workspaces_round_trip");
        let mut config = Config::load(path.clone()).unwrap();
        config.workspaces = vec![
            ("Git".to_string(), "git".to_string()),
            ("VNC Build".to_string(), "vnc:build-vm".to_string()),
            ("Editor".to_string(), String::new()),
        ];

        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();

        assert_eq!(reloaded.workspaces, config.workspaces);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn no_workspaces_section_means_an_empty_launcher_list() {
        let config = Config::load(temp_path("workspaces_absent")).unwrap();
        assert!(config.workspaces.is_empty());
    }

    #[test]
    fn loading_a_missing_file_yields_every_field_none_not_an_error() {
        let config = Config::load(temp_path("missing")).unwrap();
        assert!(config.theme.is_none());
        assert!(config.font_size.is_none());
        assert!(config.font_family.is_none());
        assert!(config.indent_width.is_none());
        assert!(config.iskeyword_extra.is_none());
        assert!(config.tab_width.is_none());
        assert!(config.animations.is_none());
        assert!(config.completion_symbols_file.is_none());
        assert!(config.value("vnc.hosts").is_none(), "a feature's setting, unset");
    }

    #[test]
    fn load_or_default_never_fails_even_when_the_path_is_unreadable() {
        let path = temp_path("unreadable");
        std::fs::create_dir_all(&path).unwrap(); // a directory, not a file -- read_to_string fails non-NotFound
        assert!(Config::load(path.clone()).is_err());
        let config = Config::load_or_default(path.clone());
        assert!(config.theme.is_none());
        std::fs::remove_dir_all(&path).ok();
    }

    #[test]
    fn all_five_fields_round_trip_through_save_and_load() {
        let path = temp_path("round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("TempleOS".to_string());
        config.font_size = Some(18.0);
        config.font_family = Some("Fira Code".to_string());
        config.indent_width = Some(2);
        config.iskeyword_extra = Some(String::new());
        config.tab_width = Some(4);
        config.completion_symbols_file = Some(PathBuf::from("/home/thomas/tcl-symbols.txt"));
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.theme, Some("TempleOS".to_string()));
        assert_eq!(reloaded.font_size, Some(18.0));
        assert_eq!(reloaded.font_family, Some("Fira Code".to_string()));
        assert_eq!(reloaded.indent_width, Some(2));
        assert_eq!(reloaded.iskeyword_extra, Some(String::new()), "an explicit empty set round-trips distinctly from unset");
        assert_eq!(reloaded.tab_width, Some(4));
        assert_eq!(reloaded.completion_symbols_file, Some(PathBuf::from("/home/thomas/tcl-symbols.txt")));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn iskeyword_extra_round_trips_a_non_trivial_value() {
        let path = temp_path("iskeyword_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.iskeyword_extra = Some("_-".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.iskeyword_extra, Some("_-".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn animations_setting_round_trips_through_save_and_load() {
        let path = temp_path("animations_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.animations = Some(false);
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.animations, Some(false));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn jira_settings_round_trip_through_save_and_load() {
        let path = temp_path("jira_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.jira_base_url = Some("https://jira.example.com".to_string());
        config.jira_token = Some("secret-token-value".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.jira_base_url, Some("https://jira.example.com".to_string()));
        assert_eq!(reloaded.jira_token, Some("secret-token-value".to_string()));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_gitlab_section_round_trips_through_a_save() {
        let path = temp_path("config_gitlab");
        let mut config = Config::load_or_default(path.clone());
        config.gitlab_base_url = Some("https://gitlab.example.com".to_string());
        config.gitlab_token = Some("glpat-secret".to_string());
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.gitlab_base_url.as_deref(), Some("https://gitlab.example.com"));
        assert_eq!(reloaded.gitlab_token.as_deref(), Some("glpat-secret"));
        // No project key: which project a repo belongs to comes from
        // its own `origin` remote, so one pair covers every repo.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[gitlab]"), "got:
{text}");
        assert!(!text.contains("project ="), "got:
{text}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn git_settings_round_trip_through_save_and_load() {
        let path = temp_path("git_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.git_graph_limit = Some(500);
        config.git_base_branch = Some("develop".to_string());
        config.git_graph_style = Some("unicode".to_string());
        config.git_layout = Some("panes".to_string());
        config.git_auto_fetch_minutes = Some(5);
        config.git_reviewers = vec!["alex".to_string(), "sam".to_string()];
        config.save().unwrap();

        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.git_graph_limit, Some(500));
        assert_eq!(reloaded.git_base_branch, Some("develop".to_string()));
        assert_eq!(reloaded.git_graph_style, Some("unicode".to_string()), "graph_style used to be dropped on save");
        assert_eq!(reloaded.git_layout, Some("panes".to_string()));
        assert_eq!(reloaded.git_auto_fetch_minutes, Some(5));
        assert_eq!(reloaded.git_reviewers, ["alex", "sam"]);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn git_settings_are_none_when_the_section_is_absent() {
        let config = Config::load(temp_path("git_absent")).unwrap();
        assert_eq!(config.git_graph_limit, None);
        assert_eq!(config.git_base_branch, None);
        assert!(config.git_reviewers.is_empty());
    }

    #[test]
    fn reviewers_are_read_with_commas_spaces_or_at_signs() {
        assert_eq!(names("alex, @sam  jo"), ["alex", "sam", "jo"]);
        assert!(names(" , ").is_empty());
    }

    #[test]
    fn explorer_bookmarks_round_trip_through_save_and_load() {
        // Unlike `vnc_hosts`, these are written by the app -- `SPC e m`
        // bookmarks wherever you are -- so a save has to carry them.
        let path = temp_path("explorer_bookmarks_round_trip");
        let mut config = Config::load_or_default(path.clone());
        config.explorer_bookmarks =
            vec![("nas".to_string(), PathBuf::from(r"\\nas\media")), ("work".to_string(), PathBuf::from(r"C:\work"))];

        config.save().unwrap();

        let reloaded = Config::load(path).unwrap();
        assert_eq!(reloaded.explorer_bookmarks, config.explorer_bookmarks);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let path = temp_path("creates_parents").parent().unwrap().join("nested-fenix-config-test").join("config.ini");
        let mut config = Config::load_or_default(path.clone());
        config.theme = Some("Orbit Dark".to_string());
        config.save().unwrap();
        assert!(path.exists());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    /// Jira's `jira.blocked` and users and MIB's roots, as fenix-gui
    /// registers them: for the migration test, which checks they come over
    /// by key.
    fn register_jira() {
        static JIRA: [Setting; 3] = [
            Setting::stored("jira.users", schema::Category::Jira, "People", Kind::Map { key: "Username", value: "Name", paths: false }, "People."),
            Setting::stored("jira.blocked", schema::Category::Jira, "What Blocked means", Kind::Map { key: "Project", value: "Meaning", paths: false }, "Blocked."),
            Setting::stored("mib.roots", schema::Category::Mib, "MIBs", Kind::Map { key: "Name", value: "Folder", paths: true }, "MIBs."),
        ];
        schema::register(&JIRA);
    }

    /// VNC's setting, as fenix-gui registers it at startup: a feature's
    /// setting, kept by key. Registered by every test that counts on it,
    /// since the tests run in any order.
    fn register_vnc() {
        static VNC: [Setting; 1] = [Setting::stored("vnc.hosts", schema::Category::Vnc, "Hosts", Kind::Records(&[
            schema::Field { name: "name", label: "Name", range: None, default: None, choices: &[] },
            schema::Field { name: "host", label: "Host", range: None, default: None, choices: &[] },
            schema::Field { name: "port", label: "Port", range: Some((1, 65535)), default: Some("5900"), choices: &[] },
        ]), "Hosts.")];
        schema::register(&VNC);
    }

    #[test]
    fn what_is_switched_off_is_read_before_anything_else() {
        let path = temp_path("disabled");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[editor]\ntheme = \"Nord\"\n\n[extensions]\ndisabled = [\"fenix.jira\", \"fenix.pdf\"]\n").unwrap();
        assert_eq!(disabled_extensions_in(&path), ["fenix.jira", "fenix.pdf"]);
        assert_eq!(Config::load(path.clone()).unwrap().extensions_disabled, ["fenix.jira", "fenix.pdf"]);
        assert!(disabled_extensions_in(&path.with_file_name("none.toml")).is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn every_setting_round_trips_through_the_file() {
        register_vnc();
        let path = temp_path("every");
        let mut config = Config::load(path.clone()).unwrap();
        let sample = |kind: &Kind| match kind {
            Kind::Bool => Value::Bool(false),
            Kind::Int { min, .. } => Value::Int(min + 1),
            Kind::Float { min, .. } => Value::Float(min + 2.5),
            Kind::Minutes => Value::Int(7),
            Kind::Choice(choices) => Value::Text(choices[1].to_string()),
            Kind::List => Value::List(vec!["alex".into(), "sam".into()]),
            Kind::Map { paths: true, .. } => Value::Map(vec![("First one".into(), r"C:\refs\a b.pdf".into()), ("b".into(), "/refs/b".into())]),
            Kind::Map { .. } => Value::Map(vec![("k 1".into(), "flag".into()), ("k2".into(), "10103: On Hold".into())]),
            Kind::Records(fields) => Value::Records(
                (0..2)
                    .map(|row| {
                        fields
                            .iter()
                            .enumerate()
                            .map(|(i, f)| match (f.range, f.choices) {
                                (Some((min, _)), _) => (min + 1 + row).to_string(),
                                (None, []) => format!("v{row} {i}"),
                                (None, choices) => choices[(row as usize + 1) % choices.len()].to_string(),
                            })
                            .collect()
                    })
                    .collect(),
            ),
            _ => Value::Text(r#"C:\a "b"\c = d"#.into()),
        };
        let settable: Vec<&Setting> = settings().into_iter().filter(|s| !matches!(s.kind, Kind::Secret(_))).collect();
        for s in &settable {
            s.set(&mut config, Some(sample(&s.kind))).unwrap_or_else(|e| panic!("{}: {e}", s.key));
        }
        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();
        assert!(reloaded.problems.is_empty(), "{:?}", reloaded.problems);
        for s in &settable {
            assert_eq!(s.get(&reloaded), s.get(&config), "{}", s.key);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[vnc.hosts]]") && text.contains("[lsp.servers]") && !text.contains("port = 5900"), "a port at its default isn't written:\n{text}");
        assert!(!text.contains("[lsp]") && !text.contains("[vnc]\n"), "no empty parent headers:\n{text}");
        assert!(text.contains("[editor]\nindent_width = 2"), "a table with settings has its header:\n{text}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn saving_changes_only_the_lines_of_the_settings_that_changed() {
        let path = temp_path("in_place");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let hand = "# my settings\n[editor]\ntheme = \"Nord\" # the blue one\nfont_size = 14\n\n[mystery]\nfrom_the_future = true\n";
        std::fs::write(&path, hand).unwrap();
        let mut config = Config::load(path.clone()).unwrap();
        assert_eq!((config.theme.as_deref(), config.font_size), (Some("Nord"), Some(14.0)));
        config.font_size = Some(18.0);
        config.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, hand.replace("font_size = 14", "font_size = 18"), "comments, the unknown section and the theme are as they were");
        config.font_size = None;
        config.save().unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("font_size"), "back at the default: the line goes");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_hand_edit_made_while_fenix_runs_survives_fenix_saving_something_else() {
        // Registered before the load, as fenix-gui does at startup: the
        // tests run in parallel, and one registered between this load
        // and its save would have no value loaded to compare with.
        register_vnc();
        let path = temp_path("hand_edit");
        let mut config = Config::load(path.clone()).unwrap();
        config.theme = Some("Nord".into());
        config.save().unwrap();
        // Edited by hand, after Fenix read it.
        let text = std::fs::read_to_string(&path).unwrap() + "\n[[vnc.hosts]]\nname = \"lab\"\nhost = \"10.1.1.1\"\n\n[[nobody.declares]]\nthis = 1\n\n[lsp.servers]\npython = \"pyright-langserver --stdio\"\n";
        std::fs::write(&path, text).unwrap();
        config.font_size = Some(20.0);
        config.save().unwrap();
        let reloaded = Config::load(path.clone()).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("[[vnc.hosts]]\nname = \"lab\""), "a feature's setting Fenix didn't change stays as written:\n{saved}");
        assert!(saved.contains("[[nobody.declares]]\nthis = 1"), "a key no setting declares stays as it was:\n{saved}");
        assert_eq!(reloaded.lsp_servers, [("python".to_string(), "pyright-langserver --stdio".to_string())]);
        assert_eq!((reloaded.theme.as_deref(), reloaded.font_size), (Some("Nord"), Some(20.0)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_bad_value_costs_only_that_setting_and_says_which_line() {
        let path = temp_path("bad_value");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[editor]\ntheme = \"Nord\"\nfont_size = \"16px\"\nindent_width = 99\n[git]\nlayout = \"tabs\"\n").unwrap();
        let config = Config::load(path.clone()).unwrap();
        assert_eq!(config.theme.as_deref(), Some("Nord"));
        assert_eq!((config.font_size, config.indent_width, config.git_layout.as_deref()), (None, None, None));
        let shown: Vec<String> = config.problems.iter().map(|p| p.to_string()).collect();
        assert_eq!(shown.len(), 3, "{shown:?}");
        assert!(shown[0].starts_with("settings.toml:3 -- editor.font_size: expected a number"), "{shown:?}");
        assert!(shown[1].contains("editor.indent_width: a whole number from 1 to 16"), "{shown:?}");
        assert!(shown[2].starts_with("settings.toml:6 -- git.layout: one of page, panes"), "{shown:?}");
        assert!(!config.is_broken(), "a bad value doesn't stop the file being saved");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_file_that_cant_be_parsed_is_reported_and_never_written_over() {
        let path = temp_path("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let broken = "[editor]\ntheme = \"Nord\nfont_size = 14\n";
        std::fs::write(&path, broken).unwrap();
        let mut config = Config::load(path.clone()).unwrap();
        assert!(config.is_broken());
        assert_eq!(config.problems[0].line, Some(2), "{:?}", config.problems);
        config.font_size = Some(20.0);
        let err = config.save().unwrap_err();
        assert!(err.to_string().contains("fix it first"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_reload_takes_hand_edits_and_keeps_the_last_good_values_through_a_mistake() {
        let path = temp_path("reload");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[editor]\nfont_size = 14\n").unwrap();
        let mut config = Config::load(path.clone()).unwrap();
        std::fs::write(&path, "[editor]\nfont_size = 18\ntheme = \"Nord\"\n").unwrap();
        config.reload();
        assert_eq!((config.font_size, config.theme.as_deref()), (Some(18.0), Some("Nord")));
        std::fs::write(&path, "[editor\nfont_size = 22\n").unwrap();
        config.reload();
        assert_eq!(config.font_size, Some(18.0), "still in use");
        assert_eq!(config.problems.len(), 1);
        std::fs::write(&path, "[editor]\n").unwrap();
        config.reload();
        assert_eq!((config.font_size, config.theme.as_deref()), (None, None), "a line taken out is back at its default");
        assert!(config.problems.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_token_is_kept_in_the_file_and_the_environment_wins_without_being_saved() {
        let path = temp_path("tokens");
        let mut config = Config::load(path.clone()).unwrap();
        config.gitlab_token = Some("glpat-in-the-file".into());
        config.save().unwrap();
        let again = Config::load(path.clone()).unwrap();
        assert!(again.problems.is_empty(), "{:?}", again.problems);
        assert_eq!(again.token(Secret::GitLab).as_deref(), Some("glpat-in-the-file"));
        assert!(std::fs::read_to_string(&path).unwrap().contains("[gitlab]\ntoken = \"glpat-in-the-file\""));
        // Jira's variable, which nothing else in the tests sets.
        std::env::set_var("FENIX_JIRA_TOKEN", "from-the-environment");
        let mut config = Config::load(path.clone()).unwrap();
        assert_eq!(config.token(Secret::Jira).as_deref(), Some("from-the-environment"));
        config.font_size = Some(20.0);
        config.save().unwrap();
        std::env::remove_var("FENIX_JIRA_TOKEN");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("from-the-environment"), "never written");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn window_placement_and_bookmarks_are_state_not_settings() {
        let path = temp_path("state");
        let mut config = Config::load(path.clone()).unwrap();
        config.windows = vec![WindowLayout { x: -8, y: -8, width: 2560, height: 1369, maximized: true }];
        config.explorer_bookmarks = vec![("src".into(), PathBuf::from("C:/code/src"))];
        config.save().unwrap();
        let dir = path.parent().unwrap();
        assert!(dir.join("state").join("windows.json").is_file() && dir.join("state").join("bookmarks.json").is_file());
        assert!(!std::fs::read_to_string(&path).unwrap().contains("2560"), "not in settings.toml");
        let reloaded = Config::load(path.clone()).unwrap();
        assert_eq!(reloaded.windows, config.windows);
        assert_eq!(reloaded.explorer_bookmarks, config.explorer_bookmarks);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_file_starts_with_what_it_is_and_nothing_else() {
        let path = temp_path("new_file");
        Config::load(path.clone()).unwrap().save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Fenix settings.") && text.lines().all(|l| l.is_empty() || l.starts_with('#')), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_default_path_is_settings_toml_in_the_fenix_folder() {
        if let Some(path) = Config::default_path() {
            assert!(path.ends_with(std::path::Path::new("fenix").join("settings.toml")), "{}", path.display());
        }
    }

    #[test]
    fn config_ini_moves_over_with_its_lists_its_tokens_and_its_state() {
        let dir = temp_path("migrate").parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let ini = dir.join("config.ini");
        std::fs::write(
            &ini,
            "[editor]\ntheme = Visual Studio Dark\nfont_size = 16\nanimations = false\n\n[lsp]\nserver1 = python|C:\\tools\\pyright-langserver.exe --stdio\n\n[mib]\nroot1 = missionc|C:\\mib\\missionc\n\n[jira]\ntoken = jira-token\nuser1 = th096939|Thomas Pedneault\nblocked1 = FNX|10103|On Hold\n\n[gitlab]\nbase_url = http://localhost:8929\ntoken = glpat-old\n\n[git]\nauto_fetch = 5m\nreviewers = alex, sam\n\n[vnc]\nhost1 = test-vm|127.0.0.1|5900\nhost2 = build-vm|10.0.0.5|5901\n\n[explorer]\nbookmark1 = src|C:\\code\\src\n\n[windows]\nwindow1 = -8,-8,2560,1369|true\nrestore_session = false\n",
        )
        .unwrap();
        let path = dir.join("settings.toml");
        register_vnc();
        register_jira();
        let config = Config::migrate_ini(&ini, path.clone(), dir.join("state")).unwrap();
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("token = \"glpat-old\"") && text.contains("token = \"jira-token\""), "tokens come along:\n{text}");
        let reloaded = Config::load_at(path, dir.join("state")).unwrap();
        assert_eq!(reloaded.theme.as_deref(), Some("Visual Studio Dark"));
        assert_eq!(reloaded.animations, Some(false));
        assert_eq!(reloaded.lsp_servers, [("python".to_string(), r"C:\tools\pyright-langserver.exe --stdio".to_string())]);
        assert_eq!(reloaded.map_value("mib.roots"), [("missionc".to_string(), r"C:\mib\missionc".to_string())]);
        assert_eq!(reloaded.map_value("jira.blocked"), [("FNX".to_string(), "10103: On Hold".to_string())]);
        assert_eq!(reloaded.git_auto_fetch_minutes, Some(5));
        assert_eq!(reloaded.git_reviewers, ["alex", "sam"]);
        assert!(matches!(reloaded.value("vnc.hosts"), Some(Value::Records(rows)) if rows.len() == 2));
        assert_eq!(reloaded.restore_session, Some(false));
        assert_eq!(reloaded.windows, [WindowLayout { x: -8, y: -8, width: 2560, height: 1369, maximized: true }]);
        assert_eq!(reloaded.explorer_bookmarks, [("src".to_string(), PathBuf::from(r"C:\code\src"))]);
        assert!(ini.is_file(), "config.ini itself is left for the caller to back up");
        let _ = std::fs::remove_dir_all(dir);
    }
}
