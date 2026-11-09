//! Standard keys, end to end: an app on `editor.keys = "standard"`,
//! driven through the real key path.

use fenix_vim::Mode;

use super::test_keys::app_on_file;
use super::App;

fn standard(app: &mut App) {
    app.config.set_value("editor.keys", Some(fenix_config::Value::Text("standard".into())));
    app.sync_key_policy();
}

fn standard_on(name: &str, text: &str) -> (App, super::test_keys::TestFile) {
    let (mut app, file) = app_on_file(name, text);
    standard(&mut app);
    (app, file)
}

#[test]
fn a_file_is_typed_into_at_once_and_escape_doesn_t_leave_it() {
    let (mut app, _f) = standard_on("a.txt", "world\n");
    assert_eq!(app.vim.mode(), Mode::Insert, "typing, from the start");
    app.type_text("hello ");
    assert_eq!(app.text(), "hello world\n");
    app.press("Esc");
    app.type_text("dd");
    assert_eq!(app.text(), "hello ddworld\n", "Vim's keys are text");
    assert_eq!(app.mode_badge_label(), "", "no mode badge while typing");
    assert!(!app.caret_is_block(), "a bar");
}

#[test]
fn shift_arrows_select_and_typing_replaces_the_selection() {
    let (mut app, _f) = standard_on("a.txt", "hello world\n");
    app.press_all("S-Right S-Right S-Right S-Right S-Right");
    assert_eq!(app.mode_badge_label(), "SELECT");
    app.type_text("bye");
    assert_eq!(app.text(), "bye world\n");
    assert_eq!(app.vim.mode(), Mode::Insert);
}

#[test]
fn the_clipboard_chords_copy_cut_and_paste() {
    let (mut app, _f) = standard_on("a.txt", "one\ntwo\n");
    app.press("C-c");
    assert_eq!(app.vim.register().0, "one\n", "the line, with nothing selected");
    assert_eq!(app.vim.mode(), Mode::Insert, "still typing");
    app.press("Down");
    app.press("C-v");
    assert_eq!(app.text(), "one\none\ntwo\n");
    app.press_all("C-Home S-End C-x");
    assert_eq!(app.text(), "\none\ntwo\n");
    assert_eq!(app.vim.mode(), Mode::Insert);
}

#[test]
fn undo_and_redo_are_ctrl_z_and_ctrl_y_while_typing() {
    let (mut app, _f) = standard_on("a.txt", "");
    app.type_text("one two");
    app.press("C-z");
    assert_eq!(app.text(), "one ", "a word at a time");
    app.press("C-y");
    assert_eq!(app.text(), "one two");
    assert_eq!(app.vim.mode(), Mode::Insert);
}

#[test]
fn ctrl_k_opens_the_leader_and_its_keys_follow() {
    let (mut app, _f) = standard_on("a.txt", "");
    app.press("C-k");
    assert!(app.leader_matcher.is_pending(), "the leader's open");
    app.press("t");
    app.press("n");
    assert!(!app.leader_matcher.is_pending());
    assert_eq!(app.text(), "", "the leader's keys aren't typed");
    // SPC is a space while typing.
    app.press("SPC");
    assert_eq!(app.text(), " ");
}

#[test]
fn ctrl_s_saves_and_the_comment_chord_comments() {
    let (mut app, file) = standard_on("a.rs", "fn main() {}\n");
    app.press("C-/");
    assert_eq!(app.text(), "// fn main() {}\n");
    app.press("C-s");
    assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "// fn main() {}\n");
}

#[test]
fn a_buffer_you_dont_type_in_browses_and_typing_comes_back_with_a_file() {
    let (mut app, _f) = standard_on("a.txt", "text\n");
    let file = app.focused_buffer_id();
    app.go_home();
    assert!(!app.focused_takes_typing());
    app.press("Down");
    assert_ne!(app.vim.mode(), Mode::Insert, "Home browses");
    let pane = app.focused_pane_id();
    app.set_pane_content(pane, file);
    app.press("End");
    assert_eq!(app.vim.mode(), Mode::Insert, "back in the file, typing");
}

#[test]
fn switching_schemes_moves_between_insert_and_normal() {
    let (mut app, _f) = app_on_file("a.txt", "text\n");
    assert_eq!(app.vim.mode(), Mode::Normal);
    app.run_cmd("view.key_scheme");
    assert_eq!(app.vim.mode(), Mode::Insert);
    app.run_cmd("view.key_scheme");
    assert_eq!(app.vim.mode(), Mode::Normal);
    assert!(app.caret_is_block());
}

#[test]
fn vim_keys_are_left_as_they_were() {
    let (mut app, _f) = app_on_file("a.txt", "one\ntwo\n");
    app.press_all("d d");
    assert_eq!(app.text(), "two\n");
    app.press("C-c");
    assert_eq!(app.text(), "two\n", "Ctrl+C isn't copy with Vim keys");
    app.press("S-Right");
    assert_eq!(app.vim.mode(), Mode::Normal, "Shift+Right is Right");
}

fn code_compose(app: &mut App) {
    app.open_compose(super::ComposePurpose::Code { ext: "fenix.test".into(), id: 1, what: "a comment".into(), verb: "send", allow_empty: true, ask: None });
}

#[test]
fn a_compose_pane_is_typed_into_ctrl_enter_sends_it_and_escape_asks_first() {
    let (mut app, _f) = standard_on("a.txt", "");
    code_compose(&mut app);
    assert_eq!(app.vim.mode(), Mode::Insert);
    app.type_text("looks good");
    app.press("Esc");
    assert!(app.compose.is_some(), "the first Esc asks");
    app.type_text("!");
    app.press("Esc");
    assert!(app.compose.is_some(), "typing in between: asked again");
    app.press("Esc");
    assert!(app.compose.is_none(), "the second throws it away");
    code_compose(&mut app);
    app.press("Esc");
    assert!(app.compose.is_none(), "an empty one goes at once");
    code_compose(&mut app);
    app.type_text("ship it");
    app.press("C-Enter");
    assert!(app.compose.is_none(), "sent");
}

#[test]
fn debug_s_letter_keys_are_text_with_standard_keys() {
    let (mut app, _f) = standard_on("a.txt", "");
    assert!(!app.debug_mode_active());
    app.type_text("nsocr");
    assert_eq!(app.text(), "nsocr");
}

#[test]
fn a_split_pane_goes_with_its_last_tab_with_standard_keys_only() {
    for standard_keys in [true, false] {
        let (mut app, _f) = app_on_file("a.txt", "a\n");
        if standard_keys {
            standard(&mut app);
        }
        app.run_cmd("window.split_vertical");
        assert_eq!(app.windows().window_count(), 2);
        app.run_cmd("tab.close");
        assert_eq!(app.windows().window_count(), if standard_keys { 1 } else { 2 }, "standard keys: {standard_keys}");
        if standard_keys {
            app.run_cmd("tab.close");
            assert_eq!(app.windows().window_count(), 1, "the last pane stays");
        }
    }
}

#[test]
fn f2_and_delete_rename_and_delete_in_a_listing() {
    let (mut app, f) = standard_on("a.txt", "a\n");
    let dir = f.path.parent().unwrap().to_path_buf();
    app.explorer_open_path(&dir);
    assert!(app.active_explorer_target().is_some());
    assert_eq!(app.standard_explorer_alias(crate::keymap::parse_key("F2").unwrap()), Some(fenix_keymap::KeyPress::char('R')));
    assert_eq!(app.standard_explorer_alias(crate::keymap::parse_key("Delete").unwrap()), Some(fenix_keymap::KeyPress::char('D')));
}

#[test]
fn the_tab_strip_s_split_button_splits_its_pane() {
    let (mut app, _f) = app_on_file("a.txt", "a\n");
    let pane = app.focused_pane_id();
    let file = app.focused_buffer_id();
    let body = fenix_window::Rect { x: 10.0, y: 0.0, w: 80.0, h: 20.0 };
    app.tab_geometry = vec![(pane, super::TabRect { buffer: file, body, close: fenix_window::Rect { x: 70.0, y: 0.0, w: 20.0, h: 20.0 } })];
    let geometry = app.frame_geometry(800.0, 0.0, 0.0, 600.0);
    let (_, button) = geometry.split_buttons.iter().find(|(p, _)| *p == pane).copied().expect("a split button");
    assert_eq!(button.x, 90.0, "right after the last tab");
    assert_eq!(super::hit_test(&geometry, (button.x + 2.0, 10.0)), Some(super::ScrollTarget::SplitPane(pane)));
    app.handle_click_at((button.x + 2.0, 10.0), 800.0, 600.0);
    assert_eq!(app.windows().window_count(), 2, "split");
}

/// The phrases only Vim keys mean, as hints write them.
const VIM_PHRASES: &[&str] = &["Esc then Enter", "Esc and Enter", "q discards", "(i to insert)", "Normal mode sends", ":w!", ":e!", ":wq", ":q!", ":w <path>", ":w to save", ":w keeps", ":w and", "K on a name", "K is the hover", "Esc then A"];

/// Every string literal in fenix-gui's source holding one of them.
fn literals_with_vim_phrases() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
    let mut found = Vec::new();
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        // Tests say what Vim keys do; the translation table says both.
        if name.contains("test") || name == "keys_host.rs" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        // Up to the file's tests.
        let text = text.split("#[cfg(test)]\nmod tests").next().unwrap_or(&text);
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") || trimmed.starts_with("#[") {
                continue;
            }
            for literal in line.split('"').skip(1).step_by(2) {
                if VIM_PHRASES.iter().any(|p| literal.contains(p)) {
                    found.push((name.clone(), literal.to_string()));
                }
            }
        }
    }
    found
}

/// A hint that names Vim's keys says standard keys' with them: nothing
/// shown with standard keys asks for Esc then Enter, `q`, or `:w!`.
#[test]
fn every_hint_naming_vim_keys_is_said_in_standard_keys_too() {
    let (mut app, _f) = standard_on("a.txt", "");
    let literals = literals_with_vim_phrases();
    assert!(!literals.is_empty(), "the scan finds the hints");
    // Help and descriptions that name a Vim key beside the command are
    // fine: they say what the command does in Vim terms.
    let described = |l: &str| l.contains("(:w!)") || l.contains("(:e!)") || l.contains("(K)") || l.starts_with("Save, overwriting") || l.starts_with("Re-read");
    let untranslated: Vec<_> = literals.iter().filter(|(_, l)| !described(l)).filter(|(_, l)| {
        let said = app.keys_text(l);
        VIM_PHRASES.iter().any(|p| said.contains(p))
    }).collect();
    assert!(untranslated.is_empty(), "say these with standard keys too (keys_host.rs's STANDARD_PHRASES): {untranslated:#?}");
    app.set_message("SPC g g opens Git; write, then Esc and Enter to send -- q discards");
    let shown = app.modeline_pieces().1;
    assert!(shown.contains("C-k g g") && shown.contains("Ctrl+Enter"), "{shown}");
}

#[test]
fn key_for_says_the_keys_the_scheme_has() {
    let (mut app, _f) = app_on_file("a.txt", "");
    assert_eq!(app.key_for("file.save").as_deref(), Some("C-s"));
    assert_eq!(app.key_for("git.open").as_deref(), Some("SPC g g"));
    standard(&mut app);
    assert_eq!(app.key_for("code.goto_definition").as_deref(), Some("F12"));
    assert_eq!(app.key_for("git.open").as_deref(), Some("C-k g g"));
    assert_eq!(app.key_for("commands.palette").as_deref(), Some("F1"), "the shortest");
    assert_eq!(super::translate_leader("SPC SPC, (SPC a a) SPCX", "C-k"), "C-k SPC, (C-k a a) SPCX", "the second key of SPC SPC is Space");
}

#[test]
fn the_modeline_says_the_leader_s_chord_until_it_s_been_used() {
    let (mut app, _f) = standard_on("a.txt", "");
    assert!(app.modeline_text().contains("C-k commands"), "{}", app.modeline_text());
    for _ in 0..3 {
        app.press_all("C-k Esc");
    }
    assert!(!app.modeline_text().contains("C-k commands"));
}

#[test]
fn enter_on_an_indented_blank_line_keeps_the_indent() {
    let (mut app, _f) = standard_on("a.rs", "");
    app.type_text("mod tests {\nuse super::*;\n\n#[test]");
    assert_eq!(app.text(), "mod tests {\n    use super::*;\n    \n    #[test]\n}", "{:?}", app.text());
}

#[test]
fn enter_on_an_indented_blank_line_keeps_the_indent_with_vim_keys_too() {
    let (mut app, _f) = app_on_file("a.rs", "");
    app.press("i");
    app.type_text("mod tests {\nuse super::*;\n\n#[test]");
    assert_eq!(app.text(), "mod tests {\n    use super::*;\n    \n    #[test]\n}", "{:?}", app.text());
}

#[test]
fn escape_closes_the_completion_popup_and_keeps_typing() {
    let (mut app, _f) = standard_on("a.txt", "alphabet\n");
    app.press_all("C-End");
    app.type_text("alp");
    assert!(app.completion.is_some(), "typing a word's start offers the rest");
    app.press("Esc");
    assert!(app.completion.is_none(), "Escape closes the popup");
    assert_eq!(app.vim.mode(), Mode::Insert);
    app.type_text("s");
    assert_eq!(app.text(), "alphabet\nalps");
}

#[test]
fn moving_the_cursor_after_a_word_does_not_open_completion() {
    let (mut app, _f) = standard_on("a.txt", "alphabet\nalp and more\n");
    app.press_all("Down Right Right Right");
    assert_eq!(app.line_col(), (1, 3));
    assert!(app.completion.is_none(), "an arrow isn't asking for completion");
    app.press_all("Left Right");
    assert!(app.completion.is_none());
}

#[test]
fn ctrl_backspace_deletes_a_word_in_a_prompt() {
    let (mut app, _f) = standard_on("old_name.txt", "x\n");
    app.run_cmd("file.rename");
    let before = app.rename_file_prompt.clone().unwrap();
    app.type_text("_v2");
    app.press("C-Backspace");
    assert_eq!(app.rename_file_prompt.as_deref(), Some(before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')), "from {before}");
    app.press("Esc");
    assert!(app.rename_file_prompt.is_none());
}

#[test]
fn references_say_how_many_and_the_key_for_the_next() {
    let (mut app, f) = standard_on("a.rs", "fn a() {}\nfn b() { a(); a(); }\n");
    let uri = fenix_lsp::path_to_uri(&f.path).unwrap();
    let at = |line, character| lsp_types::Location { uri: uri.clone(), range: lsp_types::Range::new(lsp_types::Position::new(line, character), lsp_types::Position::new(line, character + 1)) };
    app.search.quickfix_page = Some(app.focused_buffer_id());
    app.apply_lsp_references(vec![at(0, 3), at(1, 9), at(1, 14)]);
    assert_eq!(app.line_col(), (0, 3), "at the first");
    let message = app.status_message.as_ref().map(|m| m.text.clone()).unwrap_or_default();
    assert_eq!(message, "3 references -- F3 goes to the next");
    assert!(app.search.quickfix_page.is_none(), "the Search page no longer follows the list");
    app.press("F3");
    assert_eq!(app.line_col(), (1, 9));
}
