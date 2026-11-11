//! `.editorconfig` in the editor (`fenix_config::editorconfig` reads it):
//! the focused file's indent (spaces or tabs, how wide) and tab width,
//! applied as you move between files; and on every save, trailing
//! whitespace trimmed, line endings made one kind, the final newline
//! added and the charset used, as the file's sections say. `SPC f c`
//! shows what applies to the focused file, and from which files.
//!
//! The order, highest first: the project's `.fenix/settings.toml`, then
//! `.editorconfig`, then your `settings.toml`, then Fenix's defaults
//! (`effective_indent_width`, `effective_tab_width`).

use super::*;
use fenix_config::editorconfig::{self, Charset, IndentStyle, Properties};

impl App {
    /// What `.editorconfig` says of `path`: read once, until a
    /// `.editorconfig` is saved here or the disk poll's tick; nothing for
    /// a file on another machine.
    pub(super) fn editorconfig_of(&mut self, path: &Path) -> Properties {
        if fenix_vfs::is_remote(path) {
            return Properties::default();
        }
        if let Some(props) = self.editorconfig_cache.get(path) {
            return props.clone();
        }
        let props = editorconfig::properties(path);
        self.editorconfig_cache.insert(path.to_path_buf(), props.clone());
        props
    }

    /// The focused file's, as last applied.
    pub(super) fn focused_editorconfig(&self) -> Option<&Properties> {
        let path = self.open().buffer.path()?;
        self.editorconfig_cache.get(path)
    }

    /// Makes the editor indent the focused file as its `.editorconfig`
    /// says: called on every focus change, and when one is edited.
    pub(super) fn apply_editorconfig(&mut self) {
        if let Some(path) = self.open().buffer.path().map(Path::to_path_buf) {
            self.editorconfig_of(&path);
        }
        // A project that says how wide an indent is says it for spaces.
        let project_says = self.project_settings.as_ref().is_some_and(|(_, p)| p.get("editor.indent_width").is_some());
        let tabs = !project_says && self.focused_editorconfig().is_some_and(|p| p.indent_style == Some(IndentStyle::Tab));
        self.vim.set_indent_tabs(tabs);
        self.apply_setting("editor.indent_width");
    }

    /// `.editorconfig`'s indent size for the focused file, when it says.
    pub(super) fn editorconfig_indent_width(&self) -> Option<usize> {
        self.focused_editorconfig().and_then(|p| p.indent_size)
    }

    /// `.editorconfig`'s tab width for the focused file, when it says.
    pub(super) fn editorconfig_tab_width(&self) -> Option<usize> {
        self.focused_editorconfig().and_then(|p| p.tab_width)
    }

    /// Forgets what was read: a `.editorconfig` changed.
    pub(super) fn editorconfig_changed(&mut self) {
        self.editorconfig_cache.clear();
        self.apply_editorconfig();
    }

    /// Before buffer `id` is written: its `.editorconfig`'s whitespace,
    /// line endings and charset, made as one undo step. `keep_line`: a
    /// line left untrimmed (the cursor's, for an auto-save mid-typing).
    pub(super) fn editorconfig_before_save(&mut self, id: BufferId, keep_line: Option<usize>) {
        let Some(path) = self.buffers.get(id).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else { return };
        let props = self.editorconfig_of(&path);
        if props.is_empty() {
            return;
        }
        let Some(ob) = self.buffers.get_mut(id) else { return };
        if let Some(charset) = props.charset {
            let encoding = match charset {
                Charset::Utf8 => fenix_core::TextEncoding::Utf8,
                Charset::Utf8Bom => fenix_core::TextEncoding::Utf8Bom,
                Charset::Utf16Le => fenix_core::TextEncoding::Utf16Le,
                Charset::Utf16Be => fenix_core::TextEncoding::Utf16Be,
                // As the web reads `latin1`: Windows-1252.
                Charset::Latin1 => fenix_core::TextEncoding::choices().into_iter().find(|e| e.label() == "windows-1252").unwrap_or_default(),
            };
            ob.buffer.set_encoding(encoding);
        }
        let edits = editorconfig::save_edits(&ob.buffer.text(), &props, keep_line);
        if edits.is_empty() {
            return;
        }
        let replacements: Vec<(std::ops::Range<usize>, String)> = edits.iter().map(|e| (e.start..e.end, e.text.clone())).collect();
        let mut scratch = Cursor::at_start();
        ob.buffer.replace_ranges(&mut scratch, &replacements);
        let len = ob.buffer.len_chars();
        // Each pane's cursor moves with what changed before it.
        let moved = |at: usize| {
            let mut out = at;
            for e in &edits {
                let new_len = e.text.chars().count();
                if e.end <= at {
                    out = out + new_len - (e.end - e.start);
                } else if e.start < at {
                    out = out - (at - e.start) + new_len.min(at - e.start);
                }
            }
            out.min(len)
        };
        let panes: Vec<fenix_window::WindowId> = self.windows().windows().into_iter().filter(|&p| self.windows().content(p) == Some(&id)).collect();
        for pane in panes {
            let ps = self.pane_state_mut(pane);
            ps.cursor.char_idx = moved(ps.cursor.char_idx);
        }
    }

    /// `SPC f c`: what `.editorconfig` says of the focused file, and
    /// where that comes from.
    pub(crate) fn cmd_show_editorconfig(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("this buffer isn't a file -- .editorconfig applies to files");
            return;
        };
        self.editorconfig_cache.remove(&path);
        let props = self.editorconfig_of(&path);
        self.apply_editorconfig();
        if props.is_empty() {
            self.set_message(format!("no .editorconfig says anything of {}", self.shown_path(&path)));
            return;
        }
        let from: Vec<String> = props.sources.iter().map(|p| self.shown_path(p)).collect();
        let mut said = props.describe().join(", ");
        if self.project_settings.as_ref().is_some_and(|(_, p)| p.get("editor.indent_width").is_some() || p.get("editor.tab_width").is_some()) {
            said.push_str(" -- .fenix/settings.toml's indent wins");
        }
        self.set_message(format!("{said} (from {})", from.join(", ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // A project: `.fenix/settings.toml` needs one.
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    /// The focused file's `.editorconfig` sets the indent; your setting is
    /// below it and the project's `.fenix/settings.toml` above it.
    #[test]
    fn editorconfig_sits_between_yours_and_the_project_s() {
        let dir = project(&[
            (".editorconfig", "root = true\n[*]\nindent_style = space\nindent_size = 2\n[*.go]\nindent_style = tab\ntab_width = 6\n"),
            ("a.py", "x\n"),
            ("b.go", "package b\n"),
        ]);
        let mut app = App::with_file(Some(dir.path().join("a.py").display().to_string()));
        app.config.indent_width = Some(8);
        app.refresh_project_root();
        assert_eq!(app.vim.indent_width(), 2, ".editorconfig over yours");
        assert!(!app.vim.indent_tabs());
        app.open_file_from_picker(&dir.path().join("b.go"));
        assert!(app.vim.indent_tabs(), "the Go section's tabs");
        assert_eq!((app.vim.indent_width(), app.effective_tab_width()), (2, 6), "[*]'s size stays; the Go section's tab width");
        // The project's own file wins.
        std::fs::create_dir_all(dir.path().join(".fenix")).unwrap();
        std::fs::write(dir.path().join(".fenix").join("settings.toml"), "[editor]\nindent_width = 3\n").unwrap();
        app.refresh_project_settings(true);
        app.editorconfig_changed();
        assert_eq!(app.vim.indent_width(), 3);
        assert!(!app.vim.indent_tabs(), "the project said how wide, in spaces");
    }

    /// A save trims, ends lines and adds the final newline as the file's
    /// section says, in one undo step.
    #[test]
    fn a_save_follows_the_file_s_section() {
        let dir = project(&[(".editorconfig", "root = true\n[*.txt]\ntrim_trailing_whitespace = true\ninsert_final_newline = true\nend_of_line = crlf\n"), ("a.txt", "one  \ntwo\t")]);
        let path = dir.path().join("a.txt");
        let mut app = App::with_file(Some(path.display().to_string()));
        app.refresh_project_root();
        app.focused_buffer_and_cursor_mut().1.char_idx = 9;
        app.save_inner(false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\r\ntwo\r\n");
        assert!(app.cursor().char_idx <= app.open().buffer.len_chars());
        let id = app.focused_buffer_id();
        let mut c = Cursor::at_start();
        app.buffers.get_mut(id).unwrap().buffer.undo(&mut c);
        assert_eq!(app.open().buffer.text(), "one  \ntwo\t", "one step undoes it");
    }
}
