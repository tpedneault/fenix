//! TODO-style comment keywords (`TODO`, `FIXME`, `HACK`, `NOTE`, ...) --
//! the hl-todo/todo-comments.nvim convention. Recognition is only ever
//! applied to text tree-sitter already says is a comment (see
//! `SyntaxState::highlights_in_range`), so a `"TODO"` string literal or
//! an identifier named `NOTE` is never mistaken for one; this module
//! only decides which words *inside* a comment count.

use std::ops::Range;

/// A keyword family -- several spellings share one kind (and so one
/// color), the same grouping todo-comments.nvim ships with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TodoKind {
    /// `FIXME`, `FIX`, `BUG`, `FIXIT`, `ISSUE` -- known broken.
    Fix,
    /// `TODO`.
    Todo,
    /// `HACK`.
    Hack,
    /// `WARN`, `WARNING`, `XXX`.
    Warn,
    /// `PERF`, `OPTIM`, `OPTIMIZE`, `PERFORMANCE`.
    Perf,
    /// `NOTE`, `INFO`.
    Note,
    /// `TEST`, `TESTING`.
    Test,
}

impl TodoKind {
    pub const ALL: [TodoKind; 7] =
        [TodoKind::Fix, TodoKind::Todo, TodoKind::Hack, TodoKind::Warn, TodoKind::Perf, TodoKind::Note, TodoKind::Test];

    /// The canonical spelling, used as a picker label and for filtering.
    pub fn label(self) -> &'static str {
        match self {
            TodoKind::Fix => "FIX",
            TodoKind::Todo => "TODO",
            TodoKind::Hack => "HACK",
            TodoKind::Warn => "WARN",
            TodoKind::Perf => "PERF",
            TodoKind::Note => "NOTE",
            TodoKind::Test => "TEST",
        }
    }

    /// The highlight capture name this kind is reported under. Nested
    /// under `comment.` so everything that treats comments as opaque
    /// (the reindenter's skip ranges, for one) keeps doing so.
    pub fn capture_name(self) -> &'static str {
        match self {
            TodoKind::Fix => "comment.todo.fix",
            TodoKind::Todo => "comment.todo.todo",
            TodoKind::Hack => "comment.todo.hack",
            TodoKind::Warn => "comment.todo.warn",
            TodoKind::Perf => "comment.todo.perf",
            TodoKind::Note => "comment.todo.note",
            TodoKind::Test => "comment.todo.test",
        }
    }

    /// The inverse of `capture_name`.
    pub fn from_capture_name(name: &str) -> Option<TodoKind> {
        TodoKind::ALL.into_iter().find(|k| k.capture_name() == name)
    }

    fn from_word(word: &str) -> Option<TodoKind> {
        Some(match word {
            "FIXME" | "FIX" | "BUG" | "FIXIT" | "ISSUE" => TodoKind::Fix,
            "TODO" => TodoKind::Todo,
            "HACK" => TodoKind::Hack,
            "WARN" | "WARNING" | "XXX" => TodoKind::Warn,
            "PERF" | "OPTIM" | "OPTIMIZE" | "PERFORMANCE" => TodoKind::Perf,
            "NOTE" | "INFO" => TodoKind::Note,
            "TEST" | "TESTING" => TodoKind::Test,
            _ => return None,
        })
    }
}

/// Every spelling `TodoKind::from_word` accepts, as a ripgrep/regex
/// alternation -- what a project-wide scan pre-filters files with before
/// the precise, comment-aware pass decides what actually counts.
pub const TODO_SEARCH_PATTERN: &str =
    r"\b(FIXME|FIX|BUG|FIXIT|ISSUE|TODO|HACK|WARN|WARNING|XXX|PERF|OPTIM|OPTIMIZE|PERFORMANCE|NOTE|INFO|TEST|TESTING)\b";

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Finds the TODO keywords in `text`, which the caller has already
/// established is comment text (one comment node's worth, possibly
/// spanning several lines). Returned ranges are byte offsets into
/// `text` and cover the keyword plus an optional `(owner)` and trailing
/// `:` -- `TODO(tom):` is highlighted as one unit, the way it reads.
///
/// A capitalized word isn't enough on its own: `// see the NOTE field`
/// or `// INFO is logged` are ordinary prose. A keyword counts when
/// either
/// - it's followed by `:` (optionally after an `(owner)`), anywhere in
///   the comment -- the explicit, unambiguous form; or
/// - it's one of the unambiguous markers (`TODO`, `FIXME`, `FIXIT`,
///   `HACK`, `XXX`, `NOTE`) and the first word on its line of the
///   comment, after any leader punctuation (`//`, `#`, `*`, `<!--`,
///   `;`, `--`, ...) -- `// TODO fix this` is how most people actually
///   write one. The others double as ordinary words (`// INFO is
///   logged`, `// TEST the parser first`) and need the colon.
pub fn find_keywords(text: &str) -> Vec<(Range<usize>, TodoKind)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_uppercase() || (i > 0 && is_word_byte(bytes[i - 1])) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end < bytes.len() && is_word_byte(bytes[end]) {
            end += 1;
        }
        let word = &text[start..end];
        i = end;
        let Some(kind) = TodoKind::from_word(word) else { continue };

        // Optional `(owner)` directly after the keyword.
        let mut tail = end;
        if bytes.get(tail) == Some(&b'(') {
            if let Some(close) = text[tail..].find(')') {
                let inner = &text[tail + 1..tail + close];
                if !inner.contains('\n') {
                    tail += close + 1;
                }
            }
        }
        let has_colon = bytes.get(tail) == Some(&b':');
        if has_colon {
            tail += 1;
        }

        let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
        let first_word = matches!(word, "TODO" | "FIXME" | "FIXIT" | "HACK" | "XXX" | "NOTE")
            && text[line_start..start].bytes().all(|b| !b.is_ascii_alphanumeric());
        if has_colon || first_word {
            out.push((start..tail, kind));
        }
    }
    out
}

/// One TODO found in a document, for listing and navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub kind: TodoKind,
    /// Byte range of the keyword (plus owner/colon) in the source.
    pub range: Range<usize>,
    /// 0-indexed line of the keyword.
    pub line: usize,
    /// 0-indexed *char* column of the keyword on that line.
    pub col: usize,
    /// The rest of the comment line after the keyword, trimmed of
    /// surrounding whitespace and any closing comment delimiter --
    /// what the item says needs doing.
    pub message: String,
}

/// Builds `TodoItem`s from keyword ranges already located in `source`
/// (byte offsets), sorted by position.
pub(crate) fn items_from_ranges(source: &str, mut ranges: Vec<(Range<usize>, TodoKind)>) -> Vec<TodoItem> {
    ranges.sort_by_key(|(r, _)| r.start);
    ranges.dedup_by_key(|(r, _)| r.start);
    ranges
        .into_iter()
        .map(|(range, kind)| {
            let line_start = source[..range.start].rfind('\n').map_or(0, |n| n + 1);
            let line = source[..range.start].matches('\n').count();
            let col = source[line_start..range.start].chars().count();
            let line_end = source[range.end..].find('\n').map_or(source.len(), |n| range.end + n);
            let message = clean_message(&source[range.end..line_end]);
            TodoItem { kind, range, line, col, message }
        })
        .collect()
}

fn clean_message(rest: &str) -> String {
    let mut s = rest.trim();
    for closer in ["-->", "*/", "--]]", "#>", "\"\"\""] {
        if let Some(stripped) = s.strip_suffix(closer) {
            s = stripped.trim_end();
        }
    }
    s.trim_start_matches(':').trim().to_string()
}

/// TODO items in a file no grammar is registered for (plain text, logs,
/// unrecognized extensions): each line is treated as if it were comment
/// text, so the same "first word, or followed by a colon" rule applies.
/// That's deliberately narrower than it sounds -- in a README the word
/// has to open a line or carry the colon to count.
pub fn find_in_plain_text(source: &str) -> Vec<TodoItem> {
    items_from_ranges(source, find_keywords(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<(&str, TodoKind)> {
        find_keywords(text).into_iter().map(|(r, k)| (&text[r], k)).collect()
    }

    #[test]
    fn a_leading_keyword_counts_with_or_without_a_colon() {
        assert_eq!(words("// TODO fix this"), vec![("TODO", TodoKind::Todo)]);
        assert_eq!(words("// TODO: fix this"), vec![("TODO:", TodoKind::Todo)]);
        assert_eq!(words("# FIXME"), vec![("FIXME", TodoKind::Fix)]);
        assert_eq!(words("<!-- NOTE: xml -->"), vec![("NOTE:", TodoKind::Note)]);
        assert_eq!(words(" * HACK around it"), vec![("HACK", TodoKind::Hack)]);
    }

    #[test]
    fn an_owner_is_part_of_the_highlighted_keyword() {
        assert_eq!(words("// TODO(tom): later"), vec![("TODO(tom):", TodoKind::Todo)]);
        assert_eq!(words("// FIXME(#12) broken"), vec![("FIXME(#12)", TodoKind::Fix)]);
    }

    #[test]
    fn prose_uses_of_a_keyword_are_ignored() {
        assert!(words("// see the NOTE field").is_empty());
        assert!(words("// INFO is logged here").is_empty());
        assert!(words("// TEST the parser first").is_empty());
        assert_eq!(words("// INFO: explicit"), vec![("INFO:", TodoKind::Note)]);
        assert!(words("// the TODOS list").is_empty());
        assert!(words("// todo: lowercase is not a marker").is_empty());
        assert!(words("// MY_TODO constant").is_empty());
    }

    #[test]
    fn a_colon_makes_a_mid_sentence_keyword_count() {
        assert_eq!(words("// works, but XXX: slow"), vec![("XXX:", TodoKind::Warn)]);
    }

    #[test]
    fn every_line_of_a_block_comment_is_checked() {
        let text = "/*\n * TODO first\n * NOTE second\n */";
        assert_eq!(words(text), vec![("TODO", TodoKind::Todo), ("NOTE", TodoKind::Note)]);
    }

    #[test]
    fn aliases_map_onto_their_family() {
        assert_eq!(words("// BUG: x")[0].1, TodoKind::Fix);
        assert_eq!(words("// WARNING: x")[0].1, TodoKind::Warn);
        assert_eq!(words("// OPTIMIZE: x")[0].1, TodoKind::Perf);
    }

    #[test]
    fn items_carry_line_column_and_message() {
        let source = "fn a() {}\n    // TODO(tom): wire this up -->\n";
        let items = find_in_plain_text(source);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].line, 1);
        assert_eq!(items[0].col, 7);
        assert_eq!(items[0].message, "wire this up");
    }

    #[test]
    fn capture_names_round_trip() {
        for kind in TodoKind::ALL {
            assert_eq!(TodoKind::from_capture_name(kind.capture_name()), Some(kind));
        }
        assert_eq!(TodoKind::from_capture_name("comment"), None);
    }

    #[test]
    fn the_search_pattern_names_every_spelling() {
        for word in ["FIXME", "FIX", "BUG", "FIXIT", "ISSUE", "TODO", "HACK", "WARN", "WARNING", "XXX", "PERF", "OPTIM", "OPTIMIZE", "PERFORMANCE", "NOTE", "INFO", "TEST", "TESTING"] {
            assert!(TodoKind::from_word(word).is_some());
            assert!(TODO_SEARCH_PATTERN.contains(&format!("|{word}|")) || TODO_SEARCH_PATTERN.contains(&format!("({word}|")) || TODO_SEARCH_PATTERN.contains(&format!("|{word})")));
        }
    }
}
