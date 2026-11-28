//! Folders and files outside the project an agent may use too
//! (`SPC a w`): a docs checkout beside the code, a log you want looked
//! at, the other half of a pair of repositories. Each project has its
//! own list, kept with your projects' state (never in its `.fenix`: they
//! are your machine's paths). A folder goes to every session started or
//! taken back afterwards as `additionalDirectories`, when the agent takes
//! them; a file or a folder's files are what the agent may read and
//! write through Fenix, and what `@` may name, beside the project's.

use std::path::{Path, PathBuf};

use super::*;

/// The prompt line asking for a folder or file to add.
pub(crate) const PATHS_PROMPT: &str = "fenix:agent-paths";
/// The prompt line asking for a picture to send.
pub(crate) const IMAGE_PROMPT: &str = "fenix:agent-image";

/// The largest picture that goes with a prompt, in bytes.
const MAX_IMAGE: u64 = 20 * 1024 * 1024;

/// The kind of picture `path` is, by its name: what agents take.
pub(crate) fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The newest picture in the folders screenshots are usually saved to:
/// what "send a screenshot" most likely means.
fn newest_screenshot() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let pictures = dirs::picture_dir().unwrap_or_else(|| home.join("Pictures"));
    let folders = [pictures.join("Screenshots"), pictures.clone(), dirs::desktop_dir().unwrap_or_else(|| home.join("Desktop")), dirs::download_dir().unwrap_or_else(|| home.join("Downloads"))];
    folders
        .iter()
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|p| p.is_file() && image_mime(p).is_some())
        .filter_map(|p| std::fs::metadata(&p).and_then(|m| m.modified()).ok().map(|t| (t, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

impl App {
    /// The folders and files outside `root` its agents may use.
    pub(super) fn agents_extra_paths(&self, root: &Path) -> Vec<PathBuf> {
        self.project_meta.agent_paths(root).to_vec()
    }

    /// Whether an agent working in `root` may read or write `path`: in
    /// the project, one of its extra files, or in one of its extra
    /// folders.
    pub(super) fn agents_may_use(&self, root: &Path, path: &Path) -> bool {
        within(root, path) || self.agents_extra_paths(root).iter().any(|extra| extra == path || (extra.is_dir() && within(extra, path)))
    }

    /// Every project's extra folders, handed to the agents' sessions.
    pub(super) fn agents_sync_paths(&mut self) {
        let lists: Vec<(PathBuf, Vec<PathBuf>)> = self.project_meta.all_agent_paths().map(|(root, paths)| (root.clone(), paths.iter().filter(|p| p.is_dir()).cloned().collect())).collect();
        for (root, dirs) in lists {
            self.agents.hub.set_directories(&root, dirs);
        }
    }

    /// `SPC a w`: what the agents of this project may use outside it --
    /// one more added, or one taken away.
    pub(crate) fn agents_pick_paths(&mut self) {
        let root = self.agents_project_root();
        let mut candidates = vec![fenix_picker::Candidate::new("Let the agent use a folder or file outside the project…".to_string(), Pick::AddPath)];
        for path in self.agents_extra_paths(&root) {
            let kind = if path.is_dir() { "folder" } else if path.is_file() { "file" } else { "gone" };
            candidates.push(fenix_picker::Candidate::new(format!("{}   {kind} · Enter takes it away", path.display()), Pick::RemovePath(path)));
        }
        self.enter_picker(ActivePicker::Agents(fenix_picker::PickerState::new(candidates)));
    }

    /// Asks on the prompt line for the folder or file to add.
    pub(super) fn agents_add_path_prompt(&mut self) {
        let start = dirs::home_dir().map(|h| format!("{}{}", h.display(), std::path::MAIN_SEPARATOR)).unwrap_or_default();
        self.wasm.prompt = Some(wasm_host::CodePrompt { ext: PATHS_PROMPT.into(), id: 0, label: "Folder or file the agent may use too".into(), input: start, masked: false });
        self.wake_caret();
    }

    /// The folder or file typed on the prompt line: added to the
    /// project's list, if it's there and outside the project.
    pub(crate) fn agents_path_typed(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        let root = self.agents_project_root();
        let typed = fenix_fs::expand(input);
        let Ok(path) = std::fs::canonicalize(&typed).map(fenix_lsp::normalize) else {
            return self.set_error(format!("{} isn't there", typed.display()));
        };
        if within(&root, &path) {
            return self.set_message(format!("{} is in the project: the agent may use it already", path.display()));
        }
        if !self.project_meta.add_agent_path(&root, path.clone()) {
            return self.set_message(format!("{} is on the list already", path.display()));
        }
        if let Err(err) = self.project_meta.save() {
            self.set_error(format!("couldn't keep the list: {err}"));
        }
        self.agents_sync_paths();
        if path.is_dir() {
            self.set_message(format!("the agent may use {} too, from its next session (SPC a n)", path.display()));
        } else {
            self.set_message(format!("the agent may use {} too: @{} in the prompt sends it", path.display(), path.display()));
        }
    }

    /// `path` taken off the project's list.
    pub(super) fn agents_remove_path(&mut self, path: &Path) {
        let root = self.agents_project_root();
        self.project_meta.remove_agent_path(&root, path);
        if let Err(err) = self.project_meta.save() {
            self.set_error(format!("couldn't keep the list: {err}"));
        }
        // An emptied list is forgotten by the sessions to come too.
        let dirs: Vec<PathBuf> = self.agents_extra_paths(&root).into_iter().filter(|p| p.is_dir()).collect();
        self.agents.hub.set_directories(&root, dirs);
        self.set_message(format!("the agent no longer uses {}", path.display()));
    }

    /// `SPC a I`: a picture from a file -- a screenshot saved to disk --
    /// with the next prompt. The prompt line starts on the newest one in
    /// the screenshots folders: Enter sends it.
    pub(crate) fn agents_attach_image_prompt(&mut self) {
        if self.agents.shown(&self.pages).is_none() {
            self.agents_open_panel();
        }
        let start = newest_screenshot().map(|p| p.display().to_string()).or_else(|| dirs::home_dir().map(|h| format!("{}{}", h.display(), std::path::MAIN_SEPARATOR))).unwrap_or_default();
        self.wasm.prompt = Some(wasm_host::CodePrompt { ext: IMAGE_PROMPT.into(), id: 0, label: "Picture to send with the prompt".into(), input: start, masked: false });
        self.wake_caret();
    }

    /// The picture typed on the prompt line, or dropped on the window.
    pub(crate) fn agents_image_typed(&mut self, input: &str) {
        let input = input.trim();
        if !input.is_empty() {
            let path = fenix_fs::expand(input);
            if let Err(why) = self.agents_attach_image(&path) {
                self.set_error(why);
            }
        }
    }

    /// Picture file `path` goes with the next prompt, as `[image n]` in
    /// it, when the agent shown takes pictures.
    pub(crate) fn agents_attach_image(&mut self, path: &Path) -> Result<(), String> {
        let mime = image_mime(path).ok_or_else(|| format!("{} isn't a picture (PNG, JPEG, GIF or WebP)", path.display()))?;
        let thread = self.agents.shown(&self.pages).ok_or("no thread to send it with: SPC a a")?;
        let takes = self.agents.hub.connection_of(&thread).and_then(|c| self.agents.hub.connection(c)).and_then(|c| c.info.as_ref()).is_some_and(|i| i.prompt_image);
        if !takes {
            return Err("this agent doesn't take pictures".into());
        }
        let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if size > MAX_IMAGE {
            return Err(format!("{} is too big to send ({} MB; at most {} MB)", path.display(), size / (1024 * 1024), MAX_IMAGE / (1024 * 1024)));
        }
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.agents.images.push(ContentBlock::Image { data: super::context::base64(&bytes), mime_type: mime.into(), uri: Some(super::context::file_uri(path)) });
        let n = self.agents.images.len();
        self.agents_focus_composer();
        self.agents_composer_insert(&format!("[image {n}] "));
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.set_message(format!("{name} goes with the prompt as [image {n}]"));
        Ok(())
    }
}
