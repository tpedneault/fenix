//! Agents and extensions (API): the agents extensions bring
//! (`contributes.agents`, `agent-provide`), what they ask one
//! (`agents.ask`), and what they're told when a turn ends
//! (`agent-turn-ended`).

use super::*;
use fenix_acp::hub::AgentSpec;
use fenix_wasm::host::{AgentTurn, Call, Checked, Env, Notice};

/// An ask that isn't answered after this long is answered with an error,
/// and its turn cancelled -- unless it says how long (`timeoutMs`, at most
/// `MAX_ASK_TIMEOUT`).
pub(super) const ASK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The longest an ask may say it waits.
pub(super) const MAX_ASK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// The most commits an attachment's messages list.
const MAX_COMMITS: usize = 100;

/// An extension's question to an agent, waiting for its turn to end.
pub(crate) struct Ask {
    /// The extension that asked, and its service call to answer.
    pub ext: String,
    pub call: u64,
    pub since: Instant,
    /// How long it may take.
    pub timeout: Duration,
    /// The JSON Schema its answer is held to (`answerAs`), and whether it
    /// was asked again already for not fitting it.
    pub schema: Option<Value>,
    pub asked_again: bool,
    /// Its answer told as it grows (`agent-ask-progress`), and what was
    /// told last.
    pub stream: bool,
    pub streamed: String,
    /// No thread kept on `SPC a t`: deleted once it's answered.
    pub hidden: bool,
    /// Its prompt, held until the session is up and in its most
    /// restrictive mode; and whether it was seen attaching.
    pub held: Option<Vec<ContentBlock>>,
    pub attaching: bool,
}

/// An ask that carries code, waiting for you to say the extension may
/// send it to that agent (`agents.ask`'s consent, kept per extension and
/// agent).
pub(crate) struct Consenting {
    pub ext: String,
    pub call: u64,
    pub agent: String,
    pub root: PathBuf,
    pub options: AskOptions,
    pub blocks: Vec<ContentBlock>,
    /// Its question is up (a picker): gone without an answer, it's no.
    pub shown: bool,
}

/// What an extension asks with (`agents.ask`'s payload, 1.2's fields
/// optional).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AskOptions {
    pub prompt: String,
    pub agent: Option<String>,
    pub attach: Vec<Value>,
    pub answer_as: Option<Value>,
    pub stream: bool,
    /// `thread` or `hidden`; none: `agents.ask.show`.
    pub show: Option<String>,
    pub timeout: Option<Duration>,
}

impl AskOptions {
    /// The payload read: `prompt` needed, the rest as given.
    pub(crate) fn parse(payload: &str) -> Result<AskOptions, String> {
        let asked: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
        let Some(prompt) = asked.get("prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()) else {
            return Err("agents.ask: {\"prompt\": \"...\"} -- and \"agent\", to name one".into());
        };
        let show = asked.get("show").and_then(Value::as_str).map(str::to_string);
        if let Some(bad) = show.as_deref().filter(|s| !matches!(*s, "thread" | "hidden")) {
            return Err(format!("agents.ask: show is \"thread\" or \"hidden\", not {bad:?}"));
        }
        let answer_as = asked.get("answerAs").cloned().filter(|v| !v.is_null());
        if answer_as.as_ref().is_some_and(|s| !s.is_object()) {
            return Err("agents.ask: answerAs is a JSON Schema, an object".into());
        }
        let attach = match asked.get("attach") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(list)) => list.clone(),
            Some(_) => return Err("agents.ask: attach is a list".into()),
        };
        Ok(AskOptions {
            prompt: prompt.to_string(),
            agent: asked.get("agent").and_then(Value::as_str).map(str::to_string),
            attach,
            answer_as,
            stream: asked.get("stream").and_then(Value::as_bool).unwrap_or(false),
            show,
            timeout: asked.get("timeoutMs").and_then(Value::as_u64).map(Duration::from_millis),
        })
    }

    /// Whether it sends your code: a diff, a file, the selection.
    pub(crate) fn carries_code(&self) -> bool {
        self.attach.iter().any(|a| matches!(a.get("kind").and_then(Value::as_str), Some("diff" | "file" | "selection")))
    }
}

/// The mode an ask runs in: the agent's most restrictive, when it has
/// one -- one that plans, or reads only.
pub(crate) fn restrictive_mode(modes: &fenix_acp::protocol::Modes) -> Option<String> {
    let says = |m: &fenix_acp::protocol::Mode, word: &str| m.id.to_lowercase().contains(word) || m.name.to_lowercase().contains(word);
    ["read-only", "readonly", "read only", "plan", "ask"].iter().find_map(|word| modes.available.iter().find(|m| says(m, word)).map(|m| m.id.clone()))
}

impl App {
    /// The agents extensions bring, as the catalog takes them: each one an
    /// installed extension's manifest names, with the command line it gave
    /// (none yet: listed, not offered).
    pub(super) fn agents_brought(&self) -> Vec<catalog::Brought> {
        let mut brought = Vec::new();
        for ext in &self.wasm.installed {
            for agent in &ext.manifest.contributes.agents {
                let spec = self.agents.brought.get(&(ext.id.clone(), agent.id.clone())).cloned().unwrap_or_else(|| AgentSpec { id: agent.id.clone(), name: agent.name.clone(), program: String::new(), args: Vec::new(), env: Vec::new() });
                let login = agent.login.clone().or_else(|| Some(format!("set up by the {} extension", ext.name)));
                brought.push(catalog::Brought { spec, login });
            }
        }
        brought
    }

    /// `agent-provide`: one of `ext`'s agents, as the command line that
    /// starts it, or taken away.
    pub(crate) fn agents_provided(&mut self, ext: &str, id: &str, program: Option<(Checked, Env)>) {
        let Some(contribution) = self.wasm.installed.iter().find(|e| e.id == ext).and_then(|e| e.manifest.contributes.agents.iter().find(|a| a.id == id)).cloned() else {
            self.note_output(ext, format!("  warn  agent-provide: {id} isn't one of its agents (contributes.agents)"));
            return;
        };
        let key = (ext.to_string(), id.to_string());
        match program {
            Some(((program, args, _), env)) => {
                self.agents.brought.insert(key, AgentSpec { id: contribution.id, name: contribution.name, program, args, env });
            }
            None => {
                self.agents.brought.remove(&key);
            }
        }
        self.agents_configure();
    }

    /// `agents.ask`, from extension `ext` working in `root`: a thread of
    /// its own on the agent asked for (or `agents.ask.agent`, or the
    /// default one), the prompt sent with what it attaches, and the answer
    /// given when its turn ends. It may read, not change: it runs in the
    /// agent's most restrictive mode, whatever it asks permission for is
    /// refused, and a file it changes anyway is put back. An ask that
    /// sends code asks you first, once per extension and agent.
    pub(crate) fn agents_ask(&mut self, ext: &str, call: u64, payload: &str, root: Option<PathBuf>) {
        let reply = |app: &mut App, answer: Result<String, String>| app.agents_reply(ext, call, answer);
        if !self.wasm.installed.iter().any(|e| e.id == ext && e.manifest.permissions.agents) {
            return reply(self, Err("agents.ask needs agents = true in its permissions".into()));
        }
        let options = match AskOptions::parse(payload) {
            Ok(options) => options,
            Err(e) => return reply(self, Err(e)),
        };
        let entries = self.agents_configure();
        let named = options.agent.clone().or_else(|| self.config.text_value("agents.ask.agent").map(str::trim).filter(|s| !s.is_empty()).map(catalog::slug));
        let agent = match named {
            Some(id) => match entries.iter().find(|e| e.spec.id == id) {
                Some(e) if e.found => Some(id),
                Some(e) => return reply(self, Err(format!("{} isn't found on this machine", e.spec.name))),
                None => return reply(self, Err(format!("no agent {id}"))),
            },
            None => self.agents_default(&entries),
        };
        let Some(agent) = agent else { return reply(self, Err("no coding agent found: SPC a p".into())) };
        let root = root.unwrap_or_else(|| self.agents_root());
        let blocks = match self.agents_ask_blocks(ext, &root, &options) {
            Ok(blocks) => blocks,
            Err(e) => return reply(self, Err(e)),
        };
        if options.carries_code() && !self.agents_ask_consented(ext, &agent) {
            self.agents.consenting.push(Consenting { ext: ext.to_string(), call, agent, root, options, blocks, shown: false });
            self.agents_ask_next_consent();
            return;
        }
        self.agents_ask_start(ext, call, &agent, &root, options, blocks);
    }

    /// The prompt's blocks: the prompt, a line saying who asks and how to
    /// answer, then each attachment as a resource of its own.
    fn agents_ask_blocks(&self, ext: &str, root: &Path, options: &AskOptions) -> Result<Vec<ContentBlock>, String> {
        let name = self.wasm.installed.iter().find(|e| e.id == ext).map(|e| e.name.clone()).unwrap_or_else(|| ext.to_string());
        let mut how = format!("(Asked by the {name} extension in Fenix. Answer in text; don't change any files.)");
        if let Some(schema) = &options.answer_as {
            how = format!("(Asked by the {name} extension in Fenix. Don't change any files. Answer with one JSON block, in ```json fences, matching this JSON Schema:\n{schema})");
        }
        let mut blocks = vec![ContentBlock::text(options.prompt.clone()), ContentBlock::text(how)];
        for attach in &options.attach {
            blocks.push(self.agents_attachment(root, attach)?);
        }
        Ok(blocks)
    }

    /// One attachment, read by Fenix: a diff, commits' messages, a file
    /// with your unsaved edits, the selection, the problems, or text the
    /// extension gives -- as an ACP resource block.
    fn agents_attachment(&self, root: &Path, attach: &Value) -> Result<ContentBlock, String> {
        let resource = |uri: String, text: String, mime: &str| ContentBlock::Resource { uri, text: Some(super::context::cut(text)), mime_type: Some(mime.to_string()) };
        let field = |k: &str| attach.get(k).and_then(Value::as_str).map(str::to_string);
        match attach.get("kind").and_then(Value::as_str).unwrap_or_default() {
            "diff" => {
                let of = field("of").unwrap_or_else(|| "unstaged".into());
                let (uri, diff) = match of.as_str() {
                    "staged" => ("fenix://git/diff/staged".to_string(), fenix_git::file_diff(root, ".", true)),
                    "unstaged" => ("fenix://git/diff/unstaged".to_string(), fenix_git::file_diff(root, ".", false)),
                    "range" => {
                        let (Some(base), Some(head)) = (field("base"), field("head")) else { return Err("agents.ask: a range's diff has a base and a head".into()) };
                        (format!("fenix://git/diff/{base}...{head}"), fenix_git::diff_refs(root, &base, &head, true))
                    }
                    other => return Err(format!("agents.ask: a diff is of staged, unstaged or a range, not {other:?}")),
                };
                Ok(resource(uri, diff.map_err(|e| format!("agents.ask: the diff: {e}"))?, "text/x-diff"))
            }
            "commits" => {
                let range = field("range").ok_or("agents.ask: commits take a range (origin/main..HEAD)")?;
                let (base, head) = range.split_once("..").map(|(b, h)| (b.to_string(), h.trim_start_matches('.').to_string())).ok_or("agents.ask: a range is base..head")?;
                let head = if head.is_empty() { "HEAD".to_string() } else { head };
                let mut text = String::new();
                for commit in fenix_git::commits_between(root, &base, &head, MAX_COMMITS) {
                    let meta = fenix_git::commit_meta(root, &commit.hash);
                    let body = meta.map(|m| m.body).unwrap_or_default();
                    text.push_str(&format!("commit {}\nAuthor: {}\n\n    {}\n", commit.short_hash, commit.author, commit.message));
                    for line in body.lines() {
                        text.push_str(&format!("    {line}\n"));
                    }
                    text.push('\n');
                }
                if text.is_empty() {
                    text.push_str("(no commits in that range)\n");
                }
                Ok(resource(format!("fenix://git/log/{range}"), text, "text/plain"))
            }
            "file" => {
                let rel = field("path").ok_or("agents.ask: a file has a path in the project")?;
                let path = root.join(rel.trim_start_matches("/project/"));
                if !super::within(root, &path) || !path.is_file() {
                    return Err(format!("agents.ask: {rel} isn't a file in the project"));
                }
                let text = self.agents_current(&path).ok_or_else(|| format!("agents.ask: {rel} isn't text"))?;
                Ok(resource(super::context::file_uri(&path), text, "text/plain"))
            }
            "selection" => {
                let (here, _) = self.editor_where();
                if here.text.trim().is_empty() {
                    return Err("agents.ask: nothing is selected".into());
                }
                let uri = here.path.as_ref().map(|p| format!("{}#L{}", super::context::file_uri(Path::new(p)), here.line)).unwrap_or_else(|| "fenix://selection".into());
                Ok(resource(uri, here.text, "text/plain"))
            }
            "diagnostics" => self.agents_mention_block(root, "diagnostics", true).ok_or_else(|| "agents.ask: the problems couldn't be read".into()),
            "text" => {
                let text = field("text").ok_or("agents.ask: text has its text")?;
                let name = field("name").unwrap_or_else(|| "text".into());
                let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' }).collect();
                Ok(resource(format!("fenix://ext/{slug}"), text, &field("mimeType").unwrap_or_else(|| "text/plain".into())))
            }
            other => Err(format!("agents.ask: an attachment is a diff, commits, a file, the selection, diagnostics or text, not {other:?}")),
        }
    }

    /// The ask under way: a thread of its own, its session started, its
    /// prompt held until it's up (`agents_ask_changed`).
    fn agents_ask_start(&mut self, ext: &str, call: u64, agent: &str, root: &Path, options: AskOptions, blocks: Vec<ContentBlock>) {
        let thread = self.agents.hub.new_thread(agent, root);
        let name = self.wasm.installed.iter().find(|e| e.id == ext).map(|e| e.name.clone()).unwrap_or_else(|| ext.to_string());
        if let Some(t) = self.agents.hub.thread_mut(&thread) {
            t.title = Some(format!("{name}: {}", options.prompt.lines().next().unwrap_or_default().chars().take(60).collect::<String>()));
        }
        let hidden = match options.show.as_deref() {
            Some(show) => show == "hidden",
            None => self.config.text_value("agents.ask.show") == Some("hidden"),
        };
        let timeout = options.timeout.unwrap_or(ASK_TIMEOUT).min(MAX_ASK_TIMEOUT);
        self.agents.asks.insert(
            thread.clone(),
            Ask { ext: ext.to_string(), call, since: Instant::now(), timeout, schema: options.answer_as, asked_again: false, stream: options.stream, streamed: String::new(), hidden, held: Some(blocks), attaching: false },
        );
        // Attached without taking the panel's thread from it.
        let before = self.agents.hub.focused().cloned();
        let mut fx = self.agents.hub.focus(&thread);
        match before {
            Some(other) => fx.extend(self.agents.hub.focus(&other)),
            None => self.agents.hub.unfocus(),
        }
        self.agents_run(fx);
        self.agents_ask_changed(&thread);
    }

    /// An asked thread changed: once its session is up, its mode and
    /// model are set and its prompt sent; while it writes, its answer so
    /// far is told (`agent-ask-progress`).
    pub(super) fn agents_ask_changed(&mut self, thread: &ThreadId) {
        let Some(ask) = self.agents.asks.get_mut(thread) else { return };
        let Some(view) = self.agents.hub.view(thread) else { return };
        if ask.held.is_some() {
            match view.link {
                Link::Attaching { .. } => ask.attaching = true,
                Link::Live { .. } => {
                    let mut blocks = ask.held.take().unwrap_or_default();
                    // An agent that doesn't take embedded resources reads
                    // each as text of its own.
                    let embedded = view.connection.and_then(|c| c.info.as_ref()).is_some_and(|i| i.prompt_embedded_context);
                    if !embedded {
                        for block in &mut blocks {
                            if let ContentBlock::Resource { uri, text: Some(text), .. } = block {
                                *block = ContentBlock::text(format!("{uri}:\n{text}"));
                            }
                        }
                    }
                    let mode = view.thread.modes.as_ref().and_then(|m| restrictive_mode(m).filter(|id| *id != m.current));
                    let model = self.config.text_value("agents.ask.model").map(str::trim).filter(|m| !m.is_empty()).and_then(|wanted| {
                        view.thread.config_options.iter().filter(|o| o.id == "model" || o.category.as_deref() == Some("model")).find_map(|o| match &o.value {
                            fenix_acp::protocol::ConfigValue::Select { current, choices } => choices
                                .iter()
                                .find(|c| c.value.eq_ignore_ascii_case(wanted) || c.name.eq_ignore_ascii_case(wanted))
                                .filter(|c| c.value != *current)
                                .map(|c| (o.id.clone(), fenix_acp::protocol::ConfigValue::Select { current: c.value.clone(), choices: choices.clone() })),
                            _ => None,
                        })
                    });
                    let mut fx = Vec::new();
                    if let Some(mode) = mode {
                        fx.extend(self.agents.hub.set_mode(thread, &mode));
                    }
                    if let Some((id, value)) = model {
                        fx.extend(self.agents.hub.set_config_option(thread, &id, value));
                    }
                    fx.extend(self.agents.hub.prompt(thread, blocks));
                    return self.agents_run(fx);
                }
                Link::ReadOnly(why) => {
                    let why = why.clone();
                    return self.agents_ask_failed(thread, format!("the agent can't take the question: {why}"));
                }
                Link::Detached if ask.attaching => {
                    let why = self.agents_last_error(thread).unwrap_or_else(|| "the agent couldn't start a session".into());
                    return self.agents_ask_failed(thread, why);
                }
                Link::Detached => {}
            }
            return;
        }
        if ask.stream {
            let text = view.thread.last_answer();
            if !text.is_empty() && text != ask.streamed {
                ask.streamed = text.clone();
                let (ext, call) = (ask.ext.clone(), ask.call);
                self.code_ask_progress(&ext, call, text);
            }
        }
    }

    /// An ask that can't go on: answered with `why`, its thread let go.
    fn agents_ask_failed(&mut self, thread: &ThreadId, why: String) {
        let Some(ask) = self.agents.asks.remove(thread) else { return };
        self.agents_reply(&ask.ext, ask.call, Err(why));
        self.agents_ask_done_with(thread, ask.hidden);
    }

    /// An ask's thread, done with: its session let go, and the thread
    /// itself when it's hidden.
    fn agents_ask_done_with(&mut self, thread: &ThreadId, hidden: bool) {
        let fx = if hidden { self.agents.hub.delete(thread, Instant::now()) } else { self.agents.hub.close(thread, Instant::now()) };
        self.agents_run(fx);
        if hidden {
            self.agents.baselines.remove(thread);
            self.agents.unknown.remove(thread);
        }
    }

    /// `agents.cancel`: extension `ext`'s ask `call` given up on -- its
    /// turn cancelled, its thread closed, the ask answered with an error.
    /// False when there's no such ask waiting.
    pub(crate) fn agents_ask_cancel(&mut self, ext: &str, call: u64) -> bool {
        if let Some(at) = self.agents.consenting.iter().position(|c| c.ext == ext && c.call == call) {
            self.agents.consenting.remove(at);
            self.agents_reply(ext, call, Err("cancelled".into()));
            return true;
        }
        let Some(thread) = self.agents.asks.iter().find(|(_, a)| a.ext == ext && a.call == call).map(|(t, _)| t.clone()) else { return false };
        let Some(ask) = self.agents.asks.remove(&thread) else { return false };
        self.agents_reply(ext, call, Err("cancelled".into()));
        let fx = self.agents.hub.cancel(&thread, Instant::now());
        self.agents_run(fx);
        self.agents_ask_revert(&thread);
        self.agents_ask_done_with(&thread, ask.hidden);
        true
    }

    /// Whether you said extension `ext` may send code to `agent`.
    fn agents_ask_consented(&self, ext: &str, agent: &str) -> bool {
        self.agents_ask_consents().get(ext).is_some_and(|agents| agents.iter().any(|a| a == agent))
    }

    /// What you said extensions may send code to, by extension: the
    /// agents. Kept in Fenix's state (`ask-consent.json`).
    pub(crate) fn agents_ask_consents(&self) -> BTreeMap<String, Vec<String>> {
        self.agents.consent_file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn agents_ask_save_consents(&mut self, consents: &BTreeMap<String, Vec<String>>) {
        let Some(file) = &self.agents.consent_file else { return };
        if let Some(parent) = file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = fenix_storage::write(file, serde_json::to_string_pretty(consents).unwrap_or_default().as_bytes()) {
            self.set_error(format!("couldn't keep your answer: {e}"));
        }
    }

    /// Extension `ext` may no longer send code to agents without asking
    /// (`None`: every extension): what `agents.forget_ask_consent` does.
    pub(crate) fn agents_ask_forget_consent(&mut self, ext: Option<&str>) {
        let mut consents = self.agents_ask_consents();
        match ext {
            Some(ext) => {
                consents.remove(ext);
            }
            None => consents.clear(),
        }
        self.agents_ask_save_consents(&consents);
        self.set_message(match ext {
            Some(ext) => format!("{ext} asks again before it sends code to an agent"),
            None => "extensions ask again before they send code to an agent".to_string(),
        });
    }

    /// The next ask that carries code, asked about: may it be sent?
    fn agents_ask_next_consent(&mut self) {
        if self.agents.consenting.iter().any(|c| c.shown) {
            return;
        }
        let Some(first) = self.agents.consenting.first_mut() else { return };
        first.shown = true;
        let (ext, call, agent) = (first.ext.clone(), first.call, first.agent.clone());
        let what: Vec<&str> = first
            .options
            .attach
            .iter()
            .filter_map(|a| match a.get("kind").and_then(Value::as_str) {
                Some("diff") => Some("a diff"),
                Some("file") => Some("a file"),
                Some("selection") => Some("your selection"),
                _ => None,
            })
            .collect();
        let name = self.wasm.installed.iter().find(|e| e.id == ext).map(|e| e.name.clone()).unwrap_or_else(|| ext.clone());
        let agent_name = self.agents.hub.agent(&agent).map(|a| a.name.clone()).unwrap_or_else(|| agent.clone());
        let candidates = vec![
            fenix_picker::Candidate::new(format!("The {name} extension wants to send {} to {agent_name}: send it, and don't ask again for {agent_name}", what.join(", ")), Pick::AskConsent { ext: ext.clone(), call, allow: true }),
            fenix_picker::Candidate::new("Don't send it".to_string(), Pick::AskConsent { ext, call, allow: false }),
        ];
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// Your answer to an ask that carries code.
    pub(super) fn agents_ask_consent(&mut self, ext: &str, call: u64, allow: bool) {
        let Some(at) = self.agents.consenting.iter().position(|c| c.ext == ext && c.call == call) else { return };
        let c = self.agents.consenting.remove(at);
        if allow {
            let mut consents = self.agents_ask_consents();
            let agents = consents.entry(c.ext.clone()).or_default();
            if !agents.contains(&c.agent) {
                agents.push(c.agent.clone());
            }
            self.agents_ask_save_consents(&consents);
            self.agents_ask_start(&c.ext, c.call, &c.agent, &c.root, c.options, c.blocks);
        } else {
            self.agents_reply(&c.ext, c.call, Err(format!("you didn't let it send code to {}", c.agent)));
        }
        self.agents_ask_next_consent();
    }

    /// Files the ask's turn changed anyway, put back from its thread's
    /// ledger: their names, under the project.
    pub(in crate::app) fn agents_ask_revert(&mut self, thread: &ThreadId) -> Vec<String> {
        // What git shows changed, too, though no tool call announced it.
        self.agents_turn_ended(thread);
        let mut reverted = Vec::new();
        for file in self.agents_review_files(thread) {
            if matches!(file.baseline, crate::agent_review::Baseline::Unknown) {
                continue;
            }
            self.agents_reject(thread, &file, None);
            let back = match &file.baseline {
                crate::agent_review::Baseline::Text(text) => self.agents_current(&file.path).as_ref() == Some(text),
                _ => !file.path.exists(),
            };
            if back {
                reverted.push(file.name.clone());
            }
        }
        if !reverted.is_empty() {
            self.agents.baselines.remove(thread);
            self.agents_refresh_marks();
            self.agents_count_to_review();
        }
        reverted
    }

    /// A turn ended (`Effect::TurnEnded`): an ask on it answered and its
    /// session let go, and the extensions that listen told.
    pub(super) fn agents_turn_done(&mut self, thread: &ThreadId, stop: &str) {
        let (text, title) = self.agents.hub.view(thread).map(|v| (v.thread.last_answer(), v.thread.display_title())).unwrap_or_default();
        let (agent, root) = self.agents.hub.meta(thread).map(|m| (m.agent.clone(), m.root.clone())).unwrap_or_default();
        if self.agents.asks.contains_key(thread) {
            self.agents_ask_answered(thread, stop, &text, &agent);
        }
        let turn = AgentTurn { agent, thread: thread.0.clone(), title, root: root.to_string_lossy().into_owned(), stop_reason: stop.to_string(), text };
        // What was said goes only to those with `permissions.agents`; the
        // rest hear that a turn ended, and how.
        self.editor_call_each("agents", |ext| {
            let turn = if ext.manifest.permissions.agents { turn.clone() } else { AgentTurn { title: String::new(), text: String::new(), ..turn.clone() } };
            Call::AgentTurnEnded(turn)
        });
    }

    /// An ask's turn ended: its answer -- held to its schema, asked again
    /// once when it doesn't fit -- given, what it changed put back.
    fn agents_ask_answered(&mut self, thread: &ThreadId, stop: &str, text: &str, agent: &str) {
        let Some(ask) = self.agents.asks.get_mut(thread) else { return };
        let mut json = Value::Null;
        if let (Some(schema), "end_turn") = (&ask.schema, stop) {
            let found = super::answer::find_json(text);
            let wrong = match &found {
                Some(value) => super::answer::schema_errors(value, schema),
                None => vec!["there's no JSON block in it".to_string()],
            };
            if wrong.is_empty() {
                json = found.unwrap_or_default();
            } else if !ask.asked_again {
                ask.asked_again = true;
                ask.streamed.clear();
                let again = format!("Your answer doesn't match the schema: {}. Answer again with only one JSON block, in ```json fences, matching it:\n{schema}", wrong.join("; "));
                let fx = self.agents.hub.prompt(thread, vec![ContentBlock::text(again)]);
                return self.agents_run(fx);
            } else {
                let why = format!("the agent's answer doesn't match the schema: {}", wrong.join("; "));
                return self.agents_ask_failed_after(thread, why);
            }
        }
        let Some(ask) = self.agents.asks.remove(thread) else { return };
        let reverted = self.agents_ask_revert(thread);
        let answer = match stop {
            "error" => Err(self.agents_last_error(thread).unwrap_or_else(|| "the agent couldn't answer".into())),
            _ => Ok(json!({ "text": text, "json": json, "stopReason": stop, "agent": agent, "thread": thread.0, "reverted": reverted }).to_string()),
        };
        self.agents_reply(&ask.ext, ask.call, answer);
        self.agents_ask_done_with(thread, ask.hidden);
    }

    /// An ask that ended badly after its turn: what it changed put back
    /// first.
    fn agents_ask_failed_after(&mut self, thread: &ThreadId, why: String) {
        self.agents_ask_revert(thread);
        self.agents_ask_failed(thread, why);
    }

    /// The last error the thread was told of.
    fn agents_last_error(&self, thread: &ThreadId) -> Option<String> {
        let view = self.agents.hub.view(thread)?;
        view.thread.entries.iter().rev().find_map(|e| match e {
            fenix_acp::thread::Entry::Notice(n) if matches!(n.kind, NoticeKind::Error | NoticeKind::Warning | NoticeKind::Interrupted | NoticeKind::Login) => Some(n.text.clone()),
            _ => None,
        })
    }

    /// An asked thread wants a permission, or a login: refused, since
    /// nobody's there to say yes.
    pub(super) fn agents_ask_attention(&mut self, thread: &ThreadId, why: Attention) -> bool {
        if !self.agents.asks.contains_key(thread) {
            return false;
        }
        match why {
            Attention::Permission => {
                let pending: Vec<(String, Option<String>)> = self
                    .agents
                    .hub
                    .view(thread)
                    .map(|v| {
                        v.thread
                            .entries
                            .iter()
                            .filter_map(|e| match e {
                                fenix_acp::thread::Entry::Permission(card) if card.state == PermissionState::Pending => {
                                    let reject = card.options.iter().find(|o| o.kind == PermissionKind::RejectOnce).or_else(|| card.options.iter().find(|o| o.kind == PermissionKind::RejectAlways));
                                    Some((card.key.clone(), reject.map(|o| o.id.clone())))
                                }
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (key, option) in pending {
                    let fx = self.agents.hub.answer_permission(thread, &key, option.as_deref());
                    self.agents_run(fx);
                }
            }
            Attention::Login => self.agents_ask_failed(thread, "the agent wants you to log in first: SPC a p".into()),
            Attention::TurnEnded => {}
        }
        true
    }

    /// An extension's `agents.ask` answered.
    fn agents_reply(&mut self, ext: &str, call: u64, answer: Result<String, String>) {
        #[cfg(test)]
        self.agents.replies.push((ext.to_string(), call, answer.clone()));
        self.code_deliver(ext, Notice::ServiceReply((call, answer)));
    }

    /// Asks waiting too long: answered with an error, their turns
    /// cancelled; an ask whose question about sending code went without
    /// an answer, refused.
    pub(super) fn agents_asks_tick(&mut self, now: Instant) {
        let late: Vec<ThreadId> = self.agents.asks.iter().filter(|(_, a)| now.duration_since(a.since) >= a.timeout).map(|(t, _)| t.clone()).collect();
        for thread in late {
            let Some(ask) = self.agents.asks.remove(&thread) else { continue };
            self.agents_reply(&ask.ext, ask.call, Err("the agent took too long to answer".into()));
            let fx = self.agents.hub.cancel(&thread, now);
            self.agents_run(fx);
            self.agents_ask_revert(&thread);
            self.agents_ask_done_with(&thread, ask.hidden);
        }
        if !matches!(self.active_picker, Some(ActivePicker::Agents(_))) {
            if let Some(c) = self.agents.consenting.iter().find(|c| c.shown) {
                let (ext, call) = (c.ext.clone(), c.call);
                self.agents_ask_consent(&ext, call, false);
            }
        }
    }
}
