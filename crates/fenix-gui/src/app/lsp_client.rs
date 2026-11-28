//! Language servers: starting them, keeping them told, and what they answer.
//!
//! Methods of `App` moved here from `app.rs` as they were.

use super::*;

impl App {
    /// Makes sure every language server for the focused buffer -- its
    /// language's primary server and any additional servers the project
    /// configures for that language -- has an up to date picture of it:
    /// spawns each that isn't running yet (`ensure_lsp_session`/
    /// `ensure_additional_lsp_session`), sends `textDocument/didOpen` the
    /// first time (once that server is initialized), and after that
    /// compares `Buffer::edit_count()` against what was last synced with
    /// that server -- a mismatch means real edits landed since, answered
    /// with a `textDocument/didChange` (incremental or whole-text, as each
    /// server asked).
    ///
    /// Called from `refresh_project_root` (every focus change) *and*
    /// every `redraw` (every rendered frame) -- focus change alone
    /// would send `didOpen` but then never notice an edit made while
    /// staying on the same buffer, since nothing else marks "this
    /// buffer changed" the way switching away/back does.
    /// `sync_lsp_for_focused_buffer`, each frame: skipped while the
    /// focused buffer, its edit count, and the servers running and ready
    /// are what they were the last time.
    pub(super) fn sync_lsp_if_changed(&mut self) {
        let ready = self.lsp_sessions.values().filter(|s| s.capabilities.is_some()).count();
        let now = (self.focused_buffer_id(), self.open().buffer.edit_count(), self.lsp_sessions.len(), ready, self.lsp_unavailable.len());
        if self.lsp_frame_synced == Some(now) {
            return;
        }
        self.sync_lsp_for_focused_buffer();
        let ready = self.lsp_sessions.values().filter(|s| s.capabilities.is_some()).count();
        self.lsp_frame_synced = Some((self.focused_buffer_id(), self.open().buffer.edit_count(), self.lsp_sessions.len(), ready, self.lsp_unavailable.len()));
    }

    pub(super) fn sync_lsp_for_focused_buffer(&mut self) {
        let _profile = crate::profile::Scope::new("sync LSP");
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else { return };
        let edit_count = self.open().buffer.edit_count();
        let text = self.open().buffer.text();
        let buffer = self.focused_buffer_id();
        self.sync_lsp_for(&path, &text, edit_count, Some(buffer));
    }

    /// Starts `path`'s language servers if they aren't running, and tells
    /// those that are its text, if `edit_count` says that's changed.
    /// `buffer`: the buffer it's shown in, whose semantic tokens are
    /// asked for; none for an agent's look at a file.
    pub(super) fn sync_lsp_for(&mut self, path: &Path, text: &str, edit_count: u64, buffer: Option<BufferId>) {
        // Language servers for a remote project run on its machine, which
        // isn't done yet: none are started here for its files.
        if fenix_vfs::is_remote(path) {
            return;
        }
        let path = path.to_path_buf();
        let language = fenix_syntax::detect_language_from_path(&path);
        let cwd = tool_sessions::lsp_root_for_path(&path);
        if let Some(language) = language {
            let _profile = crate::profile::Scope::new("primary server");
            self.ensure_lsp_session(language, &cwd);
            for server in self.additional_lsp_servers(&cwd, language) {
                self.ensure_additional_lsp_session(&server, &cwd);
            }
        }
        // Extensions' servers: for its language, or for its name by pattern.
        let name = language.map(crate::lsp::language_config_name);
        for (ext, server, languages, _) in self.code_language_servers_for(&path, name.as_deref(), &cwd) {
            let _profile = crate::profile::Scope::new("extension's server");
            self.ensure_code_lsp_session(&ext, &server, languages, &cwd);
        }
        let _profile = crate::profile::Scope::new("didOpen");

        // `fenix_lsp::normalize` strips Windows' `\\?\` verbatim prefix
        // (which `canonicalize` always adds there) -- everywhere else
        // a canonicalized path is compared against one that came back
        // from a server's URI (`open_documents`, `App::diagnostics`'
        // own keys) needs the same normalized form, or two spellings of
        // the same file stop matching each other.
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path.clone()));
        // A buffer's semantic tokens from one of them, if they're wanted:
        // which, and the path, kept for drawing it.
        let asked = match buffer.filter(|&b| self.semantic_wanted(b)) {
            Some(b) => self.semantic_server(&path).map(|key| (b, key)),
            None => None,
        };
        match (&asked, buffer) {
            (Some((b, key)), _) => {
                self.semantic_of.insert(*b, semantic::SemanticTarget { key: key.clone(), path: canonical.clone() });
            }
            (None, Some(b)) => {
                self.semantic_of.remove(&b);
            }
            (None, None) => {}
        }
        // A buffer's lenses from every one of them that offers them.
        let lensed = self.lens_targets(buffer, &path, &canonical);
        self.inlay_targets(buffer, &path, &canonical);
        for (key, language_id, by_pattern) in self.lsp_targets(&path) {
            let semantic = match (&asked, buffer) {
                (Some((b, asked)), _) if *asked == key => semantic::SemanticSync::Ask(*b),
                (_, Some(_)) => semantic::SemanticSync::Drop,
                (_, None) => semantic::SemanticSync::Follow,
            };
            let lens = match &lensed {
                Some((b, keys)) if keys.contains(&key) => semantic::SemanticSync::Ask(*b),
                Some(_) => semantic::SemanticSync::Drop,
                None => semantic::SemanticSync::Follow,
            };
            let Some(session) = self.lsp_sessions.get_mut(&key) else { continue };
            sync_lsp_document(session, &canonical, &language_id, by_pattern, text, edit_count, semantic, lens);
        }
        // Moved with the edit, or let go: laid out again.
        if let Some(buffer) = buffer {
            self.lay_out_lenses(buffer);
        }
    }

    /// Every server for `path`, primary first, whether or not each is
    /// running: its key, the `languageId` it's told the file is, and
    /// whether it serves it by pattern rather than by language.
    pub(super) fn lsp_targets(&self, path: &Path) -> Vec<(LspKey, String, bool)> {
        let language = fenix_syntax::detect_language_from_path(path);
        let root = tool_sessions::lsp_root_for_path(path);
        let name = language.map(crate::lsp::language_config_name);
        let mut keys = Vec::new();
        if let (Some(language), Some(name)) = (language, &name) {
            keys.push((LspKey::primary(language, root.clone()), name.clone(), false));
            for server in self.lsp_additional.get(&root).into_iter().flatten().filter(|server| server.serves(name)) {
                keys.push((LspKey { server: LspServer::Additional(server.name.clone()), root: root.clone() }, name.clone(), false));
            }
        }
        for (ext, server, _, by_pattern) in self.code_language_servers_for(path, name.as_deref(), &root) {
            let id = name.clone().unwrap_or_else(|| server.clone());
            keys.push((LspKey { server: LspServer::Additional(code_server_name(&ext, &server)), root: root.clone() }, id, by_pattern));
        }
        keys
    }

    /// The additional servers `root`'s `tools.json` declares for
    /// `language`, in the order they're written -- read once per root
    /// (a broken file is reported where the primary server's
    /// configuration is read, and means none here).
    pub(super) fn additional_lsp_servers(&mut self, root: &Path, language: fenix_syntax::LanguageId) -> Vec<fenix_project::tools::AdditionalServer> {
        // The project's own, then those extensions added.
        let added = self.wasm.added_servers(root);
        if !self.lsp_additional.contains_key(root) {
            let mut all = self.project_tools(root).map(|tools| tools.lsp.additional).unwrap_or_default();
            all.extend(added);
            self.lsp_additional.insert(root.to_path_buf(), all);
        }
        let all = &self.lsp_additional[root];
        let name = crate::lsp::language_config_name(language);
        all.iter().filter(|server| server.serves(&name)).cloned().collect()
    }

    /// Every server key for `path`, primary first, whether or not each is
    /// running -- from the additional servers already read for its root.
    pub(super) fn cached_lsp_keys(&self, path: &Path) -> Vec<LspKey> {
        self.lsp_targets(path).into_iter().map(|(key, _, _)| key).collect()
    }

    /// Extension `ext`'s language server `name` for the project at `cwd`,
    /// started if it isn't running: in Fenix's own process, its messages
    /// to and from the extension's `lsp-message` and `lsp-send` through a
    /// pair of pipes, as a server's stdin and stdout would be.
    pub(super) fn ensure_code_lsp_session(&mut self, ext: &str, name: &str, languages: Vec<String>, cwd: &Path) {
        let key = LspKey { server: LspServer::Additional(code_server_name(ext, name)), root: fenix_lsp::normalize(refactor::identity(cwd)) };
        if self.lsp_sessions.contains_key(&key) || self.lsp_unavailable.contains(&key) { return; }
        {
            let _profile = crate::profile::Scope::new("start extension");
            // One running in another project serves this one from here.
            if self.wasm.stale.contains(ext) {
                self.restart_code_extension(ext);
            }
            self.start_code_extension(ext);
        }
        let _profile = crate::profile::Scope::new("connect");
        let Some(port) = self.code_lsp_port(ext) else {
            self.lsp_unavailable.insert(key);
            return;
        };
        let (Ok((from_ext, to_fenix)), Ok((from_fenix, to_ext))) = (std::io::pipe(), std::io::pipe()) else {
            self.lsp_unavailable.insert(key);
            return self.set_error(format!("couldn't start {name}: no pipes"));
        };
        let server = tool_sessions::generation();
        port.open(server, to_fenix);
        // What Fenix writes to the server, a message at a time, to the
        // extension.
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(from_fenix);
            while let Some(message) = read_lsp_frame(&mut reader) {
                if !port.send(server, message) {
                    return;
                }
            }
            port.close(server);
        });
        let (client, receiver) = fenix_lsp::LspClient::connect(from_ext, to_ext);
        let generation = tool_sessions::generation();
        let reader = self.event_proxy.clone().map(|proxy| LspReader::spawn(receiver, key.clone(), generation, move |event| proxy.send_event(event).is_ok()));
        self.lsp_sessions.insert(key.clone(), LspSession {
            generation, client, name: name.to_string(), languages, root: key.root.clone(), reader, capabilities: None, started: Instant::now(),
            open_documents: HashMap::new(), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(), semantic: HashMap::new(), lenses: HashMap::new(),
        });
        self.send_lsp_initialize(key, cwd);
    }

    /// Extension `ext`'s language servers stopped: it stopped, or starts
    /// again elsewhere. Each starts again when a buffer needs it.
    pub(crate) fn forget_code_lsp_sessions(&mut self, ext: &str) {
        let mine = |key: &LspKey| matches!(&key.server, LspServer::Additional(n) if n.starts_with(&format!("{ext}:")));
        let gone: Vec<LspSession> = self.lsp_sessions.extract_if(|key, _| mine(key)).map(|(_, session)| session).collect();
        tool_sessions::let_go(gone);
        self.lsp_unavailable.retain(|key| !mine(key));
        // The problems it gave (`diagnostics-set`) go with it.
        let problems = wasm_host::problems_key(ext);
        self.diagnostics.forget(|key| matches!(&key.server, LspServer::Additional(n) if *n == problems));
    }

    /// Spawns `language`'s primary language server for the project at
    /// `cwd` if one isn't already running (a no-op otherwise, regardless
    /// of whether the existing session is healthy -- a dead one is
    /// replaced by `apply_lsp_event`'s own `Disconnected` handling removing
    /// it first, not respawned here on every focus change), resolving its
    /// command from the project's `tools.json`, then `[lsp]`/`fenix-lsp`'s
    /// built-in per-language default, and using `cwd` as both its working
    /// directory and its advertised project root -- a server has no other
    /// way to know what "the project" even is. Silently does nothing if no
    /// command is configured or known for `language` (same posture as
    /// completion quietly having nothing to offer for a language with no
    /// ctags support).
    pub(super) fn ensure_lsp_session(&mut self, language: fenix_syntax::LanguageId, cwd: &Path) {
        let key = LspKey::primary(language, fenix_lsp::normalize(refactor::identity(cwd)));
        if self.lsp_sessions.contains_key(&key) || self.lsp_unavailable.contains(&key) || self.lsp_starting.contains_key(&key) { return; }
        let configured = match self.project_tools(&key.root) {
            Ok(config) => config,
            Err(error) => { self.lsp_unavailable.insert(key); self.set_error(error); return; }
        };
        // A project's own `.fenix/tools.json` wins; then a feature's
        // server (for C/C++ in a microcontroller project, that platform's
        // -- plain clangd can't make sense of an Arduino sketch); then
        // `[lsp]`/defaults.
        let embedded = if !configured.lsp.contains_key(&crate::lsp::language_config_name(language)) {
            self.wasm.language_server(&key.root, language)
        } else {
            None
        };
        let spec = configured.lsp.get(&crate::lsp::language_config_name(language)).cloned().or(embedded).or_else(|| {
            crate::lsp::resolve_server_command(language, &self.config.lsp_servers).map(|(program, args)| fenix_project::tools::CommandSpec::new(program, args))
        });
        let Some(spec) = spec else { self.lsp_unavailable.insert(key); return };
        let name = Path::new(&spec.executable).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| spec.executable.clone());
        self.start_lsp_session(key, name, vec![crate::lsp::language_config_name(language)], &spec, cwd);
    }

    /// Spawns an additional server for the project at `cwd` if it isn't
    /// running (or known to have failed) already -- one process for every
    /// language it serves there.
    pub(super) fn ensure_additional_lsp_session(&mut self, server: &fenix_project::tools::AdditionalServer, cwd: &Path) {
        let key = LspKey { server: LspServer::Additional(server.name.clone()), root: fenix_lsp::normalize(refactor::identity(cwd)) };
        if self.lsp_sessions.contains_key(&key) || self.lsp_unavailable.contains(&key) || self.lsp_starting.contains_key(&key) { return; }
        self.start_lsp_session(key, server.name.clone(), server.languages.clone(), &server.command(), cwd);
    }

    /// Starts `spec` as the server `key` and sends it `initialize`. A
    /// server that can't start is reported and left alone until
    /// `:lsp-restart`; the buffer's other servers carry on without it.
    ///
    /// The process is looked for (on Windows, `PATH` searched by hand)
    /// and started on a thread of its own, its session taken up when the
    /// event loop hears of it (`apply_lsp_started`): a slow `PATH`, a
    /// network drive on it, or a server slow to start never holds up the
    /// window (#33). With no event loop to hear of it, it starts here.
    pub(super) fn start_lsp_session(&mut self, key: LspKey, name: String, languages: Vec<String>, spec: &fenix_project::tools::CommandSpec, cwd: &Path) {
        let Some(send) = self.lsp_start_sender() else {
            let spawned = spawn_lsp_server(spec, &key.root);
            return self.lsp_started(key, name, languages, &spec.executable, cwd, spawned);
        };
        let generation = tool_sessions::generation();
        self.lsp_starting.insert(key.clone(), generation);
        let (spec, cwd) = (spec.clone(), cwd.to_path_buf());
        std::thread::Builder::new()
            .name("fenix-lsp-start".into())
            .spawn(move || {
                let result = spawn_lsp_server(&spec, &key.root);
                send(FenixUserEvent::LspStarted(Box::new(LspStart { key, generation, name, languages, executable: spec.executable, cwd, result })));
            })
            .expect("a thread to start a language server");
    }

    /// Where a language server started off the UI thread sends what it
    /// came to: the event loop (a window that has gone drops it, the
    /// server with it, on that thread). None without one.
    pub(super) fn lsp_start_sender(&self) -> Option<Box<dyn Fn(FenixUserEvent) + Send>> {
        #[cfg(test)]
        if let Some(sink) = self.lsp_start_sink.clone() {
            return Some(Box::new(move |event| {
                let _ = sink.send(event);
            }));
        }
        let proxy = self.event_proxy.clone()?;
        Some(Box::new(move |event| {
            let _ = proxy.send_event(event);
        }))
    }

    /// A language server started off the UI thread came back: taken up
    /// if it's still the one wanted, else let go, off this thread.
    pub(super) fn apply_lsp_started(&mut self, start: LspStart) {
        if self.lsp_starting.get(&start.key) != Some(&start.generation) {
            if let Ok((client, _)) = start.result {
                std::thread::spawn(move || drop(client));
            }
            return;
        }
        self.lsp_starting.remove(&start.key);
        self.lsp_started(start.key, start.name, start.languages, &start.executable, &start.cwd, start.result);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Server `key` (`executable`) started, or couldn't: its session, and
    /// `initialize` sent; or the reason, said and kept for Tool Status.
    pub(super) fn lsp_started(
        &mut self,
        key: LspKey,
        name: String,
        languages: Vec<String>,
        executable: &str,
        cwd: &Path,
        spawned: std::io::Result<(fenix_lsp::LspClient, std::sync::mpsc::Receiver<fenix_lsp::LspEvent>)>,
    ) {
        match spawned {
            Ok((client, receiver)) => {
                let generation = tool_sessions::generation();
                let reader = self.event_proxy.clone().map(|proxy| LspReader::spawn(receiver, key.clone(), generation, move |event| proxy.send_event(event).is_ok()));
                self.lsp_sessions.insert(key.clone(), LspSession {
                    generation, client, name, languages, root: key.root.clone(), reader, capabilities: None, started: Instant::now(),
                    open_documents: HashMap::new(), synced_text: HashMap::new(), pending: HashMap::new(), python_env: None, pulls: HashMap::new(), semantic: HashMap::new(), lenses: HashMap::new(),
                });
                self.send_lsp_initialize(key, cwd);
            }
            Err(err) => {
                self.lsp_failures.insert(key.clone(), format!("couldn't start {executable}: {err}"));
                // A built-in server that isn't installed is offered, in a
                // notice, rather than reported as an error.
                if !self.offer_server_install(&key, executable, &err) {
                    self.set_error(format!("couldn't start {name} ({executable}) for {}: {err}", cwd.display()));
                }
                self.lsp_unavailable.insert(key);
            }
        }
    }

    /// Sends the `initialize` request for a freshly-spawned session --
    /// deliberately not folded into `ensure_lsp_session` itself, so a
    /// later reconnect (once one exists) can re-send just this without
    /// re-spawning the process.
    pub(super) fn send_lsp_initialize(&mut self, key: LspKey, cwd: &Path) {
        let (semantic, lens, inlay) = (self.semantic_highlighting_on(), self.code_lens_on(), self.inlay_hints_on());
        let Some(session) = self.lsp_sessions.get_mut(&key) else { return };
        let root_uri = fenix_lsp::path_to_uri(cwd);
        let params = lsp_types::InitializeParams {
            process_id: Some(std::process::id()),
            #[allow(deprecated)] // still set alongside workspace_folders for servers that only look at this
            root_uri: root_uri.clone(),
            capabilities: crate::lsp::client_capabilities(semantic, lens, inlay),
            workspace_folders: root_uri.map(|uri| vec![lsp_types::WorkspaceFolder { uri, name: "root".to_string() }]),
            ..Default::default()
        };
        if let Ok(id) = session.client.request::<lsp_types::request::Initialize>(params) {
            session.pending.insert(id, PendingLspRequest::Initialize);
        }
    }

    /// Dispatches one decoded event from a language server's connection.
    pub(super) fn apply_lsp_event(&mut self, key: LspKey, generation: u64, event: fenix_lsp::LspEvent) {
        if !self.lsp_sessions.get(&key).is_some_and(|s| s.generation == generation) { return; }
        match event {
            fenix_lsp::LspEvent::Response { id, result } => {
                let Some(session) = self.lsp_sessions.get_mut(&key) else { return };
                let Some(pending) = session.pending.remove(&id) else { return };
                self.apply_lsp_response(key, pending, result);
            }
            fenix_lsp::LspEvent::Notification { method, params } => {
                if method == <lsp_types::notification::PublishDiagnostics as lsp_types::notification::Notification>::METHOD {
                    if let Ok(diagnostics) = serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(params) {
                        if fenix_lsp::uri_to_path(&diagnostics.uri).is_some_and(|path| tool_sessions::lsp_root_for_path(&path) == key.root) { self.apply_lsp_diagnostics(&key, diagnostics); }
                    }
                }
            }
            fenix_lsp::LspEvent::ServerRequest { id, method, params } => {
                // Its settings: each section asked for, or null.
                if method == "workspace/configuration" {
                    let settings = self.lsp_settings(&key);
                    if let Some(session) = self.lsp_sessions.get(&key) {
                        let _ = session.client.respond(id, Ok(fenix_lsp::pull::configuration(&params, &settings)));
                    }
                    return;
                }
                // Bookkeeping requests (progress tokens, capability
                // registration, a refresh) are accepted; everything else
                // is refused with "method not found" rather than left
                // unanswered.
                if method == "workspace/semanticTokens/refresh" {
                    self.refresh_semantic_tokens(&key);
                }
                if method == "workspace/codeLens/refresh" {
                    self.refresh_lenses(&key);
                }
                if method == "workspace/inlayHint/refresh" {
                    self.refresh_inlay_hints(&key);
                }
                if let Some(session) = self.lsp_sessions.get_mut(&key) {
                    let _ = session.client.respond(id, fenix_lsp::server_request_reply(&method));
                    // What it says of every open document, asked again.
                    if method == "workspace/diagnostic/refresh" {
                        let open: Vec<PathBuf> = session.open_documents.keys().cloned().collect();
                        for path in open {
                            pull_lsp_diagnostics(session, &path);
                        }
                    }
                }
            }
            fenix_lsp::LspEvent::Disconnected(reason) => {
                // Only this server goes: the buffer's other servers keep
                // their sessions, diagnostics and requests.
                let name = self.lsp_server_name(&key);
                let said = self.lsp_sessions.get(&key).map(|s| s.client.stderr_tail()).unwrap_or_default();
                self.stop_lsp_session(&key);
                self.set_error(format!("{name} disconnected: {reason}{}", last_line_of(&said)));
                self.lsp_failures.insert(key.clone(), failure_text(&format!("disconnected: {reason}"), &said));
                self.lsp_unavailable.insert(key);
            }
        }
    }

    /// Routes one arrived response to whatever `pending` says the
    /// original request was for -- the single dispatch point every
    /// request-sending method's eventual answer passes through.
    pub(super) fn apply_lsp_response(&mut self, key: LspKey, pending: PendingLspRequest, result: Result<serde_json::Value, fenix_lsp::ResponseError>) {
        match pending {
            PendingLspRequest::Agent { id } => self.agent_lsp_answered(id, result.map_err(|e| e.message)),
            PendingLspRequest::ProjectSymbols { generation } => self.project_symbols_answered(key, generation, result.map_err(|e| e.message)),
            PendingLspRequest::Initialize => match result {
                Ok(value) => {
                    let mut value = value;
                    fenix_lsp::semantic::normalize_initialize_result(&mut value);
                    let capabilities = serde_json::from_value::<lsp_types::InitializeResult>(value).ok().map(|r| r.capabilities);
                    if let Some(session) = self.lsp_sessions.get_mut(&key) {
                        session.capabilities = capabilities;
                        let _ = session.client.notify::<lsp_types::notification::Initialized>(lsp_types::InitializedParams {});
                        // pyright doesn't infer which interpreter/venv a
                        // project uses on its own -- without this, "go to
                        // definition"/hover on a third-party import
                        // resolves nowhere, since pyright never looked in
                        // that venv's site-packages at all. Pushed as a
                        // `workspace/didChangeConfiguration`, the shape
                        // every pyright integration (VS Code's own
                        // extension included) actually feeds it; this
                        // client never declared `workspace/configuration`
                        // pull support, so a push right after `initialized`
                        // is the one required step, not just an optimization.
                        // Pushed again whenever the environment changes --
                        // see `refresh_python_environments`.
                        if key.is_primary_for(fenix_syntax::LanguageId::Python) {
                            push_python_environment(session);
                        }
                    }
                    // Settings it's given, pushed too, for a server
                    // that doesn't ask.
                    let settings = self.lsp_settings(&key);
                    if !settings.is_null() && !key.is_primary_for(fenix_syntax::LanguageId::Python) {
                        if let Some(session) = self.lsp_sessions.get(&key) {
                            let _ = session.client.notify::<lsp_types::notification::DidChangeConfiguration>(lsp_types::DidChangeConfigurationParams { settings });
                        }
                    }
                    // Now that the session is actually initialized,
                    // send `didOpen` for the focused buffer if it's one
                    // this server serves (the request that triggered
                    // spawning this session in the first place couldn't
                    // send it yet -- capabilities weren't known).
                    self.sync_lsp_for_focused_buffer();
                }
                Err(err) => {
                    let name = self.lsp_server_name(&key);
                    self.set_error(format!("{name} failed to initialize: {}", err.message));
                    let said = self.lsp_sessions.get(&key).map(|s| s.client.stderr_tail()).unwrap_or_default();
                    self.lsp_failures.insert(key, failure_text(&format!("failed to initialize: {}", err.message), &said));
                }
            },
            PendingLspRequest::Control { fanout } => {
                if let Some(LspFanout::Control { gather, .. }) = self.lsp_fanouts.get_mut(&fanout) {
                    gather.answer(&key, result.ok());
                }
                self.lsp_fanout_progress(fanout);
            }
            PendingLspRequest::Hover { fanout } => {
                let text = result.ok().and_then(|v| serde_json::from_value::<Option<lsp_types::Hover>>(v).ok()).map(|hover| hover.map(|h| crate::lsp::hover_contents_text(h.contents)));
                if let Some(LspFanout::Hover { gather, .. }) = self.lsp_fanouts.get_mut(&fanout) {
                    gather.answer(&key, text);
                }
                self.lsp_fanout_progress(fanout);
            }
            PendingLspRequest::Definition { fanout } => {
                let locations = result.ok().and_then(|v| serde_json::from_value::<Option<lsp_types::GotoDefinitionResponse>>(v).ok()).map(|r| r.map(fenix_lsp::fanout::locations_of).unwrap_or_default());
                if let Some(LspFanout::Locations { gather, .. }) = self.lsp_fanouts.get_mut(&fanout) {
                    gather.answer(&key, locations);
                }
                self.lsp_fanout_progress(fanout);
            }
            PendingLspRequest::References { fanout } => {
                let locations = result.ok().and_then(|v| serde_json::from_value::<Option<Vec<lsp_types::Location>>>(v).ok()).map(Option::unwrap_or_default);
                if let Some(LspFanout::Locations { gather, .. }) = self.lsp_fanouts.get_mut(&fanout) {
                    gather.answer(&key, locations);
                }
                self.lsp_fanout_progress(fanout);
            }
            PendingLspRequest::Rename { buffer, context } => match result {
                Ok(value) => match serde_json::from_value::<Option<lsp_types::WorkspaceEdit>>(value) {
                    Ok(Some(edit)) if buffer == self.focused_buffer_id() => self.preview_lsp_edit(edit, context),
                    Ok(Some(_)) => self.set_error("rename cancelled: focus changed; request it again"),
                    Ok(None) => self.set_message("no rename changes proposed"),
                    Err(error) => self.set_error(format!("invalid rename response: {error}")),
                },
                Err(err) => self.set_error(format!("rename failed: {}", err.message)),
            },
            PendingLspRequest::CodeAction { fanout } => {
                let name = self.lsp_server_name(&key);
                if let Some(LspFanout::CodeActions { gather, errors, .. }) = self.lsp_fanouts.get_mut(&fanout) {
                    let answer = match result {
                        Ok(value) => match serde_json::from_value::<Option<Vec<lsp_types::CodeActionOrCommand>>>(value) {
                            Ok(actions) => Some(actions.unwrap_or_default()),
                            Err(error) => {
                                errors.push(format!("{name}: invalid code action response: {error}"));
                                None
                            }
                        },
                        Err(err) => {
                            errors.push(format!("{name}: {}", err.message));
                            None
                        }
                    };
                    gather.answer(&key, answer);
                }
                self.lsp_fanout_progress(fanout);
            }
            PendingLspRequest::Format { buffer, revision } => {
                if let Ok(Some(edits)) = result.map(|v| serde_json::from_value::<Option<Vec<lsp_types::TextEdit>>>(v).ok().flatten()) {
                    if self.buffers.get(buffer).is_some_and(|ob| ob.buffer.edit_count() == revision) {
                        self.apply_lsp_text_edits(buffer, edits);
                    } else {
                        self.set_error("formatting rejected: document changed while waiting for the server");
                    }
                }
            }
            PendingLspRequest::WorkspaceNames => {
                let names = result.ok().and_then(|v| serde_json::from_value::<Option<lsp_types::WorkspaceSymbolResponse>>(v).ok()).flatten().map(functions_named).unwrap_or_default();
                let asked = self.lsp_workspace_names.get(&key).map_or_else(Instant::now, |(at, _)| *at);
                let changed = self.lsp_workspace_names.get(&key).is_none_or(|(_, had)| **had != names);
                self.lsp_workspace_names.insert(key, (asked, std::sync::Arc::new(names)));
                if changed {
                    self.wake_caret();
                }
            }
            PendingLspRequest::SemanticTokens { path, edit_count, how, request } => self.semantic_tokens_answered(&key, path, edit_count, how, request, result),
            PendingLspRequest::CodeLens { path, edit_count, request } => self.lenses_answered(&key, path, edit_count, request, result),
            PendingLspRequest::InlayHint { buffer, request } => self.inlay_hints_answered(&key, buffer, request, result),
            PendingLspRequest::CodeLensResolve { path, generation, index } => self.lens_resolved(&key, path, generation, index, result),
            PendingLspRequest::LensCommand { title } => {
                if let Err(e) = result {
                    self.set_error(format!("{title} failed: {}", e.message));
                }
            }
            PendingLspRequest::PullDiagnostics { path } => {
                let name = self.lsp_server_name(&key);
                let read = result.map_err(|e| (e.code, e.data)).and_then(|value| fenix_lsp::pull::report(value).map_err(|_| (0, None)));
                let Some(session) = self.lsp_sessions.get_mut(&key) else { return };
                let pull = session.pulls.entry(path.clone()).or_default();
                pull.asking = false;
                let again = std::mem::take(&mut pull.again);
                let report = match read {
                    Ok(report) => report,
                    // The server cancelled it and asks to be asked again
                    // (`ServerCancelled`, `retriggerRequest`).
                    Err((-32802, data)) if data.as_ref().and_then(|d| d.get("retriggerRequest")).and_then(|r| r.as_bool()).unwrap_or(true) => {
                        pull_lsp_diagnostics(session, &path);
                        return;
                    }
                    Err(_) => Vec::new(),
                };
                let mut found = Vec::new();
                for (uri, result_id, items) in report {
                    let at = match uri {
                        None => {
                            pull.result_id = result_id;
                            path.clone()
                        }
                        Some(uri) => match fenix_lsp::uri_to_path(&uri) {
                            Some(p) => p,
                            None => continue,
                        },
                    };
                    // Unchanged: what it said last stands.
                    if let Some(items) = items {
                        found.push((at, items));
                    }
                }
                if again {
                    pull_lsp_diagnostics(session, &path);
                }
                for (at, items) in found {
                    self.diagnostics.pulled(at.clone(), key.clone(), &name, items);
                    self.tell_diagnostic_counts(&at);
                }
                self.request_window_redraw();
            }
            PendingLspRequest::Completion { buffer, prefix_start, generation } => {
                if generation != self.completion_generation || !self.completion_context_valid() {
                    return;
                }
                let Some(state) = self.lsp_completion.as_mut().filter(|c| c.buffer == buffer && c.prefix_start == prefix_start) else { return };
                match result.ok().and_then(|value| serde_json::from_value::<Option<lsp_types::CompletionResponse>>(value).ok()) {
                    Some(response) => state.answers.store(key, response.unwrap_or(lsp_types::CompletionResponse::Array(Vec::new()))),
                    // An error skips this server for this request.
                    None => state.answers.forget(&key),
                }
                self.apply_lsp_completion(buffer, prefix_start);
            }
        }
    }

    /// A request out to `key` that will never be answered -- its server
    /// went away. Counted as that server's failure, so the request's
    /// other servers aren't kept waiting for it.
    pub(super) fn lsp_request_failed(&mut self, key: &LspKey, pending: PendingLspRequest) {
        if let PendingLspRequest::Agent { id } = pending {
            self.agent_lsp_answered(id, Err("the request couldn't be sent".into()));
            return;
        }
        let fanout = match pending {
            PendingLspRequest::Hover { fanout } | PendingLspRequest::Definition { fanout } | PendingLspRequest::References { fanout } | PendingLspRequest::CodeAction { fanout } | PendingLspRequest::Control { fanout } => fanout,
            PendingLspRequest::Completion { .. } => {
                if let Some(state) = &mut self.lsp_completion {
                    state.answers.forget(key);
                }
                return;
            }
            _ => return,
        };
        match self.lsp_fanouts.get_mut(&fanout) {
            Some(LspFanout::Hover { gather, .. }) => { gather.answer(key, None); }
            Some(LspFanout::Locations { gather, .. }) => { gather.answer(key, None); }
            Some(LspFanout::CodeActions { gather, .. }) => { gather.answer(key, None); }
            Some(LspFanout::Control { gather, .. }) => { gather.answer(key, None); }
            None => return,
        }
        self.lsp_fanout_progress(fanout);
    }

    /// Starts gathering a request sent to `servers` (in display order):
    /// returns its id, and arranges a wake-up at its deadline so a slow
    /// server can't hold the others' answers back.
    pub(super) fn start_lsp_fanout(&mut self, make: impl FnOnce(std::time::Instant) -> LspFanout, servers: usize) -> u64 {
        let id = self.next_lsp_fanout;
        self.next_lsp_fanout += 1;
        let deadline = std::time::Instant::now() + LSP_FANOUT_DEADLINE;
        let fanout = make(deadline);
        // A newer request of the same kind supersedes one still waiting
        // on a server that may never answer; its late answers are dropped.
        let kind = std::mem::discriminant(&fanout);
        self.lsp_fanouts.retain(|_, older| std::mem::discriminant(older) != kind);
        self.lsp_fanouts.insert(id, fanout);
        if servers > 1 {
            if let Some(proxy) = self.event_proxy.clone() {
                std::thread::spawn(move || {
                    std::thread::sleep(LSP_FANOUT_DEADLINE);
                    let _ = proxy.send_event(FenixUserEvent::LspDeadline(id));
                });
            }
        }
        id
    }

    /// Acts on whatever a fan-out request has gathered so far: a hover
    /// shows every answer as it arrives; a jump, a reference list or a
    /// code action list waits for everyone, or for the deadline with
    /// something to show.
    pub(super) fn lsp_fanout_progress(&mut self, id: u64) {
        let now = std::time::Instant::now();
        let focused = self.focused_buffer_id();
        let done = match self.lsp_fanouts.get(&id) {
            None => return,
            Some(LspFanout::Hover { buffer, gather }) => {
                let text = (self.lsp_hover_fanout == Some(id) && *buffer == focused)
                    .then(|| fenix_lsp::fanout::join_hovers(gather.arrived().map(|(_, text)| text.clone())))
                    .flatten();
                let complete = gather.is_complete();
                if text.is_some() {
                    // The leader's keys in it, as the key scheme writes them.
                    self.lsp_hover = text.map(|t| if self.standard_keys() { keys_host::translate_leader(&t, &self.leader_chord_text()) } else { t });
                    self.wake_caret();
                }
                complete
            }
            Some(LspFanout::Locations { gather, .. }) => gather.is_ready(now, |locations| !locations.is_empty()),
            Some(LspFanout::CodeActions { gather, .. }) => gather.is_ready(now, |actions| !actions.is_empty()),
            Some(LspFanout::Control { gather, .. }) => gather.is_complete() || now >= gather.deadline(),
        };
        if !done {
            return;
        }
        match self.lsp_fanouts.remove(&id) {
            Some(LspFanout::Control { question, gather, reply }) => {
                let answers = gather.into_arrived().into_iter().map(|(_, answer)| answer).collect();
                self.control_lsp_answered(question, answers, reply);
            }
            Some(LspFanout::Locations { buffer, references, gather }) => {
                if buffer != focused {
                    return;
                }
                let locations = fenix_lsp::fanout::merge_locations(gather.into_arrived().into_iter().map(|(_, locations)| locations));
                if references {
                    self.apply_lsp_references(locations);
                } else {
                    self.goto_lsp_locations(locations);
                }
            }
            Some(LspFanout::CodeActions { buffer, gather, mut contexts, errors, fixes }) => {
                let answers = gather.into_arrived();
                if answers.is_empty() && !errors.is_empty() && fixes.is_empty() {
                    self.set_error(format!("code actions failed: {}", errors.join("; ")));
                    return;
                }
                if buffer != focused {
                    self.set_error("code actions cancelled: focus changed; request them again");
                    return;
                }
                let several = answers.len() > 1 || (!answers.is_empty() && !fixes.is_empty());
                let mut actions = Vec::new();
                for (key, list) in answers {
                    let name = self.lsp_server_name(&key);
                    let Some(context) = contexts.remove(&key) else { continue };
                    for action in list {
                        actions.push((action, context.clone(), several.then(|| name.clone())));
                    }
                }
                actions.extend(fixes);
                if actions.is_empty() {
                    self.set_message("no code actions available");
                } else {
                    self.preview_lsp_actions(actions);
                }
            }
            _ => {}
        }
    }

    /// `key`'s diagnostics for one document, replacing only its own
    /// earlier ones -- the document's other servers keep theirs.
    pub(super) fn apply_lsp_diagnostics(&mut self, key: &LspKey, params: lsp_types::PublishDiagnosticsParams) {
        let Some(path) = fenix_lsp::uri_to_path(&params.uri) else { return };
        let name = self.lsp_server_name(key);
        self.publish_diagnostics(path, key.clone(), &name, params.diagnostics);
    }

    /// The servers, document and current-cursor-as-LSP-`Position` for
    /// the focused buffer -- `None` unless at least one fully initialized
    /// language server is actually attached to it (checked via
    /// `open_documents`, not just `capabilities.is_some()`: a session can
    /// be initialized for a *different* buffer's `didOpen` before this
    /// one's has landed). The servers come primary first. The shared
    /// precondition every LSP request this app sends starts from
    /// (`gd`/`gr`/`K`/rename/code actions/format all begin here).
    pub(crate) fn focused_lsp_context(&self) -> Option<(Vec<LspKey>, lsp_types::TextDocumentIdentifier, lsp_types::Position)> {
        self.focused_lsp_context_at(self.cursor().char_idx)
    }

    /// `focused_lsp_context`, asking about char `at` rather than the
    /// cursor's -- where the mouse rests.
    pub(crate) fn focused_lsp_context_at(&self, at: usize) -> Option<(Vec<LspKey>, lsp_types::TextDocumentIdentifier, lsp_types::Position)> {
        let ob = self.open();
        let path = ob.buffer.path()?;
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        let keys: Vec<LspKey> = self
            .cached_lsp_keys(path)
            .into_iter()
            .filter(|key| self.lsp_sessions.get(key).is_some_and(|session| session.capabilities.is_some() && session.open_documents.contains_key(&canonical)))
            .collect();
        if keys.is_empty() {
            return None;
        }
        let uri = fenix_lsp::path_to_uri(&canonical)?;
        let position = fenix_lsp::char_offset_to_position(ob.buffer.rope(), at.min(ob.buffer.len_chars()));
        Some((keys, lsp_types::TextDocumentIdentifier { uri }, position))
    }

    /// Which of `keys` (attached, primary first) to ask for `feature` --
    /// see `fenix_lsp::fanout::route`.
    pub(super) fn lsp_route(&self, keys: &[LspKey], feature: fenix_lsp::fanout::Feature) -> Vec<LspKey> {
        self.lsp_route_with(keys, |attached| fenix_lsp::fanout::route(feature, attached))
    }

    pub(super) fn lsp_route_with(&self, keys: &[LspKey], pick: impl FnOnce(&[fenix_lsp::fanout::Attached]) -> Vec<usize>) -> Vec<LspKey> {
        let keys: Vec<&LspKey> = keys.iter().filter(|key| self.lsp_sessions.get(*key).is_some_and(|s| s.capabilities.is_some())).collect();
        let attached: Vec<fenix_lsp::fanout::Attached> = keys
            .iter()
            .map(|key| fenix_lsp::fanout::Attached { primary: key.is_primary(), capabilities: self.lsp_sessions[*key].capabilities.as_ref().unwrap() })
            .collect();
        pick(&attached).into_iter().map(|i| keys[i].clone()).collect()
    }

    /// Match all open documents by filesystem identity, including nonfocused
    /// buffers reached through an alternate path or symlink.
    pub(super) fn buffer_id_for_lsp_path(&self, path: &Path) -> Option<BufferId> {
        let canonical = refactor::identity(path);
        self.buffers.ids_sorted_by_path().into_iter().find(|id| {
            self.buffers.get(*id).and_then(|ob| ob.buffer.path()).is_some_and(|p| refactor::identity(p) == canonical)
        })
    }

    /// Moves the focused pane's cursor to an LSP `Position` within
    /// whatever buffer is focused *right now* -- callers that need to
    /// land in a *different* file open it first (`open_file_from_
    /// picker`, which this deliberately doesn't do itself, so it stays
    /// usable both for "jump to a different file" and "jump within the
    /// one already focused," e.g. a same-file `gd`).
    pub(super) fn goto_lsp_position(&mut self, position: lsp_types::Position) {
        self.mark_jump();
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        cursor.char_idx = fenix_lsp::position_to_char_offset(buffer.rope(), position).min(buffer.len_chars());
        let (_, sticky) = buffer.line_col(cursor);
        cursor.sticky_col = sticky;
    }

    /// Opens (or switches to) `location`'s file via `open_file_from_
    /// picker` -- reusing it rather than a bespoke "just point the
    /// focused pane at this path" means a `gd` into a live Git or task
    /// session's pane gets the same hijack protection any
    /// other "open this wherever I'm looking" action already has (see
    /// `open_buffer_in_focused_pane`) -- then moves the cursor to its
    /// range's start.
    pub(super) fn goto_lsp_location(&mut self, location: &lsp_types::Location) {
        let Some(path) = fenix_lsp::uri_to_path(&location.uri) else { return };
        self.open_file_as_preview(&path);
        self.goto_lsp_position(location.range.start);
    }

    /// `textDocument/definition`'s answers, merged across servers: a
    /// single location jumps straight there (recorded on the jumplist
    /// first, so `Ctrl-O` gets back); several (an overloaded/ambiguous
    /// declaration, or two servers each knowing a different place)
    /// instead populate the same generalized `quickfix` list `gr` uses, so
    /// `SPC p n`/`SPC p N` step through them exactly the same way
    /// regardless of which populated it.
    pub(super) fn goto_lsp_locations(&mut self, locations: Vec<lsp_types::Location>) {
        match locations.len() {
            0 => self.set_error("no definition found".to_string()),
            1 => {
                let from = JumpEntry { buffer: self.focused_buffer_id(), char_idx: self.cursor().char_idx };
                self.goto_lsp_location(&locations[0]);
                self.record_jump(from);
            }
            _ => self.apply_lsp_references(locations),
        }
    }

    /// Turns `textDocument/references`' merged answers (or `goto_lsp_
    /// locations`' multi-location fallback) into the generalized
    /// `quickfix` list, then jumps to the first entry immediately --
    /// matching real Vim's own `:grep`/tag-list auto-jump-to-first
    /// convention, and `run_grep`'s own precedent in this codebase.
    pub(crate) fn apply_lsp_references(&mut self, locations: Vec<lsp_types::Location>) {
        self.quickfix = locations
            .into_iter()
            .filter_map(|loc| fenix_lsp::uri_to_path(&loc.uri).map(|path| QuickfixEntry::Lsp { path, position: loc.range.start }))
            .collect();
        self.quickfix_index = None;
        // The Search page's results aren't the list any more: it stops
        // following F3 and `]q`.
        self.search.quickfix_page = None;
        if self.quickfix.is_empty() {
            self.set_error("no references found".to_string());
        } else {
            self.quickfix_step(1);
            let n = self.quickfix.len();
            if n > 1 {
                let next = self.key_for("project.quickfix_next").unwrap_or_else(|| "]q".to_string());
                self.set_message(format!("{n} references -- {next} goes to the next"));
            }
        }
    }

    /// Applies a server's `TextEdit`s to `buffer_id`'s content -- each
    /// edit's range refers to the document as it stood *before any of
    /// this batch was applied* (not progressively renumbered edit by
    /// edit, per spec), which is why this applies them back-to-front by
    /// position: an edit earlier in the document never shifts the
    /// position of one later in it as long as the later one is applied
    /// first.
    pub(super) fn apply_lsp_text_edits(&mut self, buffer_id: BufferId, edits: Vec<lsp_types::TextEdit>) {
        let Some(ob) = self.buffers.get_mut(buffer_id) else { return };
        if !ob.kind.tracks_unsaved_changes() { self.set_error("cannot edit a read-only panel"); return; }
        match fenix_lsp::workspace_edit::apply_text_edits(&ob.buffer.text(), &edits) {
            Ok(text) => {
                if text != ob.buffer.text() {
                    replace_text_by_lines(&mut ob.buffer, &mut ob.cursor, &text);
                }
            }
            Err(error) => self.set_error(format!("LSP edits rejected: {error}")),
        }
    }

    /// Every server's completion items so far merged into whatever
    /// completion popup is already open -- see `sync_completion`'s own
    /// call site for how `CompletionKind::Lsp` items reach the same
    /// picker buffer-word/ctags candidates already do. A no-op if the
    /// popup closed (moved on to something else) before this arrived,
    /// or if `buffer` isn't focused any more.
    pub(super) fn apply_lsp_completion(&mut self, buffer: BufferId, prefix_start: usize) {
        if buffer != self.focused_buffer_id() || self.vim.mode() != Mode::Insert || !self.completion_context_valid() {
            return;
        }
        // `PickerState` has no "add candidates to what's already there"
        // method (every other picker in this app is built once, up
        // front, from a source that's already fully known -- this is
        // the first one whose candidate pool can grow *after* it opens,
        // since an LSP response is inherently async relative to the
        // keystroke that triggered it). Rebuilt from scratch instead,
        // combining the always-available local candidates
        // (`completion_candidates`) with every server's items, then
        // re-applying whatever query was already typed -- cheap enough
        // (a handful of candidates, not thousands) that rebuilding isn't
        // worth avoiding.
        let Some((query, current_prefix_start)) = self.completion.as_ref().map(|s| (s.picker.query().to_string(), s.prefix_start)) else {
            return;
        };
        if current_prefix_start != prefix_start {
            return; // superseded by a newer prefix since this request was sent
        }
        let Some(state) = self.lsp_completion.as_ref().filter(|c| c.buffer == buffer && c.prefix_start == prefix_start) else { return };
        // With more than one server answering, each item names its own.
        let servers: std::collections::HashSet<&LspKey> = state.answers.items().map(|(key, _)| key).collect();
        let several = servers.len() > 1;
        let items: Vec<completion::Item> = state
            .answers
            .items()
            .filter_map(|(key, item)| {
                let mut item = completion::Item::lsp(item.clone())?;
                if several {
                    item.server = Some(self.lsp_server_name(key));
                }
                Some(item)
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let selected = self.completion.as_ref().and_then(|s| s.picker.selected())
            .map(|c| (c.payload.label.clone(), c.payload.source));
        let local = if self.completion_local { self.completion_candidates() } else { Vec::new() };
        let candidates = completion::merge(local, items);
        let mut picker = fenix_picker::PickerState::new(candidates);
        picker.set_query(&query);
        if self.completion_selected {
            let row = picker.visible_rows(0, picker.len()).position(|(_, c)| Some((c.payload.label.clone(), c.payload.source)) == selected);
            if let Some(row) = row { picker.move_selection(row as isize); }
        }
        if let Some(state) = &mut self.completion { state.picker = picker; }
    }

    /// Sends one server its part of a fan-out request; one that can't be
    /// written to is counted as having failed, so the others aren't kept
    /// waiting for it.
    pub(super) fn send_lsp_fanout_request<R: lsp_types::request::Request>(&mut self, key: &LspKey, params: R::Params, pending: PendingLspRequest) {
        let Some(session) = self.lsp_sessions.get_mut(key) else { return };
        match session.client.request::<R>(params) {
            Ok(id) => {
                session.pending.insert(id, pending);
            }
            Err(_) => self.lsp_request_failed(key, pending),
        }
    }

    /// `gd`/`gr` sent to every attached server that can answer, the
    /// answers merged once gathered (`lsp_fanout_progress`).
    pub(super) fn request_lsp_locations(&mut self, references: bool) {
        let Some((keys, text_document, position)) = self.focused_lsp_context() else { return };
        let buffer = self.focused_buffer_id();
        let feature = if references { fenix_lsp::fanout::Feature::References } else { fenix_lsp::fanout::Feature::Definition };
        let targets = self.lsp_route(&keys, feature);
        if targets.is_empty() {
            self.set_error(if references { "no language server here finds references" } else { "no language server here finds definitions" });
            return;
        }
        let order = targets.clone();
        let fanout = self.start_lsp_fanout(|deadline| LspFanout::Locations { buffer, references, gather: fenix_lsp::fanout::Gather::new(order, deadline) }, targets.len());
        let at = lsp_types::TextDocumentPositionParams { text_document, position };
        for key in targets {
            if references {
                let params = lsp_types::ReferenceParams {
                    text_document_position: at.clone(),
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                    context: lsp_types::ReferenceContext { include_declaration: true },
                };
                self.send_lsp_fanout_request::<lsp_types::request::References>(&key, params, PendingLspRequest::References { fanout });
            } else {
                let params = lsp_types::GotoDefinitionParams { text_document_position_params: at.clone(), work_done_progress_params: Default::default(), partial_result_params: Default::default() };
                self.send_lsp_fanout_request::<lsp_types::request::GotoDefinition>(&key, params, PendingLspRequest::Definition { fanout });
            }
        }
    }

    /// `SPC c f`/`SPC c F` -- requests `textDocument/formatting` when a
    /// server capable of it is attached; the caller (`format_selection`/
    /// `format_buffer`) falls back to `fenix-format`'s generic bracket-
    /// depth reindenter when this returns `false` (no server, or the
    /// attached one never advertised formatting support), same
    /// graceful-degradation posture as VNC's remote-resize falling back
    /// to client-side scaling. Always requests the *whole document* --
    /// LSP's `rangeFormatting` is a separate, optional capability this
    /// doesn't use yet, so a selection-scoped format still asks for and
    /// gets the whole file reformatted.
    pub(crate) fn request_lsp_format(&mut self) -> bool {
        self.sync_lsp_for_focused_buffer();
        let Some((keys, text_document, _)) = self.focused_lsp_context() else { return false };
        let buffer = self.focused_buffer_id();
        let revision = self.open().buffer.edit_count();
        // The primary formats; an additional server only when the
        // primary can't and it's the one other server that can.
        let Some(key) = self.lsp_route(&keys, fenix_lsp::fanout::Feature::Formatting).pop() else { return false };
        let Some(session) = self.lsp_sessions.get_mut(&key) else { return false };
        let params = lsp_types::DocumentFormattingParams {
            text_document,
            options: lsp_types::FormattingOptions { tab_size: self.vim.indent_width() as u32, insert_spaces: !self.vim.indent_tabs(), ..Default::default() },
            work_done_progress_params: Default::default(),
        };
        match session.client.request::<lsp_types::request::Formatting>(params) {
            Ok(id) => {
                session.pending.insert(id, PendingLspRequest::Format { buffer, revision });
                true
            }
            Err(_) => false,
        }
    }

    /// Sends `textDocument/completion` for the popup's current prefix --
    /// called from `sync_completion`/`force_open_completion` right after
    /// they open/update the popup with local candidates, so the LSP
    /// items (once the response arrives, `apply_lsp_completion`) merge
    /// into whatever's already showing instead of racing to replace it.
    /// A no-op if no server is attached -- the popup still works from
    /// local candidates alone, same as it always has for a language
    /// with none configured.
    ///
    /// Sent to every attached server that completes -- or, when `trigger`
    /// (a character just typed) is set, only to those that declared it a
    /// trigger character. While the popup stays anchored at
    /// `prefix_start`, a server whose last list was complete isn't asked
    /// again: its items are already in `lsp_completion` and the popup
    /// filters them as you type.
    pub(super) fn request_lsp_completion(&mut self, prefix_start: usize, trigger: Option<char>) {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        let Some((keys, text_document, position)) = self.focused_lsp_context() else { return };
        let buffer = self.focused_buffer_id();
        let all = self.lsp_route(&keys, fenix_lsp::fanout::Feature::Completion);
        let targets = match trigger {
            Some(c) => self.lsp_route_with(&keys, |attached| fenix_lsp::fanout::completion_targets(attached, Some(c))),
            None => all.clone(),
        };
        if !self.lsp_completion.as_ref().is_some_and(|c| c.buffer == buffer && c.prefix_start == prefix_start) {
            self.lsp_completion = Some(LspCompletion { buffer, prefix_start, answers: fenix_lsp::fanout::CompletionSet::new(all) });
        }
        let to_ask = self.lsp_completion.as_ref().map(|c| c.answers.to_ask(&targets)).unwrap_or_default();
        // What complete lists already said shows straight away.
        self.apply_lsp_completion(buffer, prefix_start);
        let context = trigger.map(|c| lsp_types::CompletionContext { trigger_kind: lsp_types::CompletionTriggerKind::TRIGGER_CHARACTER, trigger_character: Some(c.to_string()) });
        let generation = self.completion_generation;
        for key in to_ask {
            let params = lsp_types::CompletionParams {
                text_document_position: lsp_types::TextDocumentPositionParams { text_document: text_document.clone(), position },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: context.clone(),
            };
            self.send_lsp_fanout_request::<lsp_types::request::Completion>(&key, params, PendingLspRequest::Completion { buffer, prefix_start, generation });
        }
    }

    /// `SPC c r` -- opens the rename prompt, pre-filled with the
    /// identifier the cursor is in (real editors' own
    /// convention: rename almost always starts from "call this
    /// something else," not a blank field), same capturing-prompt shape
    /// as `start_rename_file_prompt`. A no-op (with a message) if no
    /// server is attached to the focused buffer -- there's nothing to
    /// send a rename request to.
    pub(crate) fn open_lsp_rename_prompt(&mut self) {
        if self.focused_lsp_context().is_none() {
            self.set_error("no language server attached to this buffer".to_string());
            return;
        }
        let cursor = self.cursor();
        let ob = self.open();
        let current = completion::identifier_at_cursor(&ob.buffer, &cursor).unwrap_or_default();
        self.prompt_prefill_selected = self.standard_keys() && !current.is_empty();
        self.lsp_rename_prompt = Some(current);
    }

    /// Routes one keypress to the in-progress `lsp_rename_prompt` --
    /// same shape as `rename_file_prompt_key`, except `Enter` sends the
    /// `textDocument/rename` request rather than applying anything
    /// synchronously (the edit only lands once the server answers).
    pub(super) fn lsp_rename_prompt_key(&mut self, key: KeyPress) {
        if let Some(input) = &mut self.lsp_rename_prompt {
            if take_selected_prefill(input, &mut self.prompt_prefill_selected, key) {
                self.wake_caret();
                return;
            }
        }
        if key == KeyPress::char('v').with_ctrl() {
            let pasted = self.clipboard_text();
            if let (Some(input), Some(text)) = (&mut self.lsp_rename_prompt, pasted) {
                input.push_str(&text);
            }
            self.wake_caret();
            return;
        }
        let Some(input) = &mut self.lsp_rename_prompt else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.lsp_rename_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let new_name = self.lsp_rename_prompt.take().unwrap_or_default();
                self.send_lsp_rename(&new_name);
            }
            KeyCode::Named(FenixNamedKey::Backspace) if key.mods.ctrl || key.mods.alt => delete_word_back(input),
            KeyCode::Named(FenixNamedKey::Backspace) => {
                input.pop();
            }
            KeyCode::Char(c) if key.mods == Mods::default() => input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    pub(super) fn send_lsp_rename(&mut self, new_name: &str) {
        self.sync_lsp_for_focused_buffer();
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return;
        }
        let Some((keys, text_document, position)) = self.focused_lsp_context() else { return };
        let buffer = self.focused_buffer_id();
        // The primary renames; an additional server only when the
        // primary can't and it's the one other server that can.
        let Some(key) = self.lsp_route(&keys, fenix_lsp::fanout::Feature::Rename).pop() else {
            self.set_error("no language server here can rename");
            return;
        };
        let context = self.refactor_context(&key);
        let Some(session) = self.lsp_sessions.get_mut(&key) else { return };
        let params = lsp_types::RenameParams { text_document_position: lsp_types::TextDocumentPositionParams { text_document, position }, new_name: new_name.to_string(), work_done_progress_params: Default::default() };
        if let Ok(id) = session.client.request::<lsp_types::request::Rename>(params) {
            session.pending.insert(id, PendingLspRequest::Rename { buffer, context });
        }
    }

    /// What to show in place of the modeline while `lsp_rename_prompt`
    /// is capturing keys -- same shape as `rename_file_prompt_text`.
    pub(super) fn lsp_rename_prompt_text(&self) -> Option<String> {
        self.lsp_rename_prompt.as_ref().map(|input| format!("rename to: {input}"))
    }
}
