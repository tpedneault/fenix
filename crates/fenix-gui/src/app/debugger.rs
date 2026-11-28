//! The debugger's session: starting, stepping, and what the adapter says.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// `SPC u u` -- starts a debug session for the focused buffer's
    /// language if none is running yet; sends `continue` if one is
    /// already stopped at a breakpoint/step; a no-op (with a message)
    /// if one is running but not currently stopped (nothing useful to
    /// do -- it's already going).
    pub(crate) fn debug_start_or_continue(&mut self) {
        // One that's over gives way to a new one, whichever project it
        // was in.
        self.debug_clear_ended();
        if !self.debug_scope_matches() { return; }
        if let Some(session) = &self.debug_session {
            let Some(thread_id) = session.stopped_thread else {
                // Already running -- nothing to continue, but its dock
                // shown if it was put away.
                self.debug_dock_open = true;
                self.set_error("debug session is already running".to_string());
                return;
            };
            // Stopped, and DEBUG left with Esc: back in DEBUG first.
            if self.debug_mode_left && self.debug_run_to.is_none() {
                self.debug_mode_left = false;
                return;
            }
            self.send_debug_step(fenix_dap::requests::Request::Continue(fenix_dap::requests::ContinueRequestArguments::builder().thread_id(thread_id).build()));
            return;
        }

        let Some(language) = self.focused_language() else {
            self.set_error("no recognized language for the focused buffer".to_string());
            return;
        };
        let root = self.integration_root();
        let tools = match self.project_tools(&root) {
            Ok(tools) => tools,
            Err(error) => { self.set_error(error); return; }
        };
        let Some(spec) = self.debug_adapter_command(&root, language, &tools) else { let why = no_debug_adapter(language); self.set_error(why); return; };
        // No launch configuration of its own: the extensions' (their
        // `launch/*.toml`, and those they give as they run), to pick from.
        if tools.launch.is_empty() {
            let name = crate::lsp::language_config_name(language);
            let (mut templates, problems) = crate::dap::templates_for(&crate::installed::data_dirs("launch"), &name);
            templates.extend(self.wasm.launches(&root).into_iter().filter(|t| t.languages.contains(&name)));
            // The one this project used last, again, unless `SPC u U` asks.
            let last = self.debug_last_launch.get(&root).and_then(|name| templates.iter().find(|t| &t.name == name)).cloned();
            if let Some(template) = last.filter(|_| !self.debug_pick_launch) {
                self.set_message(format!("debugging with {} -- SPC u U picks another", template.name));
                return self.debug_start_from(template);
            }
            if !templates.is_empty() {
                let candidates = templates.into_iter().map(|t| fenix_picker::Candidate::new(t.name.clone(), t)).collect();
                self.completion = None;
                self.enter_picker(ActivePicker::Launch(fenix_picker::PickerState::new(candidates)));
                return;
            }
            if let Some(problem) = problems.first() {
                self.set_error(format!("launch file {problem}"));
            }
        }
        // Its variables to fill in -- `${pickProcess}`, an extension's -- or
        // an attach and attributes of its own: as a launch file's.
        let name = crate::lsp::language_config_name(language);
        match crate::dap::Template::from_project(&tools.launch, &name) {
            Ok(template) if tools.launch.attach || !tools.launch.attributes.is_empty() || template.has_variables() => return self.debug_start_from(template),
            Ok(_) => {}
            Err(error) => { self.set_error(error); return; }
        }
        // `"program": false` sends none: the adapter's attributes,
        // or an attach, say what runs.
        let program = tools.launch.program.clone().map(|p| if p.is_absolute() { p } else { root.join(p) })
            .or_else(|| self.open().buffer.path().map(Path::to_path_buf));
        let (mut launch_attributes, what) = if tools.launch.no_program {
            let mut attributes = crate::dap::launch_arguments(language, Path::new(""));
            attributes.remove("program");
            (attributes, root.display().to_string())
        } else {
            let Some(program) = program else { self.set_error("no file to debug -- configure a launch program"); return; };
            (crate::dap::launch_arguments(language, &program), program.display().to_string())
        };
        launch_attributes.insert("args".into(), serde_json::json!(tools.launch.args.unwrap_or_default()));
        let cwd = tools.launch.cwd.map(|p| if p.is_absolute() { p } else { root.join(p) }).unwrap_or_else(|| root.clone());
        launch_attributes.insert("cwd".into(), serde_json::json!(cwd));
        launch_attributes.insert("env".into(), serde_json::json!(tools.launch.env));
        let launch = PendingLaunch { language, root, adapter: spec, attributes: launch_attributes, attach: false, what };
        self.debug_after(tools.launch.pre_launch_task, launch);
    }

    /// The debug adapter for `language` files in the project at `root`:
    /// the project's own (`tools.json`'s `dap`), else an extension's
    /// (`debug-adapter-set`), else Fenix's.
    pub(super) fn debug_adapter_command(&self, root: &Path, language: fenix_syntax::LanguageId, tools: &fenix_project::tools::ProjectTools) -> Option<crate::dap::Adapter> {
        tools.dap.get(&crate::lsp::language_config_name(language)).cloned().map(crate::dap::Adapter::stdio).or_else(|| self.wasm.debug_adapter(root, language)).or_else(|| {
            crate::dap::default_adapter_command(language).map(|(program, args)| crate::dap::Adapter::stdio(fenix_project::tools::CommandSpec::new(program, args)))
        })
    }

    /// `SPC u u` with a launch configuration an extension offers, picked:
    /// for the file you're in. Its variables are filled in first -- each
    /// with several choices picked, one picker at a time, `${pickProcess}`
    /// from this machine's processes -- then its `preLaunchTask`
    /// runs, and the session starts once that succeeds.
    pub(crate) fn debug_start_from(&mut self, mut template: crate::dap::Template) {
        // One that's over gives way, whichever project it was in; one
        // still running doesn't.
        if !self.debug_clear_ended() {
            self.debug_scope_matches();
            return;
        }
        let Some(language) = self.focused_language() else {
            self.set_error("no recognized language for the focused buffer".to_string());
            return;
        };
        let root = self.integration_root();
        let tools = match self.project_tools(&root) {
            Ok(tools) => tools,
            Err(error) => { self.set_error(error); return; }
        };
        // An adapter already listening (`debugServer`), or the language's.
        let spec = match template.debug_server() {
            Some(port) => crate::dap::Adapter { command: None, port: Some(port) },
            None => match self.debug_adapter_command(&root, language, &tools) {
                Some(spec) => spec,
                None => return self.set_error(no_debug_adapter(language)),
            },
        };
        for name in template.variables() {
            if template.filled.contains_key(&name) || crate::dap::OWN_VARIABLES.contains(&name.as_str()) {
                continue;
            }
            let choices: Vec<fenix_wasm::host::PickerItem> = if name == crate::dap::PICK_PROCESS {
                crate::processes::list().into_iter().map(|p| fenix_wasm::host::PickerItem { label: format!("{}  {}", p.pid, p.name), detail: p.command, value: p.pid.to_string() }).collect()
            } else {
                match self.wasm.launch_variable(&root, &name) {
                    Some(choices) => choices,
                    // No one fills it in: left as it is.
                    None => continue,
                }
            };
            match choices.as_slice() {
                [] => { self.set_error(format!("{}: nothing to fill in ${{{name}}} with", template.name)); return; }
                [one] => { template.filled.insert(name, one.value.clone()); }
                several => {
                    // Picked, then on to the next: each candidate is this
                    // launch with its value filled in.
                    let candidates = several.iter().map(|c| {
                        let mut picked = template.clone();
                        picked.filled.insert(name.clone(), c.value.clone());
                        let label = if c.detail.is_empty() { c.label.clone() } else { format!("{}  {}", c.label, c.detail) };
                        fenix_picker::Candidate::new(label, picked)
                    }).collect();
                    self.completion = None;
                    self.enter_picker(ActivePicker::Launch(fenix_picker::PickerState::new(candidates)));
                    return;
                }
            }
        }
        let file = self.open().buffer.path().map(Path::to_path_buf);
        // Remembered for this project: SPC u u starts it again. Not a
        // test's (`tests.debug_here`): SPC T d starts that again.
        if template.name != "tools.json" && !template.test {
            self.debug_last_launch.insert(root.clone(), template.name.clone());
        }
        let attributes = match template.arguments(language, file.as_deref(), &root) {
            Ok(attributes) => attributes,
            Err(error) => { self.set_error(error); return; }
        };
        let what = attributes.get("program").and_then(|p| p.as_str()).map(str::to_string).unwrap_or_else(|| template.name.clone());
        let launch = PendingLaunch { language, root, adapter: spec, attributes, attach: template.request == crate::dap::Request::Attach, what };
        self.debug_after(template.pre_launch_task.clone(), launch);
    }

    /// `launch` started now, or once `task` -- a task of its project's, by
    /// its name in the task picker -- has succeeded in the task pane.
    pub(super) fn debug_after(&mut self, task: Option<String>, launch: PendingLaunch) {
        let Some(name) = task.filter(|t| !t.trim().is_empty()) else {
            return self.start_debug_session(launch.language, launch.root, launch.adapter, launch.attributes, launch.attach, launch.what);
        };
        let task = match self.tasks_for(&launch.root) {
            Ok(tasks) => tasks.into_iter().find(|t| t.name == name),
            Err(error) => return self.set_error(error),
        };
        let Some(task) = task else { return self.set_error(format!("preLaunchTask {name}: no such task in this project")) };
        let root = launch.root.clone();
        self.debug_after_task = Some((name, launch));
        if !self.run_task_as(task, root, None) {
            self.debug_after_task = None;
        }
    }

    /// A task ended in the task pane, in `root`: the debug session waiting
    /// on it starts, or doesn't.
    pub(super) fn debug_task_ended(&mut self, root: &Path, success: bool) {
        let Some((name, launch)) = self.debug_after_task.take() else { return };
        if fenix_lsp::normalize(refactor::identity(&launch.root)) != fenix_lsp::normalize(refactor::identity(root)) {
            self.debug_after_task = Some((name, launch));
            return;
        }
        if success {
            self.start_debug_session(launch.language, launch.root, launch.adapter, launch.attributes, launch.attach, launch.what);
        } else {
            self.set_error(format!("preLaunchTask {name} failed: the debug session didn't start"));
        }
    }

    /// What brings connection `generation`'s events to the event loop;
    /// none without one (tests keep them to read by hand).
    pub(super) fn dap_reader(&mut self, rx: std::sync::mpsc::Receiver<fenix_dap::DapEvent>, generation: u64) -> Option<DapReader> {
        match self.event_proxy.clone() {
            Some(proxy) => Some(DapReader::spawn(rx, generation, move |event| proxy.send_event(event).is_ok())),
            None => {
                #[cfg(test)]
                self.dap_unread.push((generation, rx));
                #[cfg(not(test))]
                drop(rx);
                None
            }
        }
    }

    /// `SPC u n`/`SPC u i`/`SPC u o`/`SPC u p` -- step over/into/out, or
    /// pause. A no-op (with a message) unless actually stopped (step)
    /// or running (pause would be redundant while already stopped).
    pub(crate) fn debug_step_over(&mut self) {
        self.debug_step_request(|thread_id| fenix_dap::requests::Request::Next(fenix_dap::requests::NextRequestArguments::builder().thread_id(thread_id).build()));
    }
    pub(crate) fn debug_step_into(&mut self) {
        self.debug_step_request(|thread_id| fenix_dap::requests::Request::StepIn(fenix_dap::requests::StepInRequestArguments::builder().thread_id(thread_id).build()));
    }
    pub(crate) fn debug_step_out(&mut self) {
        self.debug_step_request(|thread_id| fenix_dap::requests::Request::StepOut(fenix_dap::requests::StepOutRequestArguments::builder().thread_id(thread_id).build()));
    }

    pub(super) fn debug_step_request(&mut self, build: impl FnOnce(i32) -> fenix_dap::requests::Request) {
        let Some(session) = &self.debug_session else {
            self.set_error("no debug session running".to_string());
            return;
        };
        let Some(thread_id) = session.stopped_thread else {
            self.set_error("debug session isn't stopped".to_string());
            return;
        };
        self.send_debug_step(build(thread_id));
    }

    /// `SPC u q` -- ends the debug session entirely: dropping
    /// `DebugSession` drops its `DapClient`, whose own `Drop` sends
    /// `disconnect` and tears down the adapter process (see `DapClient::
    /// drop`'s own doc comment) -- its child sessions' first. Doesn't close
    /// the panel's workspace -- same "leave what's already open, just stop
    /// the live session behind it" posture `finish_task_run`/
    /// `DockerHost::finish_log_stream` already have; the four panes just
    /// stop updating.
    pub(crate) fn debug_stop(&mut self) {
        if !self.debug_scope_matches() { return; }
        let Some(session) = self.debug_session.as_mut().filter(|s| !s.ended || !s.children.is_empty() || !s.client_closed) else {
            self.set_error("no debug session running".to_string());
            return;
        };
        // Its children's connections, then its own: the adapters go. The
        // session stays, ended, so its panel says so until the next one
        // takes its place.
        let children: Vec<u64> = session.children.iter().rev().map(|c| c.generation).collect();
        session.children.clear();
        session.client.close();
        session.client_closed = true;
        for id in children {
            self.tell_debug(fenix_wasm::host::DebugState::Ended(id));
        }
        self.debug_root_ended(DebugEnd::Stopped);
    }

    /// `SPC u b` -- toggles a breakpoint on the focused buffer's current
    /// line. Re-renders the Breakpoints pane immediately; if a session
    /// is running and already past its `initialized` event, also
    /// resends `setBreakpoints` for this file right away so a
    /// breakpoint set/cleared mid-session takes effect without needing
    /// a restart -- exactly what a real debugger's own "toggle
    /// breakpoint while running" already does.
    pub(crate) fn debug_toggle_breakpoint(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("buffer has no file to set a breakpoint in".to_string());
            return;
        };
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let line = line + 1; // DAP/this map's own 1-indexed convention
        // `fenix_lsp::normalize` strips Windows' `\\?\` verbatim prefix
        // (which `canonicalize` always adds there) -- confirmed live
        // that skipping this breaks DAP breakpoints exactly the way it
        // once broke LSP diagnostics: the adapter reports the
        // breakpoint as `verified` against the prefixed path, but never
        // actually matches it against the plain-form path a running
        // frame reports, so it's silently never hit. See `fenix_rpc::
        // normalize`'s own doc comment for both incidents.
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path));
        let lines = self.breakpoints.entry(canonical.clone()).or_default();
        if lines.remove(&line).is_none() {
            lines.insert(line, Default::default());
        }
        self.send_set_breakpoints_for(&canonical);
        self.refresh_debug_panes();
    }

    /// `SPC u w` -- adds the identifier immediately before the cursor
    /// as a watch (`completion::prefix_at_cursor`'s own "what would
    /// autocomplete right here" notion -- no separate prompt UI for
    /// v1: place the cursor right after the identifier to watch, the
    /// same "cursor just past what you mean" convention `ciw`/`*`
    /// already ask for elsewhere in this app). Re-evaluates immediately
    /// if currently stopped; otherwise the watch just shows `<not
    /// evaluated>` until the next stop.
    pub(crate) fn debug_add_watch(&mut self) {
        if !self.debug_scope_matches() { return; }
        let cursor = self.cursor();
        let ob = self.open();
        let Some((_, word)) = completion::prefix_at_cursor(&ob.buffer, &cursor).filter(|(_, w)| !w.is_empty()) else {
            self.set_error("no identifier before the cursor to watch".to_string());
            return;
        };
        self.debug_watch(word);
    }

    /// `FenixUserEvent::Dap` handling for the session's own connection --
    /// see `apply_dap_event_on`.
    #[cfg(test)]
    pub(super) fn apply_dap_event(&mut self, event: fenix_dap::DapEvent) {
        let Some(on) = self.debug_session.as_ref().map(|s| s.generation) else { return };
        self.apply_dap_event_on(on, event);
    }

    /// `FenixUserEvent::Dap` handling -- the single dispatch point every
    /// decoded adapter event passes through, same role `apply_lsp_
    /// event` plays for language servers: for connection `on`, the
    /// session's or one of its children's.
    pub(super) fn apply_dap_event_on(&mut self, on: u64, event: fenix_dap::DapEvent) {
        match event {
            fenix_dap::DapEvent::Response { request_seq, result } => {
                let Some(session) = self.debug_session.as_mut() else { return };
                let Some(pending) = session.pending_of(on).and_then(|p| p.remove(&request_seq)) else { return };
                self.apply_dap_response(on, pending, result);
            }
            fenix_dap::DapEvent::Event(event) => self.apply_dap_notification(on, event),
            // No reverse request this app actually needs to answer yet
            // (e.g. `runInTerminal`, which neither `debugpy` nor a
            // future native adapter needs for the `internalConsole`/
            // piped-output launch shape this app always requests) --
            // still answered, per spec, rather than left hanging.
            fenix_dap::DapEvent::ReverseRequest { seq, .. } => {
                if let Some(client) = self.debug_session.as_ref().and_then(|s| s.client_of(on)) {
                    let err = fenix_dap::responses::ErrorResponse::builder()
                        .command("unknown".to_string())
                        .message("not implemented".to_string())
                        .body(fenix_dap::responses::ErrorResponseBody::new(None))
                        .build();
                    let _ = client.respond(seq, Err(err));
                }
            }
            // A child session: answered at once, then opened.
            fenix_dap::DapEvent::RawRequest { seq, command, arguments } => {
                let Some(client) = self.debug_session.as_ref().and_then(|s| s.client_of(on)) else { return };
                if command == "startDebugging" {
                    let _ = client.respond_json(seq, &command, Ok(serde_json::json!({})));
                    self.start_debug_child(on, arguments);
                } else {
                    let _ = client.respond_json(seq, &command, Err("not implemented".into()));
                }
            }
            // See `DapEvent::Unknown`'s own doc comment -- expected,
            // not an error; simply nothing to do with it.
            fenix_dap::DapEvent::Unknown(_) => {}
            fenix_dap::DapEvent::Disconnected(reason) => {
                if self.debug_child_ended(on) {
                    return;
                }
                // After the program's end, the adapter going is expected.
                if self.debug_session.as_ref().is_some_and(|s| s.ended) {
                    return;
                }
                self.debug_root_ended(DebugEnd::Disconnected(reason.clone()));
                self.set_error(format!("debug adapter disconnected: {reason}"));
            }
        }
    }

    /// A child session `on` is over: gone from the session, and told.
    /// False when `on` is no child's.
    pub(super) fn debug_child_ended(&mut self, on: u64) -> bool {
        let Some(session) = self.debug_session.as_mut() else { return false };
        let Some(at) = session.children.iter().position(|c| c.generation == on) else { return false };
        // Its own children go with it.
        let mut gone = vec![session.children.remove(at)];
        while let Some(at) = session.children.iter().position(|c| gone.iter().any(|g| g.generation == c.parent)) {
            gone.push(session.children.remove(at));
        }
        if gone.iter().any(|g| g.generation == session.stopped_on) {
            session.stopped_thread = None;
            session.stopped_on = session.generation;
            session.current_frame_id = None;
            session.call_stack.clear();
            session.frame_ids.clear();
            session.selected_frame = 0;
            session.variables.clear();
        }
        for g in &gone {
            debug_drawer::console_push(&mut session.console, format!("{} ended", g.name));
        }
        if gone.iter().any(|g| g.generation == session.running_on) {
            session.running_on = session.generation;
        }
        let ids: Vec<u64> = gone.iter().rev().map(|g| g.generation).collect();
        drop(gone);
        self.refresh_debug_panes();
        for id in ids {
            self.tell_debug(fenix_wasm::host::DebugState::Ended(id));
        }
        true
    }

    /// The session's own program is over, for reason `end`: what it showed
    /// while stopped goes, the panel says how it ended, and the extensions
    /// are told, once. An `exited` code isn't lost to the `terminated`
    /// that follows it.
    pub(super) fn debug_root_ended(&mut self, end: DebugEnd) {
        let Some(session) = self.debug_session.as_mut() else { return };
        let keep = matches!((&session.end, &end), (Some(DebugEnd::Exited(Some(_))), DebugEnd::Exited(None)) | (Some(_), DebugEnd::Stopped));
        if !keep {
            session.end = Some(end);
        }
        session.stopped_thread = None;
        session.current_frame_id = None;
        session.call_stack.clear();
        session.frame_ids.clear();
        session.selected_frame = 0;
        session.variables.clear();
        session.watch_values.clear();
        let told = !std::mem::replace(&mut session.ended, true);
        let id = session.generation;
        self.refresh_debug_panes();
        if told {
            self.tell_debug(fenix_wasm::host::DebugState::Ended(id));
        }
    }

    /// An ended session, out of the way of a new one: dropped, and its
    /// panel's workspace closed when it's still there. False while it
    /// still runs.
    pub(super) fn debug_clear_ended(&mut self) -> bool {
        let Some(session) = self.debug_session.as_ref() else { return true };
        if !session.ended {
            return false;
        }
        self.debug_session = None;
        self.debug_drawer.focused = false;
        self.debug_drawer.line = None;
        self.refresh_debug_decorations();
        true
    }

    pub(super) fn apply_dap_response(&mut self, on: u64, pending: PendingDapRequest, result: Result<fenix_dap::responses::SuccessResponse, fenix_dap::responses::ErrorResponse>) {
        use fenix_dap::responses::SuccessResponse;
        match pending {
            PendingDapRequest::Initialize => match result {
                Ok(answer) => {
                    let Some(session) = self.debug_session.as_mut() else { return };
                    // The exceptions it can stop on, from the session's own.
                    if let (SuccessResponse::Initialize(caps), true) = (&answer, on == session.generation) {
                        session.exception_filters = caps.exception_breakpoint_filters.iter().map(|f| debug_breakpoints::ExceptionFilter { filter: f.filter.clone(), label: f.label.clone(), default: f.default }).collect();
                    }
                    let (attributes, attach) = if on == session.generation {
                        (session.launch_attributes.clone(), session.attach)
                    } else {
                        match session.children.iter().find(|c| c.generation == on) {
                            Some(child) => (child.configuration.clone(), child.attach),
                            None => return,
                        }
                    };
                    let Some(client) = session.client_of(on) else { return };
                    let sent = if attach {
                        client.attach(attributes)
                    } else {
                        let args = fenix_dap::requests::LaunchRequestArguments::builder().additional_attributes(attributes).build();
                        client.request(fenix_dap::requests::Request::Launch(args))
                    };
                    if let (Ok(seq), Some(pending)) = (sent, session.pending_of(on)) {
                        pending.insert(seq, PendingDapRequest::Launch);
                    }
                }
                Err(err) => {
                    let why = err.message.trim().to_string();
                    if self.debug_session.as_ref().is_some_and(|s| s.generation == on) {
                        self.debug_root_ended(DebugEnd::Refused(format!("initialize: {why}")));
                    }
                    self.set_error(format!("debug adapter failed to initialize: {why}"));
                }
            },
            PendingDapRequest::Launch => {
                if let Err(err) = result {
                    let why = err.message.trim().to_string();
                    if self.debug_session.as_ref().is_some_and(|s| s.generation == on) {
                        self.debug_root_ended(DebugEnd::Refused(why.clone()));
                    }
                    self.set_error(format!("launch failed: {why}"));
                }
            }
            PendingDapRequest::ConfigurationDone => {
                if let Err(err) = result {
                    let why = err.message.trim().to_string();
                    if self.debug_session.as_ref().is_some_and(|s| s.generation == on && !s.ended) {
                        self.debug_root_ended(DebugEnd::Refused(format!("configurationDone: {why}")));
                    }
                    self.set_error(format!("the debug session didn't start: {why}"));
                }
            }
            PendingDapRequest::SetBreakpoints => {
                // Nothing further to do -- the Breakpoints pane already
                // renders from `self.breakpoints` (the source of truth)
                // the moment it changes, not from this response; see
                // `DebugSession::breakpoints_buffer`'s own doc comment
                // for the disclosed gap this leaves (verified/rejected
                // status per breakpoint isn't tracked for v1).
            }
            PendingDapRequest::StackTrace => {
                if let Ok(SuccessResponse::StackTrace(body)) = result {
                    let Some(session) = self.debug_session.as_mut() else { return };
                    if session.stopped_on != on { return; }
                    let cwd = session.launch_attributes.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from).unwrap_or_else(|| session.root.clone());
                    let resolve_source = |p: String| { let p = PathBuf::from(p); if p.is_absolute() { p } else { cwd.join(p) } };
                    session.call_stack = body
                        .stack_frames
                        .iter()
                        .map(|f| (f.name.clone(), f.source.as_ref().and_then(|s| s.path.clone()).map(resolve_source).unwrap_or_default(), f.line.max(0) as usize))
                        .collect();
                    session.frame_ids = body.stack_frames.iter().map(|f| f.id).collect();
                    session.selected_frame = 0;
                    self.debug_trail_stopped();
                    let Some(session) = self.debug_session.as_mut() else { return };
                    let top = body.stack_frames.first();
                    session.current_frame_id = top.map(|f| f.id);
                    let top_location = top.and_then(|f| f.source.as_ref().and_then(|s| s.path.clone())).map(|p| (resolve_source(p), top.unwrap().line.max(0) as usize));
                    if let Some((path, line)) = top_location {
                        self.goto_debug_location(&path, line);
                    }
                    self.refresh_debug_panes();
                    self.debug_ask_frame();
                }
            }
            PendingDapRequest::Scopes { frame } => {
                if let Ok(SuccessResponse::Scopes(body)) = result {
                    self.debug_scopes_came(on, frame, body.scopes);
                }
            }
            PendingDapRequest::Variables { frame, path } => {
                let vars = match result {
                    Ok(SuccessResponse::Variables(body)) => body.variables,
                    _ => Vec::new(),
                };
                self.debug_variables_came(on, frame, path, vars);
            }
            PendingDapRequest::Repl { expression } => self.debug_repl_answered(expression, result),
            PendingDapRequest::Control { reply } => {
                let answer = match result {
                    Ok(SuccessResponse::Evaluate(body)) => Ok(serde_json::json!({ "result": body.result, "type": body.type_ })),
                    Ok(_) => Err(fenix_control::RpcError::internal("the adapter answered something else")),
                    Err(err) => Err(fenix_control::RpcError::invalid_params(err.message.trim().to_string())),
                };
                let _ = reply.send(answer);
            }
            PendingDapRequest::Lens { expression } => self.debug_lens_answered(on, expression, result),
            PendingDapRequest::Pin { pin } => {
                let value = match result {
                    Ok(SuccessResponse::Evaluate(body)) => Some(body.result),
                    _ => None,
                };
                self.debug_pin_answered(pin, value);
            }
            PendingDapRequest::LensChildren { card } => {
                if let Ok(SuccessResponse::Variables(body)) = result {
                    self.debug_lens_children(card, body.variables);
                }
            }
            PendingDapRequest::Evaluate { expression } => {
                let Some(session) = self.debug_session.as_mut() else { return };
                let value = match result {
                    Ok(SuccessResponse::Evaluate(body)) => body.result,
                    _ => "<error>".to_string(),
                };
                session.watch_values.insert(expression, value);
                self.refresh_debug_panes();
            }
        }
    }

    /// One decoded `event` message (`initialized`/`stopped`/
    /// `continued`/`terminated`/`exited`/... -- everything not routed
    /// through `apply_dap_response`, since these arrive unsolicited
    /// rather than as an answer to a specific request), on connection
    /// `on`.
    pub(super) fn apply_dap_notification(&mut self, on: u64, event: fenix_dap::events::Event) {
        use fenix_dap::events::Event;
        match event {
            Event::Initialized => {
                let Some(session) = self.debug_session.as_mut() else { return };
                if on == session.generation {
                    session.initialized = true;
                } else if let Some(child) = session.children.iter_mut().find(|c| c.generation == on) {
                    child.initialized = true;
                } else {
                    return;
                }
                let paths: Vec<PathBuf> = self.breakpoints.iter().filter(|(_, lines)| !lines.is_empty()).map(|(p, _)| p.clone()).collect();
                for path in paths {
                    self.send_set_breakpoints_to(on, &path);
                }
                self.send_exception_breakpoints_to(on);
                if let Some(session) = self.debug_session.as_mut() {
                    session.send(on, fenix_dap::requests::Request::ConfigurationDone, PendingDapRequest::ConfigurationDone);
                }
            }
            Event::Stopped(body) => {
                let Some(session) = self.debug_session.as_mut() else { return };
                if session.client_of(on).is_none() { return; }
                let thread_id = body.thread_id.unwrap_or(0);
                session.stopped_thread = Some(thread_id);
                session.last_thread = Some(thread_id);
                session.stopped_on = on;
                session.running_on = on;
                // Stopped on an exception: what it was, to the console.
                if body.reason == fenix_dap::events::StoppedEventReason::Exception {
                    let what = body.text.clone().or_else(|| body.description.clone()).unwrap_or_else(|| "an exception".into());
                    debug_drawer::console_push(&mut session.console, format!("stopped on {what}"));
                }
                let args = fenix_dap::requests::StackTraceRequestArguments::builder().thread_id(thread_id).build();
                session.send(on, fenix_dap::requests::Request::StackTrace(args), PendingDapRequest::StackTrace);
                let why = serde_json::to_value(&body.reason).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                self.debug_mode_left = false;
                self.agent_debug_stopped();
                self.debug_run_to_reached();
                self.tell_debug(fenix_wasm::host::DebugState::Paused((on, why)));
            }
            Event::Thread(body) => {
                let Some(session) = self.debug_session.as_mut() else { return };
                if session.client_of(on).is_some() && (session.last_thread.is_none() || on != session.generation) {
                    session.last_thread = Some(body.thread_id);
                    session.running_on = on;
                }
            }
            Event::Continued(_) => {
                let Some(session) = self.debug_session.as_mut() else { return };
                // The pins' values are the last stop's now.
                for value in session.pin_values.values_mut() {
                    value.1 = false;
                }
                if session.stopped_on == on {
                    session.stopped_thread = None;
                    session.current_frame_id = None;
                    session.call_stack.clear();
                    session.frame_ids.clear();
                    session.selected_frame = 0;
                    session.variables.clear();
                }
                self.refresh_debug_panes();
                self.tell_debug(fenix_wasm::host::DebugState::Resumed(on));
            }
            Event::Terminated(_) | Event::Exited(_) => {
                // A child's exit code, to the console before it goes.
                if let (Event::Exited(body), Some(session)) = (&event, self.debug_session.as_mut()) {
                    if let Some(name) = session.children.iter().find(|c| c.generation == on).map(|c| c.name.clone()) {
                        debug_drawer::console_push(&mut session.console, format!("{name} exited with code {}", body.exit_code));
                    }
                }
                if self.debug_child_ended(on) {
                    return;
                }
                let Some(session) = self.debug_session.as_ref() else { return };
                if on != session.generation { return; }
                let code = match &event { Event::Exited(body) => Some(body.exit_code as i64), _ => None };
                self.debug_root_ended(DebugEnd::Exited(code));
            }
            Event::Output(body) => {
                use fenix_dap::events::OutputCategory;
                let Some(session) = self.debug_session.as_mut() else { return };
                if session.client_of(on).is_none() { return; }
                // The program's own output for its last words; the drawer's
                // console has the adapter's console besides, where a
                // logpoint's message comes. Telemetry is neither's.
                let program = matches!(body.category, OutputCategory::Stdout | OutputCategory::Stderr | OutputCategory::Important);
                if !program && body.category != OutputCategory::Console { return; }
                for line in body.output.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
                    if program {
                        if session.output_tail.len() == DEBUG_OUTPUT_TAIL {
                            session.output_tail.pop_front();
                        }
                        session.output_tail.push_back(line.to_string());
                    }
                    debug_drawer::console_push(&mut session.console, line.to_string());
                }
            }
            _ => {} // output/module/thread/process/... -- no dedicated pane for these in v1
        }
    }
}
