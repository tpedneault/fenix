//! What goes with a prompt: `@` mentions (a file, the selection, the
//! problems, the diff), the agent's `/` commands, pictures pasted in, and
//! the prompts Fenix writes for you from the code (`SPC a e`, `SPC a d`,
//! `SPC a c`).
//!
//! A mention stays in the prompt's text as you typed it; what it names
//! goes along as content blocks: a file's text embedded -- its buffer's,
//! unsaved edits included -- when the agent takes embedded context, else
//! a link it reads itself.

use super::*;

/// What a mention names, beside files.
const SPECIALS: &[(&str, &str)] = &[
    ("selection", "what you had selected (SPC a e)"),
    ("file", "the file beside the panel"),
    ("diagnostics", "the problems the language servers found in open files"),
    ("diff", "the working tree's changes (git diff HEAD)"),
];

/// The most of a mention's text sent along.
const MENTION_BYTES: usize = 256 * 1024;

/// A selection captured for `@selection`.
#[derive(Clone, Debug)]
pub(crate) struct Selection {
    pub(crate) path: Option<PathBuf>,
    pub(crate) line: u32,
    pub(crate) text: String,
}

impl App {
    // -----------------------------------------------------------------
    // The prompt's keys

    /// Keys of the prompt's beyond sending: `@` and `/` open their
    /// pickers (Insert mode), Ctrl-V pastes a picture when there's one.
    pub(super) fn agents_composer_extra_key(&mut self, keypress: KeyPress) -> bool {
        if self.agents_composer_answer(keypress) || self.agents_composer_recall(keypress) {
            return true;
        }
        let insert = self.vim.mode() == Mode::Insert;
        if insert && keypress == KeyPress::char('@') {
            self.agents_composer_insert("@");
            self.agents_pick_mention();
            return true;
        }
        if insert && keypress == KeyPress::char('/') && self.agents.composer.and_then(|c| self.buffers.get(c)).is_some_and(|b| b.buffer.text().trim().is_empty()) {
            self.agents_composer_insert("/");
            self.agents_pick_command();
            return true;
        }
        if keypress == KeyPress::char('v').with_ctrl() && self.agents_paste_image() {
            return true;
        }
        false
    }

    /// The prompt's text.
    fn agents_composer_text(&self) -> String {
        self.agents.composer.and_then(|c| self.buffers.get(c)).map(|b| b.buffer.text()).unwrap_or_default()
    }

    /// Typing in the prompt, or in Normal mode with nothing pending: not
    /// halfway through a Vim command its keys belong to.
    fn agents_composer_calm(&self) -> bool {
        self.vim.mode() == Mode::Insert || self.vim.is_idle()
    }

    /// A question waiting, and the prompt empty: its keys (`y`, `n`, ...)
    /// answer it from here, Insert mode or not. Anything else is typed.
    fn agents_composer_answer(&mut self, keypress: KeyPress) -> bool {
        let KeyCode::Char(c) = keypress.code else { return false };
        if keypress.mods.ctrl || keypress.mods.alt || !self.agents_composer_text().is_empty() || !self.agents_composer_calm() {
            return false;
        }
        let Some(thread) = self.agents.shown(&self.pages) else { return false };
        let answer = self.agents.hub.view(&thread).and_then(|v| agent_page::answer_for(v.thread, crate::page::Key::Char(c)));
        let Some(agent_page::Action::Permission { key, option }) = answer else { return false };
        self.agents_answer(&thread, &key, Some(&option));
        self.agents_dock_titles();
        true
    }

    /// Earlier prompts, newest first: those sent since Fenix started, then
    /// the shown thread's own.
    fn agents_prompt_history(&self) -> Vec<String> {
        let own: Vec<String> = self
            .agents
            .shown(&self.pages)
            .and_then(|t| self.agents.hub.view(&t))
            .map(|v| {
                v.thread
                    .entries
                    .iter()
                    .filter_map(|e| match e {
                        fenix_acp::thread::Entry::User { blocks, .. } => blocks.iter().find_map(|b| match b {
                            ContentBlock::Text(t) => Some(t.trim_end().to_string()),
                            _ => None,
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut history: Vec<String> = Vec::new();
        for prompt in self.agents.sent.iter().rev().chain(own.iter().rev()) {
            if !prompt.trim().is_empty() && !history.contains(prompt) {
                history.push(prompt.clone());
            }
        }
        history
    }

    /// Up in an empty prompt brings back the last one sent, again the one
    /// before; Down the one after, then empty again. While the prompt
    /// holds one brought back, as it was; once it's changed, the arrows
    /// move in it as ever.
    fn agents_composer_recall(&mut self, keypress: KeyPress) -> bool {
        let up = keypress.code == KeyCode::Named(FenixNamedKey::Up);
        if !(up || keypress.code == KeyCode::Named(FenixNamedKey::Down)) || keypress.mods != Mods::default() || !self.agents_composer_calm() {
            return false;
        }
        let text = self.agents_composer_text();
        let at = match &self.agents.recalled {
            Some((at, shown)) if *shown == text => Some(*at),
            _ if text.is_empty() => None,
            _ => return false,
        };
        let history = self.agents_prompt_history();
        let next = match (at, up) {
            (None, false) => return false,
            (None, true) => Some(0),
            (Some(at), true) => Some((at + 1).min(history.len().saturating_sub(1))),
            (Some(0), false) => None,
            (Some(at), false) => Some(at - 1),
        };
        let shown = match next {
            None => {
                self.agents.recalled = None;
                String::new()
            }
            Some(i) => match history.get(i) {
                Some(prompt) => {
                    self.agents.recalled = Some((i, prompt.clone()));
                    prompt.clone()
                }
                None => {
                    self.set_message("no earlier prompt");
                    return true;
                }
            },
        };
        let Some(composer) = self.agents.composer else { return true };
        self.replace_buffer_from_disk(composer, &shown);
        if let Some(pane) = self.agents_panes_showing(composer).first().copied() {
            let end = self.buffers.get(composer).map_or(0, |b| b.buffer.len_chars());
            if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
                state.cursor = Cursor { char_idx: end, sticky_col: 0 };
            }
        }
        self.wake_caret();
        true
    }

    /// Types `text` into the prompt at its cursor.
    pub(super) fn agents_composer_insert(&mut self, text: &str) {
        let Some(composer) = self.agents.composer else { return };
        let Some(pane) = self.agents_panes_showing(composer).first().copied() else { return };
        let at = self.workspaces.active_pane_states().get(&pane).map(|s| s.cursor.char_idx).unwrap_or(0);
        let Some(ob) = self.buffers.get_mut(composer) else { return };
        let at = at.min(ob.buffer.len_chars());
        let mut cursor = Cursor { char_idx: at, sticky_col: 0 };
        ob.buffer.replace_range(&mut cursor, at, at, text);
        let after = at + text.chars().count();
        if let Some(state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
            state.cursor = Cursor { char_idx: after, sticky_col: 0 };
        }
        self.wake_caret();
    }

    /// `@` in the prompt: what to mention.
    pub(super) fn agents_pick_mention(&mut self) {
        let root = self.agents_root();
        let mut candidates: Vec<fenix_picker::Candidate<Pick>> = SPECIALS
            .iter()
            .filter(|(name, _)| *name != "selection" || self.agents.selection.is_some())
            .map(|(name, what)| fenix_picker::Candidate::new(format!("@{name}   {what}"), Pick::Mention(name.to_string())))
            .collect();
        match self.listed_files(&root) {
            Some(files) => candidates.extend(files.iter().map(|c| fenix_picker::Candidate::new(c.label.clone(), Pick::Mention(c.label.replace('\\', "/"))))),
            None => self.prewarm_file_index(&root),
        }
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// `/` at the start of the prompt: the agent's own commands.
    pub(super) fn agents_pick_command(&mut self) {
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        let commands = self.agents.hub.view(&thread).map(|v| v.thread.commands.clone()).unwrap_or_default();
        if commands.is_empty() {
            self.set_message("this agent offers no / commands (yet: they come once it's connected)");
            return;
        }
        let candidates = commands
            .iter()
            .map(|c| {
                let hint = c.hint.as_deref().map(|h| format!(" <{h}>")).unwrap_or_default();
                fenix_picker::Candidate::new(format!("/{}{hint}   {}", c.name, c.description), Pick::Command(c.name.clone()))
            })
            .collect();
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// A picture on the clipboard, pasted into the prompt: it goes with
    /// it as an image, when the agent takes them.
    fn agents_paste_image(&mut self) -> bool {
        let Some(thread) = self.agents.shown(&self.pages) else { return false };
        let takes = self.agents.hub.connection_of(&thread).and_then(|c| self.agents.hub.connection(c)).and_then(|c| c.info.as_ref()).is_some_and(|i| i.prompt_image);
        let Some(image) = self.clipboard.as_mut().and_then(|c| c.get_image().ok()) else { return false };
        if !takes {
            self.set_message("this agent doesn't take pictures");
            return true;
        }
        let Some(png) = super::super::wasm_host::encode_png(image.width as u32, image.height as u32, image.bytes.into_owned()) else { return false };
        self.agents.images.push(ContentBlock::Image { data: base64(&png), mime_type: "image/png".into(), uri: None });
        let n = self.agents.images.len();
        self.agents_composer_insert(&format!("[image {n}]"));
        true
    }

    // -----------------------------------------------------------------
    // From the prompt's text to content blocks

    /// The prompt as blocks: its text, then what its mentions name, then
    /// pictures pasted in.
    pub(super) fn agents_prompt_blocks(&mut self, thread: &ThreadId, text: &str) -> Vec<ContentBlock> {
        let mut blocks = vec![ContentBlock::text(text.to_string())];
        let root = self.agents.hub.meta(thread).map(|m| m.root.clone()).unwrap_or_else(|| self.agents_root());
        let embedded = self.agents.hub.connection_of(thread).and_then(|c| self.agents.hub.connection(c)).and_then(|c| c.info.as_ref()).is_some_and(|i| i.prompt_embedded_context);
        let mut seen = HashSet::new();
        for token in mentions(text) {
            if !seen.insert(token.clone()) {
                continue;
            }
            if let Some(block) = self.agents_mention_block(&root, &token, embedded) {
                blocks.push(block);
            }
        }
        blocks.append(&mut self.agents.images);
        blocks
    }

    fn agents_mention_block(&self, root: &Path, token: &str, embedded: bool) -> Option<ContentBlock> {
        let resource = |uri: String, text: String, name: &str| {
            if embedded {
                ContentBlock::Resource { uri, text: Some(cut(text)), mime_type: Some("text/plain".into()) }
            } else {
                ContentBlock::text(format!("{name}:\n{}", cut(text)))
            }
        };
        match token {
            "selection" => {
                let s = self.agents.selection.as_ref()?;
                let name = s.path.as_ref().map(|p| p.strip_prefix(root).unwrap_or(p).to_string_lossy().into_owned()).unwrap_or_else(|| "selection".into());
                let uri = s.path.as_ref().map(|p| format!("{}#L{}", file_uri(p), s.line)).unwrap_or_else(|| "fenix://selection".into());
                Some(resource(uri, s.text.clone(), &format!("{name}, from line {}", s.line)))
            }
            "file" => {
                let path = self.agents_code_file()?;
                self.agents_file_block(root, &path, embedded)
            }
            "diagnostics" => {
                let mut text = String::new();
                for id in self.buffers.ids_sorted_by_path() {
                    let Some(path) = self.buffers.get(id).and_then(|b| b.buffer.path()).map(Path::to_path_buf) else { continue };
                    if !path.starts_with(root) {
                        continue;
                    }
                    let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
                    for d in self.all_diagnostics(&canonical) {
                        let severity = match d.severity {
                            Some(lsp_types::DiagnosticSeverity::ERROR) => "error",
                            Some(lsp_types::DiagnosticSeverity::WARNING) => "warning",
                            _ => "note",
                        };
                        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                        text.push_str(&format!("{rel}:{}:{}: {severity}: {}\n", d.range.start.line + 1, d.range.start.character + 1, d.message));
                    }
                }
                if text.is_empty() {
                    text.push_str("(no problems in the open files)\n");
                }
                Some(resource("fenix://diagnostics".into(), text, "The problems the language servers found"))
            }
            "diff" => {
                let out = std::process::Command::new("git").args(["diff", "HEAD"]).current_dir(root).output().ok()?;
                Some(resource("fenix://git-diff".into(), String::from_utf8_lossy(&out.stdout).into_owned(), "The working tree's changes"))
            }
            path => {
                let path = root.join(path);
                if !within(root, &path) || !path.is_file() {
                    return None;
                }
                self.agents_file_block(root, &path, embedded)
            }
        }
    }

    fn agents_file_block(&self, root: &Path, path: &Path, embedded: bool) -> Option<ContentBlock> {
        let name = path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned();
        if embedded {
            let text = self.agents_current(path)?;
            Some(ContentBlock::Resource { uri: file_uri(path), text: Some(cut(text)), mime_type: Some("text/plain".into()) })
        } else {
            Some(ContentBlock::ResourceLink { uri: file_uri(path), name, mime_type: None })
        }
    }

    /// `fenix/selection` while the agent's panel has the focus: what was
    /// last sent as `@selection`, as (path, line, text).
    pub(crate) fn agents_selection(&self) -> Option<(Option<String>, u32, String)> {
        let focused = Some(self.focused_buffer_id());
        if focused != self.agents.page && focused != self.agents.composer {
            return None;
        }
        let s = self.agents.selection.as_ref()?;
        Some((s.path.as_ref().map(|p| p.to_string_lossy().into_owned()), s.line, s.text.clone()))
    }

    /// The file in the pane beside the panel.
    fn agents_code_file(&self) -> Option<PathBuf> {
        let pane = self.agents.code_pane.filter(|p| self.windows().windows().contains(p))?;
        let id = *self.windows().content(pane)?;
        self.buffers.get(id)?.buffer.path().map(Path::to_path_buf)
    }

    // -----------------------------------------------------------------
    // Prompts from the code

    /// `SPC a e`: the selection (or the line) goes to the agent's prompt
    /// as `@selection`, for you to say what to do with it.
    pub(crate) fn agents_ask_selection(&mut self) {
        let (here, _) = self.editor_where();
        if here.text.trim().is_empty() {
            self.set_message("select something first (or put the cursor on a line)");
            return;
        }
        self.agents.selection = Some(Selection { path: here.path.map(PathBuf::from), line: here.line, text: here.text });
        let pane = self.focused_pane_id();
        self.agents_open_panel();
        if self.agents.page.is_some() {
            self.agents.code_pane = Some(pane);
            self.agents_focus_composer();
            self.agents_composer_insert("@selection ");
        }
    }

    /// `SPC a d`: the agent is asked to fix the problem on the cursor's
    /// line.
    pub(crate) fn agents_fix_diagnostic(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_message("no file here");
            return;
        };
        let line = {
            let cursor = self.cursor();
            self.open().buffer.line_col(&cursor).0
        };
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone()));
        let diagnostics = self.all_diagnostics(&canonical);
        let Some(d) = diagnostics.iter().find(|d| d.range.start.line as usize <= line && line <= d.range.end.line as usize) else {
            self.set_message("no problem on this line");
            return;
        };
        let pane = self.focused_pane_id();
        let text = format!("Fix this problem, and nothing else:\n\n{}:{}:{}: {}", path.display(), d.range.start.line + 1, d.range.start.character + 1, d.message);
        self.agents_open_panel();
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        self.agents.code_pane = Some(pane);
        let root = self.agents.hub.meta(&thread).map(|m| m.root.clone()).unwrap_or_default();
        let embedded = self.agents.hub.connection_of(&thread).and_then(|c| self.agents.hub.connection(c)).and_then(|c| c.info.as_ref()).is_some_and(|i| i.prompt_embedded_context);
        let mut blocks = vec![ContentBlock::text(text)];
        blocks.extend(self.agents_file_block(&root, &path, embedded));
        let fx = self.agents.hub.prompt(&thread, blocks);
        self.agents_run(fx);
    }

    /// `SPC a c`: a commit message for what's staged, written by the
    /// agent; when its answer is in, it's on the clipboard.
    pub(crate) fn agents_commit_message(&mut self) {
        let root = self.agents_root();
        let diff = std::process::Command::new("git").args(["diff", "--cached"]).current_dir(&root).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        if diff.trim().is_empty() {
            self.set_message("nothing staged to write a commit message for");
            return;
        }
        self.agents_open_panel();
        let Some(thread) = self.agents.shown(&self.pages) else { return };
        let blocks = vec![
            ContentBlock::text("Write a commit message for this staged change: a subject line under 72 characters, a blank line, then a short body saying why. Reply with only the message, nothing before or after it, and don't change any files."),
            ContentBlock::Resource { uri: "fenix://git-diff-cached".into(), text: Some(cut(diff)), mime_type: Some("text/x-diff".into()) },
        ];
        self.agents.commit_message = Some(thread.clone());
        let fx = self.agents.hub.prompt(&thread, blocks);
        self.agents_run(fx);
    }

    /// The commit message's turn ended: its answer to the clipboard.
    pub(super) fn agents_commit_message_done(&mut self, thread: &ThreadId) {
        if self.agents.commit_message.as_ref() != Some(thread) {
            return;
        }
        self.agents.commit_message = None;
        let Some(text) = self.agents.hub.view(thread).and_then(|v| {
            v.thread.entries.iter().rev().find_map(|e| match e {
                fenix_acp::thread::Entry::Agent { text, .. } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            })
        }) else {
            return;
        };
        let message = strip_fence(&text);
        if let Some(clipboard) = &mut self.clipboard {
            let _ = clipboard.set_text(message.clone());
        }
        self.set_message("the agent's commit message is on the clipboard");
    }
}

/// The mentions in a prompt: `@` at the start or after a space, then
/// what follows up to the next space (trailing punctuation left off).
pub(crate) fn mentions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prev = ' ';
    for (i, c) in text.char_indices() {
        if c == '@' && prev.is_whitespace() {
            let rest = &text[i + 1..];
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let token = rest[..end].trim_end_matches(['.', ',', ';', ':', ')', '!', '?']);
            if !token.is_empty() {
                out.push(token.to_string());
            }
        }
        prev = c;
    }
    out
}

fn cut(mut text: String) -> String {
    if text.len() > MENTION_BYTES {
        let mut end = MENTION_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[cut: too long]");
    }
    text
}

fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// An answer wrapped in a code block, unwrapped.
fn strip_fence(text: &str) -> String {
    let t = text.trim();
    if let Some(body) = t.strip_prefix("```") {
        let body = body.split_once('\n').map(|(_, b)| b).unwrap_or("");
        return body.trim_end().trim_end_matches("```").trim().to_string();
    }
    t.to_string()
}

pub(crate) fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_are_words_after_an_at_sign() {
        assert_eq!(mentions("look at @src/a.rs and @selection, not me@there or @ alone. @diff"), vec!["src/a.rs", "selection", "diff"]);
    }

    #[test]
    fn base64_is_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_fenced_answer_is_unwrapped() {
        assert_eq!(strip_fence("```\nFix it\n\nBecause.\n```"), "Fix it\n\nBecause.");
        assert_eq!(strip_fence("Plain"), "Plain");
    }
}
