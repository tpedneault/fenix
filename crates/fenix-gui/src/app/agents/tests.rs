//! The agents host, driven against the mock agent (`fenix:mock`, playing
//! Claude Code's adapter) in this process.

use super::*;
use crate::page::Key;
use fenix_acp::thread::Entry;

struct Fixture {
    app: App,
    root: PathBuf,
    _dir: tempfile::TempDir,
}

/// An app in a project of its own, with the mock as its only agent.
fn fixture(mock_flags: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    // Without Windows' `\\?\` prefix, as Fenix keeps paths.
    let root = fenix_lsp::normalize(dir.path().canonicalize().unwrap());
    let store = root.join(".agent-store");
    let mut app = App::with_file(None);
    app.project_root = Some(root.clone());
    // As a workspace per project has it: the panel's page has no file to
    // find the project by.
    app.workspaces.active_workspace_mut().project = Some(root.clone());
    app.agents.store = Some(Store::new(root.join(".threads")));
    let line = format!("fenix:mock --profile claude --store \"{}\" {mock_flags}", store.display());
    app.config.set_value("agents.servers", Some(fenix_config::Value::Map(vec![("mock".into(), line)])));
    app.config.set_value("agents.default", Some(fenix_config::Value::Text("mock".into())));
    app.config.set_value("agents.mcp.fenix", Some(fenix_config::Value::Bool(false)));
    Fixture { app, root, _dir: dir }
}

impl Fixture {
    /// Takes what the agent sent until `until` holds (or 10 s pass).
    fn pump(&mut self, until: impl Fn(&App) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.agents_drain_inbox();
            self.app.agents_tick(Instant::now());
            if until(&self.app) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn thread(&self) -> ThreadId {
        self.app.agents.shown(&self.app.pages).expect("a thread in the panel")
    }

    fn view(&self) -> fenix_acp::hub::ThreadView<'_> {
        self.app.agents.hub.view(&self.thread()).unwrap()
    }

    fn type_prompt(&mut self, text: &str) {
        let composer = self.app.agents.composer.unwrap();
        self.app.replace_buffer_from_disk(composer, text);
        self.app.agents_send();
    }

    fn idle(&self) -> bool {
        let v = self.view();
        matches!(v.link, Link::Live { .. }) && matches!(v.turn, Turn::Idle) && v.queued == 0
    }

    fn page_text(&mut self) -> String {
        let page = self.app.agents.page.unwrap();
        self.app.restale_page(page);
        let Some(PageModel::Agent(p)) = self.app.pages.get(&page).map(|s| &s.model) else { panic!("the agent page") };
        self.app.agents.layout(p, 100).all_text()
    }

    fn key(&mut self, key: Key) {
        let page = self.app.agents.page.unwrap();
        self.app.agents_page_key(page, key);
    }

    fn turn(&mut self, prompt: &str) {
        self.type_prompt(prompt);
        assert!(self.pump(|app| app.agents.hub.status().working > 0 || app.agents.hub.status().waiting > 0) || self.idle(), "the prompt never went out");
        assert!(self.pump(|app| app.agents.hub.status() == Default::default()), "the turn never ended: {:?}", self.view().thread.entries);
    }
}

#[test]
fn the_panel_opens_a_thread_and_a_prompt_gets_an_answer() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.app.agents.page.is_some() && f.app.agents.composer.is_some());
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))), "never connected");
    f.turn("hello");
    let text = f.page_text();
    assert!(text.contains("› You") && text.contains("hello"), "{text}");
    assert!(f.view().thread.entries.iter().any(|e| matches!(e, Entry::Agent { text, .. } if !text.is_empty())));
    assert!(text.contains("Claude Agent"), "the agent's own name: {text}");
    // The prompt was cleared for the next one.
    assert_eq!(f.app.buffers.get(f.app.agents.composer.unwrap()).unwrap().buffer.text(), "");
    // The thread was saved.
    f.app.agents_tick(Instant::now());
    assert_eq!(f.app.agents.store.as_ref().unwrap().index().len(), 1);
}

/// The thread keeps its newest lines in view as it grows, with the prompt
/// under it focused: the end of the last answer is the pane's bottom row,
/// what came before scrolled away. Moved up off the newest card, it stays.
#[test]
fn the_thread_keeps_its_newest_lines_in_view_while_you_write_below() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))), "never connected");
    for prompt in ["one", "two", "three", "four"] {
        f.turn(prompt);
    }
    let page = f.app.agents.page.unwrap();
    let (Some(page_pane), Some(composer_pane)) = f.app.agents_dock_panes() else { panic!("the dock") };
    f.app.windows_mut().focus(composer_pane);
    let rows = 8;
    let lay_out = |f: &mut Fixture| {
        f.app.restale_page(page);
        f.app.note_page_rows(page, rows);
        f.app.ensure_page_layout(page, page_pane, 100);
    };
    lay_out(&mut f);
    let lines = f.app.buffers.get(page).unwrap().buffer.line_count();
    assert!(lines > rows * 2, "a thread longer than the pane: {lines} lines");
    let scroll = f.app.pane_state(page_pane).scroll_line;
    assert_eq!(scroll + rows, lines, "the last line is the bottom row");

    // Up a card: no longer following, the view isn't pulled to the end.
    f.key(Key::Char('k'));
    f.app.pane_state_mut(page_pane).scroll_line = 0;
    lay_out(&mut f);
    assert_eq!(f.app.pane_state(page_pane).scroll_line, 0);
}

/// Each pane's rect, in a 1000 × 600 window.
fn rects(app: &App) -> Vec<(fenix_window::WindowId, fenix_window::Rect)> {
    app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 1000.0, h: 600.0 })
}

#[test]
fn the_dock_is_one_column_beside_every_pane_and_never_splits_the_code() {
    let mut f = fixture("");
    f.app.split_horizontal();
    let code: Vec<_> = f.app.windows().windows();
    f.app.agents_open_panel();
    let (Some(page), Some(composer)) = f.app.agents_dock_panes() else { panic!("the dock's two panes") };
    let all = rects(&f.app);
    let rect = |id| all.iter().find(|(w, _)| *w == id).unwrap().1;
    assert!(code.iter().all(|c| rect(*c).x + rect(*c).w <= rect(page).x), "the dock is right of every pane: {all:?}");
    assert_eq!(rect(page).y, 0.0);
    assert_eq!(rect(composer).y + rect(composer).h, 600.0, "the dock runs the full height");
    assert_eq!(code.iter().map(|c| rect(*c).w).collect::<Vec<_>>(), vec![rect(code[0]).w; 2], "the code's own split is kept");
    // Titles, not tab strips; the prompt ready to write in.
    assert!(f.app.pane_titles.get(&page).is_some_and(|t| t.starts_with("Agent")));
    assert!(f.app.pane_titles.get(&composer).is_some_and(|t| t.contains("sends")), "{:?}", f.app.pane_titles.get(&composer));
    assert_eq!((f.app.focused_pane_id(), f.app.vim.mode()), (composer, Mode::Insert));
    assert_eq!(f.app.buffer_display_name(f.app.agents.composer.unwrap()), "*agent prompt*");
    // Clicked away from the prompt, still in Insert (to the code, say):
    // Normal mode there, so nothing's typed into the file by surprise.
    f.app.agents_dock_sync();
    f.app.windows_mut().focus(code[0]);
    f.app.agents_dock_sync();
    assert_eq!(f.app.vim.mode(), Mode::Normal);
    // Clicked back into the prompt (or SPC w, or a session put back with
    // it focused): Insert, at the end, so what's typed is written.
    let prompt = f.app.agents.composer.unwrap();
    f.app.replace_buffer_from_disk(prompt, "half a thought");
    f.app.windows_mut().focus(composer);
    f.app.agents_dock_sync();
    assert_eq!(f.app.vim.mode(), Mode::Insert);
    assert_eq!(f.app.pane_state(composer).cursor.char_idx, "half a thought".chars().count());
    // Esc in it stays Normal: only coming in puts it in Insert.
    f.app.agents_dock_sync();
    {
        let app = &mut f.app;
        let (ob, state) = (app.buffers.get_mut(prompt).unwrap(), app.workspaces.active_pane_states_mut().get_mut(&composer).unwrap());
        app.vim.handle_key(&mut ob.buffer, &mut state.cursor, KeyPress::named(FenixNamedKey::Escape));
    }
    f.app.agents_dock_sync();
    assert_eq!(f.app.vim.mode(), Mode::Normal);
    f.app.replace_buffer_from_disk(prompt, "");
    f.app.windows_mut().focus(code[0]);
    f.app.agents_dock_sync();
    // Another thread goes in the same dock, with the default agent.
    let first = f.thread();
    f.app.agents_new_thread();
    assert_ne!(f.thread(), first);
    assert_eq!(f.app.agents.hub.meta(&f.thread()).unwrap().agent, "mock");
    assert_eq!(f.app.windows().window_count(), 4);
    // SPC a a from the dock puts it away; the code is as it was.
    f.app.agents_open_panel();
    assert_eq!(f.app.windows().windows(), code);
}

#[test]
fn sent_with_ctrl_enter_from_insert_the_threads_keys_answer_the_question() {
    let mut f = fixture("");
    let file = f.root.join("f.txt");
    std::fs::write(&file, "a\n").unwrap();
    f.app.agents_open_panel();
    assert_eq!(f.app.vim.mode(), Mode::Insert);
    let composer = f.app.agents.composer.unwrap();
    f.app.replace_buffer_from_disk(composer, "edit f.txt a=>b");
    assert!(f.app.agents_composer_key(KeyPress::named(FenixNamedKey::Enter).with_ctrl()));
    assert_eq!(f.app.vim.mode(), Mode::Normal, "out of Insert, on the thread");
    assert_eq!(Some(f.app.focused_pane_id()), f.app.agents_dock_panes().0);
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "b\n");
}

/// Back in the prompt with a question waiting: `y` in the empty prompt,
/// even in Insert mode, answers it; with something written, it's typed.
#[test]
fn a_question_is_answered_from_the_empty_prompt() {
    let mut f = fixture("");
    let file = f.root.join("f.txt");
    std::fs::write(&file, "a\n").unwrap();
    f.app.agents_open_panel();
    let composer = f.app.agents.composer.unwrap();
    f.app.replace_buffer_from_disk(composer, "edit f.txt a=>b");
    assert!(f.app.agents_composer_key(KeyPress::named(FenixNamedKey::Enter).with_ctrl()));
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.app.agents_focus_composer();
    assert_eq!(f.app.focused_buffer_id(), composer);
    assert!(f.app.pane_titles.values().any(|t| t.contains("waiting on you")));
    // Written to: y is a letter.
    f.app.replace_buffer_from_disk(composer, "and then");
    assert!(!f.app.agents_composer_key(KeyPress::char('y')));
    f.app.replace_buffer_from_disk(composer, "");
    assert!(f.app.agents_composer_key(KeyPress::char('y')));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "b\n");
    assert_eq!(f.app.buffers.get(composer).unwrap().buffer.text(), "");
}

/// Up in the empty prompt brings back what was sent, newest first; Down
/// goes back toward it, then to an empty prompt. Changed, it's the
/// prompt's own again: the arrows move in it.
#[test]
fn up_in_the_empty_prompt_brings_back_earlier_prompts() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))), "never connected");
    f.turn("first");
    f.turn("second");
    f.app.agents_focus_composer();
    let composer = f.app.agents.composer.unwrap();
    let text = |f: &Fixture| f.app.buffers.get(composer).unwrap().buffer.text();
    let up = KeyPress::named(FenixNamedKey::Up);
    let down = KeyPress::named(FenixNamedKey::Down);
    assert!(f.app.agents_composer_key(up));
    assert_eq!(text(&f), "second");
    assert!(f.app.agents_composer_key(up));
    assert_eq!(text(&f), "first");
    assert!(f.app.agents_composer_key(up), "the oldest stays");
    assert_eq!(text(&f), "first");
    assert!(f.app.agents_composer_key(down));
    assert_eq!(text(&f), "second");
    assert!(f.app.agents_composer_key(down));
    assert_eq!(text(&f), "", "past the newest, empty again");
    assert!(!f.app.agents_composer_key(down), "an empty prompt's Down is Vim's");
    assert!(f.app.agents_composer_key(up));
    f.app.replace_buffer_from_disk(composer, "second, changed");
    assert!(!f.app.agents_composer_key(up), "changed: the arrows move in it");
}

#[test]
fn spc_a_capital_a_shows_and_puts_away_the_dock_from_anywhere() {
    let mut f = fixture("");
    let code = f.app.windows().windows();
    f.app.agents_toggle_dock();
    assert!(matches!(f.app.agents_dock_panes(), (Some(_), Some(_))), "shown");
    // Back in the code, where SPC a a would only go to the prompt.
    f.app.agents_focus_code();
    f.app.agents_toggle_dock();
    assert_eq!(f.app.windows().windows(), code, "put away from the code too");
}

#[test]
fn the_prompt_offers_no_completions_as_you_type() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert_eq!(f.app.vim.mode(), Mode::Insert);
    let composer = f.app.agents.composer.unwrap();
    // Words a plain buffer would offer back ("hello" for "hel").
    f.app.replace_buffer_from_disk(composer, "hello there hel");
    let (pane, end) = (f.app.focused_pane_id(), f.app.buffers.get(composer).unwrap().buffer.len_chars());
    f.app.workspaces.active_pane_states_mut().get_mut(&pane).unwrap().cursor.char_idx = end;
    f.app.sync_completion();
    assert!(f.app.completion.is_none());
}

#[test]
fn a_dock_put_back_by_the_session_is_taken_on_not_built_again() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let (Some(page_pane), Some(composer_pane)) = f.app.agents_dock_panes() else { panic!("the dock") };
    // As a restart left it: the thread's pane showing something else, the
    // prompt still up, and a second pane showing the same prompt.
    let other = f.app.buffers.open_text_view("old log\n");
    f.app.windows_mut().set_content(page_pane, other);
    f.app.windows_mut().focus(composer_pane);
    f.app.split_vertical();
    f.app.pane_titles.clear();
    f.app.agents_adopt_dock();
    let composer = f.app.agents.composer.unwrap();
    assert_eq!(f.app.agents_panes_showing(composer).len(), 1, "one prompt");
    let (Some(page), Some(composer_pane)) = f.app.agents_dock_panes() else { panic!("the dock again") };
    let all = rects(&f.app);
    let rect = |id| all.iter().find(|(w, _)| *w == id).unwrap().1;
    assert_eq!(rect(page).x, rect(composer_pane).x, "the thread above its prompt: {all:?}");
    assert!(rect(page).y < rect(composer_pane).y);
    assert_eq!((rect(page).y, rect(composer_pane).y + rect(composer_pane).h), (0.0, 600.0), "the full height");
    assert!(all.iter().all(|(_, r)| r.x <= rect(page).x), "at the right: {all:?}");
    assert!(f.app.pane_titles.contains_key(&page) && f.app.pane_titles.contains_key(&composer_pane));
    assert!(!f.app.agents_is_dock_pane(f.app.focused_pane_id()), "the focus is left in the code, for a file opened next");
    // A file opened in a dock pane anyway: an ordinary pane again.
    let file = f.app.buffers.open_text_view("x\n");
    f.app.windows_mut().set_content(composer_pane, file);
    f.app.agents_dock_sync();
    assert!(!f.app.pane_titles.contains_key(&composer_pane));
}

#[test]
fn the_connection_log_is_named_and_not_kept_with_the_session() {
    let mut f = connected();
    f.app.agents_open_log();
    let shown = f.app.focused_buffer_id();
    assert_eq!(f.app.buffer_display_name(shown), LOG_VIEW);
    assert!(f.app.buffers.get(shown).unwrap().kind.is_session_owned());
    assert!(!f.app.agents_is_dock_pane(f.app.focused_pane_id()), "opened by the code, not in the dock");
    // Asked for again: the same view, brought up to date.
    f.app.agents_open_log();
    assert_eq!(f.app.focused_buffer_id(), shown);
}

#[test]
fn an_edit_on_disk_is_asked_snapshotted_and_followed_by_the_clean_buffer() {
    let mut f = fixture("");
    let file = f.root.join("fruit.txt");
    std::fs::write(&file, "apple\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.agents_open_panel();
    f.type_prompt("edit fruit.txt apple=>pear");
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.pending_permissions() > 0)), "no question");
    assert!(f.page_text().contains("? The agent asks to: Edit fruit.txt"), "{}", f.page_text());
    // Snapshotted when announced, before the write.
    let thread = f.thread();
    assert_eq!(f.app.agents.baselines.get(&thread).and_then(|b| b.get(&file)), Some(&Some("apple\n".to_string())));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "apple\n", "nothing written before the answer");
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "pear\n");
    assert!(f.pump(|app| app.buffers.get(buffer).unwrap().buffer.text() == "pear\n"), "the clean buffer follows the disk");
}

#[test]
fn a_write_through_fenix_lands_in_the_buffer_saved() {
    let mut f = fixture("--fs");
    let file = f.root.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.agents_open_panel();
    f.app.config.set_value("agents.save_before_prompt", Some(fenix_config::Value::Text("never".into())));
    f.type_prompt("edit a.txt one=>two");
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.pending_permissions() > 0)));
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    let ob = f.app.buffers.get(buffer).unwrap();
    assert_eq!(ob.buffer.text(), "two\n");
    assert!(!ob.buffer.is_dirty(), "saved as made");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "two\n");
}

#[test]
fn a_write_over_your_unsaved_edits_asks_and_keep_mine_refuses_it() {
    let mut f = fixture("--fs");
    let file = f.root.join("a.txt");
    std::fs::write(&file, "one\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.replace_buffer_from_disk(buffer, "one\nmine\n");
    f.app.buffers.get_mut(buffer).unwrap().buffer.mark_unsaved();
    f.app.config.set_value("agents.save_before_prompt", Some(fenix_config::Value::Text("never".into())));
    f.app.agents_open_panel();
    f.type_prompt("edit a.txt one=>two");
    // The agent's own question first.
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.pending_permissions() > 0)));
    f.key(Key::Char('y'));
    // Then Fenix's: your edits.
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.entries.iter().any(|e| matches!(e, Entry::Permission(c) if c.key.starts_with("fenix-guard") && c.state == PermissionState::Pending)))), "{:?}", f.view().thread.entries);
    assert!(f.page_text().contains("unsaved edits of yours"));
    f.key(Key::Char('n'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert_eq!(f.app.buffers.get(buffer).unwrap().buffer.text(), "one\nmine\n", "yours kept");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\n");
}

#[test]
fn reads_outside_the_project_are_refused() {
    let mut f = fixture("--fs");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    let thread = f.thread();
    let conn = f.app.agents.hub.connection_of(&thread).unwrap();
    let session = f.view().meta.session_id.clone().unwrap();
    let outside = std::env::temp_dir().join("elsewhere.txt");
    let result = f.app.agents_serve(conn, &thread, json!(99), ClientRequest::ReadTextFile { session_id: session.clone(), path: outside.to_string_lossy().into_owned(), line: None, limit: None });
    assert!(matches!(result, Some(Err(e)) if e.code == code::INVALID_PARAMS));
    let sneaky = f.root.join("../x");
    let result = f.app.agents_serve(conn, &thread, json!(100), ClientRequest::ReadTextFile { session_id: session, path: sneaky.to_string_lossy().into_owned(), line: None, limit: None });
    assert!(matches!(result, Some(Err(_))));
}

#[test]
fn a_read_sees_the_buffer_unsaved_edits_included() {
    let mut f = fixture("--fs");
    let file = f.root.join("notes.txt");
    std::fs::write(&file, "on disk\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.replace_buffer_from_disk(buffer, "in the buffer\n");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    let thread = f.thread();
    let conn = f.app.agents.hub.connection_of(&thread).unwrap();
    let session = f.view().meta.session_id.clone().unwrap();
    let result = f.app.agents_serve(conn, &thread, json!(1), ClientRequest::ReadTextFile { session_id: session, path: file.to_string_lossy().into_owned(), line: None, limit: None });
    assert_eq!(result.unwrap().unwrap(), json!({ "content": "in the buffer\n" }));
}

#[test]
fn saving_before_a_prompt_asks_when_files_are_changed() {
    let mut f = fixture("");
    let file = f.root.join("a.txt");
    std::fs::write(&file, "x\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.replace_buffer_from_disk(buffer, "y\n");
    f.app.buffers.get_mut(buffer).unwrap().buffer.mark_unsaved();
    f.app.agents_open_panel();
    f.type_prompt("hello");
    assert!(matches!(f.app.active_picker, Some(ActivePicker::Agents(_))), "asked");
    f.app.agents_picked(Pick::Send { save: true });
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "y\n", "saved first");
    assert!(f.pump(|app| app.agents.hub.status() == Default::default() && app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.entries.len() > 1)));
}

#[test]
fn a_crash_mid_turn_is_marked_and_the_next_prompt_resumes() {
    let mut f = fixture("--crash-after 3");
    f.app.agents_open_panel();
    f.type_prompt("slow");
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.entries.iter().any(|e| matches!(e, Entry::Notice(n) if n.kind == NoticeKind::Interrupted)))), "{:?}", f.view().thread.entries);
    // The panel is focused: the agent comes back (without the crash cue
    // it would crash again; here it's enough that it's started again).
    assert!(f.pump(|app| app.agents.hub.connections().any(|(_, c)| c.generation > 1)), "restarted");
}

#[test]
fn modes_switch_from_the_picker_and_the_page_shows_them() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    assert!(f.page_text().contains("Manual"), "{}", f.page_text());
    f.app.agents_picked(Pick::Mode("plan".into()));
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.modes.as_ref().is_some_and(|m| m.current == "plan"))));
    assert!(f.page_text().contains("Plan"));
    f.app.agents_picked(Pick::Config { id: "model".into(), value: ConfigValue::Select { current: "opus".into(), choices: vec![] } });
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.config_options.iter().any(|o| o.id == "model" && o.current_label().to_lowercase().contains("opus")))));
}

#[test]
fn a_thread_comes_back_from_the_store_and_resumes() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    f.turn("hello");
    let thread = f.thread();
    f.app.agents_shutdown();
    f.app.agents_close_panel();
    // A new Fenix: the same store, nothing in memory.
    let store = f.app.agents.store.take();
    f.app.agents = AgentHost::new();
    f.app.agents.store = store;
    f.app.agents_open_panel();
    assert_eq!(f.thread(), thread, "the project's last thread");
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))), "resumed");
    assert!(f.view().thread.entries.len() >= 2, "with its history");
}

#[test]
fn the_modeline_says_when_an_agent_waits_on_you() {
    let mut f = fixture("");
    let file = f.root.join("f.txt");
    std::fs::write(&file, "a\n").unwrap();
    f.app.agents_open_panel();
    f.type_prompt("edit f.txt a=>b");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    assert_eq!(f.app.agents_status_segment(), "   ? Claude Agent is waiting on you");
    f.key(Key::CtrlC);
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert_eq!(f.app.agents_status_segment(), "");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "a\n", "cancelled before the write");
}

/// The fixture after the mock edited `fruit.txt` (apple -> pear) on disk,
/// with the file open, allowed.
fn edited() -> (Fixture, PathBuf, BufferId) {
    let mut f = fixture("");
    let file = f.root.join("fruit.txt");
    std::fs::write(&file, "apple\nbanana\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.agents_open_panel();
    f.type_prompt("edit fruit.txt apple=>pear");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default() && app.buffers.get(buffer).unwrap().buffer.text() == "pear\nbanana\n"));
    (f, file, buffer)
}

fn review_files(f: &Fixture) -> Vec<crate::agent_review::ReviewFile> {
    let id = f.app.find_page(|m| matches!(m, PageModel::AgentReview(_))).expect("the review page");
    match f.app.pages.get(&id).map(|s| &s.model) {
        Some(PageModel::AgentReview(p)) => p.files.clone(),
        _ => unreachable!(),
    }
}

fn review_key(f: &mut Fixture, key: Key) {
    let id = f.app.find_page(|m| matches!(m, PageModel::AgentReview(_))).unwrap();
    f.app.agents_review_key(id, key);
}

#[test]
fn the_review_lists_the_change_and_rejecting_it_puts_the_file_back() {
    let (mut f, file, buffer) = edited();
    f.app.agents_open_review();
    let files = review_files(&f);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].name, "fruit.txt");
    assert_eq!(files[0].hunks[0].removed, vec!["apple".to_string()]);
    assert_eq!(files[0].hunks[0].added, vec!["pear".to_string()]);
    review_key(&mut f, Key::Char('j'));
    review_key(&mut f, Key::Char('x'));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "apple\nbanana\n");
    assert_eq!(f.app.buffers.get(buffer).unwrap().buffer.text(), "apple\nbanana\n");
    assert!(review_files(&f).is_empty(), "nothing left to review");
}

#[test]
fn a_rejected_file_is_no_longer_the_threads_so_another_threads_edit_isnt_shown_as_its() {
    let (mut f, file, _) = edited();
    let first = f.thread();
    f.app.agents_open_review();
    review_key(&mut f, Key::Char('j'));
    review_key(&mut f, Key::Char('x'));
    assert!(!f.app.agents.baselines.get(&first).is_some_and(|b| b.contains_key(&file)), "let go of once settled");
    // Someone else -- another thread, you -- changes the file afterwards.
    std::fs::write(&file, "kiwi\nbanana\n").unwrap();
    f.app.agents_open_review();
    assert!(review_files(&f).is_empty(), "not the first thread's change");
}

/// The focus on `buffer` in the code, the cursor on `line` (from 0).
fn in_the_code(f: &mut Fixture, buffer: BufferId, line: usize) {
    f.app.agents_focus_code();
    f.app.open_buffer_in_focused_pane(buffer);
    let (buf, cursor) = f.app.focused_buffer_and_cursor_mut();
    cursor.char_idx = buf.line_start_char(line);
}

#[test]
fn in_the_code_bracket_a_finds_the_agents_change_and_spc_a_x_puts_it_back() {
    let (mut f, file, buffer) = edited();
    in_the_code(&mut f, buffer, 1);
    assert!(!f.app.agents_is_dock_pane(f.app.focused_pane_id()));
    f.app.agents_jump_to_hunk(false, 1);
    assert_eq!(f.app.open().buffer.line_col(&f.app.cursor()).0, 0, "[a: back to the agent's line");
    f.app.agents_preview_here();
    assert!(f.app.lsp_hover.as_deref().is_some_and(|h| h.contains("- apple") && h.contains("+ pear")), "{:?}", f.app.lsp_hover);
    f.app.agents_reject_here();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "apple\nbanana\n");
    f.app.agents_jump_to_hunk(true, 1);
    assert!(f.app.status_message.as_ref().is_some_and(|m| m.text.contains("no more agent changes")), "nothing left to go to");
}

#[test]
fn the_modeline_says_what_is_left_to_review_until_it_is_settled() {
    let (mut f, _, buffer) = edited();
    assert_eq!(f.app.agents_status_segment(), "   ✓ agent changes: 1 file to review (SPC a r, ]a)");
    in_the_code(&mut f, buffer, 0);
    f.app.agents_keep_here();
    assert_eq!(f.app.agents_status_segment(), "");
}

#[test]
fn in_the_code_spc_a_y_keeps_the_agents_change() {
    let (mut f, file, buffer) = edited();
    in_the_code(&mut f, buffer, 0);
    f.app.agents_keep_here();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "pear\nbanana\n");
    assert!(!f.app.wasm.decorations.get(&buffer).is_some_and(|sets| sets.iter().any(|(owner, _)| owner == "fenix:agents")), "no tint once kept");
    // Kept, it's an ordinary change now: nothing of the agent's here.
    f.app.agents_keep_here();
    assert!(f.app.status_message.as_ref().is_some_and(|m| m.text.contains("isn't on an agent's change")));
}

#[test]
fn keeping_the_change_takes_it_off_the_review_and_leaves_the_file() {
    let (mut f, file, buffer) = edited();
    assert!(f.app.wasm.decorations.get(&buffer).is_some_and(|sets| sets.iter().any(|(owner, set)| owner == "fenix:agents" && !set.is_empty())), "the agent's line is tinted");
    f.app.agents_open_review();
    review_key(&mut f, Key::Char('a'));
    assert!(review_files(&f).is_empty());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "pear\nbanana\n");
    assert!(!f.app.wasm.decorations.get(&buffer).is_some_and(|sets| sets.iter().any(|(owner, _)| owner == "fenix:agents")), "no tint once kept");
}

#[test]
fn a_new_file_rejected_goes_away() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    f.type_prompt("write made.txt hello");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('y'));
    let made = f.root.join("made.txt");
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()) && made.exists());
    f.app.agents_open_review();
    let files = review_files(&f);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].baseline, crate::agent_review::Baseline::Absent);
    review_key(&mut f, Key::Char('X'));
    assert!(!made.exists(), "to the Recycle Bin");
    assert!(review_files(&f).is_empty());
}

#[test]
fn a_change_no_tool_call_announced_is_caught_at_the_turn_end() {
    let mut f = fixture("--unannounced-write");
    let _ = std::process::Command::new("git").args(["init", "-q"]).current_dir(&f.root).status();
    std::fs::write(f.root.join(".gitignore"), ".agent-store/\n.threads/\n").unwrap();
    let _ = std::process::Command::new("git").args(["add", "-A"]).current_dir(&f.root).status();
    let _ = std::process::Command::new("git").args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]).current_dir(&f.root).status();
    std::fs::write(f.root.join("a.txt"), "one\n").unwrap();
    f.app.agents_open_panel();
    f.type_prompt("edit a.txt one=>two");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    f.app.agents_open_review();
    let names: Vec<String> = review_files(&f).into_iter().map(|file| file.name).collect();
    assert!(names.contains(&"a.txt".to_string()) && names.contains(&"UNANNOUNCED.txt".to_string()), "{names:?}");
}

fn connected() -> Fixture {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    f
}

fn first_prompt(f: &Fixture) -> Vec<ContentBlock> {
    f.view().thread.entries.iter().find_map(|e| if let Entry::User { blocks, .. } = e { Some(blocks.clone()) } else { None }).expect("a prompt")
}

#[test]
fn a_mentioned_file_goes_along_embedded_with_its_unsaved_edits() {
    let mut f = connected();
    let file = f.root.join("notes.txt");
    std::fs::write(&file, "on disk\n").unwrap();
    f.app.test_open_path(&file);
    let buffer = f.app.buffers.id_for_path(&file).unwrap();
    f.app.replace_buffer_from_disk(buffer, "in the buffer\n");
    f.app.config.set_value("agents.save_before_prompt", Some(fenix_config::Value::Text("never".into())));
    f.app.agents_focus_composer();
    f.type_prompt("summarise @notes.txt please, and @nope.txt");
    let blocks = first_prompt(&f);
    assert_eq!(blocks[0], ContentBlock::text("summarise @notes.txt please, and @nope.txt"));
    assert!(matches!(&blocks[1], ContentBlock::Resource { uri, text: Some(t), .. } if uri.ends_with("notes.txt") && t == "in the buffer\n"), "{blocks:?}");
    assert_eq!(blocks.len(), 2, "a mention of nothing is left as text");
}

#[test]
fn ask_about_the_selection_puts_it_in_the_prompt() {
    let mut f = fixture("");
    let file = f.root.join("code.rs");
    std::fs::write(&file, "fn main() {}\nfn other() {}\n").unwrap();
    f.app.test_open_path(&file);
    f.app.agents_ask_selection();
    let composer = f.app.agents.composer.unwrap();
    assert_eq!(f.app.buffers.get(composer).unwrap().buffer.text(), "@selection ");
    let selection = f.app.agents.selection.clone().unwrap();
    assert_eq!((selection.line, selection.text.trim()), (1, "fn main() {}"));
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    f.app.agents_composer_insert("explain this");
    f.app.agents_send();
    let blocks = first_prompt(&f);
    assert!(blocks.iter().any(|b| matches!(b, ContentBlock::Resource { text: Some(t), .. } if t.contains("fn main()"))), "{blocks:?}");
}

#[test]
fn slash_offers_the_agents_commands() {
    let mut f = connected();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| !v.thread.commands.is_empty())));
    f.app.agents_focus_composer();
    f.app.agents_pick_command();
    let Some(ActivePicker::Agents(state)) = &f.app.active_picker else { panic!("the command picker") };
    assert!(!state.is_empty());
    f.app.active_picker = None;
    f.app.agents_picked(Pick::Command("compact".into()));
    assert_eq!(f.app.buffers.get(f.app.agents.composer.unwrap()).unwrap().buffer.text(), "compact ");
}

#[test]
fn a_commit_message_is_asked_for_with_the_staged_diff() {
    let mut f = fixture("");
    let git = |args: &[&str]| std::process::Command::new("git").args(args).current_dir(&f.root).status().unwrap();
    git(&["init", "-q"]);
    std::fs::write(f.root.join("a.txt"), "one\n").unwrap();
    git(&["add", "a.txt"]);
    f.app.agents_commit_message();
    assert!(f.pump(|app| app.agents.hub.status() == Default::default() && app.agents.commit_message.is_none()), "the answer came");
    let blocks = first_prompt(&f);
    assert!(matches!(&blocks[1], ContentBlock::Resource { text: Some(t), .. } if t.contains("+one")), "{blocks:?}");
}

/// Devin's way: a whole command line as `command`, no `args` -- a shell's
/// to run, not a program's name -- in the project spelt its own way.
#[test]
fn a_command_line_runs_in_the_shell_and_the_project_in_any_letter_case_is_the_project() {
    let mut f = fixture("");
    let thread = f.app.agents.hub.new_thread("mock", &f.root);
    let cwd = if cfg!(windows) { f.root.to_string_lossy().to_uppercase().replace('\\', "/") } else { f.root.to_string_lossy().into_owned() };
    let made = f.app.agents_terminal_create(&thread, "echo one; echo two".into(), Vec::new(), Vec::new(), Some(cwd), None);
    let id = made.expect("started, not refused")["terminalId"].as_str().unwrap().to_string();
    assert!(f.pump(|app| app.agents.terminals[&id].tail(5).1.is_some()), "it ended");
    let (output, exit) = f.app.agents.terminals[&id].tail(5);
    assert!(output.contains("one") && output.contains("two"), "{output:?}");
    assert_eq!(exit.and_then(|e| e.code), Some(0));
    // Outside the project is still refused.
    let outside = std::env::temp_dir().to_string_lossy().into_owned();
    assert!(f.app.agents_terminal_create(&thread, "echo x".into(), Vec::new(), Vec::new(), Some(outside), None).is_err());
}

#[test]
fn a_command_runs_in_a_terminal_of_fenixs_and_its_output_shows() {
    let mut f = fixture("--terminal");
    f.app.agents_open_panel();
    // A program on every platform (`echo` is cmd's own on Windows).
    f.type_prompt("run git --version");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default() && app.agents.terminals.values().all(|t| t.tail(5).1.is_some())), "{:?}", f.view().thread.entries);
    assert_eq!(f.app.agents.terminals.len(), 1);
    let text = f.page_text();
    assert!(text.contains("git version") && text.contains("exited 0"), "{text}");
}

#[test]
fn a_login_the_agent_does_itself_lets_the_prompt_go_again() {
    let mut f = fixture("--logged-out --agent-auth");
    f.app.agents_open_panel();
    f.type_prompt("hello");
    assert!(f.pump(|app| app.agents.hub.connections().any(|(_, c)| c.state == ConnState::NeedsAuth)));
    assert!(f.page_text().contains("wants you to log in"), "{}", f.page_text());
    f.app.agents_pick_agent();
    let Some(ActivePicker::Agents(state)) = &f.app.active_picker else { panic!("the agents picker") };
    let login = state.visible_rows(0, state.len()).find_map(|(_, c)| matches!(c.payload, Pick::Login { .. }).then(|| c.payload.clone())).expect("a login to pick");
    f.app.active_picker = None;
    f.app.agents_picked(login);
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.entries.iter().any(|e| matches!(e, Entry::Agent { .. })))), "the prompt went again after the login: {:?}", f.view().thread.entries);
}

#[test]
fn the_panel_comes_back_with_the_session_its_prompt_too() {
    let mut f = connected();
    f.turn("hello");
    let thread = f.thread();
    f.app.agents_composer_insert("half-written");
    f.app.agents.save(&thread);
    let saved = f.app.agents_pages_to_save();
    assert_eq!(saved.iter().map(|p| p.kind.as_str()).collect::<Vec<_>>(), vec!["thread", "prompt"]);
    // A new Fenix reading the session.
    let store = f.app.agents.store.take();
    f.app.agents_shutdown();
    f.app.agents = AgentHost::new();
    f.app.agents.store = store;
    let page = f.app.agents_restore_page("thread", &saved[0].state).expect("the thread page");
    let prompt = f.app.agents_restore_page("prompt", &saved[1].state).expect("the prompt");
    assert_eq!(f.app.buffers.get(prompt).unwrap().buffer.text(), "half-written");
    f.app.show_page(page);
    assert_eq!(f.thread(), thread);
    assert!(f.view().thread.entries.len() >= 2);
    // Looked at: it attaches (the mock resumes it).
    f.key(Key::Char('j'));
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
}

#[test]
fn a_hand_off_starts_a_thread_with_another_agent_carrying_the_conversation() {
    let mut f = connected();
    let line = f.app.config.map_value("agents.servers")[0].1.clone();
    f.app.config.set_value("agents.servers", Some(fenix_config::Value::Map(vec![("mock".into(), line.clone()), ("other".into(), line)])));
    f.turn("hello");
    let from = f.thread();
    f.app.agents_picked(Pick::HandOff("other".into()));
    let to = f.thread();
    assert_ne!(to, from);
    assert_eq!(f.app.agents.hub.meta(&to).unwrap().agent, "other");
    f.turn("carry on");
    let blocks = f.view().thread.entries.iter().find_map(|e| if let Entry::User { blocks, .. } = e { Some(blocks.clone()) } else { None }).unwrap();
    assert!(blocks.iter().any(|b| matches!(b, ContentBlock::Resource { text: Some(t), .. } if t.contains("hello"))), "{blocks:?}");
}

#[test]
fn deleting_a_thread_asks_then_forgets_it() {
    let mut f = connected();
    f.turn("hello");
    let thread = f.thread();
    f.app.agents_tick(Instant::now());
    assert_eq!(f.app.agents.store.as_ref().unwrap().index().len(), 1);
    f.app.agents_delete_thread();
    assert!(matches!(f.app.active_picker, Some(ActivePicker::Agents(_))));
    f.app.active_picker = None;
    f.app.agents_picked(Pick::Delete(thread.clone()));
    assert!(f.app.agents.hub.view(&thread).is_none());
    assert!(f.app.agents.store.as_ref().unwrap().index().is_empty());
    assert!(f.app.agents.page.is_none(), "no other thread: the panel closed");
}

#[test]
fn the_registry_s_agents_are_offered_and_an_install_says_so() {
    let mut f = fixture("");
    let agents = fenix_acp::registry::parse(br#"{ "agents": [ { "id": "x", "name": "X Agent", "version": "1.0", "description": "an agent", "distribution": { "npx": { "package": "x@1.0" } } } ] }"#).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    f.app.agents.fetched = Some(rx);
    tx.send(install::Fetched::Listed(Ok(agents))).unwrap();
    f.app.agents_tick(Instant::now());
    let Some(ActivePicker::Agents(state)) = &f.app.active_picker else { panic!("the registry's agents") };
    let (_, row) = state.visible_rows(0, 1).next().unwrap();
    assert!(row.label.starts_with("Install X Agent 1.0"), "{}", row.label);
    assert!(matches!(row.payload, Pick::Install(_)));
    f.app.active_picker = None;
    let (tx, rx) = std::sync::mpsc::channel();
    f.app.agents.fetched = Some(rx);
    tx.send(install::Fetched::Installed(Err("nope: SHA-256 mismatch".into()))).unwrap();
    f.app.agents_tick(Instant::now());
    assert!(f.app.agents.fetched.is_none());
}

/// An extension like the test one, with `extra` in its manifest.
fn extension(extra: &str) -> crate::installed::CodeExtension {
    crate::app::wasm_host::tests::hello(extra)
}

#[test]
fn an_extensions_agent_is_listed_then_offered_once_it_says_how_to_start() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[[contributes.agents]]\nid = \"hello\"\nname = \"Hello Agent\"\n"));
    let entry = |app: &App| app.agents_catalog().into_iter().find(|e| e.spec.id == "hello");
    let listed = entry(&f.app).expect("listed before it runs");
    assert!(!listed.found && listed.source == catalog::Source::Extension);
    assert!(listed.login.as_deref().is_some_and(|l| l.contains("Hello")), "{listed:?}");
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    f.app.agents_provided("test.hello", "hello", Some(((exe.clone(), vec!["--acp".into()], None), vec![("HELLO".into(), "1".into())])));
    let offered = entry(&f.app).unwrap();
    assert!(offered.found);
    assert_eq!((offered.spec.program.as_str(), offered.spec.args.as_slice(), offered.spec.name.as_str()), (exe.as_str(), &["--acp".to_string()][..], "Hello Agent"));
    assert!(offered.spec.env.contains(&("HELLO".to_string(), "1".to_string())));
    assert!(f.app.agents.hub.agent("hello").is_some(), "the hub knows it at once");
    f.app.agents_provided("test.hello", "hello", None);
    assert!(!entry(&f.app).unwrap().found, "taken away");
    f.app.agents_provided("test.hello", "other", Some(((exe, Vec::new(), None), Vec::new())));
    assert!(entry(&f.app).map(|e| e.spec.id) == Some("hello".into()) && f.app.agents_catalog().iter().all(|e| e.spec.id != "other"), "only its manifest's");
}

#[test]
fn an_extension_asks_the_default_agent_and_gets_its_answer() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[permissions]\nagents = true\n"));
    f.app.agents_ask("test.hello", 7, r#"{"prompt": "hello"}"#, Some(f.root.clone()));
    assert!(f.pump(|app| !app.agents.replies.is_empty()), "never answered");
    let (ext, call, answer) = f.app.agents.replies.remove(0);
    assert_eq!((ext.as_str(), call), ("test.hello", 7));
    let answer: Value = serde_json::from_str(&answer.unwrap()).unwrap();
    assert_eq!((answer["stopReason"].as_str(), answer["agent"].as_str()), (Some("end_turn"), Some("mock")));
    assert!(!answer["text"].as_str().unwrap().is_empty(), "{answer}");
    let thread = ThreadId(answer["thread"].as_str().unwrap().to_string());
    let view = f.app.agents.hub.view(&thread).unwrap();
    assert!(matches!(&view.thread.entries[0], Entry::User { blocks, .. } if blocks[0].as_text() == "hello" && blocks[1].as_text().contains("Hello extension")));
    assert_eq!(view.link, &Link::Detached, "its session let go");
    assert!(f.app.agents.asks.is_empty());
}

#[test]
fn an_ask_is_refused_without_the_permission_or_a_prompt_and_its_edits_are_refused() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension(""));
    f.app.agents_ask("test.hello", 1, r#"{"prompt": "hi"}"#, None);
    assert!(f.app.agents.replies[0].2.as_ref().unwrap_err().contains("agents = true"));
    f.app.wasm.installed[0] = extension("[permissions]\nagents = true\n");
    f.app.agents_ask("test.hello", 2, "{}", None);
    assert!(f.app.agents.replies[1].2.as_ref().unwrap_err().contains("prompt"));
    f.app.agents_ask("test.hello", 3, r#"{"prompt": "hi", "agent": "nobody"}"#, None);
    assert!(f.app.agents.replies[2].2.as_ref().unwrap_err().contains("no agent nobody"));
    // An edit asks permission: nobody's there to give it.
    std::fs::write(f.root.join("a.txt"), "one\n").unwrap();
    f.app.agents_ask("test.hello", 4, r#"{"prompt": "edit a.txt one=>two"}"#, Some(f.root.clone()));
    assert!(f.pump(|app| app.agents.replies.len() == 4), "never answered");
    assert!(f.app.agents.replies[3].2.is_ok(), "{:?}", f.app.agents.replies[3]);
    assert_eq!(std::fs::read_to_string(f.root.join("a.txt")).unwrap(), "one\n", "not changed");
}

#[test]
fn an_api_key_is_kept_in_the_keychain_and_given_to_every_agent() {
    let mut f = fixture("");
    let keychain = Arc::new(fenix_config::keychain::Keychain::with(fenix_config::keychain::Memory::default()));
    f.app.wasm.keychain = Some(keychain.clone());
    f.app.agents_picked(Pick::Secret("ANTHROPIC_API_KEY".into()));
    for c in "sk-ant-1".chars() {
        assert!(f.app.code_prompt_key(KeyPress::char(c)));
    }
    assert!(f.app.code_prompt_text().is_some_and(|t| t.contains("••••") && !t.contains("sk-ant")), "masked");
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    assert_eq!(keychain.get("agents.ANTHROPIC_API_KEY").as_deref(), Some("sk-ant-1"));
    assert_eq!(f.app.config.list_value("agents.secrets"), ["ANTHROPIC_API_KEY"]);
    let mock = f.app.agents_catalog().into_iter().find(|e| e.spec.id == "mock").unwrap();
    assert!(mock.spec.env.contains(&("ANTHROPIC_API_KEY".to_string(), "sk-ant-1".to_string())));
    // An empty one takes it away.
    f.app.agents_picked(Pick::Secret("ANTHROPIC_API_KEY".into()));
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    assert!(keychain.get("agents.ANTHROPIC_API_KEY").is_none());
    assert!(f.app.config.list_value("agents.secrets").is_empty());
}
