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
    assert!(f.app.modeline_pieces().1.starts_with("Agent prompt"), "its plain name on the modeline: {:?}", f.app.modeline_pieces().1);
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

/// How many workspaces have a pane showing `buffer`.
fn workspaces_showing(app: &App, buffer: BufferId) -> usize {
    app.workspaces.workspaces.iter().filter(|ws| ws.windows.windows().iter().any(|p| ws.windows.content(*p) == Some(&buffer))).count()
}

#[test]
fn the_dock_comes_with_you_to_another_workspace_and_leaves_the_last_as_it_was() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let (page, composer) = (f.app.agents.page.unwrap(), f.app.agents.composer.unwrap());
    f.app.agents_focus_code();
    let code = f.app.focused_pane_id();
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.agents_dock_sync();
    assert_eq!((workspaces_showing(&f.app, page), workspaces_showing(&f.app, composer)), (1, 1), "one dock");
    let (Some(here), Some(prompt)) = f.app.agents_dock_panes() else { panic!("the dock, here") };
    assert!(f.app.pane_titles.get(&here).is_some_and(|t| t.starts_with("Agent")), "titles, not tabs");
    assert!(f.app.pane_titles.contains_key(&prompt));
    assert!(!f.app.agents_is_dock_pane(f.app.focused_pane_id()), "your focus stays in your code");
    // Back: the same, and the other workspace is one pane again.
    f.app.prev_workspace();
    f.app.agents_dock_sync();
    assert!(matches!(f.app.agents_dock_panes(), (Some(_), Some(_))));
    assert!(f.app.windows().windows().contains(&code));
    assert_eq!(f.app.workspaces.workspaces[1].windows.window_count(), 1);
    assert_eq!(*f.app.workspaces.workspaces[1].windows.focused_content(), other);
}

#[test]
fn each_workspace_shows_its_own_thread_and_the_others_go_on() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let first = f.thread();
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.agents_dock_sync();
    assert_eq!(f.thread(), first, "a workspace with no thread of its own shows the last one");
    let second = f.app.agents.hub.new_thread("mock", &f.root);
    f.app.agents_show(second.clone());
    assert_eq!(f.app.workspaces.active_workspace().agent_thread.as_ref(), Some(&second));
    // Back to the first workspace: its own thread, the second's kept.
    f.app.prev_workspace();
    f.app.agents_dock_sync();
    assert_eq!(f.thread(), first);
    f.app.next_workspace();
    f.app.agents_dock_sync();
    assert_eq!(f.thread(), second);
    assert!(f.app.agents.hub.view(&first).is_some(), "the other thread is still there");
    // SPC a T: the thread in the other workspace takes you there.
    f.app.agents_go_to_running(first.clone());
    assert_eq!((f.app.workspaces.active_index(), f.thread()), (0, first));
}

#[test]
fn the_focus_in_the_dock_comes_with_it() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let composer = f.app.agents.composer.unwrap();
    f.app.new_workspace();
    f.app.agents_dock_sync();
    assert_eq!(f.app.focused_buffer_id(), composer, "still in the prompt");
    assert_eq!(f.app.windows().window_count(), 3, "with Home beside the dock, not a thread with no prompt");
    assert!(f.app.windows().windows().iter().any(|p| f.app.windows().content(*p).and_then(|c| f.app.buffers.get(*c)).is_some_and(|ob| ob.kind == BufferKind::Dashboard)));
}

#[test]
fn the_dock_keeps_its_width_from_workspace_to_workspace() {
    let mut f = fixture("");
    f.app.config.set_value("agents.panel_width", Some(fenix_config::Value::Int(30)));
    f.app.agents_open_panel();
    f.app.agents_focus_code();
    f.app.windows_mut().resize_focused(0.05);
    let width = |app: &App| {
        let page = app.agents_dock_panes().0.unwrap();
        rects(app).into_iter().find(|(id, _)| *id == page).unwrap().1.w
    };
    let before = width(&f.app);
    assert!((before - 300.0).abs() > 10.0, "it was resized: {before}");
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.agents_dock_sync();
    assert!((width(&f.app) - before).abs() < 2.0, "{} then {before}", width(&f.app));
}

#[test]
fn putting_the_dock_away_leaves_nothing_of_it_in_any_workspace() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let composer = f.app.agents.composer.unwrap();
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.agents_dock_sync();
    f.app.agents_close_panel();
    f.app.agents_dock_sync();
    assert_eq!(workspaces_showing(&f.app, composer), 0);
    assert_eq!(f.app.windows().window_count(), 1);
    f.app.prev_workspace();
    f.app.agents_dock_sync();
    assert_eq!(f.app.agents_dock_panes(), (None, None), "and it stays away");
}

#[test]
fn a_session_with_a_dock_in_two_workspaces_comes_back_with_one() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let (page, composer) = (f.app.agents.page.unwrap(), f.app.agents.composer.unwrap());
    // As 1.0's sessions left it: the thread and the prompt elsewhere too,
    // the prompt on its own pane.
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.windows_mut().split(SplitKind::Vertical, page);
    f.app.windows_mut().split(SplitKind::Horizontal, composer);
    f.app.agents_dock_sync();
    assert_eq!((workspaces_showing(&f.app, page), workspaces_showing(&f.app, composer)), (1, 1));
    assert_eq!(f.app.workspaces.workspaces[0].windows.window_count(), 1, "the code's alone again");
}

#[test]
fn a_workspace_that_was_only_the_thread_keeps_a_pane() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    let page = f.app.agents.page.unwrap();
    f.app.workspaces.new_workspace(page, Cursor::at_start());
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.agents_dock_sync();
    let alone = &f.app.workspaces.workspaces[1].windows;
    assert_eq!(alone.window_count(), 1);
    assert_ne!(*alone.focused_content(), page);
}

#[test]
fn removing_the_workspace_with_the_dock_keeps_the_dock() {
    let mut f = fixture("");
    let other = f.app.buffers.open_text_view("other\n");
    f.app.workspaces.new_workspace(other, Cursor::at_start());
    f.app.agents_open_panel();
    let thread = f.app.agents.shown(&f.app.pages);
    f.app.remove_workspace();
    assert_eq!(f.app.workspaces.len(), 1);
    assert!(matches!(f.app.agents_dock_panes(), (Some(_), Some(_))));
    assert_eq!(f.app.agents.shown(&f.app.pages), thread);
}

#[test]
fn a_file_shown_with_the_focus_in_the_dock_opens_beside_it() {
    let mut f = fixture("");
    let file = f.root.join("a.txt");
    std::fs::write(&file, "a\n").unwrap();
    f.app.agents_open_panel();
    let (page, composer) = (f.app.agents.page.unwrap(), f.app.agents.composer.unwrap());
    let params = fenix_control::protocol::OpenParams { paths: vec![fenix_control::protocol::OpenPath { path: file.to_string_lossy().into_owned(), line: None, column: None }], new_window: false, focus: false };
    f.app.apply_control(fenix_control::Call::Open(params)).unwrap();
    assert_eq!(f.app.focused_buffer_id(), composer, "the focus wasn't asked for");
    assert_eq!(f.app.agents_panes_showing(page).len(), 1, "the thread's still up");
    assert!(f.app.windows().windows().iter().any(|p| f.app.windows().content(*p).and_then(|c| f.app.buffers.get(*c)).is_some_and(|ob| ob.buffer.path() == Some(file.as_path()))));
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

/// Following, the pane beside the dock goes to the file the agent edits
/// -- though another was open there -- and, once it writes, to the line
/// it changed, which its edit doesn't name.
#[test]
fn following_goes_to_the_file_and_the_line_the_agent_changed() {
    let mut f = fixture("--fs");
    let other = f.root.join("other.txt");
    std::fs::write(&other, "something else
").unwrap();
    let file = f.root.join("long.txt");
    std::fs::write(&file, (1..=30).map(|n| format!("e{n}
")).collect::<String>()).unwrap();
    f.app.test_open_path(&other);
    let code = f.app.focused_pane_id();
    f.app.agents_open_panel();
    f.app.config.set_value("agents.save_before_prompt", Some(fenix_config::Value::Text("never".into())));
    f.app.agents_toggle_follow();
    f.type_prompt("edit long.txt e20=>changed");
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.pending_permissions() > 0)));
    let shown = |app: &App| app.windows().content(code).and_then(|id| app.buffers.get(*id)).and_then(|ob| ob.buffer.path().map(Path::to_path_buf));
    assert_eq!(shown(&f.app).as_deref(), Some(file.as_path()), "the file it's editing, in place of the other");
    assert!(f.app.agents_is_dock_pane(f.app.focused_pane_id()), "the keyboard left in the dock");
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    let id = *f.app.windows().content(code).unwrap();
    let cursor = f.app.pane_state(code).cursor;
    let line = f.app.buffers.get(id).unwrap().buffer.line_col(&cursor).0;
    assert_eq!(line + 1, 20, "on the line it changed");
    // Another update to the same edit doesn't pull it back.
    let thread = f.thread();
    f.app.agents_follow(&thread);
    let cursor = f.app.pane_state(code).cursor;
    assert_eq!(f.app.buffers.get(id).unwrap().buffer.line_col(&cursor).0 + 1, 20);
}

/// The same, for an agent that writes to disk itself (Claude Code's
/// adapter): the buffer follows the disk, and the pane its first changed
/// line.
#[test]
fn following_an_agent_that_writes_to_disk_goes_to_the_line_it_changed() {
    let mut f = fixture("");
    let other = f.root.join("other.txt");
    std::fs::write(&other, "something else
").unwrap();
    let file = f.root.join("long.txt");
    std::fs::write(&file, (1..=30).map(|n| format!("e{n}
")).collect::<String>()).unwrap();
    f.app.test_open_path(&other);
    let code = f.app.focused_pane_id();
    f.app.agents_open_panel();
    f.app.agents_toggle_follow();
    f.type_prompt("edit long.txt e20=>changed");
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.pending_permissions() > 0)));
    f.key(Key::Char('y'));
    let id = f.app.buffers.id_for_path(&file).expect("opened by following");
    assert!(f.pump(|app| app.buffers.get(id).is_some_and(|ob| ob.buffer.text().contains("changed"))), "the buffer follows the disk");
    assert_eq!(f.app.windows().content(code), Some(&id));
    let cursor = f.app.pane_state(code).cursor;
    assert_eq!(f.app.buffers.get(id).unwrap().buffer.line_col(&cursor).0 + 1, 20, "on the line it changed");
    // Without the focus the pane wouldn't scroll to it by itself: it's
    // scrolled, its line in view, on the next frame.
    assert!(f.app.reveal_panes.contains(&code));
    f.app.reveal_cursor(code, id, 8);
    let top = f.app.pane_state(code).scroll_line;
    assert!((top..top + 8).contains(&19), "line 20 in view from {top}");
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
    let root = f.root.clone();
    let compose = open_to_normal(&mut f, ComposePurpose::GitCommit { repo_root: root, compose: crate::git_status::Compose::New(Default::default()) });
    assert!(f.app.pane_titles.values().any(|t| t.contains("A while it's untouched has the agent write it")));
    press_big_a(&mut f);
    assert_eq!(f.app.vim.mode(), Mode::Normal, "A isn't Vim's here");
    assert!(f.pump(|app| app.agents.hub.status() == Default::default() && app.agents.commit_message.is_none()), "the answer came");
    let blocks = first_prompt(&f);
    assert!(matches!(&blocks[1], ContentBlock::Resource { text: Some(t), .. } if t.contains("+one")), "{blocks:?}");
    assert!(!f.app.buffers.get(compose).unwrap().buffer.text().trim().is_empty(), "it's in the box");
    assert_eq!(f.app.focused_pane_id(), f.app.compose.as_ref().unwrap().pane, "you're still in the box");
}

/// `A`, as `route_keypress` takes it in a compose pane.
fn press_big_a(f: &mut Fixture) {
    if f.app.compose_agent_offered() {
        f.app.compose_agent_write();
    } else {
        f.app.test_dispatch_key(KeyPress::char('A'));
    }
}

/// A compose pane open, in Normal mode: where you are after writing, or
/// after Esc in a blank one. Its buffer.
fn open_to_normal(f: &mut Fixture, purpose: ComposePurpose) -> BufferId {
    f.app.open_compose(purpose);
    f.app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    assert_eq!(f.app.vim.mode(), Mode::Normal);
    f.app.compose.as_ref().unwrap().buffer
}

#[test]
fn a_is_vims_once_the_commit_box_has_text_and_in_a_box_the_agent_doesnt_write() {
    let mut f = fixture("");
    let root = f.root.clone();
    let compose = open_to_normal(&mut f, ComposePurpose::CommitMessage { repo_root: root.clone() });
    f.app.replace_buffer_from_disk(compose, "mine");
    press_big_a(&mut f);
    assert_eq!(f.app.vim.mode(), Mode::Insert, "append at the line's end");
    assert!(f.app.agents.commit_message.is_none(), "nothing asked");
    assert_eq!(f.app.buffers.get(compose).unwrap().buffer.text(), "mine");
    f.app.close_compose();
    // Half of a command: `dA` isn't asking anybody.
    open_to_normal(&mut f, ComposePurpose::CommitMessage { repo_root: root.clone() });
    f.app.test_dispatch_key(KeyPress::char('d'));
    assert!(!f.app.compose_agent_offered());
    f.app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    assert!(f.app.compose_agent_offered());
    f.app.close_compose();
    let amend = ComposePurpose::GitCommit { repo_root: root, compose: crate::git_status::Compose::Amend(Default::default()) };
    assert!(!amend.title().contains("agent"), "{}", amend.title());
    open_to_normal(&mut f, amend);
    press_big_a(&mut f);
    assert_eq!(f.app.vim.mode(), Mode::Insert, "an amend starts from its message");
    f.app.close_compose();
    f.app.open_code_compose("test.hello", 3, "Reply".into(), "send", false, "", None, false);
    f.app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    press_big_a(&mut f);
    assert_eq!(f.app.vim.mode(), Mode::Insert, "no ask, no agent");
    assert!(f.app.agents.replies.is_empty());
}

#[test]
fn a_in_an_extensions_box_has_the_agent_write_its_ask_into_the_box() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[permissions]\nagents = true\n"));
    f.app.open_code_compose("test.hello", 3, "Review summary".into(), "keep", true, "", Some(r#"{"prompt": "hello", "stream": true}"#.into()), false);
    assert!(f.app.pane_titles.values().any(|t| t.starts_with("Review summary") && t.contains("the agent")));
    f.app.test_dispatch_key(KeyPress::named(FenixNamedKey::Escape));
    press_big_a(&mut f);
    let compose = f.app.compose.as_ref().unwrap().buffer;
    assert!(f.pump(|app| app.agents.compose_ask.is_none()), "never answered");
    let (ext, _, answer) = f.app.agents.replies.remove(0);
    assert_eq!(ext, "test.hello");
    let answer: Value = serde_json::from_str(&answer.unwrap()).unwrap();
    let text = f.app.buffers.get(compose).unwrap().buffer.text();
    assert!(!text.trim().is_empty(), "{answer} / {:?}", f.app.status_message.as_ref().map(|m| m.text.clone()));
    assert_eq!(text.trim(), answer["text"].as_str().unwrap().trim(), "what it said, in the box");
    assert!(f.app.compose.is_some(), "yours to edit and send");
    assert!(f.app.agents.asks.is_empty());
}

#[test]
fn a_writes_over_a_template_until_its_touched_and_never_over_a_kept_draft() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[permissions]\nagents = true\n"));
    let ask = || Some(r#"{"prompt": "hello", "stream": true}"#.to_string());
    let open = |f: &mut Fixture, over| {
        f.app.open_code_compose("test.hello", 3, "Description of x".into(), "keep", true, "## Summary", ask(), over);
        f.app.compose.as_ref().unwrap().buffer
    };
    // It's a template, and untouched.
    let compose = open(&mut f, true);
    assert!(f.app.compose_agent_offered());
    press_big_a(&mut f);
    assert!(f.pump(|app| app.agents.compose_ask.is_none()), "never answered");
    let text = f.app.buffers.get(compose).unwrap().buffer.text();
    assert!(!text.contains("## Summary") && !text.trim().is_empty(), "{text}");
    f.app.close_compose();
    // Touched, it's yours.
    let compose = open(&mut f, true);
    f.app.replace_buffer_from_disk(compose, "## Summary\nmine");
    assert!(!f.app.compose_agent_offered());
    f.app.close_compose();
    // Not a template: what's in it is yours from the start, and clearing it
    // is how to ask.
    let compose = open(&mut f, false);
    assert!(!f.app.compose_agent_offered());
    f.app.replace_buffer_from_disk(compose, "");
    assert!(f.app.compose_agent_offered());
    f.app.close_compose();
    // Text you wrote and didn't send, come back, isn't a template.
    let file = f.app.compose_drafts_file.clone();
    write_compose_drafts(&file, &[(compose_draft_key("test.hello", "Description of x"), "half written".to_string())].into_iter().collect());
    open(&mut f, true);
    assert!(!f.app.compose_agent_offered());
    f.app.close_compose();
}

#[test]
fn closing_the_box_stops_the_agent_writing_it_and_a_refused_ask_leaves_it_empty() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[permissions]\nagents = true\n"));
    f.app.open_code_compose("test.hello", 3, "Review summary".into(), "keep", true, "", Some(r#"{"prompt": "write the review"}"#.into()), false);
    f.app.compose_agent_write();
    assert!(f.app.agents.compose_ask.is_some());
    f.app.close_compose();
    assert!(f.app.agents.compose_ask.is_none() && f.app.agents.asks.is_empty() && f.app.agents.consenting.is_empty(), "given up on");

    f.app.wasm.installed[0] = extension("");
    f.app.open_code_compose("test.hello", 4, "Review summary".into(), "keep", true, "", Some(r#"{"prompt": "write the review"}"#.into()), false);
    let compose = f.app.compose.as_ref().unwrap().buffer;
    f.app.compose_agent_write();
    assert!(f.app.agents.compose_ask.is_none(), "answered at once");
    assert!(f.app.buffers.get(compose).unwrap().buffer.text().trim().is_empty());
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

#[test]
fn an_mcp_server_is_a_command_or_an_address() {
    assert!(matches!(mcp_server("a", "my-server --flag"), Some(McpServer::Stdio { command, args, .. }) if command == "my-server" && args == ["--flag"]));
    assert!(matches!(mcp_server("b", "https://mcp.example.com/x"), Some(McpServer::Http { url, headers, .. }) if url == "https://mcp.example.com/x" && headers.is_empty()));
    assert!(matches!(mcp_server("c", "sse+https://mcp.example.com Authorization=Bearer"), Some(McpServer::Sse { url, headers, .. }) if url == "https://mcp.example.com" && headers == [("Authorization".to_string(), "Bearer".to_string())]));
    assert!(mcp_server("d", "").is_none());
}

/// 1.2's ask: what it attaches read by Fenix and sent as resources, a
/// structured answer found and held to its schema, the agent's most
/// restrictive mode, and -- since it sends a file -- your say first,
/// kept for the next one.
#[test]
fn an_ask_with_options_attaches_answers_as_json_and_asks_before_it_sends_code() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[permissions]\nagents = true\n"));
    std::fs::write(f.root.join("notes.md"), "on disk\n").unwrap();
    let payload = json!({
        "prompt": "{\"title\": \"Fix it\", \"body\": \"Because.\"}",
        "attach": [{"kind": "file", "path": "notes.md"}, {"kind": "text", "name": "ticket", "text": "FEN-1"}],
        "answerAs": {"type": "object", "required": ["title", "body"], "properties": {"title": {"type": "string"}, "body": {"type": "string"}}},
        "show": "hidden",
    })
    .to_string();
    f.app.agents_ask("test.hello", 7, &payload, Some(f.root.clone()));
    assert!(f.app.agents.replies.is_empty() && f.app.agents.asks.is_empty(), "waits for your say");
    assert!(matches!(f.app.active_picker, Some(ActivePicker::Agents(_))), "asked");
    f.app.agents_picked(Pick::AskConsent { ext: "test.hello".into(), call: 7, allow: true });
    assert_eq!(f.app.agents_ask_consents()["test.hello"], ["mock"]);
    let thread = f.app.agents.asks.keys().next().cloned().expect("asked");
    assert!(f.pump(|app| !app.agents.replies.is_empty()), "never answered");
    let (_, call, answer) = f.app.agents.replies.remove(0);
    assert_eq!(call, 7);
    let answer: Value = serde_json::from_str(&answer.unwrap()).unwrap();
    assert_eq!(answer["json"], json!({"title": "Fix it", "body": "Because."}), "{answer}");
    assert_eq!(answer["reverted"], json!([]));
    assert!(f.app.agents.hub.view(&thread).is_none(), "hidden: deleted once answered");

    // The next one from it doesn't ask; one the schema doesn't fit is
    // asked again, then fails.
    let payload = json!({"prompt": "{\"a\": 1}", "attach": [{"kind": "file", "path": "notes.md"}], "answerAs": {"type": "object", "required": ["title"]}}).to_string();
    f.app.agents_ask("test.hello", 8, &payload, Some(f.root.clone()));
    let thread = f.app.agents.asks.keys().next().cloned().expect("asked at once");
    assert!(f.pump(|app| !app.agents.replies.is_empty()), "never answered");
    let err = f.app.agents.replies.remove(0).2.unwrap_err();
    assert!(err.contains("doesn't match the schema") && err.contains("$.title is missing"), "{err}");
    let view = f.app.agents.hub.view(&thread).unwrap();
    assert_eq!(view.thread.modes.as_ref().map(|m| m.current.as_str()), Some("plan"), "the most restrictive mode");
    let prompts: Vec<&Vec<ContentBlock>> = view.thread.entries.iter().filter_map(|e| if let Entry::User { blocks, .. } = e { Some(blocks) } else { None }).collect();
    assert_eq!(prompts.len(), 2, "asked again once");
    assert!(prompts[0].iter().any(|b| matches!(b, ContentBlock::Resource { text: Some(t), .. } if t == "on disk\n")), "the file, as a resource: {:?}", prompts[0]);
    assert!(prompts[1][0].as_text().contains("doesn't match the schema"));
}

/// Said no to, an ask that sends code is refused; one that's cancelled
/// ends; a file its turn changed is put back.
#[test]
fn an_ask_refused_cancelled_or_that_changed_a_file_is_answered_so() {
    let mut f = fixture("");
    f.app.wasm.installed.push(extension("[permissions]\nagents = true\n"));
    f.app.agents_ask("test.hello", 1, &json!({"prompt": "hi", "attach": [{"kind": "diff", "of": "unstaged"}]}).to_string(), Some(f.root.clone()));
    let refused = f.app.agents.replies.first().map(|r| r.2.clone());
    if refused.is_none() {
        // Not a Git repository here: the diff can't be read, which is an
        // answer too. When it is, you're asked.
        f.app.agents_picked(Pick::AskConsent { ext: "test.hello".into(), call: 1, allow: false });
    }
    let err = f.app.agents.replies.remove(0).2.unwrap_err();
    assert!(err.contains("didn't let it send code") || err.contains("the diff"), "{err}");
    assert!(f.app.agents_ask_consents().is_empty());

    f.app.agents_ask("test.hello", 2, &json!({"prompt": "slow"}).to_string(), Some(f.root.clone()));
    assert!(f.pump(|app| app.agents.hub.status().working > 0), "it's working");
    assert!(f.app.agents_ask_cancel("test.hello", 2));
    assert_eq!(f.app.agents.replies.remove(0).2.unwrap_err(), "cancelled");
    assert!(!f.app.agents_ask_cancel("test.hello", 2), "gone");

    std::fs::write(f.root.join("a.txt"), "changed\n").unwrap();
    f.app.agents_ask("test.hello", 3, &json!({"prompt": "hello"}).to_string(), Some(f.root.clone()));
    let thread = f.app.agents.asks.keys().next().cloned().unwrap();
    f.app.agents.baselines.entry(thread).or_default().insert(f.root.join("a.txt"), Some("one\n".into()));
    assert!(f.pump(|app| !app.agents.replies.is_empty()), "never answered");
    let answer: Value = serde_json::from_str(&f.app.agents.replies.remove(0).2.unwrap()).unwrap();
    assert_eq!(answer["reverted"], json!(["a.txt"]));
    assert_eq!(std::fs::read_to_string(f.root.join("a.txt")).unwrap(), "one\n", "put back");
}

#[test]
fn an_asks_options_are_read_and_checked() {
    let o = extensions::AskOptions::parse(r#"{"prompt": " hi ", "stream": true, "show": "hidden", "timeoutMs": 5000, "attach": [{"kind": "selection"}]}"#).unwrap();
    assert_eq!((o.prompt.as_str(), o.stream, o.show.as_deref(), o.timeout), ("hi", true, Some("hidden"), Some(Duration::from_secs(5))));
    assert!(o.carries_code());
    assert!(!extensions::AskOptions::parse(r#"{"prompt": "x", "attach": [{"kind": "commits", "range": "a..b"}]}"#).unwrap().carries_code());
    assert!(extensions::AskOptions::parse(r#"{"prompt": "x", "show": "loud"}"#).unwrap_err().contains("show"));
    assert!(extensions::AskOptions::parse(r#"{"prompt": "x", "answerAs": 3}"#).unwrap_err().contains("answerAs"));
    assert!(extensions::AskOptions::parse("{}").unwrap_err().contains("prompt"));
    let modes = fenix_acp::protocol::Modes {
        current: "default".into(),
        available: ["default", "acceptEdits", "plan"].iter().map(|id| fenix_acp::protocol::Mode { id: id.to_string(), name: id.to_string(), description: None }).collect(),
    };
    assert_eq!(extensions::restrictive_mode(&modes).as_deref(), Some("plan"));
}

/// A command that fails sends its output back to the agent's step, and
/// its rounds running out stops the run.
#[test]
fn a_pipelines_command_sends_a_failure_back_to_the_agent() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    let spec = "[[steps]]\nname = \"build\"\nprompt = \"{input} {previous}\"\n[[steps]]\nname = \"test\"\nrun = \"git no-such-command\"\non_failure = \"build\"\nmax_rounds = 1\n";
    f.app.pipelines.armed = Some(fenix_acp::pipeline::PipelineSpec::parse("p", spec).unwrap());
    f.type_prompt("first");
    assert!(f.pump(|app| !app.pipelines.runs.is_empty() && app.pipelines.runs.values().all(|r| r.threads.len() == 2)), "back to build");
    let second = f.app.pipelines.runs.values().next().unwrap().threads[1].clone();
    let told = |app: &App| app.agents.hub.view(&second).is_some_and(|v| v.thread.transcript().contains("`git no-such-command` failed (exit 1)"));
    assert!(f.pump(told), "{:?}", f.app.agents.hub.view(&second).unwrap().thread.transcript());
    assert!(f.pump(|app| app.pipelines.runs.is_empty()), "out of rounds");
    assert!(f.app.status_message.as_ref().is_some_and(|m| m.text.contains("still fails")));
}

/// A command that passes hands on what came before it, what it printed
/// there for a later step to name.
#[test]
fn a_pipelines_command_that_passes_goes_on() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    let spec = "[[steps]]\nname = \"build\"\nprompt = \"{input}\"\n[[steps]]\nname = \"test\"\nrun = \"git --version\"\n[[steps]]\nname = \"review\"\nprompt = \"saw {test}\"\n";
    f.app.pipelines.armed = Some(fenix_acp::pipeline::PipelineSpec::parse("p", spec).unwrap());
    f.type_prompt("first");
    assert!(f.pump(|app| app.pipelines.runs.values().any(|r| r.threads.len() == 2)), "on to review");
    let (build, review) = {
        let threads = &f.app.pipelines.runs.values().next().unwrap().threads;
        (threads[0].clone(), threads[1].clone())
    };
    assert!(f.pump(|app| app.agents.hub.view(&review).is_some_and(|v| v.thread.transcript().contains("saw git version"))));
    assert!(format!("{:?}", f.app.agents.hub.view(&build).unwrap().thread.entries).contains("`git --version` passed"));
}

/// A step at its gate is still open: writing to it takes the question back
/// for another round, whose end asks again, and only `y` moves on.
#[test]
fn a_pipelines_gate_takes_another_round() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    let spec = "[[steps]]\nname = \"build\"\nprompt = \"{input}\"\ngate = true\n[[steps]]\nname = \"review\"\nprompt = \"Look: {previous}\"\n";
    f.app.pipelines.armed = Some(fenix_acp::pipeline::PipelineSpec::parse("p", spec).unwrap());
    f.type_prompt("first");
    let at_gate = |app: &App| app.agents.shown(&app.pages).is_some_and(|t| app.pipeline_at_gate(&t));
    assert!(f.pump(at_gate), "the gate is up");
    let build = f.thread();
    // Typing in the prompt isn't answering: y starts a sentence.
    f.app.agents_focus_composer();
    assert_eq!(f.app.vim.mode(), Mode::Insert);
    assert!(!f.app.agents_composer_key(KeyPress::char('y')));
    assert!(at_gate(&f.app));
    assert!(f.app.pane_titles.values().any(|t| t.contains("Ctrl-Enter sends another round")));
    // Another round: the question goes, and comes back at its end.
    f.type_prompt("second");
    assert!(!at_gate(&f.app) || f.view().is_working(), "taken back as it goes");
    assert!(f.pump(|app| !app.agents.hub.view(&build).unwrap().is_working() && app.pipeline_at_gate(&build)));
    assert_eq!(f.thread(), build, "still the same step");
    let answers = f.view().thread.entries.iter().filter(|e| matches!(e, Entry::User { .. })).count();
    assert_eq!(answers, 2, "both rounds went to the one thread");
    let cards: Vec<_> = f.view().thread.entries.iter().filter_map(|e| if let Entry::Permission(c) = e { Some(c.state.clone()) } else { None }).collect();
    assert_eq!(cards, vec![PermissionState::Withdrawn, PermissionState::Pending], "one question open");
    // y goes on, to the next step.
    f.key(Key::Char('y'));
    assert!(f.pump(|app| app.agents.shown(&app.pages).is_some_and(|t| t != build)), "on to review");
}

/// The agent asks for a form (`elicitation/create`): a card saying what
/// it's asking, whole; `y` asks it a field at a time, and picking or
/// typing is the answer.
#[test]
fn an_agents_form_is_answered_a_field_at_a_time() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    f.type_prompt("elicit");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1), "the card is up");
    let text = f.page_text();
    assert!(text.contains("The agent asks: How should I approach this?") && text.contains("Files to touch *") && text.contains("· Quick") && text.contains("y answer"), "{text}");
    assert!(!text.contains("the defaults"), "they don't make an answer: {text}");
    f.key(Key::Char('y'));
    // Strategy: starts on the default; move to Quick.
    let Some(ActivePicker::Agents(state)) = &mut f.app.active_picker else { panic!("the choices") };
    assert_eq!(state.selected().unwrap().label, "Strategy: Careful");
    state.move_selection(1);
    f.app.picker_confirm();
    // Files to touch: typed, and checked.
    for c in "99".chars() {
        assert!(f.app.code_prompt_key(KeyPress::char(c)));
    }
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    assert!(f.app.code_prompt_text().is_some_and(|t| t.contains("99")), "out of range: asked again");
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Backspace)));
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    // Run the tests: yes or no.
    let Some(ActivePicker::Agents(state)) = &f.app.active_picker else { panic!("yes or no") };
    assert_eq!(state.selected().unwrap().label, "Run the tests: yes");
    f.app.picker_confirm();
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    let said = f.view().thread.last_answer();
    assert!(said.contains("\"strategy\":\"quick\"") && said.contains("\"files\":9") && said.contains("\"tests\":true"), "{said}");
    assert!(f.page_text().contains("✓ How should I approach this? — accept: "));
}

/// What isn't on the list can be typed, and Esc leaves the card waiting.
#[test]
fn an_answer_of_your_own_goes_with_the_form() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    f.type_prompt("elicit");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('y'));
    f.app.picker_cancel();
    assert_eq!(f.app.agents.hub.status().waiting, 1, "Esc: still waiting");
    f.key(Key::Char('y'));
    let Some(ActivePicker::Agents(state)) = &mut f.app.active_picker else { panic!("the choices") };
    for c in "else".chars() {
        state.push_char(c);
    }
    assert_eq!(state.selected().unwrap().label, "Strategy: something else…");
    f.app.picker_confirm();
    for c in "pair on it".chars() {
        assert!(f.app.code_prompt_key(KeyPress::char(c)));
    }
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    assert!(f.app.code_prompt_key(KeyPress::char('2')));
    assert!(f.app.code_prompt_key(KeyPress::named(FenixNamedKey::Enter)));
    f.app.picker_confirm();
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    let said = f.view().thread.last_answer();
    assert!(said.contains("\"strategy\":\"pair on it\"") && said.contains("\"files\":2"), "{said}");
}

/// `Y` takes the defaults, when they make a whole answer; `n` declines.
#[test]
fn defaults_that_dont_fit_are_refused_and_n_declines() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    f.type_prompt("elicit");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    f.key(Key::Char('Y'));
    assert_eq!(f.app.agents.hub.status().waiting, 1, "files has no default: still waiting");
    f.key(Key::Char('n'));
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert!(f.view().thread.last_answer().contains("\"action\":\"decline\""));
}

/// A link: `y` opens it, and the agent's `elicitation/complete` marks the
/// card done.
#[test]
fn a_link_is_opened_and_its_card_marked_done() {
    let mut f = fixture("");
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))));
    f.type_prompt("oauth");
    assert!(f.pump(|app| app.agents.hub.status().waiting == 1));
    assert!(f.page_text().contains("https://example.com/connect?elicitationId=mock-1"));
    f.key(Key::Char('y'));
    assert_eq!(f.app.wasm.opened, vec!["https://example.com/connect?elicitationId=mock-1".to_string()]);
    assert!(f.pump(|app| app.agents.hub.status() == Default::default()));
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| v.thread.entries.iter().any(|e| matches!(e, Entry::Elicitation(c) if c.state == fenix_acp::thread::ElicitationState::Completed)))));
    assert!(f.page_text().contains("Sign in to the example service — done"));
}

fn connected_with(mock_flags: &str) -> Fixture {
    let mut f = fixture(mock_flags);
    f.app.agents_open_panel();
    assert!(f.pump(|app| app.agents.shown(&app.pages).and_then(|t| app.agents.hub.view(&t)).is_some_and(|v| matches!(v.link, Link::Live { .. }))), "never connected");
    f
}

fn usage_page_text(f: &mut Fixture) -> String {
    let id = f.app.find_page(|m| matches!(m, PageModel::AgentUsage(_))).expect("the usage page");
    let Some(PageModel::AgentUsage(p)) = f.app.pages.get(&id).map(|s| &s.model) else { panic!() };
    crate::agent_usage_page::layout(p, 110).all_text()
}

/// The mock reports as Claude's adapter does: the turn's tokens with its
/// answer, the session's running cost in `usage_update`. Both are counted
/// for the agent and the thread, with the turn and its time, said on the
/// modeline as `agents.usage.modeline` says, kept for the day, and shown
/// on `SPC a U`.
#[test]
fn what_an_agent_used_is_on_the_modeline_and_the_usage_page() {
    let mut f = connected_with("");
    f.turn("hello");
    f.turn("what now");
    let mock = f.app.agents.usage.session.agents["mock"].clone();
    assert_eq!(mock.turns, 2);
    assert_eq!(mock.reports, 2);
    assert!(mock.tokens() > 2400 && mock.cache_read > 0, "{mock:?}");
    assert_eq!(mock.currency.as_deref(), Some("USD"));
    let session_cost = f.view().thread.usage.as_ref().and_then(|u| u.cost.clone()).unwrap().0;
    assert!((mock.cost - session_cost).abs() < 1e-9, "the running total, counted once: {} vs {session_cost}", mock.cost);
    let segment = f.app.agents_usage_segment();
    assert_eq!(segment, format!("   ◆ mock {} tok", fenix_acp::usage::tokens_text(mock.tokens())));
    f.app.config.set_value("agents.usage.modeline", Some(fenix_config::Value::Text("cost".into())));
    assert!(f.app.agents_usage_segment().starts_with("   ◆ mock $0.0"), "{}", f.app.agents_usage_segment());
    f.app.config.set_value("agents.usage.modeline", Some(fenix_config::Value::Text("all".into())));
    assert!(f.app.agents_usage_segment().contains(" tok · $0.0") && f.app.agents_usage_segment().contains(" · 2 turns · "));
    f.app.config.set_value("agents.usage.modeline", Some(fenix_config::Value::Text("off".into())));
    assert_eq!(f.app.agents_usage_segment(), "");
    assert_eq!(f.app.agents_status_segment(), "", "the agents' own segment is as it was");

    // Kept for the day, beside the threads.
    let kept = fenix_acp::usage::Ledger::load(f.app.agents.usage_file.as_ref().unwrap());
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    assert_eq!(kept.days[&today].agents["mock"], mock);
    assert_eq!(kept.days[&today].threads[&f.thread().0].title, "hello");

    let thread = f.thread();
    f.app.open_agents_usage_page();
    let text = usage_page_text(&mut f);
    assert!(text.contains("Agent usage") && text.contains("Threads · today") && text.contains("hello"), "{text}");
    assert!(text.lines().any(|l| l.contains("mock") && l.contains('$')), "{text}");
    let id = f.app.find_page(|m| matches!(m, PageModel::AgentUsage(_))).unwrap();
    // Another turn while it's open: the page follows.
    f.app.agents_show(thread.clone());
    f.turn("again");
    let Some(PageModel::AgentUsage(p)) = f.app.pages.get(&id).map(|s| &s.model) else { panic!() };
    assert_eq!(p.ledger.session.agents["mock"].turns, 3);
    f.app.open_agents_usage_page();
    assert_eq!(f.app.find_page(|m| matches!(m, PageModel::AgentUsage(_))), Some(id), "one page");
    let Some(PageModel::AgentUsage(p)) = f.app.pages.get_mut(&id).map(|s| &mut s.model) else { panic!() };
    let action = p.key(Key::Enter);
    assert_eq!(action, crate::agent_usage_page::Action::Open(thread.0.clone()));
    f.app.agents_usage_page_action(id, action);
    assert_eq!(f.app.agents.shown(&f.app.pages), Some(thread));
}

/// An agent that says its tokens but no cost (Codex's adapter), and one
/// that says nothing: tokens, then turns and time.
#[test]
fn without_cost_or_tokens_the_modeline_says_what_it_can() {
    let mut f = connected_with("--usage codex");
    f.turn("hello");
    let mock = &f.app.agents.usage.session.agents["mock"];
    assert!(mock.has_tokens() && !mock.has_cost());
    f.app.config.set_value("agents.usage.modeline", Some(fenix_config::Value::Text("cost".into())));
    assert!(f.app.agents_usage_segment().ends_with(" tok"), "{}", f.app.agents_usage_segment());

    let mut f = connected_with("--usage none");
    f.turn("hello");
    let segment = f.app.agents_usage_segment();
    assert!(segment.starts_with("   ◆ mock 1 turn · ") && segment.ends_with('s'), "{segment}");
}

/// `agents.usage.daily_budget`: 80% and 100% are each said once.
#[test]
fn a_daily_budget_is_warned_of_at_80_and_100_percent() {
    let mut f = connected_with("");
    f.app.config.set_value("agents.usage.daily_budget", Some(fenix_config::Value::Text("$1.80".into())));
    let said = |app: &App| app.notifications.history().iter().filter(|n| n.summary.contains("budget")).map(|n| n.summary.clone()).collect::<Vec<_>>();
    f.turn("hello");
    assert!(said(&f.app).is_empty());
    f.turn("spend");
    let warned = said(&f.app);
    assert!(matches!(&warned[..], [w] if w.starts_with("The agents used 80% of today's budget: $1.") && w.ends_with("of $1.80")), "{warned:?}");
    f.turn("spend");
    let warned = said(&f.app);
    assert!(warned.len() == 2 && warned[1].starts_with("The agents reached today's budget"), "{warned:?}");
    f.turn("spend");
    assert_eq!(said(&f.app).len(), 2, "once a day each");
    f.app.open_agents_usage_page();
    assert!(usage_page_text(&mut f).contains("today's budget $1.80: "), "the page says it too");
}
