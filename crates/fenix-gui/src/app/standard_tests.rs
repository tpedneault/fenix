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
