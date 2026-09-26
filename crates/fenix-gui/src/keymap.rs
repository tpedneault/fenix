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
}

impl LocalContext {
    /// Shown when `SPC m` opens, so you know which menu you're in.
    pub fn name(self) -> &'static str {
        match self {
            LocalContext::Arduino => "arduino",
            LocalContext::Tcl => "tcl",
            LocalContext::Reader => "reader",
        }
    }

    fn bind(self, t: &mut KeyTrie<&'static str>) {
        match self {
            LocalContext::Arduino => {
                t.insert(&[KeyPress::char('b')], "build (verify)", "embedded.build");
                t.insert(&[KeyPress::char('u')], "upload to board", "embedded.upload");
                t.insert(&[KeyPress::char('m')], "serial monitor", "embedded.monitor");
                t.insert(&[KeyPress::char('B')], "serial monitor speed", "embedded.baud");
                t.insert(&[KeyPress::char('p')], "choose port", "embedded.port");
                t.insert(&[KeyPress::char('s')], "choose board", "embedded.board");
                t.insert(&[KeyPress::char('o')], "board options", "embedded.board_options");
                t.insert(&[KeyPress::char('l')], "install library", "embedded.library");
                t.insert(&[KeyPress::char('c')], "install board package", "embedded.package");
                t.insert(&[KeyPress::char('d')], "debug", "embedded.debug");
                t.insert(&[KeyPress::char('n')], "new sketch", "embedded.new_sketch");
                t.insert(&[KeyPress::char('i')], "project info", "embedded.info");
            }
            LocalContext::Reader => {
                t.insert(&[KeyPress::char('n')], "next page (J)", "pdf.next_page");
                t.insert(&[KeyPress::char('p')], "previous page (K)", "pdf.prev_page");
                t.insert(&[KeyPress::char('g')], "go to page ({n}G)", "pdf.goto_page");
                t.insert(&[KeyPress::char('[')], "first page (gg)", "pdf.first_page");
                t.insert(&[KeyPress::char(']')], "last page (G)", "pdf.last_page");
                t.insert(&[KeyPress::char('=')], "zoom in (+)", "pdf.zoom_in");
                t.insert(&[KeyPress::char('-')], "zoom out (-)", "pdf.zoom_out");
                t.insert(&[KeyPress::char('w')], "fit width (zw)", "pdf.fit_width");
                t.insert(&[KeyPress::char('f')], "fit page (zp)", "pdf.fit_page");
                t.insert(&[KeyPress::char('o')], "outline sidebar (o)", "pdf.toggle_outline");
                t.insert(&[KeyPress::char('t')], "go to a heading", "pdf.headings");
                t.insert(&[KeyPress::char('/')], "search (/)", "pdf.search");
                t.insert(&[KeyPress::char('d')], "documents", "pdf.documents");
            }
            LocalContext::Tcl => {
                t.insert(&[KeyPress::char('i')], "insert telecommand", "mib.insert_telecommand");
                t.insert(&[KeyPress::char('t')], "lookup telecommand", "mib.lookup_telecommand");
                t.insert(&[KeyPress::char('k')], "lookup TM packet", "mib.lookup_tm_packet");
                t.insert(&[KeyPress::char('p')], "lookup TM parameter", "mib.lookup_tm_parameter");
                t.insert(&[KeyPress::char('c')], "lookup calibration", "mib.lookup_calibration");
                t.insert(&[KeyPress::char('r')], "refresh MIB index", "mib.refresh_index");
                t.insert(&[KeyPress::char('a')], "add MIB root", "mib.add_root");
                t.insert(&[KeyPress::char('d')], "delete MIB root", "mib.delete_root");
                t.insert(&[KeyPress::char('s')], "symbols", "code.symbols");
                t.insert(&[KeyPress::char('T')], "refresh tags", "completion.refresh_tags");
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

pub fn leader_trie() -> &'static KeyTrie<&'static str> {
    static TRIE: OnceLock<KeyTrie<&'static str>> = OnceLock::new();
    TRIE.get_or_init(|| {
        let mut t = KeyTrie::new();
        let spc = KeyPress::char(' ');
        t.label_group(&[spc], "leader");
        t.label_group(&[spc, KeyPress::char('i')], "insert");
        t.insert(&[spc, KeyPress::char('i'), KeyPress::char('s')], "snippet", "insert.snippet");
        t.insert(&[spc, KeyPress::char('i'), KeyPress::char('S')], "manage snippets", "snippets.open");
        t.insert(&[spc, KeyPress::char('i'), KeyPress::char('n')], "snippet from the selection", "snippets.from_selection");
        // `SPC SPC` mirrors Doom Emacs's own "hit the leader twice for the
        // single most-used action" convention -- here, the same fuzzy
        // find-file-in-project picker as `SPC p f`.
        t.insert(&[spc, spc], "find file", "project.find_file");

        t.label_group(&[spc, KeyPress::char('f')], "files");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('s')], "save", "file.save");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('j')], "dired-jump", "explorer.jump");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('t')], "table view", "table.toggle");
        // Global rather than under `SPC m`: `SPC m` only has the Arduino
        // menu inside a sketch, and your first sketch has to come from
        // somewhere.
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('n')], "new Arduino sketch", "embedded.new_sketch");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('f')], "find file", "file.find");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('e')], "explore from home", "file.explore");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('a')], "find file (all)", "file.find_all");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('r')], "recent files", "file.recent");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('R')], "rename file", "file.rename");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('D')], "delete file", "file.delete");
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('y')], "yank file path", "file.yank_path");
        // `v` for "revive" -- `r` is recent files and `R` is rename,
        // and this belongs in the same group as both.
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('v')], "recover unsaved work", "file.recover");

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
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('a')], "motion", "view.toggle_animations");
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('d')], "inline problems", "view.cycle_diagnostics");

        t.label_group(&[spc, KeyPress::char('e')], "explorer");
        t.insert(
            &[spc, KeyPress::char('e'), KeyPress::char('t')],
            "toggle sidebar",
            "explorer.toggle_sidebar",
        );

        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('e')], "open explorer here", "explorer.jump");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('d')], "dual pane", "explorer.dual_pane");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('k')], "stop the running operation", "explorer.cancel");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('o')], "open with the system", "explorer.open_external");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('O')], "show in Explorer", "explorer.reveal");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('y')], "copy the full path", "explorer.yank_path");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('T')], "shell here", "explorer.terminal_here");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('g')], "search here", "explorer.grep_here");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('G')], "project + Git panel here", "explorer.git_here");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('w')], "edit names", "explorer.rename_mode");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('W')], "apply edited names", "explorer.rename_apply");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('p')], "go to path", "explorer.go_to_path");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('b')], "places", "explorer.places");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('r')], "recent directories", "explorer.recent_dirs");
        t.insert(&[spc, KeyPress::char('e'), KeyPress::char('m')], "bookmark this directory", "explorer.bookmark");
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
        // The hub; the quick path-only switcher stays on `P`.
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('p')], "projects", "project.hub");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('P')], "quick switch", "project.switch_project");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('h')], "doctor", "project.doctor");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char(',')], "this project's settings", "project.settings");
        t.insert(&[spc, KeyPress::char(',')], "settings", "settings.open");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('c')], "new project", "project.new");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('a')], "add project", "project.add");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('d')], "delete project", "project.delete");
        // `SPC t` is already "toggle" (theme/font-size/fullscreen/...),
        // so the build/task runner nests under `SPC p` instead --
        // Doom Emacs' own `SPC p c` ("project compile") convention,
        // generalized to any discovered task rather than just a build.
        // `T`/lowercase mirrors `n`/`N`'s existing next-match/prev-match
        // shift-variant pattern just above, here for run/rerun instead.
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('t')], "run task", "task.run");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('T')], "rerun last task", "task.rerun_last");
        t.insert(&[spc, KeyPress::char('p'), KeyPress::char('k')], "kill running task", "task.kill");

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
        // `t`/`T`, lowercase for this buffer and shifted for the whole
        // project -- the same small/large split `SPC p n`/`N` and
        // `SPC p t`/`T` already use one shift key apart.
        t.insert(&[spc, KeyPress::char('s'), KeyPress::char('t')], "TODOs in buffer", "search.todos");
        t.insert(&[spc, KeyPress::char('s'), KeyPress::char('T')], "TODOs in project", "search.todos_project");

        t.label_group(&[spc, KeyPress::char('o')], "open");
        t.insert(&[spc, KeyPress::char('o'), KeyPress::char('d')], "open home", "dashboard.open");
        t.insert(&[spc, KeyPress::char('o'), KeyPress::char('t')], "toggle terminal", "terminal.toggle");
        t.insert(&[spc, KeyPress::char('o'), KeyPress::char('T')], "terminal in this pane", "terminal.open_buffer");

        // Reserved entirely for Docker (Lazydocker-style) -- the
        // dashboard used to live at `SPC d d` but moved to `SPC o d`
        // above so this whole group is free for docker commands.
        t.label_group(&[spc, KeyPress::char('d')], "docker");
        t.insert(&[spc, KeyPress::char('d'), KeyPress::char('d')], "open docker panel", "docker.open");
        t.insert(&[spc, KeyPress::char('d'), KeyPress::char('b')], "build image", "docker.build");
        t.insert(&[spc, KeyPress::char('d'), KeyPress::char('q')], "close docker panel", "docker.close");

        // DAP debugger. `SPC d` is already Docker's own group (Doom
        // Emacs' own convention for debugging), so this nests under
        // `SPC u` instead -- the one letter in "debUg" not already
        // claimed by another top-level group (d=docker, e=explorer,
        // b=buffer, g=git are all taken).
        t.label_group(&[spc, KeyPress::char('u')], "debug");
        t.insert(
            &[spc, KeyPress::char('u'), KeyPress::char('u')],
            "start/continue",
            "debug.start_or_continue",
        );
        t.insert(
            &[spc, KeyPress::char('u'), KeyPress::char('b')],
            "toggle breakpoint",
            "debug.toggle_breakpoint",
        );
        t.insert(&[spc, KeyPress::char('u'), KeyPress::char('n')], "step over", "debug.step_over");
        t.insert(&[spc, KeyPress::char('u'), KeyPress::char('i')], "step into", "debug.step_into");
        t.insert(&[spc, KeyPress::char('u'), KeyPress::char('o')], "step out", "debug.step_out");
        t.insert(&[spc, KeyPress::char('u'), KeyPress::char('w')], "add watch", "debug.add_watch");
        t.insert(&[spc, KeyPress::char('u'), KeyPress::char('q')], "stop", "debug.stop");

        // Tool status listing (Milestone E of the LSP/DAP plan) -- `l`
        // for "lsp", the group the original plan reserved this letter
        // for. Only `m` ("manager") exists for now; the plan's own
        // `SPC l i`/`SPC l r` (per-buffer LSP status/restart) were never
        // wired up, so this group has exactly one entry rather than the
        // three gaps that would come from stubbing them out unbuilt.
        t.label_group(&[spc, KeyPress::char('l')], "lsp");
        t.insert(&[spc, KeyPress::char('l'), KeyPress::char('m')], "tool status", "tools.status");

        // Git. The status page (`g`) is the hub and has its own keys for
        // most of what's done to a repository; these are the ways in,
        // and what's done from the file being edited.
        t.label_group(&[spc, KeyPress::char('g')], "git");
        // The which-key drawer lists git's keys in these sections, in
        // this order (`WHICH_KEY_SECTIONS`).
        for (keys, section) in [("glGhHBec", "View"), ("wfprmPMz", "Branch & remote"), ("adix", "This change")] {
            for key in keys.chars() {
                t.set_section(&[spc, KeyPress::char('g'), KeyPress::char(key)], section);
            }
        }
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('g')], "status page", "git.open");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('l')], "log", "git.log");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('h')], "this file's history", "git.file_history");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('H')], "these lines' history", "git.line_history");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('G')], "graph view", "git.history");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('c')], "compare refs", "git.compare");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('z')], "undo / operation log", "git.operations");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('w')], "switch branch", "git.switch");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('f')], "fetch", "git.fetch");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('p')], "pull --rebase", "git.pull_rebase");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('r')], "rebase onto...", "git.rebase");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('m')], "merge...", "git.merge");
        // `M`, not `m` -- `SPC g m` is the merge *operation*, and the two
        // are asked for in completely different moods.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('P')], "open a pull request", "git.pull_request");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('M')], "reviews", "git.merge_requests");
        // The file being edited: its hunks (`]h`/`[h` move between them)
        // and its blame.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('a')], "stage hunk", "git.hunk_stage");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('d')], "discard hunk", "git.hunk_discard");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('i')], "preview hunk", "git.hunk_preview");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('B')], "blame", "git.blame");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('e')], "explain this line", "git.blame_explain");
        // A merge or rebase that stopped: resolving the conflicts, and the
        // two keys that end it. `c`/`a` are one pair for every kind of
        // suspended operation -- from the user's side it's one question
        // ("keep going" / "put it back"), and the banner names which
        // operation is answering.
        t.label_group(&[spc, KeyPress::char('g'), KeyPress::char('x')], "conflicts");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('x')], "resolve side by side", "git.merge_view");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('j')], "next conflict", "git.next_conflict");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('k')], "prev conflict", "git.prev_conflict");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('o')], "keep ours", "git.keep_ours");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('t')], "keep theirs", "git.keep_theirs");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('b')], "keep both", "git.keep_both");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('s')], "stage resolved", "git.stage_resolved");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('c')], "continue", "git.continue");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x'), KeyPress::char('a')], "abort", "git.abort");
        // One key closes whichever Git view is in front -- the panels,
        // the graph, a comparison, the conflicts view, the older merge
        // request view -- rather than one each.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('q')], "close this git view", "git.close_view");

        // Jira: the page (searches, issues, a preview), and what you
        // reach for from a file -- an issue by key, a search, a new one.
        // Tracking projects and people happens on the page itself.
        t.label_group(&[spc, KeyPress::char('j')], "jira");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('j')], "open jira", "jira.open");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('r')], "refresh jira", "jira.refresh");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('g')], "go to issue", "jira.goto_issue");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('/')], "search jira", "jira.search");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('n')], "new issue", "jira.create_issue");

        // The agenda: the page and its tabs, and what you reach for from
        // a file -- a new task, a task from here, the clock. A task's own
        // keys (status, priority, clock, ...) are on the page.
        t.label_group(&[spc, KeyPress::char('a')], "agenda");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('a')], "open agenda", "agenda.open");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('b')], "agenda: board", "agenda.board");
        // The board's old key, kept for a release.
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('k')], "agenda: board (now SPC a b)", "agenda.board");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('l')], "agenda: list", "agenda.list");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('r')], "agenda: this week's time", "agenda.report");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('n')], "agenda: new task", "agenda.new_task");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('h')], "agenda: task from here", "agenda.from_here");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('/')], "agenda: find a task", "agenda.find");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('t')], "agenda: clock -- stop, resume, switch", "agenda.toggle_clock");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('T')], "agenda: resume the clock", "agenda.resume");
        // The Jira side: pick which of your issues to track, refresh the
        // ones you track, and review/send time as worklogs.
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('i')], "agenda: import Jira issues", "agenda.import");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('s')], "agenda: sync with Jira", "agenda.sync");
        t.insert(&[spc, KeyPress::char('a'), KeyPress::char('w')], "agenda: review worklogs", "agenda.worklogs");

        // VNC console panes (`fenix-vnc`) -- one connection per
        // configured `Config.vnc_hosts` entry, each staying live in the
        // background once opened. Mirrors the docker/jira groups' exact
        // shape.
        t.label_group(&[spc, KeyPress::char('v')], "vnc");
        t.insert(&[spc, KeyPress::char('v'), KeyPress::char('v')], "open/switch vnc session", "vnc.open");
        t.insert(&[spc, KeyPress::char('v'), KeyPress::char('q')], "close vnc session", "vnc.close");
        t.insert(&[spc, KeyPress::char('v'), KeyPress::char('s')], "save vnc screenshot", "vnc.screenshot");

        t.label_group(&[spc, KeyPress::char('r')], "reader (pdf)");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('n')], "next page", "pdf.next_page");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('p')], "previous page", "pdf.prev_page");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('[')], "first page", "pdf.first_page");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char(']')], "last page", "pdf.last_page");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('g')], "go to page", "pdf.goto_page");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('=')], "zoom in", "pdf.zoom_in");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('-')], "zoom out", "pdf.zoom_out");
        // `f` is the document index, not fit-page: picking a reference
        // off the shelf is something you do to *start* reading, from any
        // buffer, while fit-page is a zoom adjustment you make while
        // already in a PDF pane -- where the bare `0` binding sits under
        // your fingers anyway. `SPC r 0` mirrors that bare key.
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('f')], "find document", "pdf.documents");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('0')], "fit page", "pdf.fit_page");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('w')], "fit width", "pdf.fit_width");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('o')], "outline sidebar", "pdf.toggle_outline");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('t')], "go to a heading", "pdf.headings");
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('/')], "search", "pdf.search");

        t.label_group(&[spc, KeyPress::char('c')], "code");
        // `SPC c r` used to be ctags refresh -- moved to `SPC c T`
        // ("Tags") to free `r` up for the far more frequently reached-
        // for LSP rename, mirroring the letter Neovim/most LSP configs
        // already use for it.
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('r')],
            "rename (LSP)",
            "code.lsp_rename",
        );
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('a')],
            "code action",
            "code.lsp_code_action",
        );
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('f')],
            "indent region",
            "code.format_selection",
        );
        t.insert(
            &[spc, KeyPress::char('c'), KeyPress::char('F')],
            "indent buffer",
            "code.format_buffer",
        );
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('x')], "toggle checkbox", "code.toggle_checkbox");
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('o')], "outline", "code.outline");
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('u')], "enclosing scope", "code.scope_parent");
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('b')], "breadcrumbs", "code.breadcrumbs");
        // `z`, not `f` -- `SPC c f` is already "indent region"
        // (`code.format_selection`, above); `z` matches real Vim's own
        // `zo`/`zc`/`za` fold mnemonic instead of colliding with it.
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('z')], "toggle fold", "code.toggle_fold");
        // XML. `SPC c o` (outline), `SPC c s` (elements), `SPC c z`
        // (fold) and `SPC c F` (reindent) already cover it through the
        // grammar; these two only make sense for XML.
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('v')], "validate XML", "code.xml_validate");
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('y')], "yank XML path", "code.xml_path");

        // The local leader: what's under it depends on the focused buffer
        // (see `LocalContext`) -- `App::start_local_leader` opens the
        // matching `local_trie`, the way Doom Emacs' `SPC m` does.
        t.insert(&[spc, KeyPress::char('m')], "mode", "leader.local");

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
        // OS windows ("frames") share this group with splits, one shift
        // key apart: lowercase acts on a split, uppercase on the whole
        // window. `n` is the exception -- there's no lowercase "new
        // window" for it to collide with, and `SPC w n` reads better
        // than `SPC w N` for the one command in the group you reach for
        // first.
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('n')], "new frame", "frame.new");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('W')], "cycle frame", "frame.cycle");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('Q')], "close frame", "frame.close");
        t.insert(&[spc, KeyPress::char('w'), KeyPress::char('O')], "only frame", "frame.only");

        t.label_group(&[spc, KeyPress::char('b')], "buffer");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('b')], "switch buffer", "buffer.switch");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('n')], "next buffer", "buffer.next");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('p')], "prev buffer", "buffer.prev");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('k')], "kill buffer", "buffer.kill");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('d')], "close tab", "tab.close");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('o')], "close other tabs", "tab.close_others");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('u')], "reopen closed tab", "tab.reopen");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('P')], "keep preview tab", "tab.keep");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('<')], "move tab left", "tab.move_left");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('>')], "move tab right", "tab.move_right");
        t.insert(&[spc, KeyPress::char('b'), KeyPress::char('X')], "scratch buffer", "buffer.scratch");

        let tab = KeyPress::named(FenixNamedKey::Tab);
        t.label_group(&[spc, tab], "workspace");
        t.insert(&[spc, tab, KeyPress::char('n')], "new workspace", "workspace.new");
        t.insert(&[spc, tab, KeyPress::char(']')], "next workspace", "workspace.next");
        t.insert(&[spc, tab, KeyPress::char('[')], "prev workspace", "workspace.prev");
        t.insert(&[spc, tab, KeyPress::char('d')], "remove workspace", "workspace.remove");
        t.insert(&[spc, tab, tab], "switch workspace", "workspace.switch");
        t.insert(&[spc, tab, KeyPress::char('f')], "find workspace", "workspace.find");
        t.insert(&[spc, tab, KeyPress::char('r')], "rename workspace", "workspace.rename");

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
        assert!(matches!(tcl_first.matcher().feed(KeyPress::char('i')), fenix_keymap::Step::Matched(&"mib.insert_telecommand")));
        assert!(std::ptr::eq(arduino_first, local_trie(&[LocalContext::Arduino, LocalContext::Tcl])), "built once per combination");
    }

    #[test]
    fn local_trie_resolves_tcl_commands() {
        let trie = local_trie(&[LocalContext::Tcl]);
        let cases: &[(char, &str)] = &[
            ('s', "code.symbols"),
            ('T', "completion.refresh_tags"),
            ('i', "mib.insert_telecommand"),
            ('t', "mib.lookup_telecommand"),
            ('k', "mib.lookup_tm_packet"),
            ('p', "mib.lookup_tm_parameter"),
            ('c', "mib.lookup_calibration"),
            ('r', "mib.refresh_index"),
            ('a', "mib.add_root"),
            ('d', "mib.delete_root"),
        ];
        for &(key, expected) in cases {
            match trie.matcher().feed(KeyPress::char(key)) {
                fenix_keymap::Step::Matched(&id) if id == expected => {}
                _ => panic!("expected SPC m {key} in a Tcl file to resolve to {expected}"),
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
}
