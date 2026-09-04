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
        t.insert(&[spc, KeyPress::char('f'), KeyPress::char('e')], "explore from home", "file.explore");
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
        t.insert(&[spc, KeyPress::char('t'), KeyPress::char('a')], "animations", "view.toggle_animations");

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

        // Lazygit-style Git panel.
        t.label_group(&[spc, KeyPress::char('g')], "git");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('g')], "open git panel", "git.open");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('q')], "close git panel", "git.close");
        // Purpose-built views rather than more panes on one panel: the
        // working tree and the history answer different questions.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('l')], "history/graph", "git.history");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('L')], "close history", "git.history_close");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('f')], "fetch (--all --prune)", "git.fetch");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('c')], "compare refs", "git.compare");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('C')], "close compare", "git.compare_close");

        // Operations that rewrite history, and the two keys that end one
        // that stopped for a conflict. `R`/`A` are deliberately one pair
        // for every kind of suspended operation -- from the user's side
        // it's one question ("keep going" / "put it back"), and the
        // Status banner names which operation is answering.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('r')], "rebase onto...", "git.rebase");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('R')], "continue", "git.continue");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('A')], "abort", "git.abort");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('m')], "merge...", "git.merge");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('p')], "pull --rebase", "git.pull_rebase");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('F')], "force-push (lease)", "git.force_push");

        // Conflict resolution, on whichever file is focused.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('j')], "next conflict", "git.next_conflict");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('k')], "prev conflict", "git.prev_conflict");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('o')], "keep ours", "git.keep_ours");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('t')], "keep theirs", "git.keep_theirs");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('b')], "keep both", "git.keep_both");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('s')], "stage resolved", "git.stage_resolved");

        // The Merge view -- the conflicted files, and whichever one is
        // selected shown as two columns under the names of the branches
        // they actually came from.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('x')], "resolve conflicts", "git.merge_view");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('X')], "close conflicts", "git.merge_close");

        // GitLab merge requests. `M`, not `m` -- `SPC g m` is the merge
        // *operation*, and the two are asked for in completely
        // different moods.
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('M')], "merge requests", "git.merge_requests");
        t.insert(&[spc, KeyPress::char('g'), KeyPress::char('Q')], "close merge requests", "git.merge_requests_close");

        // Jira dashboard -- read-only browsing this phase (see the Jira
        // dashboard plan's own scope notes). `p`/`u` add/delete letters
        // mirror `SPC p a`/`SPC p d`'s own add/delete-from-a-persisted-
        // list convention, same as the `m`ib group already reuses it.
        t.label_group(&[spc, KeyPress::char('j')], "jira");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('j')], "open jira panel", "jira.open");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('q')], "close jira panel", "jira.close");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('r')], "refresh jira panel", "jira.refresh");
        // Jump straight to any issue by key, even one not already showing
        // in the current Issues list -- see `App::jira_start_goto_issue_
        // prompt`'s own doc comment.
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('g')], "go to issue", "jira.goto_issue");
        t.label_group(&[spc, KeyPress::char('j'), KeyPress::char('p')], "jira projects");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('p'), KeyPress::char('a')], "add project", "jira.add_project");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('p'), KeyPress::char('d')], "delete project", "jira.delete_project");
        t.label_group(&[spc, KeyPress::char('j'), KeyPress::char('u')], "jira users");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('u'), KeyPress::char('a')], "add user", "jira.add_user");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('u'), KeyPress::char('d')], "delete user", "jira.delete_user");
        // Create/update issues, phase 2: `i` mirrors `p`/`u`'s own
        // two-level shape for one leaf so far (just create -- there's no
        // "delete an issue" analog to `p d`/`u d`).
        t.label_group(&[spc, KeyPress::char('j'), KeyPress::char('i')], "jira issue");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('i'), KeyPress::char('a')], "create issue", "jira.create_issue");
        // Submit/cancel a pending comment/description edit (`c`/`e` on
        // Issues/Detail) -- leader bindings, not pane-scoped bare keys,
        // since the edit buffer is genuinely free-typed prose (see
        // `App::jira_edit`'s own doc comment for why that's load-
        // bearing, not stylistic).
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('s')], "submit jira edit", "jira.submit_edit");
        t.insert(&[spc, KeyPress::char('j'), KeyPress::char('x')], "cancel jira edit", "jira.cancel_edit");

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
        t.insert(&[spc, KeyPress::char('r'), KeyPress::char('o')], "toggle outline", "pdf.toggle_outline");
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
            &[spc, KeyPress::char('c'), KeyPress::char('T')],
            "refresh tags",
            "completion.refresh_tags",
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
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('s')], "symbols", "code.symbols");
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('x')], "toggle checkbox", "code.toggle_checkbox");
        t.insert(&[spc, KeyPress::char('c'), KeyPress::char('o')], "outline", "code.outline");

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
        // Same letters `SPC p a`/`SPC p d` already use for the identical
        // add/delete-from-a-persisted-list pattern.
        t.insert(&[spc, KeyPress::char('m'), KeyPress::char('a')], "add MIB root", "mib.add_root");
        t.insert(&[spc, KeyPress::char('m'), KeyPress::char('d')], "delete MIB root", "mib.delete_root");

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
    fn leader_trie_resolves_jira_open_close_and_refresh() {
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        match m.feed(KeyPress::char('j')) {
            fenix_keymap::Step::Matched(&"jira.open") => {}
            _ => panic!("expected SPC j j to resolve to jira.open"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        match m.feed(KeyPress::char('q')) {
            fenix_keymap::Step::Matched(&"jira.close") => {}
            _ => panic!("expected SPC j q to resolve to jira.close"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        match m.feed(KeyPress::char('r')) {
            fenix_keymap::Step::Matched(&"jira.refresh") => {}
            _ => panic!("expected SPC j r to resolve to jira.refresh"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        match m.feed(KeyPress::char('g')) {
            fenix_keymap::Step::Matched(&"jira.goto_issue") => {}
            _ => panic!("expected SPC j g to resolve to jira.goto_issue"),
        }
    }

    #[test]
    fn leader_trie_resolves_jira_project_and_user_add_delete() {
        let trie = leader_trie();
        let cases = [
            (['j', 'p', 'a'], "jira.add_project"),
            (['j', 'p', 'd'], "jira.delete_project"),
            (['j', 'u', 'a'], "jira.add_user"),
            (['j', 'u', 'd'], "jira.delete_user"),
        ];
        for (keys, expected) in cases {
            let mut m = trie.matcher();
            m.feed(KeyPress::char(' '));
            for k in &keys[..keys.len() - 1] {
                m.feed(KeyPress::char(*k));
            }
            match m.feed(KeyPress::char(*keys.last().unwrap())) {
                fenix_keymap::Step::Matched(&matched) if matched == expected => {}
                _ => panic!("expected SPC {} to resolve to {expected}", keys.iter().collect::<String>()),
            }
        }
    }

    #[test]
    fn leader_trie_resolves_jira_create_issue_and_edit_submit_cancel() {
        let trie = leader_trie();

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        m.feed(KeyPress::char('i'));
        match m.feed(KeyPress::char('a')) {
            fenix_keymap::Step::Matched(&"jira.create_issue") => {}
            _ => panic!("expected SPC j i a to resolve to jira.create_issue"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        match m.feed(KeyPress::char('s')) {
            fenix_keymap::Step::Matched(&"jira.submit_edit") => {}
            _ => panic!("expected SPC j s to resolve to jira.submit_edit"),
        }

        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('j'));
        match m.feed(KeyPress::char('x')) {
            fenix_keymap::Step::Matched(&"jira.cancel_edit") => {}
            _ => panic!("expected SPC j x to resolve to jira.cancel_edit"),
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
    fn leader_trie_resolves_completion_refresh_tags() {
        // Moved from `SPC c r` to `SPC c T` to free `r` up for LSP
        // rename -- see `code.lsp_rename`'s own keymap comment.
        let trie = leader_trie();
        let mut m = trie.matcher();
        m.feed(KeyPress::char(' '));
        m.feed(KeyPress::char('c'));
        match m.feed(KeyPress::char('T')) {
            fenix_keymap::Step::Matched(&"completion.refresh_tags") => {}
            _ => panic!("expected SPC c T to resolve to completion.refresh_tags"),
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
    fn leader_trie_resolves_mib_commands() {
        let trie = leader_trie();
        let cases: &[(char, &str)] = &[
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
        assert!(matches!(resolve(tab), fenix_keymap::Step::Matched(&"workspace.switch")));
        assert!(matches!(resolve(KeyPress::char('f')), fenix_keymap::Step::Matched(&"workspace.find")));
        assert!(matches!(resolve(KeyPress::char('r')), fenix_keymap::Step::Matched(&"workspace.rename")));
    }
}
