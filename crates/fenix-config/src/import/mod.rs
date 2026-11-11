//! Bringing your settings over from another editor (`settings.import`):
//! VS Code's `settings.json` and `keybindings.json` (and its forks'),
//! and a Vim or Neovim config. What each says is read -- never run: no
//! Vimscript or Lua is executed, only plain option settings and simple
//! maps are understood -- and turned into a `Proposal`: the settings and
//! `[[keys]]` Fenix has for it, and what it has none for, with why.
//! `plan` sets that beside what you already have, and `apply` writes it,
//! keeping your own values unless you asked for them to be replaced.
//!
//! Nothing here touches the GUI: `Catalog` is told the themes there are,
//! and the GUI checks the keys and commands against its keymap.

pub mod jsonc;
mod vim;
mod vscode;

/// VS Code's commands the importer maps, and Fenix's for each.
pub use vscode::COMMANDS as VSCODE_COMMANDS;
/// Ex and plugin commands a Vim map may run, and Fenix's for each.
pub use vim::EX_COMMANDS as VIM_COMMANDS;
/// Window keys a Vim map may play (`<C-w>h`), and Fenix's command.
pub use vim::WINDOW_KEYS as VIM_KEY_COMMANDS;

use std::path::{Path, PathBuf};

use crate::{Config, Value};

/// Which kind of editor a config is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    /// VS Code, or one of its forks (VSCodium, Cursor, Windsurf...).
    VsCode,
    /// Vim or Neovim.
    Vim,
}

/// A config found on this machine: whose, and its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// As it's offered: `VS Code`, `VSCodium`, `Neovim`.
    pub name: String,
    pub editor: Editor,
    /// What's read: VS Code's `settings.json` and `keybindings.json`
    /// (either may be missing), a Vim or Neovim config.
    pub files: Vec<PathBuf>,
    /// VS Code with the Vim extension (VSCodeVim) installed: its keys
    /// were Vim's.
    pub vim_extension: bool,
}

/// Where configs are looked for. `current` is this machine's; tests
/// give their own.
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub home: Option<PathBuf>,
    /// Windows' `%APPDATA%` (roaming).
    pub appdata: Option<PathBuf>,
    /// Windows' `%LOCALAPPDATA%`.
    pub local_appdata: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME`, else `~/.config`.
    pub xdg_config: Option<PathBuf>,
    pub windows: bool,
    pub mac: bool,
}

impl Places {
    pub fn current() -> Places {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = dirs::home_dir();
        Places {
            xdg_config: var("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|h| h.join(".config"))),
            appdata: var("APPDATA"),
            local_appdata: var("LOCALAPPDATA"),
            home,
            windows: cfg!(windows),
            mac: cfg!(target_os = "macos"),
        }
    }
}

/// VS Code and the forks people use, by the folder each keeps its
/// settings in (`<config>/<folder>/User/settings.json`) and the one its
/// extensions are in (`~/<folder>/extensions`).
const VSCODES: &[(&str, &str, &str)] = &[
    ("VS Code", "Code", ".vscode"),
    ("VS Code Insiders", "Code - Insiders", ".vscode-insiders"),
    ("VSCodium", "VSCodium", ".vscode-oss"),
    ("Code - OSS", "Code - OSS", ".vscode-oss"),
    ("Cursor", "Cursor", ".cursor"),
    ("Windsurf", "Windsurf", ".windsurf"),
];

/// The configs found on this machine, VS Code's first.
pub fn find_sources(places: &Places) -> Vec<Source> {
    let mut found = Vec::new();
    for (name, folder, extensions) in VSCODES {
        let mut roots = Vec::new();
        if places.windows {
            roots.extend(places.appdata.iter().map(|a| a.join(folder)));
        } else if places.mac {
            roots.extend(places.home.iter().map(|h| h.join("Library/Application Support").join(folder)));
        } else {
            roots.extend(places.xdg_config.iter().map(|c| c.join(folder)));
            // Flatpak keeps its own.
            if *folder == "Code" {
                roots.extend(places.home.iter().map(|h| h.join(".var/app/com.visualstudio.code/config/Code")));
            }
            if *folder == "VSCodium" {
                roots.extend(places.home.iter().map(|h| h.join(".var/app/com.vscodium.codium/config/VSCodium")));
            }
        }
        for root in roots {
            let user = root.join("User");
            let files: Vec<PathBuf> = ["settings.json", "keybindings.json"].iter().map(|f| user.join(f)).filter(|p| p.is_file()).collect();
            if files.is_empty() || found.iter().any(|s: &Source| s.files == files) {
                continue;
            }
            let vim_extension = places.home.as_ref().is_some_and(|h| has_vim_extension(&h.join(extensions).join("extensions")));
            found.push(Source { name: name.to_string(), editor: Editor::VsCode, files, vim_extension });
            break;
        }
    }
    // Neovim: init.vim or init.lua, in its own folder.
    let mut nvim = Vec::new();
    if places.windows {
        nvim.extend(places.local_appdata.iter().map(|l| l.join("nvim")));
    }
    nvim.extend(places.xdg_config.iter().map(|c| c.join("nvim")));
    if let Some(file) = nvim.iter().flat_map(|d| [d.join("init.lua"), d.join("init.vim")]).find(|p| p.is_file()) {
        found.push(Source { name: "Neovim".into(), editor: Editor::Vim, files: vec![file], vim_extension: false });
    }
    // Vim: ~/.vimrc, ~/.vim/vimrc, and on Windows ~/_vimrc, ~/vimfiles/vimrc.
    if let Some(home) = &places.home {
        let mut vimrcs = vec![home.join(".vimrc"), home.join(".vim").join("vimrc")];
        if places.windows {
            vimrcs.splice(0..0, [home.join("_vimrc"), home.join("vimfiles").join("vimrc")]);
        }
        if let Some(file) = vimrcs.into_iter().find(|p| p.is_file()) {
            found.push(Source { name: "Vim".into(), editor: Editor::Vim, files: vec![file], vim_extension: false });
        }
    }
    found
}

/// Whether VSCodeVim is in an extensions folder.
fn has_vim_extension(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|entries| entries.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().starts_with("vscodevim.vim-"))).unwrap_or(false)
}

/// What the importer is told about this Fenix: its themes, by name, and
/// whether each is light.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub themes: Vec<(String, bool)>,
}

/// One setting to write.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingChange {
    pub key: &'static str,
    pub value: Value,
    /// What it came from, as it was written there: `"editor.tabSize": 2`.
    pub from: String,
    /// When it isn't quite the same thing: why it's the nearest.
    pub note: Option<String>,
}

/// One chord to bind, as a `[[keys]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChange {
    /// As `keybindings.toml` writes it: `C-S-p`, `F12`, `M-Up`.
    pub keys: String,
    pub command: String,
    /// `vim` or `standard`.
    pub scheme: &'static str,
    /// `text`, `typing`, `not_typing`, `terminal`; `None` is anywhere.
    pub when: Option<&'static str>,
    pub from: String,
}

/// Something the other editor's config says that Fenix has nothing for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmapped {
    pub from: String,
    pub why: String,
}

/// What a config would bring over.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Proposal {
    /// Whose it is: `VS Code`.
    pub source: String,
    pub settings: Vec<SettingChange>,
    pub keys: Vec<KeyChange>,
    pub unmapped: Vec<Unmapped>,
}

impl Proposal {
    fn set(&mut self, key: &'static str, value: Value, from: impl Into<String>, note: Option<String>) {
        let from = from.into();
        // A later line says it again: the last one is what the editor used.
        self.settings.retain(|s| s.key != key);
        self.settings.push(SettingChange { key, value, from, note });
    }

    fn unmapped(&mut self, from: impl Into<String>, why: impl Into<String>) {
        self.unmapped.push(Unmapped { from: from.into(), why: why.into() });
    }

    fn key(&mut self, keys: String, command: &str, scheme: &'static str, when: Option<&'static str>, from: impl Into<String>) {
        self.keys.retain(|k| !(k.keys == keys && k.scheme == scheme && k.when == when));
        self.keys.push(KeyChange { keys, command: command.to_string(), scheme, when, from: from.into() });
    }

    /// Keeps the chords `ok` says Fenix can bind (the key parses, the
    /// command exists); the rest go to what couldn't be brought over.
    pub fn retain_keys(&mut self, ok: impl Fn(&KeyChange) -> Result<(), String>) {
        let mut kept = Vec::new();
        for key in std::mem::take(&mut self.keys) {
            match ok(&key) {
                Ok(()) => kept.push(key),
                Err(why) => self.unmapped.push(Unmapped { from: key.from, why }),
            }
        }
        self.keys = kept;
    }
}

/// Reads `source`'s files. A file that can't be read or parsed is said
/// in `unmapped`, and the rest still read.
pub fn read(source: &Source, catalog: &Catalog) -> Proposal {
    let mut out = Proposal { source: source.name.clone(), ..Proposal::default() };
    for file in &source.files {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => {
                out.unmapped(file.display().to_string(), format!("couldn't be read: {e}"));
                continue;
            }
        };
        let name = file.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        match (source.editor, name.as_str()) {
            (Editor::VsCode, "keybindings.json") => vscode::keybindings(&text, &mut out),
            (Editor::VsCode, _) => vscode::settings(&text, source.vim_extension, catalog, &mut out),
            (Editor::Vim, n) if n.ends_with(".lua") => vim::lua(&text, catalog, &mut out),
            (Editor::Vim, _) => vim::vimscript(&text, catalog, &mut out),
        }
    }
    out
}

/// Reads VS Code's settings and keybindings from text: for tests, and a
/// file given by hand.
pub fn read_vscode(settings: Option<&str>, keybindings: Option<&str>, vim_extension: bool, catalog: &Catalog) -> Proposal {
    let mut out = Proposal { source: "VS Code".into(), ..Proposal::default() };
    if let Some(text) = settings {
        vscode::settings(text, vim_extension, catalog, &mut out);
    }
    if let Some(text) = keybindings {
        vscode::keybindings(text, &mut out);
    }
    out
}

/// Reads a Vim (`lua = false`) or Neovim Lua config from text.
pub fn read_vim(text: &str, lua: bool, catalog: &Catalog) -> Proposal {
    let mut out = Proposal { source: if lua { "Neovim" } else { "Vim" }.into(), ..Proposal::default() };
    if lua {
        vim::lua(text, catalog, &mut out);
    } else {
        vim::vimscript(text, catalog, &mut out);
    }
    out
}

/// A theme for `name` (VS Code's `workbench.colorTheme`, Vim's
/// `colorscheme`): Fenix's of the same name, or the nearest -- another
/// of its family, or one as light or dark -- with why.
pub(crate) fn theme_for(name: &str, light: Option<bool>, catalog: &Catalog) -> Option<(String, Option<String>)> {
    let norm = norm(name);
    let has = |n: &str| catalog.themes.iter().find(|(t, _)| norm_eq(t, n)).map(|(t, _)| t.clone());
    // Names Fenix's themes go by elsewhere.
    const ALIASES: &[(&str, &str)] = &[
        ("defaultdarkmodern", "Visual Studio Dark"),
        ("defaultdark", "Visual Studio Dark"),
        ("defaultdarkplus", "Visual Studio Dark"),
        ("darkplus", "Visual Studio Dark"),
        ("darkmodern", "Visual Studio Dark"),
        ("defaultlightmodern", "Visual Studio Light"),
        ("defaultlight", "Visual Studio Light"),
        ("defaultlightplus", "Visual Studio Light"),
        ("lightplus", "Visual Studio Light"),
        ("lightmodern", "Visual Studio Light"),
        ("onedarkpro", "One Dark"),
        ("atomonedark", "One Dark"),
        ("atomonelight", "One Light"),
        ("tokyonight", "Tokyo Night Storm"),
        ("tokyonightmoon", "Tokyo Night Storm"),
        ("tokyonightnight", "Tokyo Night Storm"),
        ("palenight", "Material Palenight"),
        ("materialthemepalenight", "Material Palenight"),
        ("kanagawawave", "Kanagawa"),
        ("everforest", "Everforest Dark"),
        ("ayu", "Ayu Mirage"),
        ("ayudark", "Ayu Mirage"),
        ("rosepinemoon", "Rosé Pine"),
        ("draculaofficial", "Dracula"),
        ("monokaipro", "Monokai"),
    ];
    // Families whose light and dark Fenix both has: Vim's `background`
    // picks.
    const FAMILIES: &[(&str, &str, &str)] = &[
        ("gruvbox", "Gruvbox Dark", "Gruvbox Light"),
        ("solarized", "Solarized Dark", "Solarized Light"),
        ("catppuccin", "Catppuccin Mocha", "Catppuccin Latte"),
        ("rosepine", "Rosé Pine", "Rosé Pine Dawn"),
        ("github", "GitHub Dark", "GitHub Light"),
        ("onedark", "One Dark", "One Light"),
        ("one", "One Dark", "One Light"),
        ("tokyonight", "Tokyo Night Storm", "Tokyo Night Day"),
        ("visualstudio", "Visual Studio Dark", "Visual Studio Light"),
    ];
    // Vim's `background` says which of a family's, over its name alone
    // (`rose-pine` with a light background is Rosé Pine Dawn).
    let exact = has(&norm);
    if exact.is_some() && light.is_none() {
        return exact.map(|t| (t, None));
    }
    for (family, dark, bright) in FAMILIES {
        if norm == *family {
            let pick = if light == Some(true) { bright } else { dark };
            if let Some(t) = has(&self::norm(pick)) {
                return Some((t, Some(format!("{name} with {} background", if light == Some(true) { "a light" } else { "a dark" }))));
            }
        }
    }
    if let Some(exact) = exact {
        return Some((exact, None));
    }
    if let Some((_, target)) = ALIASES.iter().find(|(alias, _)| *alias == norm) {
        if let Some(t) = has(&self::norm(target)) {
            let same = target.to_ascii_lowercase().starts_with("visual studio");
            return Some((t, (!same).then(|| format!("the nearest Fenix has to {name}"))));
        }
    }
    // A theme of Fenix's whose name begins it ("GitHub Dark Default"), the
    // longest; or one whose name it begins ("Rosé Pine Moon"), as light
    // or dark as asked.
    let mut within: Vec<&(String, bool)> = catalog.themes.iter().filter(|(t, _)| norm.starts_with(&self::norm(t))).collect();
    within.sort_by_key(|(t, _)| std::cmp::Reverse(self::norm(t).len()));
    if let Some((t, _)) = within.first() {
        return Some((t.clone(), Some(format!("the nearest Fenix has to {name}"))));
    }
    let mut longer: Vec<&(String, bool)> = catalog.themes.iter().filter(|(t, _)| self::norm(t).starts_with(&norm) && norm.len() >= 3).collect();
    if let Some(l) = light {
        longer.sort_by_key(|(_, tl)| *tl != l);
    }
    if let Some((t, _)) = longer.first() {
        return Some((t.clone(), Some(format!("the nearest Fenix has to {name}"))));
    }
    // Nothing like it: when it's known to be light, a light one.
    if light == Some(true) || (light.is_none() && norm.contains("light")) {
        let pick = ["Visual Studio Light", "GitHub Light"].iter().find_map(|t| has(&self::norm(t))).or_else(|| catalog.themes.iter().find(|(_, l)| *l).map(|(t, _)| t.clone()))?;
        return Some((pick, Some(format!("Fenix has no {name}: a light theme, as it was"))));
    }
    None
}

/// A name for comparing: lower case, letters and digits only, accents
/// off (`Rosé Pine` is `rosepine`).
fn norm(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'É' => 'e',
            'á' | 'à' | 'â' => 'a',
            c => c,
        })
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn norm_eq(theme: &str, normed: &str) -> bool {
    norm(theme) == normed
}

/// What becomes of one setting or chord.
#[derive(Debug, Clone, PartialEq)]
pub enum Fate {
    /// You haven't set it: it's written.
    New,
    /// You already have it so: nothing to write.
    Same,
    /// You set it otherwise (as shown): yours stays, unless replacing.
    Yours(String),
}

/// A proposal beside what you have.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub source: String,
    pub settings: Vec<(SettingChange, Fate)>,
    pub keys: Vec<(KeyChange, Fate)>,
    pub unmapped: Vec<Unmapped>,
}

impl Plan {
    /// How many would be written: the new ones, and with `replace`
    /// yours that differ.
    pub fn writes(&self, replace: bool) -> (usize, usize) {
        let counts = |f: &Fate| matches!(f, Fate::New) || (replace && matches!(f, Fate::Yours(_)));
        (self.settings.iter().filter(|(_, f)| counts(f)).count(), self.keys.iter().filter(|(_, f)| counts(f)).count())
    }

    /// Whether any of yours differ.
    pub fn conflicts(&self) -> bool {
        self.settings.iter().any(|(_, f)| matches!(f, Fate::Yours(_))) || self.keys.iter().any(|(_, f)| matches!(f, Fate::Yours(_)))
    }

    /// Marks the chords `same` says Fenix already binds so.
    pub fn mark_same_keys(&mut self, same: impl Fn(&KeyChange) -> bool) {
        for (key, fate) in &mut self.keys {
            if *fate == Fate::New && same(key) {
                *fate = Fate::Same;
            }
        }
    }
}

/// Sets `proposal` beside your settings (`config`) and your keybindings
/// file's text.
pub fn plan(proposal: Proposal, config: &Config, keybindings: Option<&str>) -> Plan {
    let settings = proposal
        .settings
        .into_iter()
        .map(|s| {
            let yours = crate::setting(s.key).and_then(|setting| setting.get(config)).or_else(|| config.value(s.key).cloned());
            let fate = match yours {
                None => Fate::New,
                Some(v) if same_value(&v, &s.value) => Fate::Same,
                Some(v) => Fate::Yours(v.show()),
            };
            (s, fate)
        })
        .collect();
    let mine = your_chords(keybindings.unwrap_or(""));
    let keys = proposal
        .keys
        .into_iter()
        .map(|k| {
            let fate = match mine.iter().find(|(keys, scheme, _)| keys.eq_ignore_ascii_case(&k.keys) && (scheme == k.scheme || scheme == "any")) {
                None => Fate::New,
                Some((_, _, command)) if command == &k.command => Fate::Same,
                Some((_, _, command)) => Fate::Yours(if command.is_empty() { "unbound".into() } else { command.clone() }),
            };
            (k, fate)
        })
        .collect();
    Plan { source: proposal.source, settings, keys, unmapped: proposal.unmapped }
}

fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(x), Value::Int(y)) | (Value::Int(y), Value::Float(x)) => (*x - *y as f64).abs() < f64::EPSILON,
        (Value::Text(x), Value::Text(y)) => x.eq_ignore_ascii_case(y),
        _ => a == b,
    }
}

/// Your `[[keys]]` entries: keys, scheme, command (empty when unbound).
fn your_chords(text: &str) -> Vec<(String, String, String)> {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return Vec::new() };
    let Some(entries) = doc.get("keys").and_then(|k| k.as_array_of_tables()) else { return Vec::new() };
    entries
        .iter()
        .filter_map(|t| {
            let keys = t.get("keys")?.as_str()?.to_string();
            let scheme = t.get("scheme").and_then(|s| s.as_str()).unwrap_or("any").to_string();
            let command = t.get("command").and_then(|c| c.as_str()).unwrap_or("").to_string();
            Some((keys, scheme, command))
        })
        .collect()
}

/// What `apply` wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub settings: usize,
    pub keys: usize,
    /// What couldn't be written, and why.
    pub failed: Vec<String>,
}

/// Writes `plan`: the new settings into `config` (then saved) and the
/// new chords into the keybindings file at `keybindings`; with `replace`,
/// yours that differ too. What you already have so is left alone.
pub fn apply(plan: &Plan, replace: bool, config: &mut Config, keybindings: &Path) -> Applied {
    let mut applied = Applied::default();
    let wanted = |f: &Fate| matches!(f, Fate::New) || (replace && matches!(f, Fate::Yours(_)));
    for (change, _) in plan.settings.iter().filter(|(_, f)| wanted(f)) {
        let Some(setting) = crate::setting(change.key) else {
            applied.failed.push(format!("{}: Fenix has no such setting", change.key));
            continue;
        };
        match setting.set(config, Some(change.value.clone())) {
            Ok(()) => applied.settings += 1,
            Err(why) => applied.failed.push(format!("{} = {}: {why}", change.key, change.value.show())),
        }
    }
    if applied.settings > 0 {
        if let Err(e) = config.save() {
            applied.failed.push(format!("settings.toml couldn't be saved: {e}"));
            applied.settings = 0;
        }
    }
    let note = format!("from {}", plan.source);
    for (change, _) in plan.keys.iter().filter(|(_, f)| wanted(f)) {
        match crate::keybindings::write_chord_noted(keybindings, &change.keys, &change.command, change.scheme, change.when, Some(&note)) {
            Ok(()) => applied.keys += 1,
            Err(e) => {
                applied.failed.push(format!("{}: {e}", change.keys));
                // The file can't be written: the rest won't be either.
                if e.kind() == std::io::ErrorKind::InvalidData {
                    break;
                }
            }
        }
    }
    applied
}

#[cfg(test)]
mod tests;
