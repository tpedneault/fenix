//! The host half of the tutorial (`tutorial`): its page (`SPC h T`, and
//! `t` on the welcome), the practice buffer each lesson opens in a window
//! beside it, and the look at the editor after every key -- and every so
//! often, for a step's pause before its hint -- that moves a lesson on.

use super::pages::PageModel;
use super::*;
use crate::tutorial::{self, Action as TutorialAction, Run, Seen, Start, Term, TutorialPage};

/// How long a step waits before it shows its hint.
const HINT_AFTER: Duration = Duration::from_secs(20);

impl App {
    pub(super) fn tutorial_page_id(&self) -> Option<BufferId> {
        self.find_page(|m| matches!(m, PageModel::Tutorial(_)))
    }

    fn tutorial_page(&mut self, id: BufferId) -> Option<&mut TutorialPage> {
        match self.pages.get_mut(&id).map(|s| {
            s.stale = true;
            &mut s.model
        }) {
            Some(PageModel::Tutorial(p)) => Some(p),
            _ => None,
        }
    }

    /// `SPC h T`: the tutorial's page -- the one open, or a new one with
    /// the lessons read afresh (an extension's too).
    pub(crate) fn open_tutorial(&mut self) {
        if let Some(id) = self.tutorial_page_id() {
            // Where it is already -- not a second copy in the window you're
            // in, which the next lesson's practice would then split again.
            match self.windows().windows().into_iter().find(|&p| self.windows().content(p) == Some(&id)) {
                Some(pane) => {
                    self.windows_mut().focus(pane);
                    self.wake_caret();
                }
                None => self.show_page(id),
            }
            return;
        }
        let (lessons, problems) = tutorial::load(&crate::installed::data_dirs("tutorial"));
        let done = tutorial::load_done(&self.tutorial_progress);
        self.open_page(PageModel::Tutorial(Box::new(TutorialPage::new(lessons, problems, done))));
    }

    pub(super) fn tutorial_action(&mut self, id: BufferId, action: TutorialAction) {
        match action {
            TutorialAction::None => {}
            TutorialAction::Close => {
                if let Some(practice) = self.tutorial_page(id).and_then(|p| p.run.take()).map(|r| r.practice) {
                    self.tutorial_close_practice(practice);
                }
                self.close_page(id);
            }
            TutorialAction::Start(lesson) => self.tutorial_start(id, lesson),
            TutorialAction::Practice => {
                if let Some(practice) = self.tutorial_page(id).and_then(|p| p.run.as_ref()).map(|r| r.practice) {
                    self.tutorial_show_practice(practice);
                }
            }
            TutorialAction::Skip => {
                let start = self.tutorial_start_now();
                let Some(page) = self.tutorial_page(id) else { return };
                let edits = page.run.as_ref().map_or(0, |r| r.edits);
                let mut finished = page.advance(start, edits, true);
                // Skipping what opens a page skips closing it too: there's
                // nothing to close.
                let practice = page.run.as_ref().map(|r| r.practice);
                if let (false, Some(step), Some(practice)) = (finished, page.step().cloned(), practice) {
                    if tutorial::going_there(&step, &self.tutorial_seen(practice)) == Some(false) {
                        let Some(page) = self.tutorial_page(id) else { return };
                        finished = page.advance(start, edits, true);
                    }
                }
                self.tutorial_said_next(id, finished);
            }
            TutorialAction::Keybindings => {
                let source = self.buffers.open_text_view_like(tutorial::KEYBINDINGS, Path::new("KEYBINDINGS.md"));
                let mut page = crate::reading_page::ReadingPage::new(source, None, true);
                page.back = Some(id);
                self.open_page(PageModel::Reading(Box::new(page)));
            }
        }
    }

    /// Lesson `lesson`, from its first step: a fresh practice buffer with
    /// its text, in a window beside the page -- for one about Git, the
    /// file of a practice repository made fresh for it (`[repo]`).
    fn tutorial_start(&mut self, id: BufferId, lesson: usize) {
        let Some(page) = self.tutorial_page(id) else { return };
        let Some((text, repo)) = page.lessons.get(lesson).map(|l| (l.text.clone(), l.repo.clone())) else { return };
        let practice = match repo {
            None => self.buffers.open_text_view(&text),
            Some(repo) => {
                let dir = self.tutorial_progress.with_file_name("tutorial-repo");
                match fenix_git::practice_repository(&dir, &repo.file, &repo.committed, &text) {
                    Ok(file) => {
                        // The last lesson's file, if this one's: read again.
                        if let Some(old) = self.buffers.id_for_path(&file) {
                            self.buffers.close(old);
                        }
                        self.buffers.open_path(&file)
                    }
                    Err(why) => return self.set_error(format!("couldn't make the practice repository: {why}")),
                }
            }
        };
        let Some(page) = self.tutorial_page(id) else { return };
        let before = page.run.take().map(|r| r.practice);
        if let Some(before) = before.filter(|b| self.buffers.get(*b).is_some()) {
            self.buffers.close(before);
            self.repoint_panes_showing(before, practice);
        }
        self.tutorial_show_practice(practice);
        {
            let (_, cursor) = self.focused_buffer_and_cursor_mut();
            *cursor = Cursor::at_start();
        }
        let start = self.tutorial_start_now();
        let Some(page) = self.tutorial_page(id) else { return };
        page.focus = lesson;
        page.run = Some(Run { lesson, step: 0, practice, start, since: Instant::now(), hint: false, edits: 0, skipped: 0, there: false, finished: false });
        let first = page.step().map(|s| s.say.clone()).unwrap_or_default();
        self.set_message(first);
    }

    /// Focuses the window showing `practice`; else shows it in a window
    /// that's a second copy of the tutorial's page; else splits one off the
    /// one focused. A lesson started over and over keeps to two windows.
    fn tutorial_show_practice(&mut self, practice: BufferId) {
        let windows = self.windows().windows();
        let shown = windows.iter().copied().find(|&p| self.windows().content(p) == Some(&practice));
        let page = self.tutorial_page_id();
        let copies: Vec<_> = windows.iter().copied().filter(|&p| page.is_some() && self.windows().content(p) == page.as_ref()).collect();
        match (shown, copies.as_slice()) {
            (Some(pane), _) => {
                self.windows_mut().focus(pane);
                self.wake_caret();
            }
            // The page twice: the first copy keeps it, the second practises.
            (None, [_, second, ..]) => {
                let pane = *second;
                self.windows_mut().focus(pane);
                self.set_pane_content(pane, practice);
            }
            (None, _) => {
                self.split_vertical();
                let pane = self.focused_pane_id();
                self.set_pane_content(pane, practice);
            }
        }
    }

    fn tutorial_close_practice(&mut self, practice: BufferId) {
        if self.buffers.get(practice).is_none() {
            return;
        }
        self.buffers.close(practice);
        let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
        self.repoint_panes_showing(practice, fallback);
    }

    /// What a practice buffer is called.
    pub(super) fn tutorial_buffer_name(&self, buffer: BufferId) -> Option<String> {
        let id = self.tutorial_page_id()?;
        let Some(PageModel::Tutorial(page)) = self.pages.get(&id).map(|s| &s.model) else { return None };
        let run = page.run.as_ref().filter(|r| r.practice == buffer)?;
        page.lessons.get(run.lesson).map(|l| format!("*practice: {}*", l.title))
    }

    fn tutorial_start_now(&self) -> Start {
        Start { windows: self.windows().window_count(), pane: self.focused_pane_id() }
    }

    /// What kind of page `id` is, and for some where on it you are -- as
    /// a step's `page` names it.
    fn tutorial_page_kind(&self, id: BufferId) -> Option<(String, Option<String>)> {
        let Some(state) = self.pages.get(&id) else {
            return (self.buffers.get(id)?.kind == BufferKind::Dashboard).then(|| ("home".to_string(), None));
        };
        let kind = match &state.model {
            PageModel::Code(p) => format!("{}/{}", p.ext, p.kind),
            PageModel::Wizard(_) => "new-project".into(),
            PageModel::Hub(_) => "projects".into(),
            PageModel::Doctor(_) => "doctor".into(),
            PageModel::Git(_) => "git".into(),
            PageModel::Log(_) => "git-log".into(),
            PageModel::Compare(_) => "git-compare".into(),
            PageModel::Rebase(_) => "git-rebase".into(),
            PageModel::UserSettings(_) => "settings".into(),
            PageModel::Snippets(_) => "snippets".into(),
            PageModel::Reading(_) => "reading".into(),
            PageModel::Extensions(e) if e.welcome => "welcome".into(),
            PageModel::Extensions(e) => return Some(("extensions".into(), Some(format!("extensions/{}", e.tab.label().to_lowercase())))),
            PageModel::ExtensionOutput(_) => "extensions-output".into(),
            PageModel::Tutorial(_) => "tutorial".into(),
            PageModel::Explorer(..) => "explorer".into(),
            PageModel::Preview(_) => "explorer-preview".into(),
            PageModel::Search(_) => "search".into(),
            PageModel::Agent(_) => "agent".into(),
            PageModel::AgentReview(_) => "agent-changes".into(),
        };
        Some((kind, None))
    }

    /// What's true of page `id`, for a step's `page`: the Git page's
    /// `git/staged`, `git/clean`, its menu.
    fn tutorial_page_facts(&self, id: BufferId) -> Vec<String> {
        match self.pages.get(&id).map(|s| &s.model) {
            Some(PageModel::Git(g)) => g.tutorial_facts(),
            _ => Vec::new(),
        }
    }

    /// The picker open, as a step's `picker` names it.
    fn tutorial_picker(&self) -> Option<&'static str> {
        Some(match self.active_picker.as_ref()? {
            ActivePicker::FindFile(_) => "files",
            ActivePicker::Commands(_) => "commands",
            ActivePicker::Bindings(_) => "bindings",
            ActivePicker::BufferSearch(_) => "lines",
            ActivePicker::SwitchBuffer(_) => "buffers",
            ActivePicker::Task(_) => "tasks",
            _ => "other",
        })
    }

    fn tutorial_seen(&self, practice: BufferId) -> Seen {
        let focused = self.focused_buffer_id();
        let practice = (focused == practice && !self.terminal_focused).then(|| self.buffers.get(practice)).flatten().map(|ob| {
            let (line, col) = ob.buffer.line_col(&self.cursor());
            (ob.buffer.text(), (line + 1, col + 1))
        });
        let mode = match self.vim.mode() {
            Mode::Normal => "normal",
            Mode::Insert => "insert",
            Mode::Visual => "visual",
            Mode::Replace => "replace",
            Mode::Command => "command",
            Mode::Search => "search",
        };
        let pane = self.focused_pane_id();
        let term = Term {
            panel: self.terminal_open,
            pane: self.buffers.get(focused).is_some_and(|ob| ob.kind == BufferKind::Terminal),
            // A shell still starting takes the keyboard once it's up: not
            // yet given back with `Ctrl-\`.
            keyboard: self.terminal_focused
                || (self.terminal_open && self.terminal_spawning)
                || self.terminal_buffers_spawning.contains(&focused)
                || self.terminal_buffer_focused.is_some_and(|t| self.windows().content(pane) == Some(&t)),
        };
        Seen {
            practice,
            mode,
            idle: self.vim.mode() == Mode::Normal && self.vim.is_idle(),
            windows: self.windows().window_count(),
            pane,
            page: self.tutorial_page_kind(focused),
            pages: self.pages.keys().filter_map(|id| self.tutorial_page_kind(*id)).map(|(kind, _)| kind).collect(),
            picker: self.tutorial_picker(),
            leader: self.leader_matcher.is_pending() || self.local_matcher.is_some(),
            term,
            facts: self.tutorial_page_facts(focused),
        }
    }

    /// After a key: whether the step under way passed, or was a try
    /// that didn't (which shows its hint).
    pub(super) fn tutorial_check(&mut self) {
        let Some(id) = self.tutorial_page_id() else { return };
        let Some(PageModel::Tutorial(page)) = self.pages.get(&id).map(|s| &s.model) else { return };
        let (Some(run), Some(step)) = (page.run.as_ref(), page.step().cloned()) else { return };
        let text_before = page.lessons.get(run.lesson).map(|l| tutorial::text_before(l, run.step).to_string()).unwrap_or_default();
        let (practice, start, before, was_there) = (run.practice, run.start, run.edits, run.there);
        let edits = self.buffers.get(practice).map_or(0, |ob| ob.buffer.edit_count());
        // Practice, not work: nothing to ask about saving -- unless it's
        // a practice repository's file, whose changes Git is to see.
        if let Some(ob) = self.buffers.get_mut(practice).filter(|ob| ob.buffer.is_dirty() && ob.buffer.path().is_none()) {
            ob.buffer.mark_saved();
        }
        let seen = self.tutorial_seen(practice);
        let going = tutorial::going_there(&step, &seen);
        let there = going == Some(true) || was_there;
        if going.is_some() {
            if let Some(run) = self.tutorial_page(id).and_then(|p| p.run.as_mut()) {
                run.there = there;
            }
        }
        if tutorial::passes(&step, &text_before, &seen, start) && (going.is_none() || there) {
            let start = Start { windows: seen.windows, pane: seen.pane };
            let Some(page) = self.tutorial_page(id) else { return };
            let finished = page.advance(start, edits, false);
            // What the next step closes may have opened with this key.
            if let Some(next) = page.step().cloned() {
                let there = tutorial::going_there(&next, &seen) == Some(true);
                if let Some(run) = page.run.as_mut() {
                    run.there = there;
                }
            }
            self.tutorial_said_next(id, finished);
        } else if edits != before && seen.idle && seen.practice.is_some() {
            let Some(run) = self.tutorial_page(id).and_then(|p| p.run.as_mut()) else { return };
            run.edits = edits;
            if !run.hint && !step.hint.is_empty() {
                run.hint = true;
                self.set_message(format!("hint: {}", step.hint));
            }
        }
    }

    /// Says what's next after a step: the next one, or that the lesson's
    /// done -- kept, so it stays done.
    fn tutorial_said_next(&mut self, id: BufferId, finished: bool) {
        let Some(PageModel::Tutorial(page)) = self.pages.get(&id).map(|s| &s.model) else { return };
        if !finished {
            if let Some(next) = page.step() {
                let said = next.say.clone();
                self.set_message(said);
            }
            return;
        }
        let title = page.run.as_ref().and_then(|r| page.lessons.get(r.lesson)).map(|l| l.title.clone()).unwrap_or_default();
        let all = page.all_done();
        let done = page.done.clone();
        if let Err(e) = tutorial::save_done(&self.tutorial_progress, &done) {
            self.set_error(format!("couldn't keep the tutorial's progress in {}: {e}", self.tutorial_progress.display()));
            return;
        }
        self.set_message(if all {
            "Every lesson done -- d on the tutorial page (SPC h T) opens the keybindings doc".to_string()
        } else {
            format!("{title}: done -- the tutorial page (SPC h T) has the next")
        });
    }

    /// Every so often: a step that's waited long enough shows its hint,
    /// and anything that changed without a key is looked at. Whether to
    /// draw again.
    pub(super) fn tutorial_tick(&mut self, now: Instant) -> bool {
        let Some(id) = self.tutorial_page_id() else { return false };
        let Some(PageModel::Tutorial(page)) = self.pages.get(&id).map(|s| &s.model) else { return false };
        let Some(run) = page.run.as_ref().filter(|r| !r.finished) else { return false };
        let (step, since) = (run.step, run.since);
        self.tutorial_check();
        let Some(page) = self.tutorial_page(id) else { return false };
        let hint = page.step().map(|s| s.hint.clone()).unwrap_or_default();
        let Some(run) = page.run.as_mut().filter(|r| !r.finished) else { return true };
        if run.step != step {
            return true;
        }
        if run.hint || hint.is_empty() || now.duration_since(since) < HINT_AFTER {
            return false;
        }
        run.hint = true;
        self.set_message(format!("hint: {hint}"));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `seq` as keys: characters as they are, `<Esc>`, `<CR>`, `<Tab>`,
    /// `<Space>` and `<C-x>` as Vim writes them.
    fn parse_keys(seq: &str) -> Vec<KeyPress> {
        let mut keys = Vec::new();
        let mut rest = seq;
        while let Some(c) = rest.chars().next() {
            if c == '<' {
                if let Some(end) = rest.find('>') {
                    let token = &rest[1..end];
                    let key = match token {
                        "Esc" => Some(KeyPress::named(FenixNamedKey::Escape)),
                        "CR" => Some(KeyPress::named(FenixNamedKey::Enter)),
                        "Tab" => Some(KeyPress::named(FenixNamedKey::Tab)),
                        "Space" => Some(KeyPress::char(' ')),
                        t if t.starts_with("C-") && t.chars().count() == 3 => t.chars().nth(2).map(|c| KeyPress::char(c).with_ctrl()),
                        _ => None,
                    };
                    if let Some(key) = key {
                        keys.push(key);
                        rest = &rest[end + 1..];
                        continue;
                    }
                }
            }
            keys.push(KeyPress::char(c));
            rest = &rest[c.len_utf8()..];
        }
        keys
    }

    /// A leader command, as `run_command` would run it -- those that need
    /// an event loop by what they call.
    fn run_test_command(app: &mut App, id: &str) {
        match id {
            "window.split_vertical" => app.split_vertical(),
            "window.split_horizontal" => app.split_horizontal(),
            "window.navigate_left" => app.navigate_window(fenix_window::NavDirection::Left),
            "window.navigate_right" => app.navigate_window(fenix_window::NavDirection::Right),
            "window.close" => app.close_window(),
            "window.only" => app.only_window(),
            "commands.palette" => app.open_command_palette(),
            "keys.bindings" => app.open_bindings_picker(),
            "search.buffer" => app.picker_search_buffer(),
            "git.open" => app.open_git(),
            "git.log" => app.open_git_log(crate::git_log::Scope::Branch),
            "git.switch" => app.start_switch_picker(),
            _ => {
                let commands = Rc::clone(&app.commands);
                commands.run_without_event_loop(app, id).unwrap_or_else(|e| panic!("{id}: {e:?}"));
            }
        }
    }

    /// One key the way `route_keypress` takes it, for what the lessons
    /// use: a terminal's, a picker's, a page's, the
    /// leader's, else Vim's.
    fn route(app: &mut App, key: KeyPress) {
        if app.terminal_focused {
            if key == KeyPress::char('\\').with_ctrl() {
                app.terminal_focused = false;
            }
        } else if app.active_terminal_buffer().is_some() {
            if key == KeyPress::char('\\').with_ctrl() {
                app.unfocus_terminal_buffer();
            }
        } else if app.active_picker.is_some() {
            app.picker_key(key);
        } else if app.compose.as_ref().is_some_and(|c| c.pane == app.focused_pane_id()) && app.vim.mode() == Mode::Normal && (key == KeyPress::named(FenixNamedKey::Enter) || key == KeyPress::char('q')) {
            // The compose pane's own two keys, as `App` takes them.
            if key == KeyPress::char('q') {
                app.compose_cancel();
            } else {
                app.compose_submit();
            }
        } else if app.page_key(key) {
            // The page's.
        } else if app.leader_matcher.is_pending() || (matches!(app.vim.mode(), Mode::Normal | Mode::Visual) && key == KeyPress::char(' ')) {
            if let fenix_keymap::Step::Matched(&id) = app.leader_matcher.feed(key) {
                run_test_command(app, id);
            }
        } else {
            app.test_dispatch_key(key);
        }
        app.tutorial_check();
    }

    fn page(app: &App) -> &TutorialPage {
        let id = app.tutorial_page_id().expect("the tutorial page");
        match &app.pages[&id].model {
            PageModel::Tutorial(p) => p,
            _ => unreachable!(),
        }
    }

    /// A way through each step of each built-in lesson, from where the
    /// step before left off.
    const SOLUTIONS: &[(&str, &[&str])] = &[
        ("editing.modes", &["jjjjjjllllllll", "i", "<Esc>jj$hhhii<Esc>", "j$hhhx", "oI can do this.<Esc>", "gg"]),
        ("editing.words", &["j$b", "/is is<CR>dw", "j/Remove<CR>D", "jdd", "J", "j3dd"]),
        ("editing.objects", &["/hello<CR>ci\"goodbye<Esc>", "j/first<CR>di(", "/banana<CR>ciwcherry<Esc>", "/paragraph<CR>dap", "Gyyp"]),
        ("editing.visual", &["/make this<CR>VU", "jVjd", "0<C-v>jjI- <Esc>", "j/two<CR>vecdeux<Esc>"]),
        ("editing.search", &["/needle<CR>", "j:s/cat/bird/g<CR>", "0/dog<CR>n", ":%s/color/colour/g<CR>"]),
        ("editing.macros", &["/Copy me<CR>\"ayyj\"ap", "/- item<CR>qaA done<Esc>jq3@a", "/TODO<CR>cwDONE<Esc>w.w."]),
        ("fenix.leader", &["<Space>", "<Esc>", "<Space>:", "<Esc>", "<Space>hb", "<Esc>"]),
        ("fenix.find", &["<Space><Space>", "<Esc>", "<Space>slneedle<CR>", "<Space>ps", "<Esc>q", "<Space>bb"]),
        ("fenix.windows", &["<Space>wv", "<Space>wh", "<Space>ws", "<Space>wq", "<Space>wo", "<Space>hT"]),
        ("fenix.pages", &["<Space>pp", "q", "<Space>gg", "q", "<Space>pt", "<Esc>"]),
        ("fenix.terminals", &["<Space>ot", "<C-\\>", "<Space>ot", "<Space>oT", "<C-\\>"]),
        ("fenix.settings", &["<Space>,", "q", "<Space>hx", "<Tab><Tab>", "q"]),
        ("git.basics", &["<Space>gg", "s", "c", "ciFirst commit<Esc><CR>", "<Space>gl", "q", "<Space>ggb", "<Esc>q"]),
    ];

    #[test]
    fn every_built_in_step_can_be_passed_with_fenixs_keys() {
        let mut app = App::with_file(None);
        // A workspace on a project with a repository and tasks (Cargo's),
        // for the pages lesson's Git and tasks.
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        app.workspaces.active_workspace_mut().project = Some(root.clone());
        app.project_root = Some(root);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        let lessons: Vec<(String, usize)> = page(&app).lessons.iter().map(|l| (l.id.clone(), l.steps.len())).collect();
        assert_eq!(lessons.len(), SOLUTIONS.len());
        for (i, ((lesson, steps), (solved, keys))) in lessons.iter().zip(SOLUTIONS).enumerate() {
            assert_eq!(lesson, solved);
            assert_eq!(*steps, keys.len(), "{lesson}: a way through each step");
            // From the page, as you'd start one.
            let shown = app.windows().windows().into_iter().find(|&p| app.windows().content(p) == Some(&id));
            match shown {
                Some(pane) => {
                    app.windows_mut().focus(pane);
                }
                None => app.open_tutorial(),
            }
            app.tutorial_action(id, TutorialAction::Start(i));
            assert!(app.tutorial_buffer_name(app.focused_buffer_id()).is_some_and(|n| n.starts_with("*practice: ")), "{lesson}: in the practice buffer");
            for (n, seq) in keys.iter().enumerate() {
                assert_eq!(page(&app).run.as_ref().map(|r| r.step), Some(n), "{lesson}: step {}", n + 1);
                for key in parse_keys(seq) {
                    route(&mut app, key);
                }
                let run = page(&app).run.clone().unwrap();
                let text = app.buffers.get(run.practice).map(|ob| ob.buffer.text()).unwrap_or_default();
                assert!(run.step > n || run.finished, "{lesson}: step {} ({seq}) didn't pass -- the practice buffer:\n{text}\nseen: {:?}", n + 1, app.tutorial_seen(run.practice));
            }
            assert!(page(&app).run.as_ref().unwrap().finished, "{lesson}");
            assert!(page(&app).done.contains(lesson.as_str()));
            // Whatever a lesson left open, closed, as you'd go on.
            if app.active_picker.is_some() {
                route(&mut app, KeyPress::named(FenixNamedKey::Escape));
            }
            if app.terminal_open {
                app.toggle_terminal();
            }
        }
        assert!(page(&app).all_done());
        // Kept between runs.
        assert_eq!(tutorial::load_done(&app.tutorial_progress).len(), 13);
    }

    #[test]
    fn a_failed_try_shows_the_hint_and_a_pause_does_too() {
        let mut app = App::with_file(None);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        app.tutorial_action(id, TutorialAction::Start(0));
        assert_eq!(app.windows().window_count(), 2, "beside the page");
        // A try at step 1 (the cursor) that isn't one: nothing's edited.
        for key in parse_keys("jj") {
            route(&mut app, key);
        }
        assert!(!page(&app).run.as_ref().unwrap().hint);
        let later = Instant::now() + HINT_AFTER + Duration::from_secs(1);
        assert!(app.tutorial_tick(later));
        assert!(page(&app).run.as_ref().unwrap().hint, "after a pause");
        assert!(app.status_message.as_ref().is_some_and(|m| m.text.starts_with("hint: ")));
        // Step 3 is an edit: a wrong one shows its hint at once.
        for key in parse_keys("jjjjllllllll") {
            route(&mut app, key);
        }
        for key in parse_keys("i<Esc>") {
            route(&mut app, key);
        }
        assert_eq!(page(&app).run.as_ref().unwrap().step, 2);
        assert!(!page(&app).run.as_ref().unwrap().hint);
        for key in parse_keys("x") {
            route(&mut app, key);
        }
        assert!(page(&app).run.as_ref().unwrap().hint, "a try that didn't do it");
        let practice = page(&app).run.as_ref().unwrap().practice;
        assert!(!app.buffers.get(practice).unwrap().buffer.is_dirty(), "practice isn't work to save");
        // s on the page skips it; q closes the page and the practice.
        app.tutorial_action(id, TutorialAction::Skip);
        assert_eq!(page(&app).run.as_ref().unwrap().step, 3);
        app.tutorial_action(id, TutorialAction::Close);
        assert!(app.tutorial_page_id().is_none() && app.buffers.get(practice).is_none());
    }

    /// A lesson done, `SPC h T` from the practice as the message says, the
    /// next lesson: still the page and its practice, side by side. It was
    /// the page twice, then three windows, then four.
    #[test]
    fn going_back_to_the_page_and_on_to_the_next_lesson_keeps_two_windows() {
        let mut app = App::with_file(None);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        app.tutorial_action(id, TutorialAction::Start(0));
        assert_eq!(app.windows().window_count(), 2);
        let practice = app.focused_pane_id();
        app.open_tutorial();
        assert_ne!(app.focused_pane_id(), practice, "the page where it is");
        assert_eq!(app.windows().windows().into_iter().filter(|&p| app.windows().content(p) == Some(&id)).count(), 1, "not a second copy");
        for lesson in 1..4 {
            app.tutorial_action(id, TutorialAction::Start(lesson));
            assert_eq!(app.windows().window_count(), 2, "lesson {lesson}");
            app.open_tutorial();
        }
        // Were it shown twice anyway, the next practice takes the copy.
        let page_pane = app.focused_pane_id();
        let other = app.windows().windows().into_iter().find(|&p| p != page_pane).unwrap();
        app.windows_mut().focus(other);
        app.set_pane_content(other, id);
        app.tutorial_action(id, TutorialAction::Start(4));
        assert_eq!(app.windows().window_count(), 2);
    }

    /// A page opened over the tutorial (`SPC p s` in the find lesson) and
    /// closed with `q` gives the pane back to the tutorial, not to the
    /// practice beside it that was used last.
    #[test]
    fn a_page_closed_over_the_tutorial_brings_the_tutorial_back() {
        let mut app = App::with_file(None);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        app.tutorial_action(id, TutorialAction::Start(0));
        let practice = app.focused_buffer_id();
        app.open_tutorial();
        let page_pane = app.focused_pane_id();
        app.tutorial_action(id, TutorialAction::Keybindings);
        let doc = app.focused_buffer_id();
        assert_ne!(doc, id);
        assert_eq!(app.focused_pane_id(), page_pane);
        app.buffers.touch(practice);
        app.close_page(doc);
        assert_eq!(app.windows().content(page_pane), Some(&id));
    }

    /// Skipping the step that opens a page skips the one that closes it;
    /// a close step doesn't pass on a page that was never open.
    #[test]
    fn a_page_never_opened_isnt_closed() {
        let mut app = App::with_file(None);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        let pages = page(&app).lessons.iter().position(|l| l.id == "fenix.pages").unwrap();
        app.tutorial_action(id, TutorialAction::Start(pages));
        app.tutorial_check();
        assert_eq!(page(&app).run.as_ref().unwrap().step, 0);
        app.tutorial_action(id, TutorialAction::Skip);
        assert_eq!(page(&app).run.as_ref().unwrap().step, 2, "SPC p p skipped, q too");
        app.tutorial_check();
        assert_eq!(page(&app).run.as_ref().unwrap().step, 2);
        app.tutorial_action(id, TutorialAction::Skip);
        // The tasks picker never opened: Esc isn't done for it.
        app.tutorial_action(id, TutorialAction::Skip);
        let run = page(&app).run.as_ref().unwrap();
        assert!(run.finished && run.skipped == 6, "{run:?}");
    }

    /// The panel's shell starts on a thread and takes the keyboard when
    /// it's up: until then `Ctrl-\` hasn't given it back.
    #[test]
    fn a_shell_still_starting_has_the_keyboard() {
        let mut app = App::with_file(None);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        app.tutorial_action(id, TutorialAction::Start(0));
        let practice = app.focused_buffer_id();
        app.terminal_open = true;
        app.terminal_spawning = true;
        assert!(app.tutorial_seen(practice).term.keyboard);
        app.terminal_spawning = false;
        assert!(!app.tutorial_seen(practice).term.keyboard);
    }

    #[test]
    fn the_keybindings_doc_opens_from_the_page_and_spc_h_t_is_bound() {
        let mut app = App::with_file(None);
        app.open_tutorial();
        let id = app.tutorial_page_id().unwrap();
        app.tutorial_action(id, TutorialAction::Keybindings);
        let focused = app.focused_buffer_id();
        assert!(matches!(app.pages.get(&focused).map(|s| &s.model), Some(PageModel::Reading(p)) if p.back == Some(id)));
        let bound = keymap::leader_trie().bindings().into_iter().find(|(_, c)| **c == "tutorial.open").map(|(seq, _)| seq.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" "));
        assert_eq!(bound.as_deref(), Some("SPC h T"));
    }
}
