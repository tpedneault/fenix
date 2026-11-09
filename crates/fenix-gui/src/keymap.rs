use std::sync::OnceLock;

use fenix_keymap::{KeyCode, KeyPress, KeyTrie, Mods, NamedKey as FenixNamedKey};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

pub mod chords;
pub use chords::{Chord, ChordContext, ChordMap, Scheme, When};

/// Translates a winit key event into fenix-keymap's UI-agnostic `KeyPress`.
/// Named `Space` is normalized to `KeyCode::Char(' ')` -- treating it like
/// any other printable key keeps the leader trie's sequences (`SPC f s`)
/// just a plain char sequence, no special-casing needed downstream.
/// Shift is kept as `KeyPress` describes: not on a printable character
/// typed without Ctrl or Alt. Returns `None` for keys with nothing
/// sensible to bind (media keys, F13 and up, ...).
pub fn to_keypress(event: &KeyEvent, mods: ModifiersState) -> Option<KeyPress> {
    // Ctrl+Alt+Q is a second Escape. Synthetic input (UI automation,
    // some remote-desktop tools) can fail to deliver Escape itself; this
    // combination is bound to nothing else. Matched on the physical key
    // so the layout's idea of Ctrl+Alt+Q (AltGr on some) doesn't matter.
    if mods.control_key() && mods.alt_key() && event.physical_key == PhysicalKey::Code(winit::keyboard::KeyCode::KeyQ) {
        return Some(KeyPress { code: KeyCode::Named(FenixNamedKey::Escape), mods: Mods::default() });
    }
    let code = match &event.logical_key {
        Key::Named(NamedKey::Space) => KeyCode::Char(' '),
        Key::Named(NamedKey::Escape) => KeyCode::Named(FenixNamedKey::Escape),
        Key::Named(NamedKey::Enter) => KeyCode::Named(FenixNamedKey::Enter),
        Key::Named(NamedKey::Tab) => KeyCode::Named(FenixNamedKey::Tab),
        Key::Named(NamedKey::Backspace) => KeyCode::Named(FenixNamedKey::Backspace),
        Key::Named(NamedKey::Delete) => KeyCode::Named(FenixNamedKey::Delete),
        Key::Named(NamedKey::ArrowLeft) => KeyCode::Named(FenixNamedKey::Left),
        Key::Named(NamedKey::ArrowRight) => KeyCode::Named(FenixNamedKey::Right),
        Key::Named(NamedKey::ArrowUp) => KeyCode::Named(FenixNamedKey::Up),
        Key::Named(NamedKey::ArrowDown) => KeyCode::Named(FenixNamedKey::Down),
        Key::Named(NamedKey::Home) => KeyCode::Named(FenixNamedKey::Home),
        Key::Named(NamedKey::End) => KeyCode::Named(FenixNamedKey::End),
        Key::Named(NamedKey::PageUp) => KeyCode::Named(FenixNamedKey::PageUp),
        Key::Named(NamedKey::PageDown) => KeyCode::Named(FenixNamedKey::PageDown),
        Key::Named(named) => KeyCode::Named(FenixNamedKey::F(function_key_number(named)?)),
        Key::Character(s) => KeyCode::Char(s.chars().next()?),
        _ => return None,
    };
    Some(keypress_with(code, mods))
}

/// `code` with the modifiers held, Shift kept as `KeyPress` says.
pub fn keypress_with(code: KeyCode, mods: ModifiersState) -> KeyPress {
    let (ctrl, alt) = (mods.control_key(), mods.alt_key());
    let printable = matches!(code, KeyCode::Char(_));
    let shift = mods.shift_key() && (!printable || ctrl || alt);
    KeyPress { code, mods: Mods { ctrl, alt, super_: mods.super_key(), shift } }
}

/// F1 to F12's number.
pub fn function_key_number(named: &NamedKey) -> Option<u8> {
    Some(match named {
        NamedKey::F1 => 1,
        NamedKey::F2 => 2,
        NamedKey::F3 => 3,
        NamedKey::F4 => 4,
        NamedKey::F5 => 5,
        NamedKey::F6 => 6,
        NamedKey::F7 => 7,
        NamedKey::F8 => 8,
        NamedKey::F9 => 9,
        NamedKey::F10 => 10,
        NamedKey::F11 => 11,
        NamedKey::F12 => 12,
        _ => return None,
    })
}

/// A short human-readable label for a keypress, for the which-key popup
/// (`SPC` rather than a literal space, `C-r` for Ctrl-r, `Esc` for Escape,
/// `S-Down` for Shift-Down, `s-` for Super).
pub fn describe_keypress(kp: &KeyPress) -> String {
    let mut s = String::new();
    if kp.mods.ctrl {
        s.push_str("C-");
    }
    if kp.mods.alt {
        s.push_str("M-");
    }
    if kp.mods.super_ {
        s.push_str("s-");
    }
    if kp.mods.shift {
        s.push_str("S-");
    }
    match kp.code {
        KeyCode::Char(' ') => s.push_str("SPC"),
        KeyCode::Char(c) => s.push(c),
        KeyCode::Named(n) => s.push_str(&named_key_name(n)),
    }
    s
}

/// A named key as the keymap writes it.
fn named_key_name(n: FenixNamedKey) -> String {
    match n {
        FenixNamedKey::Escape => "Esc".into(),
        FenixNamedKey::Enter => "Enter".into(),
        FenixNamedKey::Tab => "Tab".into(),
        FenixNamedKey::Backspace => "Backspace".into(),
        FenixNamedKey::Delete => "Delete".into(),
        FenixNamedKey::Left => "Left".into(),
        FenixNamedKey::Right => "Right".into(),
        FenixNamedKey::Up => "Up".into(),
        FenixNamedKey::Down => "Down".into(),
        FenixNamedKey::Home => "Home".into(),
        FenixNamedKey::End => "End".into(),
        FenixNamedKey::PageUp => "PgUp".into(),
        FenixNamedKey::PageDown => "PgDn".into(),
        FenixNamedKey::F(n) => format!("F{n}"),
    }
}

/// A named key from its name, as `named_key_name` writes it or as people
/// do (`Escape`, `PageDown`, `Space`, `Return`), any case.
fn named_key(name: &str) -> Option<KeyCode> {
    let lower = name.to_ascii_lowercase();
    Some(KeyCode::Named(match lower.as_str() {
        "esc" | "escape" => FenixNamedKey::Escape,
        "enter" | "return" | "ret" | "cr" => FenixNamedKey::Enter,
        "tab" => FenixNamedKey::Tab,
        "backspace" | "bs" => FenixNamedKey::Backspace,
        "delete" | "del" => FenixNamedKey::Delete,
        "left" => FenixNamedKey::Left,
        "right" => FenixNamedKey::Right,
        "up" => FenixNamedKey::Up,
        "down" => FenixNamedKey::Down,
        "home" => FenixNamedKey::Home,
        "end" => FenixNamedKey::End,
        "pgup" | "pageup" => FenixNamedKey::PageUp,
        "pgdn" | "pagedown" => FenixNamedKey::PageDown,
        "spc" | "space" => return Some(KeyCode::Char(' ')),
        _ => {
            let n: u8 = lower.strip_prefix('f')?.parse().ok()?;
            if !(1..=12).contains(&n) {
                return None;
            }
            FenixNamedKey::F(n)
        }
    }))
}

/// A kind of buffer or project `SPC m` has bindings for. A buffer can be
/// in several at once (a Tcl file in an Arduino sketch would be both);
/// `App::local_contexts` lists them most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalContext {
    /// Tcl files: the menus the MIB and Tcl extensions give them.
    Tcl,

    /// Markdown (notes, READMEs): the reading view.
    Markdown,
    /// A Mermaid diagram (`.mmd`) or its preview.
    Mermaid,
    /// An extension's (`[[contributes.contexts]]`), by its name: its
    /// keymaps' `[[local.<name>]]` entries are its menu.
    Named(&'static str),
}

impl LocalContext {
    /// Shown when `SPC m` opens, so you know which menu you're in.
    pub fn name(self) -> &'static str {
        match self {
            LocalContext::Tcl => "tcl",

            LocalContext::Markdown => "markdown",
            LocalContext::Mermaid => "diagram",
            LocalContext::Named(name) => name,
        }
    }

    /// An extension's context, by name: one `&'static str` per name, kept.
    pub fn named(name: &str) -> LocalContext {
        static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
        let mut names = NAMES.lock().unwrap_or_else(|p| p.into_inner());
        let name = match names.iter().find(|n| **n == name) {
            Some(n) => *n,
            None => {
                let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
                names.push(leaked);
                leaked
            }
        };
        LocalContext::Named(name)
    }

    fn bind(self, t: &mut KeyTrie<&'static str>) {
        insert_data(t, &[], default_local(self.name()));
        // The installed extensions' `[[local.<name>]]` entries.
        for entry in EXTENSION_LOCALS.read().unwrap_or_else(|p| p.into_inner()).iter().filter(|e| e.menu == self.name()) {
            match entry.command {
                Some(command) => t.insert(&entry.keys, entry.label, command),
                None => t.label_group(&entry.keys, entry.label),
            }
        }
    }
}

/// One `[[local.<menu>]]` entry from an installed extension's keymap:
/// keys, checked, bound to a command or naming a group.
struct ExtensionLocal {
    menu: String,
    keys: Vec<KeyPress>,
    label: &'static str,
    command: Option<&'static str>,
}

/// What the installed extensions' keymaps put in `SPC m` menus, read with
/// the leader map (`reload_leader`).
static EXTENSION_LOCALS: std::sync::RwLock<Vec<ExtensionLocal>> = std::sync::RwLock::new(Vec::new());

/// The `SPC m` menus built so far, by their contexts.
static LOCAL_TRIES: OnceLock<std::sync::Mutex<std::collections::HashMap<Vec<LocalContext>, &'static KeyTrie<&'static str>>>> = OnceLock::new();

/// An extension keymap's `[[local.*]]` entries, checked: what's wrong with
/// one is said, and it's left out.
fn extension_locals(data: &'static KeymapData, known: &dyn Fn(&str) -> bool, problems: &mut Vec<String>) -> Vec<ExtensionLocal> {
    let mut out = Vec::new();
    for (menu, entries) in &data.local {
        for entry in entries {
            let keys = match parse_keys(&entry.keys) {
                Ok(keys) if !keys.is_empty() => keys,
                Ok(_) => continue,
                Err(e) => {
                    problems.push(format!("[[local.{menu}]] {e}"));
                    continue;
                }
            };
            match (entry.command.as_deref(), entry.group.as_deref()) {
                (Some(command), _) if !known(command) => problems.push(format!("[[local.{menu}]] {}: {command} isn't a command", entry.keys)),
                (Some(command), _) => out.push(ExtensionLocal { menu: menu.clone(), keys, label: entry.label.as_deref().unwrap_or(command), command: Some(command) }),
                (None, Some(group)) => out.push(ExtensionLocal { menu: menu.clone(), keys, label: group, command: None }),
                (None, None) => problems.push(format!("[[local.{menu}]] {}: needs a command or a group", entry.keys)),
            }
        }
    }
    out
}

/// The `SPC m` menu of context `name` from an extension keymap `text`,
/// and what was wrong with it: as `reload_leader` reads one.
#[cfg(test)]
pub(crate) fn extension_local_menu_for_tests(name: &str, text: &str) -> (&'static KeyTrie<&'static str>, Vec<String>) {
    let data: &'static KeymapData = Box::leak(Box::new(toml::from_str(text).expect("a keymap")));
    let mut problems = Vec::new();
    let registry = crate::commands::CommandRegistry::with_builtins();
    let known = |id: &str| registry.get(id).is_some() || id.starts_with("hello.");
    *EXTENSION_LOCALS.write().unwrap_or_else(|p| p.into_inner()) = extension_locals(data, &known, &mut problems);
    if let Some(tries) = LOCAL_TRIES.get() {
        tries.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
    (local_trie(&[LocalContext::named(name)]), problems)
}

/// The `SPC m` menu for `contexts` (most specific first): each context's
/// bindings, with a more specific context winning a key both bind. Built
/// once per combination and kept -- there are only ever a handful -- until
/// the keymaps are read again.
pub fn local_trie(contexts: &[LocalContext]) -> &'static KeyTrie<&'static str> {
    let mut tries = LOCAL_TRIES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    tries.entry(contexts.to_vec()).or_insert_with(|| {
        let mut t = KeyTrie::new();
        for context in contexts.iter().rev() {
            context.bind(&mut t);
        }
        Box::leak(Box::new(t))
    })
}

/// The `SPC`-leader menu. Includes the leading space itself as the trie's
/// first key, so the whole leader interaction -- from the initial `SPC`
/// through to a resolved command -- is just one uniform walk of this trie.
///
/// Deliberately sparse: only wires groups that have a real command
/// behind them today.
/// The order sections come in within a which-key group; any other
/// section follows these, and unsectioned keys come last.
pub const WHICH_KEY_SECTIONS: &[&str] = &["View", "Branch & remote", "This change"];

/// The leader map in use: built on first use, and again when your
/// keybindings change (`reload_leader`). A rebuilt map replaces it; one a
/// matcher still walks stays alive (they're leaked, and few).
static LEADER: std::sync::RwLock<Option<&'static KeyTrie<&'static str>>> = std::sync::RwLock::new(None);

pub fn leader_trie() -> &'static KeyTrie<&'static str> {
    if let Some(t) = *LEADER.read().unwrap_or_else(|p| p.into_inner()) {
        return t;
    }
    static CONFLICTS: std::sync::Once = std::sync::Once::new();
    CONFLICTS.call_once(|| {
        // A key bound twice silently goes to whichever came last.
        for conflict in keymap_conflicts() {
            eprintln!("fenix: keymap: {conflict}");
        }
    });
    reload_leader();
    LEADER.read().unwrap_or_else(|p| p.into_inner()).expect("just built")
}

/// Builds the leader map again -- Fenix's keys, the switched-on
/// features', then yours (`keybindings.toml`) -- and puts it in use.
/// Returns what's wrong with your file (also kept: `user_key_problems`).
pub fn reload_leader() -> Vec<String> {
    let mut t = build_leader();
    let registry = crate::commands::CommandRegistry::with_builtins();
    // A code extension's commands are registered once it's loaded; its
    // manifest says them now.
    let contributed: Vec<String> = crate::installed::code_extensions().into_iter().flat_map(|e| e.manifest.contributes.commands.into_iter().map(|c| c.id)).collect();
    let known = |id: &str| registry.get(id).is_some() || contributed.iter().any(|c| c == id);
    // The installed extensions' keymaps, then yours over them.
    let mut problems = Vec::new();
    let mut locals = Vec::new();
    let mut chords = ChordMap::new();
    lay_chords(&mut chords, default_keymap(), None, &known, ChordSource::Fenix, &mut Vec::new());
    for path in extension_keymaps() {
        match read_user_keymap(&path) {
            Ok(Some(keys)) => {
                let mut wrong = Vec::new();
                locals.extend(extension_locals(keys, &known, &mut wrong));
                let from: &'static str = Box::leak(path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default().into_boxed_str());
                lay_chords(&mut chords, keys, Some(from), &known, ChordSource::Extension, &mut wrong);
                let leader = if keys.local.is_empty() && keys.keys.is_empty() { keys } else { leader_only_of(keys) };
                wrong.extend(apply_user_keys(&mut t, leader, &known).1);
                problems.extend(wrong.into_iter().map(|w| format!("{}: {w}", path.display())));
            }
            Ok(None) => {}
            Err(problem) => problems.push(format!("{}: {problem}", path.display())),
        }
    }
    let bound = match user_keymap_path() {
        Some(path) => match read_user_keymap(&path) {
            Ok(Some(user)) => {
                let mut wrong = Vec::new();
                let mut bound = lay_chords(&mut chords, user, Some("keybindings.toml"), &known, ChordSource::Yours, &mut wrong);
                let leader = if user.keys.is_empty() { user } else { leader_only_of_with_local(user) };
                let (leader_bound, leader_wrong) = apply_user_keys(&mut t, leader, &known);
                bound.splice(0..0, leader_bound);
                problems.extend(leader_wrong);
                problems.extend(wrong);
                bound
            }
            Ok(None) => Vec::new(),
            Err(problem) => {
                problems.push(problem);
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    *CHORDS.write().unwrap_or_else(|p| p.into_inner()) = Some(Box::leak(Box::new(chords)));
    *USER_KEYS.lock().unwrap_or_else(|p| p.into_inner()) = (bound, problems.clone());
    *EXTENSION_LOCALS.write().unwrap_or_else(|p| p.into_inner()) = locals;
    if let Some(tries) = LOCAL_TRIES.get() {
        tries.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
    *LEADER.write().unwrap_or_else(|p| p.into_inner()) = Some(Box::leak(Box::new(t)));
    problems
}

/// The chords in use: Fenix's `[[keys]]`, the extensions', yours.
pub fn chord_map() -> &'static ChordMap {
    if let Some(map) = *CHORDS.read().unwrap_or_else(|p| p.into_inner()) {
        return map;
    }
    leader_trie();
    CHORDS.read().unwrap_or_else(|p| p.into_inner()).expect("built with the leader")
}

static CHORDS: std::sync::RwLock<Option<&'static ChordMap>> = std::sync::RwLock::new(None);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChordSource {
    Fenix,
    Extension,
    Yours,
}

/// Lays `data`'s `[[keys]]` into `map`. What can't be laid is said in
/// `problems`; yours come back as `UserKey`s for `SPC h b`.
fn lay_chords(map: &mut ChordMap, data: &'static KeymapData, from: Option<&'static str>, known: &dyn Fn(&str) -> bool, source: ChordSource, problems: &mut Vec<String>) -> Vec<UserKey> {
    let mut bound = Vec::new();
    for entry in &data.keys {
        let key = match parse_keys(&entry.keys) {
            Ok(keys) if keys.len() == 1 => keys[0],
            Ok(_) => {
                problems.push(format!("{}: a [[keys]] entry is one key with its modifiers (C-s, F12); sequences go in [[leader]]", entry.keys));
                continue;
            }
            Err(e) => {
                if source == ChordSource::Fenix {
                    panic!("the built-in keymap: {e}");
                }
                problems.push(e);
                continue;
            }
        };
        let text = describe_keypress(&key);
        let Some(scheme) = Scheme::parse(entry.scheme.as_deref().unwrap_or("any")) else {
            problems.push(format!("{text}: scheme is vim, standard or any"));
            continue;
        };
        let Some(when) = When::parse(entry.when.as_deref().unwrap_or("anywhere")) else {
            problems.push(format!("{text}: when is anywhere, text, typing, not_typing, page or terminal"));
            continue;
        };
        if source == ChordSource::Extension && when == When::Terminal {
            problems.push(format!("{text}: only Fenix's keys and yours reach past a terminal"));
            continue;
        }
        if source == ChordSource::Extension && RESERVED_CHORDS.iter().any(|r| parse_key(r) == Some(key)) {
            problems.push(format!("{text} is Fenix's own and can't be taken"));
            continue;
        }
        let at = ChordContext { scheme: if scheme == Scheme::Any { Scheme::Vim } else { scheme }, ..ChordContext::default() };
        let replaced = map.lookup(key, &at);
        if entry.unbind {
            if replaced.is_none() && source == ChordSource::Yours {
                problems.push(format!("{text}: nothing to unbind"));
            }
            map.push(Chord { key, scheme, when, command: None, label: None, from });
            bound.push(UserKey { keys: text, command: None, replaced });
            continue;
        }
        let Some(command) = entry.command.as_deref() else {
            problems.push(format!("{text}: needs a command, or unbind = true"));
            continue;
        };
        if !known(command) {
            if source == ChordSource::Fenix {
                // A command of a feature that isn't built in: its key goes
                // with it, as the leader's do.
                continue;
            }
            problems.push(format!("{text}: {command} isn't a command (SPC h c lists them)"));
            continue;
        }
        map.push(Chord { key, scheme, when, command: Some(command), label: entry.label.as_deref(), from });
        bound.push(UserKey { keys: text, command: Some(command.to_string()), replaced: replaced.filter(|r| *r != command) });
    }
    bound
}

/// Chords no extension may bind: the leader's, the palette's, the
/// terminal's way out.
pub const RESERVED_CHORDS: &[&str] = &["C-k", "C-S-p", "C-\\", "M-x"];

/// Your file's `[[leader]]` and `[[local.*]]`, without its `[[keys]]`.
fn leader_only_of_with_local(data: &'static KeymapData) -> &'static KeymapData {
    let copy = |e: &KeyEntry| KeyEntry { keys: e.keys.clone(), label: e.label.clone(), command: e.command.clone(), group: e.group.clone(), section: e.section.clone(), unbind: e.unbind };
    let leader = data.leader.iter().map(copy).collect();
    let local = data.local.iter().map(|(k, v)| (k.clone(), v.iter().map(copy).collect())).collect();
    Box::leak(Box::new(KeymapData { leader, local, keys: Vec::new() }))
}

/// `data`'s `[[leader]]` entries alone: an extension's `[[local.*]]`
/// entries are read apart. Kept, like the file.
fn leader_only_of(data: &'static KeymapData) -> &'static KeymapData {
    let leader = data.leader.iter().map(|e| KeyEntry { keys: e.keys.clone(), label: e.label.clone(), command: e.command.clone(), group: e.group.clone(), section: e.section.clone(), unbind: e.unbind }).collect();
    Box::leak(Box::new(KeymapData { leader, local: Default::default(), keys: Vec::new() }))
}

/// Your keybindings: `keybindings.toml`, beside `settings.toml`. None
/// in tests, which mustn't read whoever runs them's file.
/// The installed extensions' keymap files (`keymaps/*.toml`), sorted.
fn extension_keymaps() -> Vec<std::path::PathBuf> {
    let mut files: Vec<std::path::PathBuf> = crate::installed::data_dirs("keymaps")
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.filter_map(|e| e.ok()).map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("toml")))
        .collect();
    files.sort();
    files
}

pub fn user_keymap_path() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    fenix_storage::paths::Roots::current().map(|roots| roots.settings_file().with_file_name("keybindings.toml"))
}

/// One of your bindings, as the map has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserKey {
    /// `SPC o s`.
    pub keys: String,
    /// What it runs; `None` when you unbound it.
    pub command: Option<String>,
    /// What Fenix had there, if anything.
    pub replaced: Option<&'static str>,
}

/// Your bindings, and what was wrong with the file, from its last read.
static USER_KEYS: std::sync::Mutex<(Vec<UserKey>, Vec<String>)> = std::sync::Mutex::new((Vec::new(), Vec::new()));

/// Your bindings in use (`SPC h b` marks them).
pub fn user_keys() -> Vec<UserKey> {
    USER_KEYS.lock().unwrap_or_else(|p| p.into_inner()).0.clone()
}

/// What's wrong with `keybindings.toml`, from its last read.
pub fn user_key_problems() -> Vec<String> {
    USER_KEYS.lock().unwrap_or_else(|p| p.into_inner()).1.clone()
}

/// Your file, parsed: `None` when there isn't one. It's kept for good
/// (the map's labels and commands borrow from it); a few are, one per
/// reload.
fn read_user_keymap(path: &std::path::Path) -> Result<Option<&'static KeymapData>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let data: KeymapData = toml::from_str(&text).map_err(|e| format!("keybindings.toml: {}", e.message()))?;
    Ok(Some(Box::leak(Box::new(data))))
}

/// Lays your `[[leader]]` entries over `t`: each binds keys to a command
/// (shown as its `label`), names a `group`, or (`unbind = true`) takes
/// Fenix's binding away. What can't be done is said, not done.
fn apply_user_keys(t: &mut KeyTrie<&'static str>, user: &'static KeymapData, known: &dyn Fn(&str) -> bool) -> (Vec<UserKey>, Vec<String>) {
    let mut bound = Vec::new();
    let mut problems = Vec::new();
    if !user.local.is_empty() {
        problems.push("[[local.*]] menus aren't yours to change yet -- only [[leader]] keys".to_string());
    }
    for entry in &user.leader {
        let keys = match parse_keys(&entry.keys) {
            Ok(keys) if keys.first() == Some(&KeyPress::char(' ')) && keys.len() > 1 => keys,
            Ok(_) => {
                problems.push(format!("{}: your keys start with SPC, then at least one more", entry.keys));
                continue;
            }
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        let text = keys.iter().map(describe_keypress).collect::<Vec<_>>().join(" ");
        if entry.unbind {
            let replaced = t.remove(&keys);
            if replaced.is_none() {
                problems.push(format!("{text}: nothing to unbind"));
            }
            bound.push(UserKey { keys: text, command: None, replaced });
            continue;
        }
        if let Some(group) = entry.group.as_deref() {
            t.label_group(&keys, group);
        }
        let Some(command) = entry.command.as_deref() else {
            if entry.group.is_none() {
                problems.push(format!("{text}: needs a command, a group, or unbind = true"));
            }
            continue;
        };
        if !known(command) {
            problems.push(format!("{text}: {command} isn't a command (SPC h c lists them)"));
            continue;
        }
        let replaced = t.get(&keys).copied().filter(|old| *old != command);
        t.insert(&keys, entry.label.as_deref().unwrap_or(command), command);
        bound.push(UserKey { keys: text, command: Some(command.to_string()), replaced });
    }
    (bound, problems)
}

/// The leader map: core's keys, less those bound to a command core
/// doesn't have (an extension's, whose keys come with it) -- and groups
/// and sections left with nothing under them.
fn build_leader() -> KeyTrie<&'static str> {
    let registry = crate::commands::CommandRegistry::with_builtins();
    let off: Vec<&str> = default_keymap().leader.iter().filter_map(|e| e.command.as_deref()).filter(|c| registry.get(c).is_none()).collect();
    let mut t = KeyTrie::new();
    insert_data_without(&mut t, &[], &default_keymap().leader, &off);
    t
}

/// Fenix's own keys as data: the leader map and core's `SPC m` entries
/// (see the file's own comment). Loaded through the same code your own
/// keymap and an extension's will be.
const DEFAULT_KEYMAP: &str = include_str!("default_keymap.toml");

/// A keymap file: `[[leader]]` entries, and `[[local.<menu>]]` entries for
/// a `SPC m` menu (keys after `SPC m`).
#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct KeymapData {
    #[serde(default)]
    leader: Vec<KeyEntry>,
    #[serde(default)]
    local: std::collections::HashMap<String, Vec<KeyEntry>>,
    /// Chords: `C-s`, `F12`, outside the leader.
    #[serde(default)]
    keys: Vec<ChordEntry>,
}

/// One `[[keys]]` entry: a chord bound to a command (or `unbind`), for a
/// key `scheme` (`vim`, `standard`, `any`), `when` it applies.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ChordEntry {
    keys: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    when: Option<String>,
    #[serde(default)]
    scheme: Option<String>,
    #[serde(default)]
    unbind: bool,
}

/// One entry: `keys` bound to `command` (shown as `label`), or naming a
/// `group`, and/or filed under a which-key `section`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyEntry {
    keys: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    section: Option<String>,
    /// Yours only (`keybindings.toml`): take Fenix's binding at `keys` away.
    #[serde(default)]
    unbind: bool,
}

fn default_keymap() -> &'static KeymapData {
    static DATA: OnceLock<KeymapData> = OnceLock::new();
    DATA.get_or_init(|| toml::from_str(DEFAULT_KEYMAP).unwrap_or_else(|e| panic!("the built-in keymap doesn't parse: {e}")))
}

/// Core's entries for `SPC m` menu `name`.
fn default_local(name: &str) -> &'static [KeyEntry] {
    default_keymap().local.get(name).map(Vec::as_slice).unwrap_or(&[])
}

/// Keys as the which-key menu writes them (`SPC f s`, `SPC Tab n`, `x`,
/// `SPC C-i`), and chords as `[[keys]]` writes them (`C-S-p`, `F12`,
/// `M-Left`, `<F2>`). Modifiers are `C-` (Ctrl), `M-` (Alt), `s-` (Super)
/// and `S-` (Shift); a named key may be written in angle brackets.
pub(crate) fn parse_keys(text: &str) -> Result<Vec<KeyPress>, String> {
    text.split_whitespace().map(|token| parse_key(token).ok_or_else(|| format!("`{token}` in `{text}` isn't a key"))).collect()
}

/// One key: `x`, `SPC`, `Tab`, `C-s`, `S-<Down>`, `F12`.
pub(crate) fn parse_key(token: &str) -> Option<KeyPress> {
    let mut mods = Mods::default();
    let mut rest = token;
    while let (Some(prefix), true) = (rest.get(..2), rest.chars().count() > 2) {
        match prefix {
            "C-" => mods.ctrl = true,
            "M-" => mods.alt = true,
            "s-" => mods.super_ = true,
            "S-" => mods.shift = true,
            _ => break,
        }
        rest = &rest[2..];
    }
    let rest = rest.strip_prefix('<').and_then(|r| r.strip_suffix('>')).unwrap_or(rest);
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        (Some(_), Some(_)) => named_key(rest)?,
        (None, _) => return None,
    };
    let mut key = KeyPress { code, mods };
    // A plain printable character carries its Shift in its case.
    if let KeyCode::Char(c) = key.code {
        if key.mods.shift && !key.mods.ctrl && !key.mods.alt {
            key.code = KeyCode::Char(c.to_uppercase().next().unwrap_or(c));
            key.mods.shift = false;
        }
    }
    Some(key.chord())
}

/// Lays `entries` into `t` under `prefix`.
fn insert_data(t: &mut KeyTrie<&'static str>, prefix: &[KeyPress], entries: &'static [KeyEntry]) {
    insert_data_without(t, prefix, entries, &[]);
}

/// `insert_data`, leaving out the bindings to `off` commands, and a group
/// or section that then has nothing under it.
fn insert_data_without(t: &mut KeyTrie<&'static str>, prefix: &[KeyPress], entries: &'static [KeyEntry], off: &[&str]) {
    let keys_of = |entry: &KeyEntry| -> Vec<KeyPress> {
        let keys = parse_keys(&entry.keys).unwrap_or_else(|e| panic!("the built-in keymap: {e}"));
        prefix.iter().copied().chain(keys).collect()
    };
    let bound: Vec<Vec<KeyPress>> = entries.iter().filter(|e| e.command.as_deref().is_some_and(|c| !off.contains(&c))).map(keys_of).collect();
    for entry in entries {
        if entry.command.as_deref().is_some_and(|c| off.contains(&c)) {
            continue;
        }
        if entry.command.is_none() && !off.is_empty() && !bound.iter().any(|b| b.starts_with(&keys_of(entry))) {
            continue;
        }
        let seq = keys_of(entry);
        if let Some(command) = entry.command.as_deref() {
            t.insert(&seq, entry.label.as_deref().unwrap_or(command), command);
        }
        if let Some(group) = entry.group.as_deref() {
            t.label_group(&seq, group);
        }
        if let Some(section) = entry.section.as_deref() {
            t.set_section(&seq, section);
        }
    }
}

/// Keys bound or named twice -- in the leader map, or in one `SPC m`
/// menu -- in core's keymap, as
/// `SPC f s: file.save and file.other`. A later one would silently win.
pub(crate) fn keymap_conflicts() -> Vec<String> {
    use std::collections::HashMap;
    fn note(seen: &mut HashMap<String, String>, conflicts: &mut Vec<String>, keys: String, what: String) {
        if let Some(before) = seen.insert(keys.clone(), what.clone()) {
            conflicts.push(format!("{keys}: {before} and {what}"));
        }
    }
    fn data(seen: &mut HashMap<String, String>, conflicts: &mut Vec<String>, prefix: &str, entries: &[KeyEntry]) {
        for e in entries {
            let keys = format!("{prefix}{}", e.keys);
            if let Some(command) = &e.command {
                note(seen, conflicts, keys.clone(), command.clone());
            }
            if let Some(group) = &e.group {
                note(seen, conflicts, keys, format!("group {group}"));
            }
        }
    }
    let mut conflicts = Vec::new();
    let mut leader = HashMap::new();
    data(&mut leader, &mut conflicts, "", &default_keymap().leader);
    let menus = [

        ("markdown", LocalContext::Markdown),
        ("tcl", LocalContext::Tcl),
        ("mermaid", LocalContext::Mermaid),
    ];
    for (name, context) in menus {
        let mut menu = HashMap::new();
        data(&mut menu, &mut conflicts, &format!("SPC m ({name}) "), default_local(context.name()));
    }
    // A chord bound twice for the same scheme and place.
    let mut chords: HashMap<String, String> = HashMap::new();
    for e in &default_keymap().keys {
        let Some(key) = parse_key(&e.keys) else { continue };
        let scheme = e.scheme.as_deref().unwrap_or("any");
        let when = e.when.as_deref().unwrap_or("anywhere");
        let what = e.command.clone().unwrap_or_else(|| "unbind".to_string());
        // A scheme's own entry over an `any` one is how a scheme differs:
        // only the same key twice for the same scheme is a clash.
        note(&mut chords, &mut conflicts, format!("{} ({scheme}, {when})", describe_keypress(&key)), what.clone());
    }
    conflicts
}

/// The keys that run `command` under `scheme`, as which-key writes them:
/// a chord in use (the shortest, yours over Fenix's), else its leader
/// keys, `SPC` written as `leader` (`C-k` with standard keys).
pub fn key_for(command: &str, scheme: Scheme, leader: &str) -> Option<String> {
    let mut chords: Vec<String> = chord_map().in_use(scheme).into_iter().filter(|c| c.command == Some(command)).map(|c| describe_keypress(&c.key)).collect();
    chords.sort_by_key(|k| (k.len(), k.clone()));
    if let Some(chord) = chords.into_iter().next() {
        return Some(chord);
    }
    let keys = leader_keys().get(command)?.clone();
    Some(if scheme == Scheme::Standard { crate::app::translate_leader(&keys, leader) } else { keys })
}

/// The leader sequence that runs each command, as the which-key menu
/// writes it (`SPC a a`): what `fenix/commands` reports. A command bound
/// twice gets its shortest sequence, the first in order among equals, so
/// the answer doesn't change from one run to the next.
pub fn leader_keys() -> std::collections::HashMap<&'static str, String> {
    let mut keys: std::collections::HashMap<&'static str, (usize, String)> = std::collections::HashMap::new();
    for (seq, id) in leader_trie().bindings() {
        let text = seq.iter().map(describe_keypress).collect::<Vec<_>>().join(" ");
        let candidate = (seq.len(), text);
        match keys.get(*id) {
            Some(best) if *best <= candidate => {}
            _ => {
                keys.insert(*id, candidate);
            }
        }
    }
    keys.into_iter().map(|(id, (_, text))| (id, text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chords that were an `if/else` in `route_keypress` before
    /// 1.5, as `[[keys]]`: every key that chain took, with and without
    /// Insert, runs what it ran then; nothing else is bound in Vim mode
    /// but `M-x`, which ran the palette from earlier in the same function.
    #[test]
    fn the_default_chords_are_what_route_keypress_ran_before() {
        let map = chord_map();
        let at = |typing: bool| ChordContext { scheme: Scheme::Vim, text: true, typing, ..ChordContext::default() };
        // What the old chain ran for Ctrl + `c` (Shift held or not), and
        // whether it did while typing.
        let old = |c: char, shift: bool, typing: bool| -> Option<&'static str> {
            let lower = c.to_ascii_lowercase();
            Some(match lower {
                's' => "file.save",
                'z' if shift => "edit.redo",
                'z' => "edit.undo",
                'y' => "edit.redo",
                'q' => "app.quit",
                '=' | '+' => "view.increase_font_size",
                '-' => "view.decrease_font_size",
                '0' => "view.reset_font_size",
                'o' if !typing => "nav.jump_back",
                'i' if !typing => "nav.jump_forward",
                _ => return None,
            })
        };
        for typing in [false, true] {
            for c in ('a'..='z').chain("0123456789=+-[];',./".chars()) {
                for shift in [false, true] {
                    let mut key = KeyPress::char(c).with_ctrl();
                    if shift {
                        key = key.with_shift();
                    }
                    assert_eq!(map.lookup(key, &at(typing)), old(c, shift, typing), "C-{}{c} (typing: {typing})", if shift { "S-" } else { "" });
                }
            }
            let named = |n: FenixNamedKey| KeyPress::named(n).with_ctrl();
            assert_eq!(map.lookup(named(FenixNamedKey::PageDown), &at(typing)), Some("tab.next"));
            assert_eq!(map.lookup(named(FenixNamedKey::PageUp), &at(typing)), Some("tab.prev"));
            assert_eq!(map.lookup(named(FenixNamedKey::Tab), &at(typing)), Some("tab.last"));
            for n in [FenixNamedKey::Enter, FenixNamedKey::Escape, FenixNamedKey::Left, FenixNamedKey::Backspace, FenixNamedKey::F(5)] {
                assert_eq!(map.lookup(KeyPress::named(n), &at(typing)), None, "{n:?}");
                assert_eq!(map.lookup(named(n), &at(typing)), None, "C-{n:?}");
            }
        }
        assert_eq!(map.lookup(KeyPress::char('x').with_alt(), &at(false)), Some("commands.palette"));
    }

    #[test]
    fn keys_parse_as_which_key_writes_them() {
        for text in ["C-s", "C-S-p", "M-Left", "F12", "S-Down", "C-PgDn", "s-e", "SPC", "Tab", "x", "C-+"] {
            let key = parse_key(text).unwrap_or_else(|| panic!("{text}"));
            assert_eq!(describe_keypress(&key), text, "{text} round-trips");
        }
        assert_eq!(parse_key("<F2>"), Some(KeyPress::named(FenixNamedKey::F(2))));
        assert_eq!(parse_key("C-P"), parse_key("C-S-p"), "a Ctrl letter's case is its Shift");
        assert_eq!(parse_key("S-a"), Some(KeyPress::char('A')), "a plain letter's Shift is its case");
        assert_eq!(parse_key("escape"), Some(KeyPress::named(FenixNamedKey::Escape)));
        assert_eq!(parse_key("F13"), None);
        assert_eq!(parse_keys("SPC C-i").unwrap(), vec![KeyPress::char(' '), KeyPress::char('i').with_ctrl()]);
        assert!(parse_keys("SPC blah").is_err());
    }

    #[test]
    fn your_chords_go_over_fenix_s_and_say_what_s_wrong() {
        let leak = |text: &str| -> &'static KeymapData { Box::leak(Box::new(toml::from_str(text).unwrap())) };
        let known = |id: &str| ["file.save", "commands.palette", "project.find_file"].contains(&id);
        let mut map = ChordMap::new();
        lay_chords(&mut map, leak("[[keys]]\nkeys = \"C-s\"\ncommand = \"file.save\"\n"), None, &known, ChordSource::Fenix, &mut Vec::new());
        let mut problems = Vec::new();
        let yours = leak(
            "[[keys]]\nkeys = \"C-s\"\nunbind = true\n\n\
             [[keys]]\nkeys = \"C-p\"\ncommand = \"project.find_file\"\nscheme = \"standard\"\n\n\
             [[keys]]\nkeys = \"C-e\"\ncommand = \"nope.nothing\"\n\n\
             [[keys]]\nkeys = \"C-x C-s\"\ncommand = \"file.save\"\n\n\
             [[keys]]\nkeys = \"C-t\"\ncommand = \"file.save\"\nwhen = \"sometimes\"\n",
        );
        let bound = lay_chords(&mut map, yours, Some("keybindings.toml"), &known, ChordSource::Yours, &mut problems);
        let vim = ChordContext { scheme: Scheme::Vim, ..ChordContext::default() };
        let standard = ChordContext { scheme: Scheme::Standard, ..ChordContext::default() };
        assert_eq!(map.lookup(KeyPress::char('s').with_ctrl(), &vim), None, "unbound");
        assert_eq!(map.lookup(KeyPress::char('p').with_ctrl(), &standard), Some("project.find_file"));
        assert_eq!(map.lookup(KeyPress::char('p').with_ctrl(), &vim), None, "standard's only");
        assert_eq!(bound.len(), 2, "{bound:?}");
        assert_eq!(bound[0], UserKey { keys: "C-s".into(), command: None, replaced: Some("file.save") });
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems[0].contains("nope.nothing") && problems[1].contains("one key") && problems[2].contains("when is"), "{problems:?}");
    }

    #[test]
    fn an_extension_cant_take_fenix_s_own_chords_or_reach_past_a_terminal() {
        let leak = |text: &str| -> &'static KeymapData { Box::leak(Box::new(toml::from_str(text).unwrap())) };
        let mut map = ChordMap::new();
        let mut problems = Vec::new();
        let theirs = leak("[[keys]]\nkeys = \"C-k\"\ncommand = \"file.save\"\n\n[[keys]]\nkeys = \"C-p\"\ncommand = \"file.save\"\nwhen = \"terminal\"\n\n[[keys]]\nkeys = \"C-Enter\"\ncommand = \"file.save\"\n");
        lay_chords(&mut map, theirs, Some("database"), &|_| true, ChordSource::Extension, &mut problems);
        assert_eq!(problems.len(), 2, "{problems:?}");
        let at = ChordContext { scheme: Scheme::Vim, ..ChordContext::default() };
        assert_eq!(map.lookup(KeyPress::named(FenixNamedKey::Enter).with_ctrl(), &at), Some("file.save"));
    }

    /// Fenix's own chords name commands Fenix has: a typo'd one would be
    /// dropped without a word (as a feature's key is when it's off).
    #[test]
    fn every_default_chord_runs_a_command_fenix_has() {
        let registry = crate::commands::CommandRegistry::with_builtins();
        let missing: Vec<&str> = default_keymap().keys.iter().filter_map(|e| e.command.as_deref()).filter(|c| registry.get(c).is_none()).collect();
        assert!(missing.is_empty(), "{missing:?}");
        // Standard keys have the shortcuts the plan promised.
        let map = chord_map();
        let standard = ChordContext { scheme: Scheme::Standard, text: true, ..ChordContext::default() };
        for (key, id) in [("C-c", "edit.copy"), ("C-v", "edit.paste"), ("C-x", "edit.cut"), ("C-p", "project.find_file"), ("C-S-p", "commands.palette"), ("F12", "code.goto_definition"), ("F2", "code.lsp_rename"), ("C-f", "search.file"), ("C-/", "code.toggle_comment"), ("C-w", "tab.close"), ("C-Tab", "buffer.switch_recent"), ("C-0", "window.focus_sidebar")] {
            assert_eq!(map.lookup(parse_key(key).unwrap(), &standard), Some(id), "{key}");
        }
        // Every leader key is reached with standard keys too: Ctrl+K is SPC.
        assert!(RESERVED_CHORDS.contains(&"C-k"));
    }

    #[test]
    fn fenix_s_keymap_binds_nothing_twice() {
        assert_eq!(keymap_conflicts(), Vec::<String>::new());
    }

    #[test]
    fn describe_keypress_formats_plain_and_modified_chars() {
        assert_eq!(describe_keypress(&KeyPress::char('f')), "f");
        assert_eq!(describe_keypress(&KeyPress::char(' ')), "SPC");
        assert_eq!(describe_keypress(&KeyPress::char('r').with_ctrl()), "C-r");
    }

    #[test]
    fn describe_keypress_formats_named_keys() {
        assert_eq!(describe_keypress(&KeyPress::named(FenixNamedKey::Escape)), "Esc");
    }

    /// `SPC g P` and `SPC g M` are the Review extension's: their section is
    /// named ahead in Fenix's keymap, but without the extension there's no
    /// key to show, so the menu has no blank row for them.
    #[test]
    fn an_extension_s_keys_are_not_offered_without_it() {
        let mut m = leader_trie().matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('g'));
        let keys: Vec<KeyPress> = m.pending_hints().iter().map(|h| h.key).collect();
        assert!(!keys.contains(&KeyPress::char('P')) && !keys.contains(&KeyPress::char('M')), "{keys:?}");
        assert!(m.pending_hints().iter().all(|h| !h.label.is_empty()), "no blank row");
    }

    #[test]
    fn spc_g_is_short_and_conflicts_have_a_group_of_their_own() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('g'));
        let listed = m.pending_children();
        assert!(listed.len() <= 22, "{} entries under SPC g: {listed:?}", listed.len());
        let resolve = |keys: &str| {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            let mut last = None;
            for c in keys.chars() {
                if let fenix_keymap::Step::Matched(action) = m.feed(KeyPress::char(c)) {
                    last = Some(*action);
                }
            }
            last
        };
        assert_eq!(resolve("gxx"), Some("git.merge_view"));
        assert_eq!(resolve("gxc"), Some("git.continue"));
        assert_eq!(resolve("gxo"), Some("git.keep_ours"));
        assert_eq!(resolve("gq"), Some("git.close_view"));
        assert_eq!(resolve("gP"), None, "the review extension's, with it");
    }

    #[test]
    fn leader_trie_resolves_save_and_quit() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('f'));
        match m.feed(KeyPress::char('s')) {
            fenix_keymap::Step::Matched(&"file.save") => {}
            _ => panic!("expected SPC f s to resolve to file.save"),
        }
    }

    #[test]
    fn leader_trie_resolves_line_number_toggle() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('t'));
        match m.feed(KeyPress::char('n')) {
            fenix_keymap::Step::Matched(&"view.cycle_line_numbers") => {}
            _ => panic!("expected SPC t n to resolve to view.cycle_line_numbers"),
        }
    }

    #[test]
    fn leader_trie_resolves_theme_picker() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('t'));
        match m.feed(KeyPress::char('p')) {
            fenix_keymap::Step::Matched(&"view.pick_theme") => {}
            _ => panic!("expected SPC t p to resolve to view.pick_theme"),
        }
    }

    #[test]
    fn leader_trie_resolves_fullscreen_toggle() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('t'));
        match m.feed(KeyPress::char('f')) {
            fenix_keymap::Step::Matched(&"view.toggle_fullscreen") => {}
            _ => panic!("expected SPC t f to resolve to view.toggle_fullscreen"),
        }
    }

    #[test]
    fn leader_trie_resolves_animations_toggle() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('t'));
        match m.feed(KeyPress::char('a')) {
            fenix_keymap::Step::Matched(&"view.toggle_animations") => {}
            _ => panic!("expected SPC t a to resolve to view.toggle_animations"),
        }
    }

    #[test]
    fn leader_trie_resolves_font_size_adjustments() {
        let trie = leader_trie();

        let resolve = |key: char| {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('t'));
            m.feed(KeyPress::char(key))
        };

        assert!(matches!(resolve('='), fenix_keymap::Step::Matched(&"view.increase_font_size")));
        assert!(matches!(resolve('-'), fenix_keymap::Step::Matched(&"view.decrease_font_size")));
        assert!(matches!(resolve('0'), fenix_keymap::Step::Matched(&"view.reset_font_size")));
    }

    #[test]
    fn leader_trie_resolves_explorer_jump_and_sidebar_toggle() {
        let trie = leader_trie();

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('f'));
        match m.feed(KeyPress::char('j')) {
            fenix_keymap::Step::Matched(&"explorer.jump") => {}
            _ => panic!("expected SPC f j to resolve to explorer.jump"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('f'));
        match m.feed(KeyPress::char('t')) {
            fenix_keymap::Step::Matched(&"table.toggle") => {}
            _ => panic!("expected SPC f t to resolve to table.toggle"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('e'));
        match m.feed(KeyPress::char('t')) {
            fenix_keymap::Step::Matched(&"explorer.toggle_sidebar") => {}
            _ => panic!("expected SPC e t to resolve to explorer.toggle_sidebar"),
        }
    }

    #[test]
    fn leader_trie_resolves_double_space_to_find_file() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        match m.feed(KeyPress::char(' ')) {
            fenix_keymap::Step::Matched(&"project.find_file") => {}
            _ => panic!("expected SPC SPC to resolve to project.find_file"),
        }
    }

    #[test]
    fn leader_trie_resolves_project_find_file_grep_and_switch_project() {
        let trie = leader_trie();

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('f')) {
            fenix_keymap::Step::Matched(&"project.find_file") => {}
            _ => panic!("expected SPC p f to resolve to project.find_file"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('s')) {
            fenix_keymap::Step::Matched(&"project.grep") => {}
            _ => panic!("expected SPC p s to resolve to project.grep"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('n')) {
            fenix_keymap::Step::Matched(&"project.quickfix_next") => {}
            _ => panic!("expected SPC p n to resolve to project.quickfix_next"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('N')) {
            fenix_keymap::Step::Matched(&"project.quickfix_prev") => {}
            _ => panic!("expected SPC p N to resolve to project.quickfix_prev"),
        }

        for (key, id) in [('p', "project.hub"), ('P', "project.switch_project"), ('c', "project.new"), ('h', "project.doctor"), (',', "project.settings")] {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('p'));
            match m.feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&found) => assert_eq!(found, id, "SPC p {key}"),
                _ => panic!("expected SPC p {key} to resolve to {id}"),
            }
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('a')) {
            fenix_keymap::Step::Matched(&"project.add") => {}
            _ => panic!("expected SPC p a to resolve to project.add"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('d')) {
            fenix_keymap::Step::Matched(&"project.delete") => {}
            _ => panic!("expected SPC p d to resolve to project.delete"),
        }
    }

    #[test]
    fn leader_trie_resolves_dashboard_open() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('o'));
        match m.feed(KeyPress::char('d')) {
            fenix_keymap::Step::Matched(&"dashboard.open") => {}
            _ => panic!("expected SPC o d to resolve to dashboard.open"),
        }
    }

    #[test]
    fn leader_trie_resolves_terminal_toggle() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('o'));
        match m.feed(KeyPress::char('t')) {
            fenix_keymap::Step::Matched(&"terminal.toggle") => {}
            _ => panic!("expected SPC o t to resolve to terminal.toggle"),
        }
    }

    #[test]
    fn leader_trie_resolves_the_pane_terminal() {
        // A capital `T` beside the panel's own `t` -- the two shells
        // answer different questions (see `TerminalTarget`), so they get
        // adjacent keys rather than one overloaded one.
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('o'));
        match m.feed(KeyPress::char('T')) {
            fenix_keymap::Step::Matched(&"terminal.open_buffer") => {}
            _ => panic!("expected SPC o T to resolve to terminal.open_buffer"),
        }
    }

    /// An installed extension's keymap goes over Fenix's, and yours over
    /// it -- `reload_leader`'s order -- with only yours counted as yours.
    #[test]
    fn an_extensions_keys_sit_between_fenixs_and_yours() {
        let mut t = build_leader();
        let leak = |text: &str| -> &'static KeymapData { Box::leak(Box::new(toml::from_str(text).unwrap())) };
        let known = |id: &str| id == "file.save" || id == "agenda.open";
        let extension = leak("[[leader]]\nkeys = \"SPC o z\"\ncommand = \"file.save\"\n\n[[leader]]\nkeys = \"SPC o y\"\ncommand = \"agenda.open\"\n");
        apply_user_keys(&mut t, extension, &known);
        let yours = leak("[[leader]]\nkeys = \"SPC o z\"\ncommand = \"agenda.open\"\n");
        let (bound, _) = apply_user_keys(&mut t, yours, &known);
        let seq = |text: &str| parse_keys(text).unwrap();
        assert_eq!(t.get(&seq("SPC o y")), Some(&"agenda.open"), "the extension's");
        assert_eq!(t.get(&seq("SPC o z")), Some(&"agenda.open"), "yours over the extension's");
        assert_eq!(bound.iter().map(|b| b.keys.as_str()).collect::<Vec<_>>(), ["SPC o z"]);
    }

    /// Your keys go over Fenix's: a new binding, one moved to another
    /// command, one taken away, and what can't be done said.
    #[test]
    fn your_keys_go_over_the_defaults() {
        let mut t = build_leader();
        let user: &'static KeymapData = Box::leak(Box::new(
            toml::from_str(
                r#"
[[leader]]
keys = "SPC o s"
command = "file.save"
label = "save it"

[[leader]]
keys = "SPC f s"
command = "agenda.open"

[[leader]]
keys = "SPC o T"
unbind = true

[[leader]]
keys = "SPC o x"
command = "no.such"

[[leader]]
keys = "x y"
command = "file.save"
"#,
            )
            .unwrap(),
        ));
        let known = |id: &str| id == "file.save" || id == "agenda.open";
        let (bound, problems) = apply_user_keys(&mut t, user, &known);
        let seq = |text: &str| parse_keys(text).unwrap();
        assert_eq!(t.get(&seq("SPC o s")), Some(&"file.save"));
        assert_eq!(t.get(&seq("SPC f s")), Some(&"agenda.open"));
        assert_eq!(t.get(&seq("SPC o T")), None);
        assert!(bound.contains(&UserKey { keys: "SPC f s".into(), command: Some("agenda.open".into()), replaced: Some("file.save") }), "{bound:?}");
        assert!(bound.iter().any(|b| b.keys == "SPC o T" && b.command.is_none() && b.replaced == Some("terminal.open_buffer")), "{bound:?}");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("no.such isn't a command")) && problems.iter().any(|p| p.contains("start with SPC")));
    }

    #[test]
    fn spc_v_is_left_to_the_vnc_extension() {
        // VNC is an extension: its keymap brings `SPC v`.
        let taken: Vec<String> = leader_keys().into_values().filter(|k| k.starts_with("SPC v ")).collect();
        assert!(taken.is_empty(), "{taken:?}");
    }

    #[test]
    fn leader_trie_resolves_lsp_rename_and_code_action() {
        let trie = leader_trie();

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('r')) {
            fenix_keymap::Step::Matched(&"code.lsp_rename") => {}
            _ => panic!("expected SPC c r to resolve to code.lsp_rename"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('a')) {
            fenix_keymap::Step::Matched(&"code.lsp_code_action") => {}
            _ => panic!("expected SPC c a to resolve to code.lsp_code_action"),
        }
    }

    #[test]
    fn leader_trie_resolves_format_selection_and_format_buffer() {
        let trie = leader_trie();

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('f')) {
            fenix_keymap::Step::Matched(&"code.format_selection") => {}
            _ => panic!("expected SPC c f to resolve to code.format_selection"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('F')) {
            fenix_keymap::Step::Matched(&"code.format_buffer") => {}
            _ => panic!("expected SPC c F to resolve to code.format_buffer"),
        }
    }

    #[test]
    fn leader_trie_resolves_toggle_checkbox() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('x')) {
            fenix_keymap::Step::Matched(&"code.toggle_checkbox") => {}
            _ => panic!("expected SPC c x to resolve to code.toggle_checkbox"),
        }
    }

    #[test]
    fn leader_trie_resolves_outline() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('o')) {
            fenix_keymap::Step::Matched(&"code.outline") => {}
            _ => panic!("expected SPC c o to resolve to code.outline"),
        }
    }

    #[test]
    fn spc_m_is_the_local_leader_and_nothing_else_lives_under_it() {
        let mut m = leader_trie().matcher();
        m.feed(KeyPress::char(' '));
        assert!(matches!(m.feed(KeyPress::char('m')), fenix_keymap::Step::Matched(&"leader.local")));
    }

    #[test]
    fn a_more_specific_context_wins_a_shared_key() {
        // An extension's Tcl menu and Fenix's own Markdown one both bind
        // `p`.
        let (_, problems) = extension_local_menu_for_tests("tcl", "[[local.tcl]]
keys = \"p\"
label = \"a parameter\"
command = \"hello.parameter\"

[[local.tcl]]
keys = \"k\"
label = \"a packet\"
command = \"hello.packet\"
");
        assert!(problems.is_empty(), "{problems:?}");
        let tcl_first = local_trie(&[LocalContext::Tcl, LocalContext::Markdown]);
        assert!(matches!(tcl_first.matcher().feed(KeyPress::char('p')), fenix_keymap::Step::Matched(&"hello.parameter")));
        let markdown_first = local_trie(&[LocalContext::Markdown, LocalContext::Tcl]);
        assert!(matches!(markdown_first.matcher().feed(KeyPress::char('p')), fenix_keymap::Step::Matched(&"notebook.preview")));
        assert!(matches!(markdown_first.matcher().feed(KeyPress::char('k')), fenix_keymap::Step::Matched(_)), "the other's keys stay");
        assert!(std::ptr::eq(tcl_first, local_trie(&[LocalContext::Tcl, LocalContext::Markdown])), "built once per combination");
    }

    #[test]
    fn leader_trie_resolves_window_split_navigate_and_close_commands() {
        let trie = leader_trie();

        let resolve = |keys: &[char]| {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('w'));
            let mut last = None;
            for &k in keys {
                last = Some(m.feed(KeyPress::char(k)));
            }
            last.unwrap()
        };

        assert!(matches!(resolve(&['v']), fenix_keymap::Step::Matched(&"window.split_vertical")));
        assert!(matches!(resolve(&['s']), fenix_keymap::Step::Matched(&"window.split_horizontal")));
        assert!(matches!(resolve(&['h']), fenix_keymap::Step::Matched(&"window.navigate_left")));
        assert!(matches!(resolve(&['l']), fenix_keymap::Step::Matched(&"window.navigate_right")));
        assert!(matches!(resolve(&['k']), fenix_keymap::Step::Matched(&"window.navigate_up")));
        assert!(matches!(resolve(&['j']), fenix_keymap::Step::Matched(&"window.navigate_down")));
        assert!(matches!(resolve(&['w']), fenix_keymap::Step::Matched(&"window.cycle")));
        assert!(matches!(resolve(&['q']), fenix_keymap::Step::Matched(&"window.close")));
        assert!(matches!(resolve(&['o']), fenix_keymap::Step::Matched(&"window.only")));
        assert!(matches!(resolve(&['=']), fenix_keymap::Step::Matched(&"window.balance")));
    }

    #[test]
    fn leader_trie_resolves_buffer_switch_next_prev_kill_and_scratch() {
        let trie = leader_trie();

        let resolve = |keys: &[char]| {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('b'));
            let mut last = None;
            for &k in keys {
                last = Some(m.feed(KeyPress::char(k)));
            }
            last.unwrap()
        };

        assert!(matches!(resolve(&['b']), fenix_keymap::Step::Matched(&"buffer.switch")));
        assert!(matches!(resolve(&['n']), fenix_keymap::Step::Matched(&"buffer.next")));
        assert!(matches!(resolve(&['p']), fenix_keymap::Step::Matched(&"buffer.prev")));
        assert!(matches!(resolve(&['k']), fenix_keymap::Step::Matched(&"buffer.kill")));
        assert!(matches!(resolve(&['d']), fenix_keymap::Step::Matched(&"tab.close")));
        assert!(matches!(resolve(&['o']), fenix_keymap::Step::Matched(&"tab.close_others")));
        assert!(matches!(resolve(&['u']), fenix_keymap::Step::Matched(&"tab.reopen")));
        assert!(matches!(resolve(&['P']), fenix_keymap::Step::Matched(&"tab.keep")));
        assert!(matches!(resolve(&['<']), fenix_keymap::Step::Matched(&"tab.move_left")));
        assert!(matches!(resolve(&['>']), fenix_keymap::Step::Matched(&"tab.move_right")));
        assert!(matches!(resolve(&['X']), fenix_keymap::Step::Matched(&"buffer.scratch")));
    }

    #[test]
    fn leader_trie_resolves_workspace_new_next_prev_and_remove() {
        let trie = leader_trie();
        let tab = KeyPress::named(FenixNamedKey::Tab);

        let resolve = |key: KeyPress| {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(tab);
            m.feed(key)
        };

        assert!(matches!(resolve(KeyPress::char('n')), fenix_keymap::Step::Matched(&"workspace.new")));
        assert!(matches!(resolve(KeyPress::char(']')), fenix_keymap::Step::Matched(&"workspace.next")));
        assert!(matches!(resolve(KeyPress::char('[')), fenix_keymap::Step::Matched(&"workspace.prev")));
        assert!(matches!(resolve(KeyPress::char('d')), fenix_keymap::Step::Matched(&"workspace.remove")));
        assert!(matches!(resolve(tab), fenix_keymap::Step::Matched(&"workspace.switch")));
        assert!(matches!(resolve(KeyPress::char('f')), fenix_keymap::Step::Matched(&"workspace.find")));
        assert!(matches!(resolve(KeyPress::char('r')), fenix_keymap::Step::Matched(&"workspace.rename")));
    }

    // The default keymap is data (`default_keymap.toml`).

    #[test]
    fn the_default_keymap_has_no_key_bound_twice() {
        assert_eq!(keymap_conflicts(), Vec::<String>::new());
    }

    #[test]
    fn every_key_in_the_default_keymap_parses_and_writes_back_the_same() {
        let data = default_keymap();
        let all = data.leader.iter().chain(data.local.values().flatten());
        for entry in all {
            let keys = parse_keys(&entry.keys).unwrap_or_else(|e| panic!("{e}"));
            let written = keys.iter().map(describe_keypress).collect::<Vec<_>>().join(" ");
            assert_eq!(written, entry.keys);
        }
        assert!(parse_keys("SPC ff").is_err() && parse_keys("SPC <nope>").is_err());
    }

    #[test]
    fn every_command_the_default_keymap_names_exists() {
        let registry = crate::commands::CommandRegistry::with_builtins();
        let data = default_keymap();
        for command in data.leader.iter().chain(data.local.values().flatten()).filter_map(|e| e.command.as_deref()) {
            assert!(registry.iter().any(|c| c.id == command), "{command} isn't a command");
        }
    }

    #[test]
    fn a_conflict_between_two_bindings_of_one_key_is_reported() {
        let text = "[[leader]]
keys = \"SPC x\"
command = \"one\"

[[leader]]
keys = \"SPC x\"
command = \"two\"
";
        let data: KeymapData = toml::from_str(text).unwrap();
        let mut seen = std::collections::HashMap::new();
        let mut conflicts = Vec::new();
        for e in &data.leader {
            if let Some(before) = seen.insert(e.keys.clone(), e.command.clone()) {
                conflicts.push((before, e.command.clone()));
            }
        }
        assert_eq!(conflicts.len(), 1);
        assert!(toml::from_str::<KeymapData>("[[leader]]
keys = \"SPC x\"
what = 1
").is_err(), "an unknown field is refused");
    }
}
