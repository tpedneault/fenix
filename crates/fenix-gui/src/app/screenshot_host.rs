//! `SPC c s`: a picture of the code -- the Visual selection's lines, or
//! the whole buffer -- in its syntax colours and the theme's, framed as
//! `screenshot.rs` draws it, copied to the clipboard and saved as a PNG.
//! `SPC c S` highlights the selected lines instead, with a few lines
//! around them.

use super::*;
use crate::notify::Request;
use crate::screenshot::{self, LongLines, Options, Palette, Style};

/// The most lines a screenshot of a whole buffer takes.
const MAX_LINES: usize = 500;
/// `screenshot.context`'s default: lines around the highlighted ones.
const CONTEXT_LINES: usize = 3;

impl App {
    /// `SPC c s`.
    pub(crate) fn code_screenshot(&mut self) {
        self.screenshot(false);
    }

    /// `SPC c S`: the selected lines highlighted, with
    /// `screenshot.context` lines either side.
    pub(crate) fn code_screenshot_highlighted(&mut self) {
        self.screenshot(true);
    }

    fn screenshot(&mut self, highlight: bool) {
        let id = self.focused_buffer_id();
        if self.open().kind != BufferKind::Text {
            self.set_message("a screenshot is of a file's text");
            return;
        }
        let cursor = self.cursor();
        let buffer = &self.open().buffer;
        let total = buffer.visual_line_count().max(1);
        let selected = (self.vim.mode() == Mode::Visual).then(|| {
            let (a, b) = (self.vim.visual_anchor(), cursor.char_idx);
            let line = |c: usize| buffer.line_col(&Cursor { char_idx: c, sticky_col: 0 }).0;
            (line(a.min(b)), line(a.max(b)).min(total - 1))
        });
        let mut note = None;
        let (first, last, marked) = match selected {
            Some((a, b)) if highlight => {
                let around = self.config.int_value("screenshot.context").map_or(CONTEXT_LINES, |n| n.max(0) as usize);
                (a.saturating_sub(around), (b + around).min(total - 1), Some((a, b)))
            }
            Some((a, b)) => (a, b.min(a + MAX_LINES - 1), None),
            None if highlight => {
                self.set_message("select the lines to highlight first (V)");
                return;
            }
            None => {
                if total > MAX_LINES {
                    note = Some(format!("the first {MAX_LINES} of {total} lines"));
                }
                (0, total.min(MAX_LINES) - 1, None)
            }
        };
        let pane = self.focused_pane_id();
        if let Some(pane_state) = self.workspaces.active_pane_states_mut().get_mut(&pane) {
            self.vim.exit_visual_mode(&pane_state.cursor);
        }

        let lines = self.screenshot_lines(id, first, last);
        let tab_width = self.effective_tab_width();
        let prepared = screenshot::prepare(lines, tab_width);
        if prepared.lines.is_empty() {
            self.set_message("nothing to take a picture of: the lines are blank");
            return;
        }
        let top = first + prepared.dropped_above;
        let marked = marked.map(|(a, b)| (a.saturating_sub(top)..=b.saturating_sub(top)).collect()).unwrap_or_default();
        let options = self.screenshot_options(top, marked);
        let palette = self.screenshot_palette();
        let image = match self.fonts.clone() {
            Some(fonts) => {
                let mut fonts = fonts.borrow_mut();
                let family = fonts.body_family(self.config.font_family.as_deref().or(self.theme.font_family));
                let options = Options { family: Some(family), ..options };
                screenshot::render(&prepared.lines, &palette, fonts.font_system(), &options)
            }
            None => {
                let mut fonts = screenshot::offscreen_fonts();
                let family = self.config.font_family.as_deref().or(self.theme.font_family).filter(|f| text::is_monospace_installed(&fonts, f)).map(str::to_string);
                screenshot::render(&prepared.lines, &palette, &mut fonts, &Options { family, ..options })
            }
        };
        let last_shown = top + prepared.lines.len() - 1;
        self.screenshot_export(&image, top + 1, last_shown + 1, note);
    }

    /// Lines `first..=last` of buffer `id` as runs of their syntax
    /// colours, tabs and all.
    fn screenshot_lines(&mut self, id: BufferId, first: usize, last: usize) -> Vec<screenshot::Line> {
        let highlights = self.syntax_highlights_for_visible_range(id, first, last + 1 - first);
        let fg = self.theme.fg;
        let Some(ob) = self.buffers.get(id) else { return Vec::new() };
        let buffer = &ob.buffer;
        (first..=last)
            .map(|line| {
                let start = buffer.line_start_char(line);
                let text = buffer.text_range(start, start + buffer.line_len(line));
                let text = text.trim_end_matches(['\r', '\n']);
                let byte = buffer.char_to_byte(start);
                split_line_by_highlights(text, byte, byte + text.len(), &highlights, fg).into_iter().map(|(t, c)| (t, [c.r(), c.g(), c.b(), c.a()])).collect()
            })
            .collect()
    }

    /// The settings' options, for lines numbered from `top` (0-based)
    /// with `marked` (indexes into them) highlighted.
    fn screenshot_options(&self, top: usize, marked: Vec<usize>) -> Options {
        let c = &self.config;
        let float = |key: &str| match c.value(key) {
            Some(fenix_config::Value::Float(n)) => Some(*n as f32),
            Some(fenix_config::Value::Int(n)) => Some(*n as f32),
            _ => None,
        };
        let title = c.bool_value("screenshot.title").unwrap_or(true).then(|| self.screenshot_name());
        Options {
            style: c.text_value("screenshot.style").and_then(Style::parse).unwrap_or(Style::Accent),
            title,
            line_numbers: c.bool_value("screenshot.line_numbers").unwrap_or(false),
            first_number: top + 1,
            highlight: marked,
            scale: float("screenshot.scale").unwrap_or(2.0),
            padding: c.int_value("screenshot.padding").map_or(56.0, |n| n as f32),
            font_size: self.logical_font_size(),
            family: None,
            max_columns: c.int_value("screenshot.max_width").map_or(100, |n| n.max(8) as usize),
            long_lines: if c.text_value("screenshot.long_lines") == Some("cut") { LongLines::Cut } else { LongLines::Wrap },
        }
    }

    fn screenshot_palette(&self) -> Palette {
        let t = self.theme;
        let rgba = |c: [f32; 4]| [(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8, 255];
        let text = |c: glyphon::Color| [c.r(), c.g(), c.b(), 255];
        Palette { bg: rgba(t.bg), fg: text(t.fg), gutter: text(t.gutter_fg), accent: rgba(t.caret) }
    }

    /// The focused buffer's file name, or `untitled`.
    fn screenshot_name(&self) -> String {
        self.open().buffer.path().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "untitled".to_string())
    }

    /// Where pictures go: `screenshot.folder`, else the system's Pictures
    /// folder, else Fenix's data folder's `screenshots`.
    pub(crate) fn screenshot_folder(&self) -> PathBuf {
        if let Some(dir) = self.config.text_value("screenshot.folder").filter(|d| !d.trim().is_empty()) {
            return match dir.strip_prefix("~/") {
                Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
                None => PathBuf::from(dir),
            };
        }
        if cfg!(test) {
            return isolated_test_path("screenshots");
        }
        dirs::picture_dir()
            .or_else(|| fenix_storage::paths::Roots::current().map(|r| r.data.join("screenshots")))
            .unwrap_or_else(|| PathBuf::from("screenshots"))
    }

    /// Copies `image` (of lines `from..=to`, 1-based) and saves it, then
    /// says where.
    fn screenshot_export(&mut self, image: &screenshot::Image, from: usize, to: usize, note: Option<String>) {
        let Some(png) = image.png() else {
            self.set_error("couldn't make the picture");
            return;
        };
        let folder = self.screenshot_folder();
        let path = folder.join(screenshot_file_name(&self.screenshot_name(), from, to, chrono::Local::now()));
        if let Err(e) = std::fs::create_dir_all(&folder).and_then(|_| std::fs::write(&path, &png)) {
            self.set_error(format!("couldn't save the picture in {}: {e}", folder.display()));
            return;
        }
        let copied = match self.clipboard.as_mut() {
            Some(clipboard) => clipboard
                .set_image(arboard::ImageData { width: image.width as usize, height: image.height as usize, bytes: image.rgba.as_slice().into() })
                .is_ok(),
            None => false,
        };
        let summary = if copied { "Screenshot copied" } else { "Screenshot saved" };
        let mut body = path.display().to_string();
        if let Some(note) = note {
            body = format!("{note} -- {body}");
        }
        self.notify(
            Request::new("fenix", summary)
                .body(body)
                .tag("screenshot")
                .action("Open the image", format!("reveal:{}", path.display()))
                .action("Open the folder", format!("reveal:{}", folder.display())),
        );
    }
}

/// `fenix-<file>-<from>-<to>-<when>.png`, the file's name kept to what's
/// safe in a file name anywhere.
fn screenshot_file_name(file: &str, from: usize, to: usize, when: chrono::DateTime<chrono::Local>) -> String {
    let file: String = file.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' }).collect();
    let lines = if from == to { from.to_string() } else { format!("{from}-{to}") };
    format!("fenix-{file}-{lines}-{}.png", when.format("%Y%m%d-%H%M%S"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(name: &str, text: &str) -> (PathBuf, App) {
        let dir = isolated_test_path(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.rs");
        std::fs::write(&file, text).unwrap();
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        // Never the real clipboard, nor the real Pictures folder.
        app.clipboard = None;
        let out = dir.join("shots");
        app.config.set_value("screenshot.folder", Some(fenix_config::Value::Text(out.display().to_string())));
        app.config.set_value("screenshot.scale", Some(fenix_config::Value::Float(1.0)));
        (out, app)
    }

    fn saved(out: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(out).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default()
    }

    #[test]
    fn the_selection_is_saved_as_a_png_named_for_its_lines() {
        let (out, mut app) = app_with("shot_selection", "fn main() {\n    let x = 1;\n    let y = 2;\n}\n");
        app.test_vim_key(KeyPress::char('j'));
        app.test_vim_key(KeyPress::char('V'));
        app.test_vim_key(KeyPress::char('j'));
        app.code_screenshot();
        let files = saved(&out);
        assert_eq!(files.len(), 1, "{files:?}");
        let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("fenix-main.rs-2-3-") && name.ends_with(".png"), "{name}");
        let png = std::fs::read(&files[0]).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert!(img.width() > 300 && img.height() > 100);
        assert_eq!(app.vim.mode(), Mode::Normal, "the selection is let go");
        let note = app.notifications.newest().and_then(|id| app.notifications.get(id)).expect("a notification");
        assert_eq!(note.summary, "Screenshot saved");
        assert!(note.actions.iter().any(|a| a.command == format!("reveal:{}", files[0].display())));
    }

    #[test]
    fn with_nothing_selected_the_whole_buffer_is_taken() {
        let (out, mut app) = app_with("shot_buffer", "a\nb\nc\n");
        app.code_screenshot();
        let files = saved(&out);
        assert!(files[0].file_name().unwrap().to_string_lossy().starts_with("fenix-main.rs-1-3-"), "{files:?}");
    }

    #[test]
    fn highlighting_needs_a_selection() {
        let (out, mut app) = app_with("shot_highlight_none", "a\nb\n");
        app.code_screenshot_highlighted();
        assert!(saved(&out).is_empty());
    }

    #[test]
    fn highlighting_takes_the_lines_around_the_selection() {
        let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let (out, mut app) = app_with("shot_highlight", &text);
        for _ in 0..9 {
            app.test_vim_key(KeyPress::char('j'));
        }
        app.test_vim_key(KeyPress::char('V'));
        app.code_screenshot_highlighted();
        let files = saved(&out);
        assert!(files[0].file_name().unwrap().to_string_lossy().starts_with("fenix-main.rs-7-13-"), "{files:?}");
    }

    /// A picture of real code to look at:
    /// `FENIX_SHOT=/some/folder cargo test -p fenix-gui write_a_sample_screenshot -- --ignored`.
    #[test]
    #[ignore]
    fn write_a_sample_screenshot() {
        let code = "use std::collections::HashMap;\n\n/// Counts each word in `text`, ignoring case.\npub fn word_counts(text: &str) -> HashMap<String, usize> {\n    let mut counts = HashMap::new();\n    for word in text.split_whitespace() {\n        *counts.entry(word.to_lowercase()).or_insert(0) += 1;\n    }\n    counts // 42 words, give or take\n}\n";
        let (_, mut app) = app_with("shot_sample", code);
        let out = std::env::var("FENIX_SHOT").expect("FENIX_SHOT, the folder to write it to");
        app.config.set_value("screenshot.folder", Some(fenix_config::Value::Text(out)));
        app.config.set_value("screenshot.scale", Some(fenix_config::Value::Float(2.0)));
        for (key, value) in std::env::vars().filter_map(|(k, v)| Some((k.strip_prefix("FENIX_SHOT_")?.to_lowercase(), v))) {
            let value = match value.as_str() {
                "on" => fenix_config::Value::Bool(true),
                "off" => fenix_config::Value::Bool(false),
                v => v.parse().map(fenix_config::Value::Int).unwrap_or(fenix_config::Value::Text(v.to_string())),
            };
            app.config.set_value(&format!("screenshot.{key}"), Some(value));
        }
        if let Ok(theme) = std::env::var("FENIX_THEME") {
            app.theme = crate::theme::by_name(&theme).expect("a theme");
        }
        for _ in 0..5 {
            app.test_vim_key(KeyPress::char('j'));
        }
        app.test_vim_key(KeyPress::char('V'));
        app.test_vim_key(KeyPress::char('j'));
        app.code_screenshot_highlighted();
        app.code_screenshot();
    }

    #[test]
    fn the_file_name_is_safe_anywhere() {
        let when = chrono::TimeZone::with_ymd_and_hms(&chrono::Local, 2026, 11, 7, 9, 5, 3).unwrap();
        assert_eq!(screenshot_file_name("my file?.rs", 4, 4, when), "fenix-my-file-.rs-4-20261107-090503.png");
        assert_eq!(screenshot_file_name("a.rs", 1, 9, when), "fenix-a.rs-1-9-20261107-090503.png");
    }
}
