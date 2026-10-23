use std::sync::OnceLock;

use fenix_keymap::{KeyCode, KeyPress, KeyTrie, Mods, NamedKey as FenixNamedKey};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey, PhysicalKey};

/// Translates a winit key event into fenix-keymap's UI-agnostic `KeyPress`.
/// Named `Space` is normalized to `KeyCode::Char(' ')` -- treating it like
/// any other printable key keeps the leader trie's sequences (`SPC f s`)
/// just a plain char sequence, no special-casing needed downstream.
/// Returns `None` for keys with nothing sensible to bind (F-keys, media
/// keys, ...).
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
        Key::Character(s) => KeyCode::Char(s.chars().next()?),
        _ => return None,
    };
    Some(KeyPress {
        code,
        mods: Mods { ctrl: mods.control_key(), alt: mods.alt_key(), super_: mods.super_key() },
    })
}

/// A short human-readable label for a keypress, for the which-key popup
/// (`SPC` rather than a literal space, `C-r` for Ctrl-r, `Esc` for Escape).
pub fn describe_keypress(kp: &KeyPress) -> String {
    let mut s = String::new();
    if kp.mods.ctrl {
        s.push_str("C-");
    }
    if kp.mods.alt {
        s.push_str("M-");
    }
    if kp.mods.super_ {
        s.push_str("S-");
    }
    match kp.code {
        KeyCode::Char(' ') => s.push_str("SPC"),
        KeyCode::Char(c) => s.push(c),
        KeyCode::Named(FenixNamedKey::Escape) => s.push_str("Esc"),
        KeyCode::Named(FenixNamedKey::Enter) => s.push_str("Enter"),
        KeyCode::Named(FenixNamedKey::Tab) => s.push_str("Tab"),
        KeyCode::Named(FenixNamedKey::Backspace) => s.push_str("Backspace"),
        KeyCode::Named(FenixNamedKey::Delete) => s.push_str("Delete"),
        KeyCode::Named(FenixNamedKey::Left) => s.push_str("Left"),
        KeyCode::Named(FenixNamedKey::Right) => s.push_str("Right"),
        KeyCode::Named(FenixNamedKey::Up) => s.push_str("Up"),
        KeyCode::Named(FenixNamedKey::Down) => s.push_str("Down"),
        KeyCode::Named(FenixNamedKey::Home) => s.push_str("Home"),
        KeyCode::Named(FenixNamedKey::End) => s.push_str("End"),
        KeyCode::Named(FenixNamedKey::PageUp) => s.push_str("PgUp"),
        KeyCode::Named(FenixNamedKey::PageDown) => s.push_str("PgDn"),
    }
    s
}

/// A kind of buffer or project `SPC m` has bindings for. A buffer can be
/// in several at once (a Tcl file in an Arduino sketch would be both);
/// `App::local_contexts` lists them most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalContext {
    /// Anything inside an Arduino sketch: build, upload, the serial
    /// monitor, boards, ports, libraries.
    Arduino,
    /// Tcl files: the SCOS-2000 MIB lookups and insertion (letters kept
    /// from the reference elisp implementation's own `SPC M` scheme),
    /// plus Tcl's ctags-based symbols.
    Tcl,
    /// A PDF pane: the reader's commands, for finding them without
    /// knowing its keys.
    Reader,
    /// Markdown (notes, READMEs): the reading view.
    Markdown,
    /// A Mermaid diagram (`.mmd`) or its preview.
    Mermaid,
}

impl LocalContext {
    /// Shown when `SPC m` opens, so you know which menu you're in.
    pub fn name(self) -> &'static str {
        match self {
            LocalContext::Arduino => "arduino",
            LocalContext::Tcl => "tcl",
            LocalContext::Reader => "reader",
            LocalContext::Markdown => "markdown",
            LocalContext::Mermaid => "diagram",
        }
    }

    fn bind(self, t: &mut KeyTrie<&'static str>) {
        match self {
            LocalContext::Arduino => insert_spec_menu(t, &crate::app::EMBEDDED_SPEC),
            LocalContext::Reader => insert_data(t, &[], default_local("reader")),
            LocalContext::Markdown => {
                insert_data(t, &[], default_local("markdown"));
                insert_spec_menu(t, &crate::app::NOTEBOOK_SPEC);
            }
            LocalContext::Mermaid => insert_spec_menu(t, &crate::app::DIAGRAMS_SPEC),
            LocalContext::Tcl => {
                insert_spec_menu(t, &crate::app::MIB_SPEC);
                insert_spec_menu(t, &crate::app::TCL_SPEC);
            }
        }
    }
}

/// The `SPC m` menu for `contexts` (most specific first): each context's
/// bindings, with a more specific context winning a key both bind. Built
/// once per combination and kept -- there are only ever a handful.
pub fn local_trie(contexts: &[LocalContext]) -> &'static KeyTrie<&'static str> {
    static TRIES: OnceLock<std::sync::Mutex<std::collections::HashMap<Vec<LocalContext>, &'static KeyTrie<&'static str>>>> = OnceLock::new();
    let mut tries = TRIES.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
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

/// Adds a feature's leader groups and bindings (`FeatureSpec::leader`)
/// under `spc`.
fn insert_feature_keys(t: &mut KeyTrie<&'static str>, spc: KeyPress, spec: &crate::app::feature::FeatureSpec) {
    insert_entries(t, &[spc], spec.leader);
}

/// A feature's `SPC m` entries, when it's switched on.
fn insert_spec_menu(t: &mut KeyTrie<&'static str>, spec: &crate::app::feature::FeatureSpec) {
    if crate::app::feature::enabled(spec.id) {
        insert_entries(t, &[], spec.local_menu);
    }
}

/// Adds `entries` (a feature's leader or `SPC m` keys) under `prefix`.
fn insert_entries(t: &mut KeyTrie<&'static str>, prefix: &[KeyPress], entries: &[crate::app::feature::LeaderEntry]) {
    use crate::app::feature::LeaderEntry;
    let seq = |keys: &[char]| prefix.iter().copied().chain(keys.iter().map(|&c| KeyPress::char(c))).collect::<Vec<_>>();
    for entry in entries {
        match entry {
            LeaderEntry::Group(keys, label) => t.label_group(&seq(keys), label),
            LeaderEntry::Bind(keys, label, command) => t.insert(&seq(keys), label, *command),
        }
    }
}

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
    let mut t = build_leader(&crate::app::feature::enabled);
    let registry = crate::commands::CommandRegistry::with_builtins();
    let known = |id: &str| registry.get(id).is_some();
    let (bound, problems) = match user_keymap_path() {
        Some(path) => match read_user_keymap(&path) {
            Ok(Some(user)) => apply_user_keys(&mut t, user, &known),
            Ok(None) => (Vec::new(), Vec::new()),
            Err(problem) => (Vec::new(), vec![problem]),
        },
        None => (Vec::new(), Vec::new()),
    };
    *USER_KEYS.lock().unwrap_or_else(|p| p.into_inner()) = (bound, problems.clone());
    *LEADER.write().unwrap_or_else(|p| p.into_inner()) = Some(Box::leak(Box::new(t)));
    problems
}

/// Your keybindings: `keybindings.toml`, beside `settings.toml`. None
/// in tests, which mustn't read whoever runs them's file.
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

/// The leader map with the features `on` says are on: core's keys, less
/// those bound to a switched-off feature's commands (and groups and
/// sections left with nothing under them), then the features' own.
fn build_leader(on: &dyn Fn(&str) -> bool) -> KeyTrie<&'static str> {
    let off = crate::app::feature::commands_switched_off(on);
    let mut t = KeyTrie::new();
    insert_data_without(&mut t, &[], &default_keymap().leader, &off, on);
    let spc = KeyPress::char(' ');
    for spec in LEADER_FEATURES.iter().filter(|spec| on(spec.id)) {
        insert_feature_keys(&mut t, spc, spec);
    }
    t
}

/// The features whose keys go under the leader, after core's.
const LEADER_FEATURES: &[&crate::app::feature::FeatureSpec] = &[
    &crate::app::CCSDS_SPEC,
    // Global rather than under `SPC m`: `SPC m` only has the Arduino
    // menu inside a sketch, and your first sketch has to come from
    // somewhere.
    &crate::app::EMBEDDED_SPEC,
    #[cfg(feature = "docker")]
    &crate::app::DOCKER_SPEC,
    &crate::app::REVIEW_SPEC,
    &crate::app::JIRA_SPEC,
    &crate::app::NOTEBOOK_SPEC,
    &crate::app::AGENDA_SPEC,
    &crate::app::VNC_SPEC,
    &crate::app::PDF_SPEC,
];

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

/// Keys as the which-key menu writes them (`SPC f s`, `SPC Tab n`, `x`).
pub(crate) fn parse_keys(text: &str) -> Result<Vec<KeyPress>, String> {
    text.split_whitespace()
        .map(|token| match token {
            "SPC" => Ok(KeyPress::char(' ')),
            "Tab" => Ok(KeyPress::named(FenixNamedKey::Tab)),
            _ => {
                let mut chars = token.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(KeyPress::char(c)),
                    _ => Err(format!("`{token}` in `{text}` isn't a key")),
                }
            }
        })
        .collect()
}

/// Lays `entries` into `t` under `prefix`.
fn insert_data(t: &mut KeyTrie<&'static str>, prefix: &[KeyPress], entries: &'static [KeyEntry]) {
    insert_data_without(t, prefix, entries, &[], &|_| true);
}

/// `insert_data`, leaving out the bindings to `off` commands, and a group
/// or section that then has nothing under it.
fn insert_data_without(t: &mut KeyTrie<&'static str>, prefix: &[KeyPress], entries: &'static [KeyEntry], off: &[&str], on: &dyn Fn(&str) -> bool) {
    let keys_of = |entry: &KeyEntry| -> Vec<KeyPress> {
        let keys = parse_keys(&entry.keys).unwrap_or_else(|e| panic!("the built-in keymap: {e}"));
        prefix.iter().copied().chain(keys).collect()
    };
    let bound: Vec<Vec<KeyPress>> = entries.iter().filter(|e| e.command.as_deref().is_some_and(|c| !off.contains(&c))).map(keys_of).collect();
    for entry in entries {
        if entry.command.as_deref().is_some_and(|c| off.contains(&c)) {
            continue;
        }
        if entry.command.is_none() && !off.is_empty() && !bound.iter().any(|b| b.starts_with(&keys_of(entry))) && !feature_keys_under(&keys_of(entry), on) {
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

/// Whether a switched-on feature binds something under `seq` (a group
/// core names that a feature fills, like the review feature's keys in
/// `SPC g`).
fn feature_keys_under(seq: &[KeyPress], on: &dyn Fn(&str) -> bool) -> bool {
    use crate::app::feature::LeaderEntry;
    let spc = KeyPress::char(' ');
    LEADER_FEATURES.iter().filter(|spec| on(spec.id)).flat_map(|spec| spec.leader).any(|entry| match entry {
        LeaderEntry::Bind(keys, ..) => std::iter::once(spc).chain(keys.iter().map(|&c| KeyPress::char(c))).collect::<Vec<_>>().starts_with(seq),
        LeaderEntry::Group(..) => false,
    })
}

/// Keys bound or named twice -- in the leader map, or in one `SPC m`
/// menu -- across core's keymap and the features' specs, as
/// `SPC f s: file.save and file.other`. A later one would silently win.
pub(crate) fn keymap_conflicts() -> Vec<String> {
    use crate::app::feature::LeaderEntry;
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
    fn spec(seen: &mut HashMap<String, String>, conflicts: &mut Vec<String>, prefix: &str, entries: &[LeaderEntry]) {
        for e in entries {
            let (keys, what) = match e {
                LeaderEntry::Bind(keys, _, command) => (keys, command.to_string()),
                LeaderEntry::Group(keys, label) => (keys, format!("group {label}")),
            };
            let keys = keys.iter().map(|&c| describe_keypress(&KeyPress::char(c))).collect::<Vec<_>>().join(" ");
            note(seen, conflicts, format!("{prefix}{keys}"), what);
        }
    }
    let mut conflicts = Vec::new();
    let mut leader = HashMap::new();
    data(&mut leader, &mut conflicts, "", &default_keymap().leader);
    for s in LEADER_FEATURES {
        spec(&mut leader, &mut conflicts, "SPC ", s.leader);
    }
    let menus: [(&str, &[&[LeaderEntry]], &str); 5] = [
        ("reader", &[], "reader"),
        ("markdown", &[crate::app::NOTEBOOK_SPEC.local_menu], "markdown"),
        ("arduino", &[crate::app::EMBEDDED_SPEC.local_menu], "arduino"),
        ("tcl", &[crate::app::MIB_SPEC.local_menu, crate::app::TCL_SPEC.local_menu], "tcl"),
        ("mermaid", &[crate::app::DIAGRAMS_SPEC.local_menu], "mermaid"),
    ];
    for (name, specs, data_name) in menus {
        let mut menu = HashMap::new();
        data(&mut menu, &mut conflicts, &format!("SPC m ({name}) "), default_local(data_name));
        for entries in specs {
            spec(&mut menu, &mut conflicts, &format!("SPC m ({name}) "), entries);
        }
    }
    conflicts
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
        assert_eq!(resolve("gP"), Some("git.pull_request"));
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

    #[test]
    fn leader_trie_resolves_docker_open_and_build() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('d'));
        match m.feed(KeyPress::char('d')) {
            fenix_keymap::Step::Matched(&"docker.open") => {}
            _ => panic!("expected SPC d d to resolve to docker.open"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('d'));
        match m.feed(KeyPress::char('b')) {
            fenix_keymap::Step::Matched(&"docker.build") => {}
            _ => panic!("expected SPC d b to resolve to docker.build"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('d'));
        match m.feed(KeyPress::char('q')) {
            fenix_keymap::Step::Matched(&"docker.close") => {}
            _ => panic!("expected SPC d q to resolve to docker.close"),
        }
    }

    #[test]
    fn leader_trie_resolves_the_jira_keys() {
        let trie = leader_trie();
        for (key, command) in [('j', "jira.open"), ('r', "jira.refresh"), ('g', "jira.goto_issue"), ('/', "jira.search"), ('n', "jira.create_issue")] {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('j'));
            match m.feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&c) if c == command => {}
                _ => panic!("expected SPC j {key} to resolve to {command}"),
            }
        }
    }

    #[test]
    fn leader_trie_resolves_the_agenda_keys() {
        let trie = leader_trie();
        let keys = [('a', "agenda.open"), ('b', "agenda.board"), ('k', "agenda.board"), ('l', "agenda.list"), ('r', "agenda.report"), ('n', "agenda.new_task"), ('h', "agenda.from_here"), ('/', "agenda.find"), ('t', "agenda.toggle_clock"), ('T', "agenda.resume")];
        for (key, command) in keys {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('a'));
            match m.feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&c) if c == command => {}
                _ => panic!("expected SPC a {key} to resolve to {command}"),
            }
        }
    }

    #[test]
    fn leader_trie_resolves_the_agendas_jira_commands() {
        let trie = leader_trie();
        for (key, command) in [('i', "agenda.import"), ('s', "agenda.sync"), ('w', "agenda.worklogs")] {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('a'));
            match m.feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&c) if c == command => {}
                _ => panic!("expected SPC a {key} to resolve to {command}"),
            }
        }
    }

    #[test]
    fn a_new_sketch_can_be_made_from_anywhere() {
        let mut m = leader_trie().matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('f'));
        assert!(matches!(m.feed(KeyPress::char('n')), fenix_keymap::Step::Matched(&"embedded.new_sketch")));
    }

    /// Your keys go over Fenix's: a new binding, one moved to another
    /// command, one taken away, and what can't be done said.
    #[test]
    fn your_keys_go_over_the_defaults() {
        let mut t = build_leader(&|_| true);
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
keys = "SPC a k"
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
        assert_eq!(t.get(&seq("SPC a k")), None);
        assert!(bound.contains(&UserKey { keys: "SPC f s".into(), command: Some("agenda.open".into()), replaced: Some("file.save") }), "{bound:?}");
        assert!(bound.iter().any(|b| b.keys == "SPC a k" && b.command.is_none() && b.replaced == Some("agenda.board")), "{bound:?}");
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("no.such isn't a command")) && problems.iter().any(|p| p.contains("start with SPC")));
    }

    /// A switched-off feature's keys aren't in the leader map: its own
    /// (`SPC j`), core's bindings to its commands (`SPC k` for MIB and
    /// CCSDS), and the groups that leaves empty.
    #[test]
    fn a_switched_off_feature_has_no_keys() {
        let all = build_leader(&|_| true);
        let bindings = |t: &KeyTrie<&'static str>| t.bindings().into_iter().map(|(_, id)| *id).collect::<Vec<&'static str>>();
        assert!(bindings(&all).contains(&"jira.open") && bindings(&all).contains(&"mib.page"));
        let some = build_leader(&|id| id != "fenix.jira" && id != "fenix.mib" && id != "fenix.ccsds");
        let ids = bindings(&some);
        assert!(!ids.iter().any(|id| id.starts_with("jira.") || id.starts_with("mib.") || id.starts_with("ccsds.")), "{ids:?}");
        assert!(ids.contains(&"agenda.open") && ids.contains(&"file.save"), "the rest are all there");
        let mut m = some.matcher();
        m.feed(KeyPress::char(' '));
        let hints: Vec<String> = m.pending_hints().into_iter().map(|h| describe_keypress(&h.key)).collect();
        assert!(!hints.contains(&"j".to_string()) && !hints.contains(&"k".to_string()), "no empty jira or mission group: {hints:?}");
        assert!(hints.contains(&"a".to_string()) && hints.contains(&"f".to_string()));
    }

    #[test]
    fn local_trie_resolves_every_arduino_command() {
        let trie = local_trie(&[LocalContext::Arduino]);
        for (key, command) in [('b', "embedded.build"), ('u', "embedded.upload"), ('m', "embedded.monitor"), ('B', "embedded.baud"), ('p', "embedded.port"), ('s', "embedded.board"), ('o', "embedded.board_options"), ('l', "embedded.library"), ('c', "embedded.package"), ('d', "embedded.debug"), ('n', "embedded.new_sketch"), ('i', "embedded.info")] {
            match trie.matcher().feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&c) if c == command => {}
                _ => panic!("expected SPC m {key} in a sketch to resolve to {command}"),
            }
        }
    }

    #[test]
    fn leader_trie_resolves_vnc_open_close_and_screenshot() {
        let trie = leader_trie();

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('v'));
        match m.feed(KeyPress::char('v')) {
            fenix_keymap::Step::Matched(&"vnc.open") => {}
            _ => panic!("expected SPC v v to resolve to vnc.open"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('v'));
        match m.feed(KeyPress::char('q')) {
            fenix_keymap::Step::Matched(&"vnc.close") => {}
            _ => panic!("expected SPC v q to resolve to vnc.close"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('v'));
        match m.feed(KeyPress::char('s')) {
            fenix_keymap::Step::Matched(&"vnc.screenshot") => {}
            _ => panic!("expected SPC v s to resolve to vnc.screenshot"),
        }
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
        let arduino_first = local_trie(&[LocalContext::Arduino, LocalContext::Tcl]);
        assert!(matches!(arduino_first.matcher().feed(KeyPress::char('b')), fenix_keymap::Step::Matched(&"embedded.build")));
        assert!(matches!(arduino_first.matcher().feed(KeyPress::char('i')), fenix_keymap::Step::Matched(&"embedded.info")));
        assert!(matches!(arduino_first.matcher().feed(KeyPress::char('T')), fenix_keymap::Step::Matched(&"completion.refresh_tags")));
        let tcl_first = local_trie(&[LocalContext::Tcl, LocalContext::Arduino]);
        assert!(matches!(tcl_first.matcher().feed(KeyPress::char('i')), fenix_keymap::Step::Matched(&"mib.insert")));
        assert!(std::ptr::eq(arduino_first, local_trie(&[LocalContext::Arduino, LocalContext::Tcl])), "built once per combination");
    }

    #[test]
    fn local_trie_resolves_tcl_commands() {
        let trie = local_trie(&[LocalContext::Tcl]);
        let cases: &[(char, &str)] = &[
            ('s', "code.symbols"),
            ('T', "completion.refresh_tags"),
            ('i', "mib.insert"),
            ('e', "mib.edit_call"),
            ('t', "mib.telecommand"),
            ('k', "mib.tm_packet"),
            ('p', "mib.tm_parameter"),
            ('c', "mib.calibration"),
            ('m', "mib.page"),
            ('r', "mib.reload"),
        ];
        for &(key, expected) in cases {
            match trie.matcher().feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&id) if id == expected => {}
                _ => panic!("expected SPC m {key} in a Tcl file to resolve to {expected}"),
            }
        }
    }

    #[test]
    fn spc_k_holds_the_mib_commands() {
        let trie = leader_trie();
        let cases: &[(char, &str)] = &[
            ('k', "mib.page"),
            ('/', "mib.search"),
            ('t', "mib.telecommand"),
            ('p', "mib.tc_parameter"),
            ('m', "mib.tm_packet"),
            ('n', "mib.tm_parameter"),
            ('c', "mib.calibration"),
            ('i', "mib.insert"),
            ('e', "mib.edit_call"),
            ('r', "mib.reload"),
            (',', "mib.settings"),
        ];
        let commands = crate::commands::CommandRegistry::with_builtins();
        for &(key, expected) in cases {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('k'));
            match m.feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&id) if id == expected => {}
                _ => panic!("expected SPC k {key} to resolve to {expected}"),
            }
            assert!(commands.iter().any(|c| c.id == expected), "{expected} is a command");
        }
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
