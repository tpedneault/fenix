//! Tcl, as a `Feature`: a language pack. Built-in hover (`K` with no
//! language server), completion (keywords, `ctags` definitions, the
//! `[completion] symbols_file`, an ensemble's subcommands, a command's
//! `-options`), which command words highlighting may color as calls, the
//! `SPC c s` symbol picker and `SPC m T` tag refresh. The Tcl grammar
//! itself stays compiled into `fenix-syntax` for 2.0.
//!
//! The caches and the background `ctags` scan live on `TclHost`; the
//! stateless lookups are functions on `SPEC`.

use std::any::Any;

use super::feature::{
    CompletionAt, Feature, FeatureCommand, FeatureEvent, FeatureSpec, LeaderEntry,
};
use super::*;

/// Tcl's commands, its `SPC m` entries and its language lookups.
pub(crate) static SPEC: FeatureSpec = FeatureSpec {
    id: "fenix.tcl",
    commands: &[
        FeatureCommand {
            id: "completion.refresh_tags",
            description: "Refresh Tcl completion tags (re-scans the project with ctags)",
            external: false,
            run: |app| {
                app.with_feature::<TclHost, _>(|tcl, app| tcl.refresh_tags(app));
            },
        },
        FeatureCommand {
            id: "code.symbols",
            description: "Fuzzy-find a Tcl definition by its fully-qualified name and jump to it",
            external: false,
            run: |app| {
                app.with_feature::<TclHost, _>(|tcl, app| tcl.picker_symbols(app));
            },
        },
    ],
    leader: &[],
    local_menu: &[
        LeaderEntry::Bind(&['s'], "symbols", "code.symbols"),
        LeaderEntry::Bind(&['T'], "refresh tags", "completion.refresh_tags"),
    ],
    local_context: Some(local_context),
    language_server: None,
    tasks: None,
    hover_fallback: Some(builtin_hover),
    completion_at_cursor: Some(option_completion),
    completion_pool_at: Some(subcommand_candidates),
    hover_first: None,
    picker_confirm: None,
    picker_cancel: None,
    modeline_clock: None,
    page_action: None,
    page_context: None,
    page_title: None,
    page_after_layout: None,
    on_event: None,
    settings: &[],
};

/// A Tcl file gets the Tcl `SPC m` menu (which the MIB fills too).
fn local_context(app: &App) -> Option<crate::keymap::LocalContext> {
    let language = app
        .open()
        .buffer
        .path()
        .and_then(fenix_syntax::detect_language_from_path);
    (language == Some(fenix_syntax::LanguageId::Tcl)).then_some(crate::keymap::LocalContext::Tcl)
}

/// What `K` shows in a Tcl buffer with no language server: Tcl's own
/// usage line for the built-in the cursor is on (or is inside an
/// argument of -- `K` on the `$s` in `string compare $s $t` still names
/// `string compare`), plus the subcommand list for an ensemble. `None`
/// on a user proc, a variable, whitespace, or in a non-Tcl buffer -- see
/// `tcl::hover_text`.
fn builtin_hover(app: &App) -> Option<String> {
    if app.focused_language() != Some(fenix_syntax::LanguageId::Tcl) {
        return None;
    }
    let cursor = app.cursor();
    let buffer = &app.open().buffer;
    let (line, col) = buffer.line_col(&cursor);
    let text = buffer.line(line).to_string();
    let (word_start, word_end) = fenix_completion::tcl::word_at(&text, col)?;
    let chars: Vec<char> = text.chars().collect();
    let before: String = chars[..word_start].iter().collect();
    let word: String = chars[word_start..word_end].iter().collect();
    let words = fenix_completion::tcl::command_words_before(&before)?;
    fenix_completion::tcl::hover_text(&words, &word)
}

/// Subcommand candidates for the Tcl ensemble the cursor is inside,
/// when `at` (the completion prefix's start) sits right after its
/// leading words: `string |` and `string co|` both get `string`'s
/// subcommands, `binary encode |` gets `hex`/`base64`/`uuencode`,
/// `string is |` gets the character classes. Empty for a non-Tcl
/// buffer, for a position that isn't right after a command's leading
/// words, or for a command that has no subcommands (`puts |`). Each
/// candidate's detail is Tcl's own usage line for it. Keyed as
/// `Keyword` -- the same "this is the language's own vocabulary" bucket
/// the command names themselves use.
fn subcommand_candidates(app: &App, at: usize) -> Vec<fenix_picker::Candidate<completion::Item>> {
    if app.focused_language() != Some(fenix_syntax::LanguageId::Tcl) {
        return Vec::new();
    }
    let before = app.line_before(at);
    let Some(words) = fenix_completion::tcl::command_words_before(&before) else {
        return Vec::new();
    };
    if words.is_empty() {
        return Vec::new();
    }
    fenix_completion::tcl::subcommands(&words.join(" "))
        .into_iter()
        .map(|(name, usage)| {
            let mut item = completion::Item::text(name.to_string(), completion::Source::Keyword);
            item.detail = usage.to_string();
            fenix_picker::Candidate::new(name.to_string(), item)
        })
        .collect()
}

/// The `-flag` completion in progress at the cursor, if the cursor is
/// at the end of a dash-word inside a Tcl command that takes options:
/// `lsort -|` and `lsort -uni|` both give `(char offset of the dash,
/// "-"/"-uni", lsort's options)`, and `string compare -nocase -|` gives
/// `string compare`'s (see `tcl::options_at`). `None` when there's no
/// dash-word at the cursor, or the command (`set x -`, `expr {$a -`) has
/// no options for it to be -- so an arithmetic minus never opens a
/// popup. Same `Keyword`/detail shape as `subcommand_candidates`.
fn option_completion(app: &App) -> Option<CompletionAt> {
    if app.focused_language() != Some(fenix_syntax::LanguageId::Tcl) {
        return None;
    }
    let cursor = app.cursor();
    let before = app.line_before(cursor.char_idx);
    let (dash_col, flag) = fenix_completion::tcl::option_prefix(&before)?;
    let words = fenix_completion::tcl::command_words_before(&before[..before.len() - flag.len()])?;
    let options = fenix_completion::tcl::options_at(&words);
    if options.is_empty() {
        return None;
    }
    let line_start = cursor.char_idx - before.chars().count();
    let candidates = options
        .into_iter()
        .map(|(name, usage)| {
            let mut item = completion::Item::text(name.to_string(), completion::Source::Keyword);
            item.detail = usage.to_string();
            fenix_picker::Candidate::new(name.to_string(), item)
        })
        .collect();
    Some((line_start + dash_col, flag.to_string(), candidates))
}

/// What a background `ctags` scan reports.
#[derive(Debug)]
struct TagsScanned {
    scan: u64,
    root: PathBuf,
    tags: Vec<fenix_completion::ctags::TagEntry>,
}

/// See `TclHost::tags_scan`.
pub(super) struct TclTagsScan {
    pub(super) id: u64,
    pub(super) root: PathBuf,
    /// A manual `SPC m T` refresh, which reports its count when it lands.
    pub(super) report: bool,
}

/// Everything Tcl keeps between calls.
#[derive(Default)]
pub(crate) struct TclHost {
    /// One cached `(project_root, candidates)` pair for Tcl completion,
    /// rebuilt only when the current buffer's project root differs from
    /// what's cached -- not a map of every project root ever visited
    /// this session (disclosed simplification: matches the common
    /// single-active-project case, see the plan's Scope). Cleared by
    /// `SPC m T` to force a fresh `ctags` scan.
    pub(super) candidates_cache: Option<(
        Option<PathBuf>,
        Vec<fenix_picker::Candidate<fenix_completion::CompletionItem>>,
    )>,
    /// The raw `ctags`-shelled Tcl tag list `candidates` derives its own
    /// `Tag`-kind `CompletionItem`s from (see `tags`) -- a separate
    /// cache because completion only ever needs the name, but the
    /// `SPC c s` symbol picker also needs the file/line that conversion
    /// throws away. Same staleness rule and `SPC m T` reset as
    /// `candidates_cache`.
    pub(super) tags_cache: Option<(Option<PathBuf>, Vec<fenix_completion::ctags::TagEntry>)>,
    /// The `ctags` scan running in the background, if any -- a big
    /// project takes seconds to scan, so `tags` never waits on it: it
    /// answers from whatever `tags_cache` holds (nothing yet, for a
    /// newly opened project) and the result lands through `on_event`,
    /// which refills both caches and redraws so highlighting and
    /// completion pick it up.
    pub(super) tags_scan: Option<TclTagsScan>,
    next_tags_scan: u64,
}

impl TclHost {
    /// The Tcl completion candidate pool (keywords + `ctags`-sourced
    /// definitions + any entries from `config.completion_symbols_file`,
    /// deduped against each other), rebuilding only when `root` differs
    /// from what's cached -- `ctags::run` shells a real subprocess, so
    /// this is worth avoiding on every keystroke. The symbols file is
    /// re-read on the same cadence, not independently -- both sources
    /// share one manual refresh (`refresh_tags`/`SPC m T`). Cloning the
    /// cached `Vec` on a cache hit is cheap: dozens to low hundreds of
    /// small entries. Also rebuilt when a background `ctags` scan lands
    /// (`apply_scanned` drops this cache), since a pool built while the
    /// scan was still running has no tags in it.
    pub(super) fn candidates(
        &mut self,
        app: &mut App,
        root: Option<&Path>,
    ) -> Vec<fenix_picker::Candidate<fenix_completion::CompletionItem>> {
        let stale = match &self.candidates_cache {
            Some((cached_root, _)) => cached_root.as_deref() != root,
            None => true,
        };
        if stale {
            let mut seen = std::collections::HashSet::new();
            let mut candidates = Vec::new();
            for &keyword in fenix_completion::tcl::KEYWORDS {
                if seen.insert(keyword.to_string()) {
                    let item = fenix_completion::CompletionItem {
                        label: keyword.to_string(),
                        kind: fenix_completion::CompletionKind::Keyword,
                        detail: fenix_completion::tcl::signature(keyword)
                            .unwrap_or_default()
                            .to_string(),
                    };
                    candidates.push(fenix_picker::Candidate::new(item.label.clone(), item));
                }
            }
            for tag in self.tags(app, root) {
                if seen.insert(tag.name.clone()) {
                    let item = fenix_completion::CompletionItem {
                        label: tag.name,
                        kind: fenix_completion::CompletionKind::Tag,
                        detail: String::new(),
                    };
                    candidates.push(fenix_picker::Candidate::new(item.label.clone(), item));
                }
            }
            if let Some(symbols_file) = &app.config.completion_symbols_file {
                for item in fenix_completion::custom::load(symbols_file) {
                    if seen.insert(item.label.clone()) {
                        candidates.push(fenix_picker::Candidate::new(item.label.clone(), item));
                    }
                }
            }
            self.candidates_cache = Some((root.map(PathBuf::from), candidates));
        }
        self.candidates_cache
            .as_ref()
            .expect("just populated above if it was missing")
            .1
            .clone()
    }

    /// The raw `ctags`-shelled Tcl tag list (name/file/line preserved)
    /// for `root` -- same cache shape as `candidates`, which this backs
    /// alongside the `SPC c s` symbol picker (`picker_symbols`), which
    /// needs the location `candidates`'s `CompletionItem` conversion
    /// throws away. `None` root (no known project) always yields an
    /// empty list without shelling anything.
    ///
    /// Never blocks on `ctags`: when nothing is cached for `root` yet,
    /// this starts a background scan (once -- not again while it's in
    /// flight) and answers with an empty list in the meantime; the scan
    /// landing refills the caches and redraws.
    fn tags(
        &mut self,
        app: &mut App,
        root: Option<&Path>,
    ) -> Vec<fenix_completion::ctags::TagEntry> {
        let cached = |cache: &Option<(Option<PathBuf>, Vec<fenix_completion::ctags::TagEntry>)>| {
            cache
                .as_ref()
                .is_some_and(|(cached_root, _)| cached_root.as_deref() == root)
        };
        if !cached(&self.tags_cache) {
            match root {
                None => self.tags_cache = Some((None, Vec::new())),
                Some(root) => self.start_tags_scan(app, root, false),
            }
        }
        match &self.tags_cache {
            Some((cached_root, tags)) if cached_root.as_deref() == root => tags.clone(),
            _ => Vec::new(),
        }
    }

    /// Scans `root` with `ctags` off the UI thread, reporting back as a
    /// `FeatureEvent`. A scan of the same root already in flight is left
    /// to finish, unless this is a manual refresh (`report`), which
    /// starts over so it sees files saved since.
    fn start_tags_scan(&mut self, app: &mut App, root: &Path, report: bool) {
        if let Some(scan) = &self.tags_scan {
            if scan.root == root && !report {
                return;
            }
        }
        self.next_tags_scan += 1;
        let id = self.next_tags_scan;
        let root = root.to_path_buf();
        self.tags_scan = Some(TclTagsScan {
            id,
            root: root.clone(),
            report,
        });
        match app.event_proxy.clone() {
            Some(proxy) => {
                std::thread::spawn(move || {
                    let tags = fenix_completion::ctags::run(&root, "Tcl");
                    // The label the hang log knew it by as a `FenixUserEvent` variant.
                    let event = FeatureEvent {
                        feature: SPEC.id,
                        label: "TclTagsScanned",
                        payload: Box::new(TagsScanned {
                            scan: id,
                            root,
                            tags,
                        }),
                    };
                    let _ = proxy.send_event(FenixUserEvent::Feature(event));
                });
            }
            None => {
                // No event loop to report back through (every test) --
                // scan and apply inline, same posture as `explorer_navigate`.
                let tags = fenix_completion::ctags::run(&root, "Tcl");
                self.apply_scanned(app, id, root, tags);
            }
        }
    }

    /// A `ctags` scan landed: swaps in the new tags, unless a newer scan
    /// has superseded this one, and drops the derived completion pool so
    /// the next frame's highlighting and completion rebuild from them.
    pub(super) fn apply_scanned(
        &mut self,
        app: &mut App,
        id: u64,
        root: PathBuf,
        tags: Vec<fenix_completion::ctags::TagEntry>,
    ) {
        if self.tags_scan.as_ref().is_none_or(|scan| scan.id != id) {
            return;
        }
        let scan = self.tags_scan.take().expect("checked just above");
        let count = tags.len();
        self.tags_cache = Some((Some(root.clone()), tags));
        self.candidates_cache = None;
        if scan.report {
            app.set_message(format!(
                "refreshed Tcl completion tags for {} -- {count} definition(s) found",
                root.display()
            ));
        }
    }

    /// `SPC c s` -- fuzzy-find any known Tcl definition (proc/namespace)
    /// by its fully-qualified name and jump straight to where it's
    /// defined. Sourced from `tags`, so it shares that cache (and
    /// `SPC m T`'s manual refresh) with the Insert-mode completion
    /// popup's own `Tag`-kind entries -- opening this picker never
    /// re-shells `ctags` on its own.
    pub(super) fn picker_symbols(&mut self, app: &mut App) {
        let root = app.project_root.clone();
        let candidates = self
            .tags(app, root.as_deref())
            .into_iter()
            .map(|tag| fenix_picker::Candidate::new(tag.name.clone(), tag))
            .collect();
        app.enter_picker(ActivePicker::Symbol(fenix_picker::PickerState::new(
            candidates,
        )));
    }

    /// `SPC m T` -- clears the cached Tcl completion candidates *and*
    /// eagerly re-shells `ctags` right away in the background (rather
    /// than leaving it to the next completion to trigger lazily), so
    /// pressing this key has a visible result: a message reporting how
    /// many definitions were found once the scan lands, or, if there's
    /// no known project root at all, that there was nothing to scan.
    /// The previous tags keep serving completion and highlighting until
    /// the new ones arrive. `ctags::run` itself already reports *why* a
    /// scan came back empty (missing binary, non-zero exit, its stderr,
    /// ...) -- this is the "did the refresh actually do anything"
    /// summary on top of that. Manual only (no save-hook/file-watcher
    /// refresh) -- see the plan's Scope.
    pub(super) fn refresh_tags(&mut self, app: &mut App) {
        self.candidates_cache = None;
        match app.project_root.clone() {
            Some(root) => {
                if app.event_proxy.is_some() {
                    app.set_message(format!(
                        "scanning {} for Tcl definitions...",
                        root.display()
                    ));
                }
                self.start_tags_scan(app, &root, true);
            }
            None => {
                app.set_error(
                    "no project root detected for the focused buffer -- open a file inside a recognized project \
                     (a `.git`/`Cargo.toml`/etc. marker somewhere above it) first",
                );
            }
        }
    }
}

impl Feature for TclHost {
    fn spec(&self) -> &'static FeatureSpec {
        &SPEC
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn on_event(&mut self, app: &mut App, payload: Box<dyn Any + Send>) {
        let Ok(scanned) = payload.downcast::<TagsScanned>() else {
            return;
        };
        let TagsScanned { scan, root, tags } = *scanned;
        self.apply_scanned(app, scan, root, tags);
    }

    /// The Tcl pool, for a Tcl buffer.
    fn completion_items(
        &mut self,
        app: &mut App,
    ) -> Vec<fenix_picker::Candidate<completion::Item>> {
        if app.focused_language() != Some(fenix_syntax::LanguageId::Tcl) {
            return Vec::new();
        }
        let root = app.project_root.clone();
        self.candidates(app, root.as_deref())
            .into_iter()
            .map(|c| fenix_picker::Candidate::new(c.label, completion::Item::from(c.payload)))
            .collect()
    }

    /// `tcl.scm`'s own `(command name: (_) @function)` rule captures
    /// *every* word in command position, since a tree-sitter query has
    /// no way to know whether it's a real command -- only Fenix's own
    /// known-symbols set (built-in keywords, ctags, the external symbols
    /// file: the exact same three sources `candidates` already merges
    /// for completion) can validate that.
    fn known_commands(
        &mut self,
        app: &mut App,
        buffer: BufferId,
    ) -> Option<std::collections::HashSet<String>> {
        let is_tcl = app.buffers.get(buffer).is_some_and(|ob| {
            ob.buffer
                .path()
                .and_then(|p| p.extension())
                .and_then(|e| e.to_str())
                .and_then(fenix_syntax::detect_language)
                == Some(fenix_syntax::LanguageId::Tcl)
        });
        if !is_tcl {
            return None;
        }
        let root = app.project_root.clone();
        let mut known: std::collections::HashSet<String> = self
            .candidates(app, root.as_deref())
            .into_iter()
            .map(|c| c.payload.label)
            .collect();
        // Plus whatever this file itself defines. Without it, a call to a
        // proc defined twenty lines up renders as plain body text
        // whenever `ctags` has nothing to say -- which is every
        // single-file script (no project root, so `tags` never runs) and
        // every proc added since the last index.
        if let Some(ob) = app.buffers.get(buffer) {
            known.extend(fenix_completion::tcl::procs_defined_in(&ob.buffer.text()));
        }
        Some(known)
    }
}

/// Test-only ways in, under the names the tests have always used.
#[cfg(test)]
impl App {
    pub(super) fn tcl(&self) -> &TclHost {
        self.features
            .get::<TclHost>()
            .expect("Tcl is a built-in feature")
    }

    pub(super) fn tcl_mut(&mut self) -> &mut TclHost {
        self.features
            .get_mut::<TclHost>()
            .expect("Tcl is a built-in feature")
    }

    pub(super) fn tcl_candidates(
        &mut self,
        root: Option<&Path>,
    ) -> Vec<fenix_picker::Candidate<fenix_completion::CompletionItem>> {
        self.with_feature::<TclHost, _>(|tcl, app| tcl.candidates(app, root))
            .expect("Tcl is a built-in feature")
    }

    pub(super) fn picker_symbols(&mut self) {
        self.with_feature::<TclHost, _>(|tcl, app| tcl.picker_symbols(app));
    }

    pub(super) fn tags_scanned_event(
        scan: u64,
        root: PathBuf,
        tags: Vec<fenix_completion::ctags::TagEntry>,
    ) -> FenixUserEvent {
        FenixUserEvent::Feature(FeatureEvent {
            feature: SPEC.id,
            label: "TclTagsScanned",
            payload: Box::new(TagsScanned { scan, root, tags }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `App` with `name` holding `text` open and the cursor at the end
    /// of its first line.
    fn app_with(dir: &tempfile::TempDir, name: &str, text: &str) -> App {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        let mut app = App::with_file(None);
        app.test_open_path(&path);
        let end = text.lines().next().unwrap_or_default().chars().count();
        app.test_set_cursor(Cursor {
            char_idx: end,
            sticky_col: 0,
        });
        app
    }

    #[test]
    fn tcl_contributes_its_commands_and_spc_m_keys_from_its_spec() {
        let registry = CommandRegistry::with_builtins();
        for id in ["completion.refresh_tags", "code.symbols"] {
            let command = registry
                .iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("{id} is registered"));
            assert!(!command.external());
        }
        let menu = keymap::local_trie(&[crate::keymap::LocalContext::Tcl]);
        assert!(matches!(
            menu.matcher().feed(KeyPress::char('T')),
            fenix_keymap::Step::Matched(&"completion.refresh_tags")
        ));
        // The MIB's entries in the same menu are untouched.
        assert!(matches!(
            menu.matcher().feed(KeyPress::char('i')),
            fenix_keymap::Step::Matched(&"mib.insert")
        ));
    }

    #[test]
    fn only_a_tcl_buffer_gets_the_tcl_menu_and_hover() {
        let dir = tempfile::tempdir().unwrap();
        let tcl = app_with(&dir, "a.tcl", "string compare $a $b\n");
        assert_eq!(tcl.local_contexts(), [crate::keymap::LocalContext::Tcl]);
        let mut at_string = tcl;
        at_string.test_set_cursor(Cursor {
            char_idx: 2,
            sticky_col: 0,
        });
        let hover = SPEC.hover_fallback.unwrap()(&at_string);
        assert!(hover.is_some_and(|h| h.contains("string")));

        let rust = app_with(&dir, "a.rs", "fn main() {}\n");
        assert!(rust.local_contexts().is_empty());
        assert!(SPEC.hover_fallback.unwrap()(&rust).is_none());
    }

    #[test]
    fn options_and_subcommands_come_from_the_spec() {
        let dir = tempfile::tempdir().unwrap();
        let app = app_with(&dir, "a.tcl", "lsort -\n");
        let (start, prefix, candidates) =
            SPEC.completion_at_cursor.unwrap()(&app).expect("lsort takes options");
        assert_eq!((start, prefix.as_str()), (6, "-"));
        assert!(candidates.iter().any(|c| c.label == "-unique"));

        let app = app_with(&dir, "b.tcl", "string \n");
        let pool = app.completion_pool_at(7);
        assert!(
            pool.iter().any(|c| c.label == "compare"),
            "{:?}",
            pool.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
        assert!(
            SPEC.completion_at_cursor.unwrap()(&app).is_none(),
            "no dash-word at the cursor"
        );
    }

    #[test]
    fn the_keyword_pool_and_known_commands_are_for_tcl_buffers_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut tcl = app_with(&dir, "a.tcl", "proc greet {} {}\ngreet\n");
        let id = tcl.focused_buffer_id();
        let mut items = Vec::new();
        tcl.each_feature(|feature, app| items.extend(feature.completion_items(app)));
        assert!(items.iter().any(|c| c.label == "puts"));
        let mut known = None;
        tcl.each_feature(|feature, app| known = known.take().or(feature.known_commands(app, id)));
        let known = known.expect("a Tcl buffer");
        assert!(known.contains("puts") && known.contains("greet"));

        let mut rust = app_with(&dir, "a.rs", "fn main() {}\n");
        let id = rust.focused_buffer_id();
        let mut items = Vec::new();
        rust.each_feature(|feature, app| items.extend(feature.completion_items(app)));
        assert!(items.is_empty());
        let mut known = None;
        rust.each_feature(|feature, app| known = known.take().or(feature.known_commands(app, id)));
        assert!(known.is_none());
    }
}
