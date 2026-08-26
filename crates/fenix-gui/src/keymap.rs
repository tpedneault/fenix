use std::sync::OnceLock;

use fenix_keymap::{KeyCode, KeyPress, KeyTrie, Mods, NamedKey as FenixNamedKey};
use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Translates a winit key event into fenix-keymap's UI-agnostic `KeyPress`.
/// Named `Space` is normalized to `KeyCode::Char(' ')` -- treating it like
/// any other printable key keeps the leader trie's sequences (`SPC f s`)
/// just a plain char sequence, no special-casing needed downstream.
/// Returns `None` for keys with nothing sensible to bind (F-keys, media
/// keys, ...).
pub fn to_keypress(event: &KeyEvent, mods: ModifiersState) -> Option<KeyPress> {
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

/// The `SPC`-leader menu. Includes the leading space itself as the trie's
/// first key, so the whole leader interaction -- from the initial `SPC`
/// through to a resolved command -- is just one uniform walk of this trie.
///
/// Deliberately sparse: only wires groups that have a real command
/// behind them today.
pub fn leader_trie() -> &'static KeyTrie<&'static str> {
    static TRIE: OnceLock<KeyTrie<&'static str>> = OnceLock::new();
    TRIE.get_or_init(|| {
        let mut t = KeyTrie::new();
        let spc = KeyPress::char(' ');
        t.label_group(&[spc], "leader");
        // `SPC SPC` mirrors Doom Emacs's own "hit the leader twice for the
        // single most-used action" convention -- here, the same fuzzy
        // find-file-in-project picker as `SPC p f`.
        t.insert(&[spc, spc], "find file", "project.find_file");

        t.label_group(&[spc, KeyPress::char('f')], "files");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('s')], "save", "file.save");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('j')], "dired-jump", "explorer.jump");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('t')], "table view", "table.toggle");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('f')], "find file", "file.find");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('a')], "find file (all)", "file.find_all");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('r')], "recent files", "file.recent");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('R')], "rename file", "file.rename");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('D')], "delete file", "file.delete");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('y')], "yank file path", "file.yank_path");

        t.label_group(&[spc, KeyPress::char('q')], "quit");
        t.insert(&[spc, KeyPress::char('q'), KeyPress::char('q')], "quit", "app.quit");

        t.label_group(&[spc, KeyPress::char('t')], "toggle");
        t.insert(
            &[spc, KeyPress::char('t'), KeyPress::char('n')],
            "line numbers",
            "view.cycle_line_numbers",
        );
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('p')], "pick theme", "view.pick_theme");
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('=')], "font size +", "view.increase_font_size");
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('-')], "font size -", "view.decrease_font_size");
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('0')], "font size reset", "view.reset_font_size");
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('f')], "fullscreen", "view.toggle_fullscreen");

        t.label_group(&[spc, KeyPress::char('e')], "explorer");
        t.insert(
            &[spc, KeyPress::char('e'), KeyPress::char('t')],
            "toggle sidebar",
            "explorer.toggle_sidebar",
        );

        t.label_group(&[spc, KeyPress::char('p')], "project");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('f')], "find file", "project.find_file");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('s')], "search", "project.grep");
        t.insert(
            &[spc, KeyPress::char('p'), KeyPress::char('n')],
            "next match",
            "project.quickfix_next",
        );
        t.insert(
            &[spc, KeyPress::char('p'), KeyPress::char('N')],
            "prev match",
            "project.quickfix_prev",
        );
        t.insert(
            &[spc, KeyPress::char('p'), KeyPress::char('p')],
            "switch project",
            "project.switch_project",
        );
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('a')], "add project", "project.add");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('d')], "delete project", "project.delete");

        t.label_group(&[spc, KeyPress::char('s')], "search");
        t.insert(&[spc, KeyPress::char('s'), KeyPress::char('s')], "search buffer", "search.buffer");
        t.insert(
            &[spc, KeyPress::char('s'), KeyPress::char('r')],
            "replace in buffer",
            "search.replace_buffer",
        );
        t.insert(
            &[spc, KeyPress::char('s'), KeyPress::char('p')],
            "replace in project",
            "search.replace_project",
        );

        t.label_group(&[spc, KeyPress::char('o')], "open");
        t.insert(&[spc, KeyPress::char('o'), KeyPress::char('d')], "open dashboard", "dashboard.open");
        t.insert(&[spc, KeyPress::char('o'), KeyPress::char('t')], "toggle terminal", "terminal.toggle");

        // Reserved entirely for Docker (Lazydocker-style) -- the
        // dashboard used to live at `SPC d d` but moved to `SPC o d`
        // above so this whole group is free for docker commands.
        t.label_group(&[spc, KeyPress::char('d')], "docker");
        t.insert(&[spc, KeyPress::char('d'), KeyPress::char('d')], "open docker panel", "docker.open");
        t.insert(&[spc, KeyPress::char('d'), KeyPress::char('b')], "build image", "docker.build");
        t.insert(&[spc, KeyPress::char('d'), KeyPress::char('q')], "close docker panel", "docker.close");

        // Lazygit-style Git panel.
        t.label_group(&[spc, KeyPress::char('g')], "git");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('g')], "open git panel", "git.open");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('q')], "close git panel", "git.close");

        t.label_group(&[spc, KeyPress::char('c')], "code");
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('r')],
            "refresh tags",
            "completion.refresh_tags",
        );
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('f')],
            "format selection",
            "code.format_selection",
        );
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('F')],
            "format buffer",
            "code.format_buffer",
        );
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('s')], "symbols", "code.symbols");

        // SCOS-2000 MIB lookup/insertion -- letters kept identical to
        // the reference elisp implementation's own scheme for muscle-
        // memory continuity (there it's `SPC M`, capitalized, since
        // Doom Emacs splits a global leader from a mode-local one;
        // Fenix has one flat leader tree, so this is lowercase `m`,
        // matching the user's own `SPC m i` example).
        t.label_group(&[spc, KeyPress::char('m')], "mib");
        t.insert(
            &[spc, KeyPress::char('m'), KeyPress::char('i')],
            "insert telecommand",
            "mib.insert_telecommand",
        );
        t.insert(
            &[spc, KeyPress::char('m'), KeyPress::char('t')],
            "lookup telecommand",
            "mib.lookup_telecommand",
        );
        t.insert(&[spc, KeyPress::char('m'), KeyPress::char('k')], "lookup TM packet", "mib.lookup_tm_packet");
        t.insert(
            &[spc, KeyPress::char('m'), KeyPress::char('p')],
            "lookup TM parameter",
            "mib.lookup_tm_parameter",
        );
        t.insert(
            &[spc, KeyPress::char('m'), KeyPress::char('c')],
            "lookup calibration",
            "mib.lookup_calibration",
        );
        t.insert(&[spc, KeyPress::char('m'), KeyPress::char('r')], "refresh MIB index", "mib.refresh_index");

        t.label_group(&[spc, KeyPress::char('w')], "window");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('v')], "split vertical", "window.split_vertical");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('s')], "split horizontal", "window.split_horizontal");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('h')], "focus left", "window.navigate_left");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('l')], "focus right", "window.navigate_right");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('k')], "focus up", "window.navigate_up");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('j')], "focus down", "window.navigate_down");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('w')], "cycle window", "window.cycle");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('q')], "close window", "window.close");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('o')], "only", "window.only");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('=')], "balance", "window.balance");

        t.label_group(&[spc, KeyPress::char('b')], "buffer");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('b')], "switch buffer", "buffer.switch");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('n')], "next buffer", "buffer.next");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('p')], "prev buffer", "buffer.prev");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('k')], "kill buffer", "buffer.kill");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('X')], "scratch buffer", "buffer.scratch");

        let tab = KeyPress::named(FenixNamedKey::Tab);
        t.label_group(&[spc, tab], "workspace");
        t.insert(&[spc, tab, KeyPress::char('n')], "new workspace", "workspace.new");
        t.insert(&[spc, tab, KeyPress::char(']')], "next workspace", "workspace.next");
        t.insert(&[spc, tab, KeyPress::char('[')], "prev workspace", "workspace.prev");
        t.insert(&[spc, tab, KeyPress::char('d')], "remove workspace", "workspace.remove");

        t
    })
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

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('p'));
        match m.feed(KeyPress::char('p')) {
            fenix_keymap::Step::Matched(&"project.switch_project") => {}
            _ => panic!("expected SPC p p to resolve to project.switch_project"),
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
    fn leader_trie_resolves_completion_refresh_tags() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('r')) {
            fenix_keymap::Step::Matched(&"completion.refresh_tags") => {}
            _ => panic!("expected SPC c r to resolve to completion.refresh_tags"),
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
    fn leader_trie_resolves_symbols() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('s')) {
            fenix_keymap::Step::Matched(&"code.symbols") => {}
            _ => panic!("expected SPC c s to resolve to code.symbols"),
        }
    }

    #[test]
    fn leader_trie_resolves_mib_commands() {
        let trie = leader_trie();
        let cases: &[(char, &str)] = &[
            ('i', "mib.insert_telecommand"),
            ('t', "mib.lookup_telecommand"),
            ('k', "mib.lookup_tm_packet"),
            ('p', "mib.lookup_tm_parameter"),
            ('c', "mib.lookup_calibration"),
            ('r', "mib.refresh_index"),
        ];
        for &(key, expected) in cases {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            m.feed(KeyPress::char('m'));
            match m.feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&id) if id == expected => {}
                _ => panic!("expected SPC m {key} to resolve to {expected}"),
            }
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
    }
}
