//! The PDF reader as a feature (`fenix.pdf`): its commands, `SPC r`, and
//! the state its code reaches by type. That code is still `App` methods
//! in `reader.rs` -- the reader's panes are drawn by core -- and its
//! pages go through the core surface pipeline (`crate::surface`).

use super::*;

/// The reader's commands and its `SPC r` group.
/// How PDFs are shown: `[reader]` in `settings.toml`.
static SETTINGS: [fenix_config::Setting; 5] = {
    use fenix_config::{Category::Reader, Kind, Setting};
    [
        Setting::stored("reader.colors", Reader, "Page colours", Kind::Choice(&["paper", "theme"]), "Paper shows pages as printed. Theme draws them in the current theme's colours. SPC r c switches; open PDFs change at once.").with_default("paper"),
        Setting::stored("reader.zoom", Reader, "Default zoom", Kind::Choice(&["width", "page"]), "How a PDF opens the first time: its pages' width fills the pane, or a whole page fits. After that it opens as you left it.").with_default("width"),
        Setting::stored("reader.page_gap", Reader, "Page gap", Kind::Int { min: 0, max: 40 }, "Pixels between pages.").with_default("12"),
        Setting::stored("reader.remember", Reader, "Remember position", Kind::Bool, "Reopen each PDF where you left it, with its zoom and marks.").with_default("on"),
        Setting::stored("reader.sidebar", Reader, "Sidebar", Kind::Choice(&["auto", "beside", "over"]), "Where o opens the outline: beside the pages, over them, or beside them when the pane is wide enough.").with_default("auto"),
    ]
};

pub(crate) static SPEC: super::feature::FeatureSpec = {
    use super::feature::FeatureCommand as C;
    use super::feature::LeaderEntry::{Bind, Group};
    super::feature::FeatureSpec {
        id: "fenix.pdf",
        commands: &[
            C { id: "pdf.next_page", description: "Turn the focused PDF session to the next page", external: false, run: |app| app.pdf_next_page() },
            C { id: "pdf.prev_page", description: "Turn the focused PDF session to the previous page", external: false, run: |app| app.pdf_prev_page() },
            C { id: "pdf.documents", description: "Open a document from the document index (SPC , Documents)", external: true, run: |app| app.start_document_picker() },
            C { id: "pdf.first_page", description: "Jump the focused PDF session to the first page", external: false, run: |app| app.pdf_first_page() },
            C { id: "pdf.last_page", description: "Jump the focused PDF session to the last page", external: false, run: |app| app.pdf_last_page() },
            C { id: "pdf.goto_page", description: "Prompt for a page number and jump to it", external: false, run: |app| app.start_pdf_goto_page_prompt() },
            C { id: "pdf.zoom_in", description: "Zoom the focused PDF session in", external: false, run: |app| app.pdf_zoom_in() },
            C { id: "pdf.zoom_out", description: "Zoom the focused PDF session out", external: false, run: |app| app.pdf_zoom_out() },
            C { id: "pdf.fit_page", description: "Fit the focused PDF session's page to the pane", external: false, run: |app| app.pdf_zoom_fit_page() },
            C { id: "pdf.fit_width", description: "Fit the focused PDF session's page to the pane's width", external: false, run: |app| app.pdf_zoom_fit_width() },
            C { id: "pdf.toggle_outline", description: "Toggle the focused PDF session's outline/bookmarks panel", external: false, run: |app| app.pdf_toggle_outline() },
            C { id: "pdf.search", description: "Search the focused PDF session's text for a word or phrase", external: false, run: |app| app.start_pdf_search_prompt() },
            C { id: "pdf.headings", description: "Go to one of the PDF's headings, picked by name", external: false, run: |app| app.pdf_pick_heading() },
            C { id: "pdf.colors", description: "Draw PDF pages as printed or in the theme's colours", external: false, run: |app| app.pdf_toggle_colors() },
            C { id: "pdf.copy_link", description: "Copy a link to the PDF page being read", external: false, run: |app| app.pdf_copy_link() },
        ],
        leader: &[
            Group(&['r'], "reader (pdf)"),
            Bind(&['r', 'n'], "next page", "pdf.next_page"),
            Bind(&['r', 'p'], "previous page", "pdf.prev_page"),
            Bind(&['r', '['], "first page", "pdf.first_page"),
            Bind(&['r', ']'], "last page", "pdf.last_page"),
            Bind(&['r', 'g'], "go to page", "pdf.goto_page"),
            Bind(&['r', '='], "zoom in", "pdf.zoom_in"),
            Bind(&['r', '-'], "zoom out", "pdf.zoom_out"),
            // `f` is the document index, not fit-page: picking a reference
            // off the shelf is something you do to *start* reading, from any
            // buffer, while fit-page is a zoom adjustment you make while
            // already in a PDF pane -- where the bare `0` binding sits under
            // your fingers anyway. `SPC r 0` mirrors that bare key.
            Bind(&['r', 'f'], "find document", "pdf.documents"),
            Bind(&['r', '0'], "fit page", "pdf.fit_page"),
            Bind(&['r', 'w'], "fit width", "pdf.fit_width"),
            Bind(&['r', 'o'], "outline sidebar", "pdf.toggle_outline"),
            Bind(&['r', 't'], "go to a heading", "pdf.headings"),
            Bind(&['r', 'c'], "paper / theme colours", "pdf.colors"),
            Bind(&['r', 'y'], "copy a link to this page", "pdf.copy_link"),
            Bind(&['r', '/'], "search", "pdf.search"),
        ],
        local_menu: &[],
        local_context: None,
        language_server: None,
        tasks: None,
        hover_fallback: None,
        completion_at_cursor: None,
        completion_pool_at: None,
        hover_first: None,
        picker_confirm: None,
        picker_cancel: None,
        modeline_clock: None,
        page_action: None,
        page_context: None,
        page_title: None,
        page_after_layout: None,
        on_event: Some(|app, event| {
            if let Ok(response) = event.downcast::<fenix_pdf::PdfResponse>() {
                app.apply_pdf_response(*response);
            }
        }),
        settings: &SETTINGS,
        services: &[],
        contributes: &[],
        // Each open PDF, by its path; reopened in the reader, where it was
        // left coming from the places file.
        session_documents: Some(|app| {
            let mut pdfs: Vec<(BufferId, PathBuf)> = app.features.pdf().docs.iter().map(|(&id, doc)| (id, doc.path.clone())).collect();
            pdfs.sort_by(|a, b| a.1.cmp(&b.1));
            pdfs
        }),
        session_reopen: Some(|app, path| app.pdf_buffer_for(path)),
    }
};

/// The reader's state: open documents, each pane's view of one, the
/// worker that renders them, where each was left, and its prompts. Its
/// code is still `App` methods, so it reaches this by type
/// (`Features::pdf`) and has no instance hooks that run code.
#[derive(Default)]
pub(crate) struct PdfState {
    /// `SPC r g`'s type-a-page-number prompt, when in progress -- same
    /// self-contained capturing-prompt shape as `find_file_prompt`, just
    /// digits-only (see `pdf_goto_page_prompt_key`).
    pub(super) goto_page_prompt: Option<String>,
    /// `SPC r /`'s type-a-query prompt, when in progress -- same self-
    /// contained capturing-prompt shape as `goto_page_prompt`, free
    /// text instead of digits-only (see `pdf_search_prompt_key`).
    pub(super) search_prompt: Option<String>,
    /// Every open PDF document, by its buffer -- see `reader::PdfDoc`.
    pub(super) docs: HashMap<BufferId, super::reader::PdfDoc>,
    /// Each pane's view of a document it shows, by pane and buffer -- see
    /// `reader::PdfView`.
    pub(super) views: HashMap<super::reader::ViewKey, super::reader::PdfView>,
    /// The view read last -- what `SPC r` acts on from outside a PDF pane.
    pub(super) last_view: Option<super::reader::ViewKey>,
    /// The reader's keys: a count or a `g`/`z` waiting for its next key,
    /// and the view it was typed in (a count doesn't follow you to
    /// another pane).
    pub(super) keys: crate::reader::Keys,
    pub(super) keys_view: Option<super::reader::ViewKey>,
    /// Hands out view and request ids, unique across every document.
    pub(super) next_id: u64,
    /// Renders asked for and not back yet: request -> the view and page.
    pub(super) requests: HashMap<u64, (super::reader::ViewKey, u32)>,
    /// Where each PDF was left (`reader.remember`), from the places file
    /// next to the recent files; written when it changed.
    pub(super) places: HashMap<PathBuf, crate::reader::SavedPlace>,
    pub(super) places_path: PathBuf,
    pub(super) places_dirty: bool,
    /// `f`'s link labels, while they show.
    pub(super) hints: Option<super::reader::Hints>,
    /// The one pdfium worker for every document (pdfium can't be called
    /// from two threads), spawned with the first PDF opened.
    pub(super) worker: Option<fenix_pdf::PdfWorker>,
}

#[derive(Default)]
pub(crate) struct PdfHost {
    pub(super) state: PdfState,
}

impl super::feature::Feature for PdfHost {
    fn spec(&self) -> &'static super::feature::FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl super::feature::Features {
    pub(super) fn pdf(&self) -> &PdfState {
        &self.get::<PdfHost>().expect("the reader has no instance hooks that run code, so it's always in place").state
    }

    pub(super) fn pdf_mut(&mut self) -> &mut PdfState {
        &mut self.get_mut::<PdfHost>().expect("the reader has no instance hooks that run code, so it's always in place").state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Through its spec and its state on the host, the way `App` reaches
    // the reader now.

    #[test]
    fn the_reader_contributes_its_commands_and_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        let pdf: Vec<(&str, bool)> = registry.iter().filter(|c| c.id.starts_with("pdf.")).map(|c| (c.id.as_ref(), c.external())).collect();
        assert_eq!(pdf.len(), 15, "{pdf:?}");
        assert!(pdf.contains(&("pdf.documents", true)) && pdf.contains(&("pdf.search", false)));
        let keys = keymap::leader_keys();
        assert_eq!(keys.get("pdf.documents").map(String::as_str), Some("SPC r f"));
        assert_eq!(keys.get("pdf.search").map(String::as_str), Some("SPC r /"));
    }

    #[test]
    fn where_each_pdf_was_left_is_read_into_the_host_at_startup() {
        let app = App::with_file(None);
        let pdf = app.features.pdf();
        assert!(pdf.places_path.to_string_lossy().ends_with("pdf_places.tsv"), "{:?}", pdf.places_path);
        assert!(pdf.docs.is_empty() && pdf.worker.is_none(), "nothing open, no worker yet");
    }
}
