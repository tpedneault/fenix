//! Export: a diagram to the clipboard (PNG, SVG, a Markdown block, its
//! source) or to a file (SVG, PNG, a `.mmd` in the project); a note to
//! Markdown that renders anywhere or one self-contained HTML page, as a
//! file or on the clipboard. Exports use `diagrams.export_theme` and
//! `diagrams.background`, `diagrams.export_with = "mmdc"` hands files to
//! mermaid-cli, and every file export is remembered so `E` writes them
//! all again after an edit.

use super::notebook_host::{NotebookPick, NotebookPrompt, NotebookPromptKind};
use super::pages::PageModel;
use super::*;
use fenix_notebook::export::{self, LinkOut};
use fenix_notebook::{ExportRecord, Kind as NbKind, Target};

/// What's exported.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ExportWhat {
    /// The diagram in this buffer.
    Diagram(BufferId),
    /// A note (or day), by entry id.
    Note(String),
}

/// How.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportHow {
    ClipPng,
    ClipSvg,
    ClipMarkdown,
    ClipSource,
    ClipHtml,
    FilePng,
    FileSvg,
    FileMmd,
    FileMarkdown,
    FileHtml,
    /// Every file it was exported to before, again.
    Again,
}

impl ExportHow {
    fn extension(self) -> &'static str {
        match self {
            ExportHow::FilePng => "png",
            ExportHow::FileSvg => "svg",
            ExportHow::FileMmd => "mmd",
            ExportHow::FileMarkdown => "md",
            ExportHow::FileHtml => "html",
            _ => "",
        }
    }
}

/// Base64, for pictures put inside an HTML page.
fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

impl App {
    fn export_theme(&self) -> Option<String> {
        self.config.diagrams_export_theme.clone().filter(|t| !t.trim().is_empty() && !t.eq_ignore_ascii_case("same"))
    }

    fn export_background(&self) -> fenix_diagram::Background {
        match self.config.diagrams_background.as_deref() {
            Some("transparent") => fenix_diagram::Background::Transparent,
            Some("white") => fenix_diagram::Background::White,
            _ => fenix_diagram::Background::Theme,
        }
    }

    /// `e` on the notebook page.
    pub(super) fn notebook_export(&mut self, id: &str) {
        if let Some(path) = id.strip_prefix("file:").map(PathBuf::from) {
            let b = self.buffer_for_path(&path);
            return self.diagram_or_note_menu(b, None);
        }
        let Some(e) = self.notebook.book.as_ref().and_then(|nb| nb.get(id)).cloned() else { return };
        if e.kind == NbKind::Diagram {
            let b = self.buffer_for_path(&e.path);
            self.export_menu(ExportWhat::Diagram(b));
        } else {
            self.notebook_flush(id);
            self.export_menu(ExportWhat::Note(id.to_string()));
        }
    }

    /// A buffer for `path`: the open one, or one opened out of sight.
    fn buffer_for_path(&mut self, path: &Path) -> BufferId {
        match self.buffers.id_for_path(path) {
            Some(b) => b,
            None => {
                let b = self.buffers.open_path(path);
                self.note_disk_state(b);
                b
            }
        }
    }

    fn diagram_or_note_menu(&mut self, buffer: BufferId, note: Option<String>) {
        let is_diagram = self.buffers.get(buffer).and_then(|ob| ob.buffer.path()).and_then(|p| p.extension()).is_some_and(|e| e == "mmd" || e == "mermaid");
        match (is_diagram, note) {
            (true, _) => self.export_menu(ExportWhat::Diagram(buffer)),
            (false, Some(id)) => self.export_menu(ExportWhat::Note(id)),
            (false, None) => self.set_message("export is for diagrams and notebook notes"),
        }
    }

    /// `SPC m e` / `e` on a diagram, `SPC n e` anywhere.
    pub(crate) fn diagram_export_here(&mut self) {
        let focused = self.focused_buffer_id();
        if let Some(PageModel::Diagram(p)) = self.pages.get(&focused).map(|s| &s.model) {
            if let Some(src) = p.source {
                return self.export_menu(ExportWhat::Diagram(src));
            }
        }
        let note = self.open().buffer.path().and_then(|p| self.notebook.book.as_ref().and_then(|nb| nb.by_path(p))).filter(|e| e.kind != NbKind::Diagram).map(|e| e.id.clone());
        if let Some(id) = &note {
            self.notebook_flush(id);
        }
        self.diagram_or_note_menu(focused, note);
    }

    pub(super) fn diagram_export_buffer(&mut self, source: BufferId) {
        self.export_menu(ExportWhat::Diagram(source));
    }

    fn export_menu(&mut self, what: ExportWhat) {
        let again = match &what {
            ExportWhat::Diagram(b) => self.export_records(&what).len().max(usize::from(self.buffers.get(*b).is_none())),
            ExportWhat::Note(_) => self.export_records(&what).len(),
        };
        let mmdc = self.config.diagrams_export_with.as_deref() == Some("mmdc");
        let rows: Vec<(ExportHow, String)> = match &what {
            ExportWhat::Diagram(_) => vec![
                (ExportHow::ClipPng, "clipboard  PNG picture (2x)".into()),
                (ExportHow::ClipSvg, "clipboard  SVG markup".into()),
                (ExportHow::ClipMarkdown, "clipboard  a ```mermaid block, as it renders anywhere".into()),
                (ExportHow::ClipSource, "clipboard  the source".into()),
                (ExportHow::FileSvg, format!("file       SVG...{}", if mmdc { " (mmdc)" } else { "" })),
                (ExportHow::FilePng, format!("file       PNG (2x)...{}", if mmdc { " (mmdc)" } else { "" })),
                (ExportHow::FileMmd, "file       .mmd into the project...".into()),
                (ExportHow::Again, format!("again      everywhere it went ({again})")),
            ],
            ExportWhat::Note(_) => vec![
                (ExportHow::ClipMarkdown, "clipboard  Markdown, as it renders anywhere".into()),
                (ExportHow::ClipHtml, "clipboard  rich text (for Word, Outlook, Jira)".into()),
                (ExportHow::FileMarkdown, "file       Markdown, pictures beside it...".into()),
                (ExportHow::FileHtml, "file       one HTML page, diagrams drawn in...".into()),
                (ExportHow::Again, format!("again      everywhere it went ({again})")),
            ],
        };
        let candidates = rows.into_iter().map(|(how, label)| fenix_picker::Candidate::new(label, NotebookPick::Export { what: what.clone(), how })).collect();
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "EXPORT", picker: fenix_picker::PickerState::new(candidates) });
    }

    fn export_records(&self, what: &ExportWhat) -> Vec<ExportRecord> {
        let nb = self.notebook.book.as_ref();
        let entry = match what {
            ExportWhat::Note(id) => nb.and_then(|nb| nb.get(id)),
            ExportWhat::Diagram(b) => self.buffers.get(*b).and_then(|ob| ob.buffer.path()).and_then(|p| nb.and_then(|nb| nb.by_path(p))),
        };
        entry.map(|e| e.exports.clone()).unwrap_or_default()
    }

    /// The name an export is called by: the entry's, else the file's.
    fn export_name(&self, what: &ExportWhat) -> String {
        match what {
            ExportWhat::Note(id) => self.notebook.book.as_ref().and_then(|nb| nb.get(id)).map(|e| fenix_notebook::slug(&e.name)).unwrap_or_else(|| "note".into()),
            ExportWhat::Diagram(b) => {
                let path = self.buffers.get(*b).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf);
                match path.as_deref().and_then(|p| self.notebook.book.as_ref().and_then(|nb| nb.by_path(p))) {
                    Some(e) => fenix_notebook::slug(&e.name),
                    None => path.and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string())).unwrap_or_else(|| "diagram".into()),
                }
            }
        }
    }

    /// Where a file export goes unless you say otherwise: where it went
    /// last time, else the project's docs, else Downloads.
    fn export_default_path(&self, what: &ExportWhat, how: ExportHow) -> PathBuf {
        let ext = how.extension();
        if let Some(r) = self.export_records(what).into_iter().find(|r| r.path.extension().is_some_and(|e| e == ext)) {
            return r.path;
        }
        let file = format!("{}.{ext}", self.export_name(what));
        let dir = match (&self.project_root, what) {
            (Some(root), ExportWhat::Diagram(_)) => root.join("docs").join("diagrams"),
            (Some(root), ExportWhat::Note(_)) => root.join("docs"),
            (None, _) => dirs::download_dir().unwrap_or_else(|| env::temp_dir()),
        };
        dir.join(file)
    }

    /// A choice in the export menu.
    pub(super) fn export_picked(&mut self, what: ExportWhat, how: ExportHow) {
        match how {
            ExportHow::FilePng | ExportHow::FileSvg | ExportHow::FileMmd | ExportHow::FileMarkdown | ExportHow::FileHtml => {
                let path = self.export_default_path(&what, how);
                self.notebook.prompt = Some(NotebookPrompt { kind: NotebookPromptKind::ExportPath { what, how }, input: path.display().to_string() });
                self.wake_caret();
            }
            ExportHow::Again => {
                let records = self.export_records(&what);
                if records.is_empty() {
                    self.set_message("it hasn't been exported to a file yet");
                    return;
                }
                let mut done = 0;
                for r in &records {
                    let how = match r.format.as_str() {
                        "png" => ExportHow::FilePng,
                        "svg" => ExportHow::FileSvg,
                        "mmd" => ExportHow::FileMmd,
                        "md" => ExportHow::FileMarkdown,
                        _ => ExportHow::FileHtml,
                    };
                    match self.export_file(&what, how, &r.path) {
                        Ok(()) => done += 1,
                        Err(e) => {
                            self.set_error(format!("{}: {e}", r.path.display()));
                            return;
                        }
                    }
                }
                self.set_message(format!("exported again to {done} place{}", if done == 1 { "" } else { "s" }));
            }
            _ => match self.export_clipboard(&what, how) {
                Ok(what) => self.set_message(format!("copied {what}")),
                Err(e) => self.set_error(format!("couldn't export: {e}")),
            },
        }
    }

    /// The diagram's source, made portable with the export theme.
    fn diagram_export_source(&self, buffer: BufferId) -> Result<String, String> {
        let text = self.buffers.get(buffer).map(|ob| ob.buffer.text()).ok_or("the diagram isn't open")?;
        Ok(fenix_diagram::portable(&text, &self.diagram_themes(), self.export_theme().as_deref()))
    }

    fn diagram_export_svg(&self, buffer: BufferId) -> Result<String, String> {
        let text = self.buffers.get(buffer).map(|ob| ob.buffer.text()).ok_or("the diagram isn't open")?;
        let r = fenix_diagram::render(&text, &self.diagram_themes(), self.export_theme().as_deref()).map_err(|d| format!("line {}: {}", d.line + 1, d.message))?;
        Ok(fenix_diagram::svg_with(&r.svg, self.export_background()))
    }

    fn export_clipboard(&mut self, what: &ExportWhat, how: ExportHow) -> Result<&'static str, String> {
        let (text, html, image, label): (Option<String>, Option<(String, String)>, Option<(u32, u32, Vec<u8>)>, &'static str) = match (what, how) {
            (ExportWhat::Diagram(b), ExportHow::ClipPng) => {
                let svg = self.diagram_export_svg(*b)?;
                let (w, h, px) = fenix_diagram::rasterize(&svg, 2.0, fenix_diagram::Background::Theme)?;
                (None, None, Some((w, h, px)), "the picture")
            }
            (ExportWhat::Diagram(b), ExportHow::ClipSvg) => (Some(self.diagram_export_svg(*b)?), None, None, "the SVG"),
            (ExportWhat::Diagram(b), ExportHow::ClipMarkdown) => (Some(format!("```mermaid\n{}\n```\n", self.diagram_export_source(*b)?.trim_end())), None, None, "a ```mermaid block"),
            (ExportWhat::Diagram(b), _) => (self.buffers.get(*b).map(|ob| ob.buffer.text()), None, None, "the source"),
            (ExportWhat::Note(id), ExportHow::ClipHtml) => {
                let (md, html) = self.note_export(id, None)?;
                (None, Some((html, md)), None, "the note as rich text")
            }
            (ExportWhat::Note(id), _) => (Some(self.note_export(id, None)?.0), None, None, "the note as Markdown"),
        };
        let clipboard = self.clipboard.as_mut().ok_or("no clipboard")?;
        if let Some(t) = text {
            clipboard.set_text(t).map_err(|e| e.to_string())?;
        }
        if let Some((html, alt)) = html {
            clipboard.set_html(html, Some(alt)).map_err(|e| e.to_string())?;
        }
        if let Some((w, h, px)) = image {
            clipboard.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: px.into() }).map_err(|e| e.to_string())?;
        }
        Ok(label)
    }

    /// A note as portable Markdown, and as an HTML page. With `files`,
    /// pictures are copied there (for a Markdown file); without, an
    /// HTML page carries them inside it.
    fn note_export(&mut self, id: &str, files: Option<&Path>) -> Result<(String, String), String> {
        let nb = self.notebook.book.as_ref().ok_or("no notebook")?;
        let entry = nb.get(id).cloned().ok_or("no such note")?;
        let themes = self.diagram_themes();
        let over = self.export_theme();
        let out = |l: &fenix_notebook::Link| -> LinkOut {
            match nb.resolve(l, Some(&entry.path)) {
                Target::Entry { id, .. } => match nb.get(&id) {
                    Some(e) if l.embed && e.kind == NbKind::Diagram => LinkOut::Diagram(fenix_diagram::portable(&e.text, &themes, over.as_deref())),
                    Some(e) if l.embed => LinkOut::Note(e.text.clone()),
                    _ => LinkOut::Text,
                },
                Target::Url(u) => LinkOut::Href(u),
                _ => LinkOut::Text,
            }
        };
        let mut md = export::to_markdown(&entry.text, &out);
        // ```mermaid blocks in the note itself, made portable too.
        for (lang, _, src) in fenix_notebook::meta::fenced_blocks(&md.clone()) {
            if lang == "mermaid" {
                let portable = fenix_diagram::portable(&src, &themes, over.as_deref());
                if portable.trim_end() != src.trim_end() {
                    md = md.replacen(&src, portable.trim_end(), 1);
                }
            }
        }
        // Pictures: copied beside a Markdown file, or carried in the page.
        let base = entry.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut inline_md = md.clone();
        let mut at = 0;
        while let Some(start) = md[at..].find("](").map(|i| i + at + 2) {
            let Some(end) = md[start..].find(')').map(|i| i + start) else { break };
            let src = md[start..end].to_string();
            at = end;
            if src.contains("://") || src.is_empty() {
                continue;
            }
            let file = base.join(src.replace("%20", " "));
            let picture = file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).is_some_and(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp"));
            if !picture || !file.is_file() {
                continue;
            }
            if let Some(dir) = files {
                let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                std::fs::copy(&file, dir.join(&name)).map_err(|e| e.to_string())?;
                let rel = format!("{}/{}", dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), name).replace(' ', "%20");
                md = format!("{}{}{}", &md[..start], rel, &md[end..]);
                at = start + rel.len();
            }
            if let Ok(bytes) = std::fs::read(&file) {
                let mime = match file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                    Some("jpg" | "jpeg") => "image/jpeg",
                    Some("gif") => "image/gif",
                    Some("svg") => "image/svg+xml",
                    Some("webp") => "image/webp",
                    _ => "image/png",
                };
                inline_md = inline_md.replace(&format!("]({src})"), &format!("](data:{mime};base64,{})", base64(&bytes)));
            }
        }
        let bg = self.export_background();
        let svg = |src: &str| fenix_diagram::render(src, &themes, over.as_deref()).ok().map(|r| fenix_diagram::svg_with(&r.svg, bg));
        let html = export::to_html(&inline_md, &entry.display_name(), &export::Palette::light(), &svg);
        Ok((md, html))
    }

    /// Writes one file export.
    fn export_file(&mut self, what: &ExportWhat, how: ExportHow, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
        }
        let mmdc = self.config.diagrams_export_with.as_deref() == Some("mmdc");
        let bg = match self.export_background() {
            fenix_diagram::Background::Transparent => "transparent",
            _ => "white",
        };
        let (format, theme) = match (what, how) {
            (ExportWhat::Diagram(b), ExportHow::FileSvg | ExportHow::FilePng) if mmdc => {
                let src = self.diagram_export_source(*b)?;
                fenix_diagram::export_with_mmdc(&src, path, bg, 2.0)?;
                (how.extension(), self.export_theme().unwrap_or_default())
            }
            (ExportWhat::Diagram(b), ExportHow::FileSvg) => {
                let svg = self.diagram_export_svg(*b)?;
                fenix_storage::write(path, svg.as_bytes()).map_err(|e| e.to_string())?;
                ("svg", self.export_theme().unwrap_or_default())
            }
            (ExportWhat::Diagram(b), ExportHow::FilePng) => {
                let svg = self.diagram_export_svg(*b)?;
                let png = fenix_diagram::png(&svg, 2.0, fenix_diagram::Background::Theme)?;
                fenix_storage::write(path, &png).map_err(|e| e.to_string())?;
                ("png", self.export_theme().unwrap_or_default())
            }
            (ExportWhat::Diagram(b), _) => {
                let src = self.diagram_export_source(*b)?;
                fenix_storage::write(path, src.as_bytes()).map_err(|e| e.to_string())?;
                ("mmd", String::new())
            }
            (ExportWhat::Note(id), ExportHow::FileMarkdown) => {
                let files = path.parent().unwrap_or(Path::new(".")).join(format!("{}_files", path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
                let (md, _) = self.note_export(id, Some(&files))?;
                fenix_storage::write(path, md.as_bytes()).map_err(|e| e.to_string())?;
                ("md", String::new())
            }
            (ExportWhat::Note(id), _) => {
                let (_, html) = self.note_export(id, None)?;
                fenix_storage::write(path, html.as_bytes()).map_err(|e| e.to_string())?;
                ("html", String::new())
            }
        };
        // Remembered on the entry, for `E`.
        let entry = match what {
            ExportWhat::Note(id) => Some(id.clone()),
            ExportWhat::Diagram(b) => self.buffers.get(*b).and_then(|ob| ob.buffer.path()).and_then(|p| self.notebook.book.as_ref().and_then(|nb| nb.by_path(p))).map(|e| e.id.clone()),
        };
        if let (Some(id), Some(nb)) = (entry, self.notebook.book.as_mut()) {
            let _ = nb.add_export(&id, ExportRecord { path: path.to_path_buf(), format: format.to_string(), theme });
        }
        Ok(())
    }

    /// The path prompt's Enter.
    pub(super) fn export_to_path(&mut self, what: ExportWhat, how: ExportHow, input: &str) {
        let path = PathBuf::from(input.trim());
        let path = if path.is_absolute() { path } else { self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_default()).join(path) };
        match self.export_file(&what, how, &path) {
            Ok(()) => {
                self.refresh_notebook_pages();
                self.set_message(format!("exported to {} -- E there writes it again after edits", path.display()));
            }
            Err(e) => self.set_error(format!("couldn't export: {e}")),
        }
    }

    // ------------------------------------------------------------------
    // Mermaid in Markdown files, and import
    // ------------------------------------------------------------------

    /// The ```mermaid block the cursor is in: its first and last content
    /// lines and its source.
    pub(super) fn mermaid_block_here(&self) -> Option<(std::ops::Range<usize>, String)> {
        if !self.in_markdown() {
            return None;
        }
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let text = self.open().buffer.text();
        fenix_notebook::meta::fenced_blocks(&text).into_iter().find(|(lang, r, _)| lang == "mermaid" && line + 1 >= r.start && line <= r.end).map(|(_, r, s)| (r, s))
    }

    /// `K` in a ```mermaid block: the reading view beside, on it.
    pub(super) fn mermaid_block_hover(&mut self) -> bool {
        if self.mermaid_block_here().is_none() {
            return false;
        }
        if self.find_page(|m| matches!(m, PageModel::Reading(p) if p.source == self.focused_buffer_id() && !p.replaces)).is_none() {
            self.open_reading_view(false);
        }
        true
    }

    /// `SPC n s`: the ```mermaid block under the cursor, saved to the
    /// notebook as a diagram.
    pub(crate) fn notebook_save_block(&mut self) {
        let Some((_, source)) = self.mermaid_block_here() else {
            self.set_error("SPC n s saves the ```mermaid block the cursor is in");
            return;
        };
        if self.notebook().is_none() {
            return;
        }
        let suggested = self.open().buffer.path().and_then(|p| p.file_stem()).map(|s| format!("{} diagram", s.to_string_lossy())).unwrap_or_default();
        let name = self.notebook.book.as_ref().map(|nb| nb.free_name(&suggested)).unwrap_or(suggested);
        self.notebook.prompt = Some(NotebookPrompt { kind: NotebookPromptKind::SaveBlock { text: format!("{}\n", source.trim_end()) }, input: name });
        self.wake_caret();
    }

    /// `SPC i D`: a notebook diagram into this buffer, as a block or as an
    /// exported SVG and a link to it.
    pub(crate) fn notebook_insert_diagram(&mut self) {
        if !self.open().kind.tracks_unsaved_changes() {
            self.set_error("diagrams go into a document");
            return;
        }
        let Some(nb) = self.notebook() else { return };
        let mut candidates = Vec::new();
        for e in nb.recent().into_iter().filter(|e| e.kind == NbKind::Diagram) {
            candidates.push(fenix_picker::Candidate::new(format!("block  {}", e.name), NotebookPick::InsertDiagram { id: e.id.clone(), svg: false }));
            candidates.push(fenix_picker::Candidate::new(format!("SVG    {}  (exported beside, linked)", e.name), NotebookPick::InsertDiagram { id: e.id.clone(), svg: true }));
        }
        if candidates.is_empty() {
            self.set_message("no diagrams in the notebook yet -- SPC n d makes one");
            return;
        }
        self.completion = None;
        self.enter_picker(ActivePicker::Notebook { title: "DIAGRAM", picker: fenix_picker::PickerState::new(candidates) });
    }

    pub(super) fn insert_diagram_picked(&mut self, id: &str, svg: bool) {
        let Some(e) = self.notebook.book.as_ref().and_then(|nb| nb.get(id)).cloned() else { return };
        let target = self.focused_buffer_id();
        if !svg {
            let src = fenix_diagram::portable(&e.text, &self.diagram_themes(), self.export_theme().as_deref());
            self.notebook_insert_text(&format!("```mermaid\n{}\n```\n", src.trim_end()));
            return;
        }
        let here = self.open().buffer.path().and_then(Path::parent).map(Path::to_path_buf).or_else(|| self.project_root.clone()).unwrap_or_else(|| env::temp_dir());
        let file = here.join(format!("{}.svg", fenix_notebook::slug(&e.name)));
        let b = self.buffer_for_path(&e.path);
        match self.export_file(&ExportWhat::Diagram(b), ExportHow::FileSvg, &file) {
            Ok(()) => {
                // Back to the buffer the link goes into.
                if self.focused_buffer_id() != target {
                    self.open_buffer_in_focused_pane(target);
                }
                self.notebook_insert_text(&format!("![{}]({})", e.name, file.file_name().unwrap_or_default().to_string_lossy().replace(' ', "%20")));
                self.set_message(format!("exported {} -- E on the diagram writes it again", file.display()));
            }
            Err(err) => self.set_error(format!("couldn't export: {err}")),
        }
    }

    /// `SPC n i`: a `.md` or `.mmd` file copied into the notebook.
    pub(crate) fn notebook_import(&mut self) {
        if self.notebook().is_none() {
            return;
        }
        let start = self.open().buffer.path().map(|p| p.display().to_string()).unwrap_or_default();
        self.notebook.prompt = Some(NotebookPrompt { kind: NotebookPromptKind::Import, input: start });
        self.wake_caret();
    }

    pub(super) fn notebook_import_path(&mut self, input: &str) {
        let path = PathBuf::from(input.trim());
        let Some(nb) = self.notebook() else { return };
        match nb.import(&path) {
            Ok(id) => {
                let name = nb.get(&id).map(|e| e.name.clone()).unwrap_or_default();
                self.refresh_notebook_pages();
                self.open_notebook_entry(&id);
                self.set_message(format!("imported as {name}"));
            }
            Err(e) => self.set_error(format!("couldn't import {}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::with_file(None);
        app.config.notebook_folder = Some(dir.path().join("nb"));
        app.agenda_path = dir.path().join("agenda.json");
        (dir, app)
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }

    #[test]
    fn diagrams_export_to_files_and_again() {
        let (dir, mut app) = app();
        app.notebook_create_diagram("TC flow", "flowchart TD\n    a --> b\n");
        let b = app.buffers.id_for_path(&app.notebook.book.as_ref().unwrap().by_name("TC flow").unwrap().path.clone()).unwrap();
        let what = ExportWhat::Diagram(b);
        let svg = dir.path().join("out").join("flow.svg");
        app.export_to_path(what.clone(), ExportHow::FileSvg, &svg.display().to_string());
        assert!(std::fs::read_to_string(&svg).unwrap().starts_with("<svg"));
        let png = dir.path().join("out").join("flow.png");
        app.export_to_path(what.clone(), ExportHow::FilePng, &png.display().to_string());
        assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));
        let mmd = dir.path().join("out").join("flow.mmd");
        app.export_to_path(what.clone(), ExportHow::FileMmd, &mmd.display().to_string());
        let portable = std::fs::read_to_string(&mmd).unwrap();
        assert!(portable.starts_with("---\nconfig:\n  theme: base\n"), "fenix is written out: {portable}");
        // Remembered, newest first; E writes them all again.
        let records = app.export_records(&what);
        assert_eq!(records.iter().map(|r| r.format.as_str()).collect::<Vec<_>>(), vec!["mmd", "png", "svg"]);
        std::fs::remove_file(&svg).unwrap();
        app.export_picked(what.clone(), ExportHow::Again);
        assert!(svg.exists());
        // The default path is where it went last.
        assert_eq!(app.export_default_path(&what, ExportHow::FileSvg), svg);
    }

    #[test]
    fn notes_export_to_markdown_and_html_with_diagrams() {
        let (dir, mut app) = app();
        app.notebook_create_diagram("TC flow", "flowchart TD\n    a --> b\n");
        app.notebook_create_note("Bench", "# Bench\n\nSee [[TC flow]].\n\n![[TC flow]]\n");
        let id = app.notebook.book.as_ref().unwrap().by_name("Bench").unwrap().id.clone();
        let md = dir.path().join("out").join("bench.md");
        app.export_to_path(ExportWhat::Note(id.clone()), ExportHow::FileMarkdown, &md.display().to_string());
        let text = std::fs::read_to_string(&md).unwrap();
        assert!(text.contains("See TC flow.") && text.contains("```mermaid\n---\nconfig:\n  theme: base"), "{text}");
        let html = dir.path().join("out").join("bench.html");
        app.export_to_path(ExportWhat::Note(id), ExportHow::FileHtml, &html.display().to_string());
        let page = std::fs::read_to_string(&html).unwrap();
        assert!(page.contains("<figure class=\"diagram\"><svg"), "the diagram drawn in");
    }

    #[test]
    fn a_mermaid_block_is_saved_to_the_notebook() {
        let (dir, mut app) = app();
        let file = dir.path().join("README.md");
        std::fs::write(&file, "# Readme\n\n```mermaid\nflowchart LR\n  a --> b\n```\n").unwrap();
        app.open_file_from_picker(&file);
        app.notebook_place_cursor(4, 0);
        app.notebook_save_block();
        let Some(NotebookPrompt { kind: NotebookPromptKind::SaveBlock { text }, input }) = app.notebook.prompt.clone() else { panic!("the name prompt") };
        assert_eq!(text, "flowchart LR\n  a --> b\n");
        assert_eq!(input, "README diagram");
        app.notebook.prompt = None;
        app.notebook_create_diagram(&input, &text);
        assert_eq!(app.notebook.book.as_ref().unwrap().by_name("README diagram").unwrap().kind, NbKind::Diagram);
    }
}
