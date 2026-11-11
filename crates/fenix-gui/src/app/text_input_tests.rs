//! Text that arrives with no key of its own -- Windows' `VK_PACKET`
//! (`SendInput` with `KEYEVENTF_UNICODE`: dictation, the on-screen
//! keyboard, auto-type, automation tools) and an input method's commit --
//! lands wherever typing would (#37). `to_keypresses` turns the event into
//! keypresses (tested in `keymap`); these go from there, through the real
//! key path.

use std::path::PathBuf;

use super::test_keys::app_on_file;
use super::{picker_query, ActivePicker, App};

#[test]
fn unicode_text_types_into_a_buffer_in_insert() {
    let (mut app, _f) = app_on_file("a.txt", "\n");
    app.press("i");
    app.receive_text("ledger", None);
    assert_eq!(app.text(), "ledger\n");
    // Non-Latin text, and a line break, too.
    app.receive_text("\r\nעברית 😀", None);
    assert_eq!(app.text(), "ledger\nעברית 😀\n");
}

#[test]
fn unicode_text_in_normal_mode_is_keys_like_any_other() {
    let (mut app, _f) = app_on_file("a.txt", "one\ntwo\n");
    // `j` sent as text moves down, as the key would.
    app.receive_text("j", None);
    assert_eq!(app.line_col(), (1, 0));
}

#[test]
fn unicode_text_types_into_a_prompt() {
    let (mut app, _f) = app_on_file("old.txt", "x\n");
    app.run_cmd("file.rename");
    app.press("C-u");
    app.receive_text("ledger.txt", None);
    assert!(app.rename_file_prompt.as_deref().is_some_and(|p| p.ends_with("ledger.txt")), "{:?}", app.rename_file_prompt);
}

#[test]
fn unicode_text_types_into_a_picker() {
    let candidates = vec![fenix_picker::Candidate::new("ledger.rs", PathBuf::from("ledger.rs")), fenix_picker::Candidate::new("main.rs", PathBuf::from("main.rs"))];
    let mut app = App::with_file(None);
    app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    app.receive_text("ledg", None);
    assert_eq!(picker_query(app.active_picker.as_ref().unwrap()), "ledg");
}

#[test]
fn unicode_text_on_a_terminal_is_its_utf8() {
    // What the terminal is sent for each keypress the text becomes.
    let bytes: Vec<u8> = crate::keymap::text_keypresses("é😀").into_iter().flat_map(|k| super::terminal_keys::encode(k, false, false, false).unwrap()).collect();
    assert_eq!(bytes, "é😀".as_bytes());
}
