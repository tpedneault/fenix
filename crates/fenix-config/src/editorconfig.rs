//! `.editorconfig` (<https://editorconfig.org>): the indentation, line
//! endings, charset and whitespace rules a repository gives every editor.
//!
//! For a file, every `.editorconfig` from its folder up is read, stopping
//! at one that says `root = true`; sections whose glob matches the file
//! give its properties, a later section over an earlier one and a nearer
//! file over a farther one, as the specification says.
//!
//! Where these sit among Fenix's settings, highest first:
//!
//! 1. the project's own `.fenix/settings.toml`;
//! 2. `.editorconfig`;
//! 3. your `settings.toml`;
//! 4. Fenix's defaults.
//!
//! So a repository's `.editorconfig` beats your own preferences, as it
//! does in every editor, and a project that wants Fenix to do something
//! else says so in its own Fenix file.
//!
//! This module only reads: what Fenix does with the answer is `App`'s.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `indent_style`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    Space,
    Tab,
}

/// `end_of_line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfLine {
    Lf,
    Crlf,
    Cr,
}

impl EndOfLine {
    pub fn as_str(self) -> &'static str {
        match self {
            EndOfLine::Lf => "\n",
            EndOfLine::Crlf => "\r\n",
            EndOfLine::Cr => "\r",
        }
    }
}

/// `charset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Latin1,
}

/// What `.editorconfig` says of one file. `None` is "not said": the
/// setting below it applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    pub indent_style: Option<IndentStyle>,
    /// Columns of one indent level. `indent_size = tab` reads as
    /// `tab_width`.
    pub indent_size: Option<usize>,
    pub tab_width: Option<usize>,
    pub end_of_line: Option<EndOfLine>,
    pub charset: Option<Charset>,
    pub trim_trailing_whitespace: Option<bool>,
    pub insert_final_newline: Option<bool>,
    /// The `.editorconfig` files that said something, nearest first.
    pub sources: Vec<PathBuf>,
}

impl Properties {
    pub fn is_empty(&self) -> bool {
        *self == Properties { sources: self.sources.clone(), ..Properties::default() }
    }

    /// One line each, for `SPC f e` to show.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(style) = self.indent_style {
            lines.push(format!("indent_style = {}", if style == IndentStyle::Tab { "tab" } else { "space" }));
        }
        if let Some(n) = self.indent_size {
            lines.push(format!("indent_size = {n}"));
        }
        if let Some(n) = self.tab_width {
            lines.push(format!("tab_width = {n}"));
        }
        if let Some(eol) = self.end_of_line {
            lines.push(format!("end_of_line = {}", match eol {
                EndOfLine::Lf => "lf",
                EndOfLine::Crlf => "crlf",
                EndOfLine::Cr => "cr",
            }));
        }
        if let Some(charset) = self.charset {
            lines.push(format!("charset = {}", match charset {
                Charset::Utf8 => "utf-8",
                Charset::Utf8Bom => "utf-8-bom",
                Charset::Utf16Le => "utf-16le",
                Charset::Utf16Be => "utf-16be",
                Charset::Latin1 => "latin1",
            }));
        }
        if let Some(b) = self.trim_trailing_whitespace {
            lines.push(format!("trim_trailing_whitespace = {b}"));
        }
        if let Some(b) = self.insert_final_newline {
            lines.push(format!("insert_final_newline = {b}"));
        }
        lines
    }
}

/// One `.editorconfig`, parsed.
#[derive(Debug, Clone, Default)]
pub struct File {
    pub root: bool,
    /// `[glob]` and its `key = value` pairs, keys lowercased, in order.
    pub sections: Vec<(String, Vec<(String, String)>)>,
}

/// Reads one file's text. Lines that aren't a section, a pair or a
/// comment are skipped, as other editors do.
pub fn parse(text: &str) -> File {
    let mut file = File::default();
    let mut in_preamble = true;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(glob) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_preamble = false;
            file.sections.push((glob.to_string(), Vec::new()));
            continue;
        }
        let Some((key, value)) = line.split_once(['=', ':']) else { continue };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if in_preamble {
            if key == "root" {
                file.root = value.eq_ignore_ascii_case("true");
            }
            continue;
        }
        if let Some((_, pairs)) = file.sections.last_mut() {
            pairs.push((key, value));
        }
    }
    file
}

/// What applies to `path` (absolute), from the `.editorconfig` files in
/// its folder and those above, read with `read` (a file that isn't there
/// is `None`).
pub fn properties_with(path: &Path, read: impl Fn(&Path) -> Option<String>) -> Properties {
    // Farthest first, so nearer ones overwrite.
    let mut files: Vec<(PathBuf, File)> = Vec::new();
    let mut dir = path.parent();
    while let Some(d) = dir {
        let config = d.join(".editorconfig");
        if let Some(text) = read(&config) {
            let file = parse(&text);
            let root = file.root;
            files.push((config, file));
            if root {
                break;
            }
        }
        dir = d.parent();
    }
    let mut raw: HashMap<String, String> = HashMap::new();
    let mut sources = Vec::new();
    for (config, file) in files.iter().rev() {
        let base = config.parent().unwrap_or(Path::new(""));
        let Ok(relative) = path.strip_prefix(base) else { continue };
        let relative = relative.to_string_lossy().replace('\\', "/");
        let mut said = false;
        for (glob, pairs) in &file.sections {
            if section_matches(glob, &relative) {
                for (key, value) in pairs {
                    raw.insert(key.clone(), value.clone());
                    said = true;
                }
            }
        }
        if said {
            sources.insert(0, config.clone());
        }
    }
    let mut props = resolve(&raw);
    props.sources = sources;
    props
}

/// `properties_with` from the disk.
pub fn properties(path: &Path) -> Properties {
    properties_with(path, |p| std::fs::read_to_string(p).ok())
}

/// The raw pairs as properties: unknown values and `unset` are none, and
/// the specification's defaults between `indent_size` and `tab_width`.
fn resolve(raw: &HashMap<String, String>) -> Properties {
    let get = |key: &str| raw.get(key).map(|v| v.to_ascii_lowercase()).filter(|v| v != "unset");
    let number = |key: &str| get(key).and_then(|v| v.parse::<usize>().ok()).filter(|n| (1..=64).contains(n));
    let flag = |key: &str| match get(key).as_deref() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let indent_style = match get("indent_style").as_deref() {
        Some("tab") => Some(IndentStyle::Tab),
        Some("space") => Some(IndentStyle::Space),
        _ => None,
    };
    let mut tab_width = number("tab_width");
    let size_is_tab = get("indent_size").as_deref() == Some("tab");
    let mut indent_size = if size_is_tab { tab_width } else { number("indent_size") };
    // `indent_style = tab` with no size: a level is a tab.
    if indent_size.is_none() && (size_is_tab || indent_style == Some(IndentStyle::Tab)) {
        indent_size = tab_width;
    }
    // A size with no tab width: a tab is that wide.
    if tab_width.is_none() && !size_is_tab {
        tab_width = indent_size;
    }
    Properties {
        indent_style,
        indent_size,
        tab_width,
        end_of_line: match get("end_of_line").as_deref() {
            Some("lf") => Some(EndOfLine::Lf),
            Some("crlf") => Some(EndOfLine::Crlf),
            Some("cr") => Some(EndOfLine::Cr),
            _ => None,
        },
        charset: match get("charset").as_deref() {
            Some("utf-8") => Some(Charset::Utf8),
            Some("utf-8-bom") => Some(Charset::Utf8Bom),
            Some("utf-16le") => Some(Charset::Utf16Le),
            Some("utf-16be") => Some(Charset::Utf16Be),
            Some("latin1") => Some(Charset::Latin1),
            _ => None,
        },
        trim_trailing_whitespace: flag("trim_trailing_whitespace"),
        insert_final_newline: flag("insert_final_newline"),
        sources: Vec::new(),
    }
}

/// Whether section `glob` matches `relative` (the file's path from the
/// `.editorconfig`'s folder, `/`-separated). A glob without a `/` matches
/// the name in any folder; one with a `/` is from the file's folder.
pub fn section_matches(glob: &str, relative: &str) -> bool {
    let glob = glob.trim();
    if glob.is_empty() {
        return false;
    }
    let pattern = if glob.contains('/') { glob.strip_prefix('/').unwrap_or(glob).to_string() } else { format!("**/{glob}") };
    let tokens = tokenize(&pattern);
    matches(&tokens, relative)
}

#[derive(Debug, Clone)]
enum Token {
    Char(char),
    /// `?`: one character, not `/`.
    Any,
    /// `*`: any run without `/`.
    Star,
    /// `**`: any run.
    StarStar,
    /// `[...]`, or `[!...]` negated.
    Class(Vec<(char, char)>, bool),
    /// `{a,b}`: one of these, each a pattern of its own.
    Alt(Vec<Vec<Token>>),
    /// `{1..10}`: a whole number in the range.
    Range(i64, i64),
}

fn tokenize(pattern: &str) -> Vec<Token> {
    let chars: Vec<char> = pattern.chars().collect();
    tokens_of(&chars)
}

fn tokens_of(chars: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' if i + 1 < chars.len() => {
                out.push(Token::Char(chars[i + 1]));
                i += 2;
                continue;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push(Token::StarStar);
                i += 2;
                continue;
            }
            '*' => out.push(Token::Star),
            '?' => out.push(Token::Any),
            '[' => {
                if let Some(end) = (i + 1..chars.len()).find(|&j| chars[j] == ']' && j > i + 1) {
                    let mut body = &chars[i + 1..end];
                    let negated = body.first() == Some(&'!');
                    if negated {
                        body = &body[1..];
                    }
                    // A class never matches a path's `/`; one holding it
                    // is taken literally, as the specification has it.
                    if !body.contains(&'/') {
                        let mut ranges = Vec::new();
                        let mut k = 0;
                        while k < body.len() {
                            if k + 2 < body.len() && body[k + 1] == '-' {
                                ranges.push((body[k], body[k + 2]));
                                k += 3;
                            } else {
                                ranges.push((body[k], body[k]));
                                k += 1;
                            }
                        }
                        out.push(Token::Class(ranges, negated));
                        i = end + 1;
                        continue;
                    }
                }
                out.push(Token::Char('['));
            }
            '{' => {
                if let Some(end) = closing_brace(chars, i) {
                    let body = &chars[i + 1..end];
                    let text: String = body.iter().collect();
                    if let Some((a, b)) = text.split_once("..").and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))) {
                        out.push(Token::Range(a.min(b), a.max(b)));
                    } else if body.contains(&',') {
                        out.push(Token::Alt(split_top_level(body).into_iter().map(tokens_of).collect()));
                    } else {
                        // `{single}` is literal.
                        out.extend(std::iter::once(Token::Char('{')).chain(tokens_of(body)).chain(std::iter::once(Token::Char('}'))));
                    }
                    i = end + 1;
                    continue;
                }
                out.push(Token::Char('{'));
            }
            c => out.push(Token::Char(c)),
        }
        i += 1;
    }
    out
}

fn closing_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn split_top_level(body: &[char]) -> Vec<&[char]> {
    let mut parts = Vec::new();
    let (mut depth, mut start, mut i) = (0, 0, 0);
    while i < body.len() {
        match body[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&body[start..]);
    parts
}

fn matches(tokens: &[Token], text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    match_at(tokens, &chars)
}

fn match_at(tokens: &[Token], text: &[char]) -> bool {
    let Some((first, rest)) = tokens.split_first() else { return text.is_empty() };
    match first {
        Token::Char(c) => text.first() == Some(c) && match_at(rest, &text[1..]),
        Token::Any => text.first().is_some_and(|&c| c != '/') && match_at(rest, &text[1..]),
        Token::Class(ranges, negated) => {
            text.first().is_some_and(|&c| c != '/' && ranges.iter().any(|&(a, b)| a <= c && c <= b) != *negated) && match_at(rest, &text[1..])
        }
        Token::Star => {
            for n in 0..=text.len() {
                if match_at(rest, &text[n..]) {
                    return true;
                }
                if text.get(n) == Some(&'/') {
                    break;
                }
            }
            false
        }
        Token::StarStar => {
            // `**/` also matches no folder at all.
            if let Some((Token::Char('/'), after)) = rest.split_first() {
                if match_at(after, text) {
                    return true;
                }
            }
            (0..=text.len()).any(|n| match_at(rest, &text[n..]))
        }
        Token::Alt(choices) => choices.iter().any(|choice| {
            let joined: Vec<Token> = choice.iter().cloned().chain(rest.iter().cloned()).collect();
            match_at(&joined, text)
        }),
        Token::Range(a, b) => {
            let sign = usize::from(text.first() == Some(&'-'));
            let digits = text[sign..].iter().take_while(|c| c.is_ascii_digit()).count();
            (1..=digits).any(|n| {
                let number: String = text[..sign + n].iter().collect();
                number.parse::<i64>().is_ok_and(|v| *a <= v && v <= *b) && match_at(rest, &text[sign + n..])
            })
        }
    }
}

/// An edit to make before a save: chars `start..end` become `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// What saving `text` under `props` changes, in order and not
/// overlapping, as char ranges into `text`: trailing whitespace trimmed
/// (`trim_trailing_whitespace = true`), every line ending made
/// `end_of_line`'s, and a final line ending added
/// (`insert_final_newline = true`; `false` leaves the end as it is, as
/// editorconfig-vim does). `keep_line` (0-based) is left untrimmed: the
/// line you're typing on, when an auto-save comes mid-word.
pub fn save_edits(text: &str, props: &Properties, keep_line: Option<usize>) -> Vec<Edit> {
    let trim = props.trim_trailing_whitespace == Some(true);
    let eol = props.end_of_line.map(EndOfLine::as_str);
    let mut edits = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let (mut line, mut first_ending): (usize, Option<&'static str>) = (0, None);
    let mut line_start = 0;
    loop {
        // The line's end, and its ending's length.
        let mut end = line_start;
        while end < chars.len() && chars[end] != '\n' && chars[end] != '\r' {
            end += 1;
        }
        let ending_len = match (chars.get(end), chars.get(end + 1)) {
            (Some('\r'), Some('\n')) => 2,
            (Some(_), _) => 1,
            (None, _) => 0,
        };
        if trim && keep_line != Some(line) {
            let mut content_end = end;
            while content_end > line_start && matches!(chars[content_end - 1], ' ' | '\t') {
                content_end -= 1;
            }
            if content_end < end {
                edits.push(Edit { start: content_end, end, text: String::new() });
            }
        }
        if ending_len > 0 {
            let found: &'static str = match ending_len {
                2 => "\r\n",
                _ if chars[end] == '\n' => "\n",
                _ => "\r",
            };
            first_ending.get_or_insert(found);
            if let Some(want) = eol.filter(|w| *w != found) {
                edits.push(Edit { start: end, end: end + ending_len, text: want.to_string() });
            }
        }
        if ending_len == 0 {
            break;
        }
        line += 1;
        line_start = end + ending_len;
    }
    let ends_open = !chars.is_empty() && !matches!(chars.last(), Some('\n' | '\r'));
    if props.insert_final_newline == Some(true) && ends_open {
        let ending = eol.or(first_ending).unwrap_or("\n");
        edits.push(Edit { start: chars.len(), end: chars.len(), text: ending.to_string() });
    }
    edits
}

/// `text` with `edits` made: what the file will hold.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        out.extend(&chars[at..edit.start]);
        out.push_str(&edit.text);
        at = edit.end;
    }
    out.extend(&chars[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(files: &[(&str, &str)], path: &str) -> Properties {
        let files: HashMap<PathBuf, String> = files.iter().map(|(p, t)| (PathBuf::from(p), t.to_string())).collect();
        properties_with(Path::new(path), |p| files.get(p).cloned())
    }

    #[test]
    fn globs_match_as_the_specification_says() {
        assert!(section_matches("*", "a.rs") && section_matches("*", "src/deep/a.rs"), "no slash: any folder");
        assert!(section_matches("*.py", "x/y.py") && !section_matches("*.py", "y.pyc"));
        assert!(section_matches("{*.js,*.ts}", "web/app.ts") && !section_matches("{*.js,*.ts}", "app.rs"));
        assert!(section_matches("src/*.rs", "src/a.rs") && !section_matches("src/*.rs", "src/b/a.rs") && !section_matches("src/*.rs", "x/src/a.rs"));
        assert!(section_matches("/src/**.rs", "src/b/a.rs"));
        assert!(section_matches("lib/**/x.c", "lib/x.c") && section_matches("lib/**/x.c", "lib/a/b/x.c"));
        assert!(section_matches("Makefile", "sub/Makefile") && !section_matches("Makefile", "Makefile.am"));
        assert!(section_matches("file[0-9].txt", "file3.txt") && !section_matches("file[!0-9].txt", "file3.txt"));
        assert!(section_matches("a?c", "abc") && !section_matches("a?c", "a/c"));
        assert!(section_matches("f{1..12}.md", "f7.md") && !section_matches("f{1..12}.md", "f13.md"));
        assert!(section_matches("{single}.txt", "{single}.txt"));
        assert!(section_matches("\\*.txt", "*.txt") && !section_matches("\\*.txt", "a.txt"));
    }

    #[test]
    fn nearer_files_and_later_sections_win_and_root_stops_the_walk() {
        let files = [
            ("/.editorconfig", "[*]\nindent_size = 8\ncharset = latin1\n"),
            ("/repo/.editorconfig", "root = true\n\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\n\n[*.go]\nindent_style = tab\n"),
            ("/repo/web/.editorconfig", "# web\n[*.ts]\nindent_size = 2\ninsert_final_newline = true\ntrim_trailing_whitespace = true\n"),
        ];
        let ts = props(&files, "/repo/web/app.ts");
        assert_eq!(ts.indent_size, Some(2), "the nearer file wins");
        assert_eq!(ts.indent_style, Some(IndentStyle::Space));
        assert_eq!(ts.end_of_line, Some(EndOfLine::Lf));
        assert_eq!(ts.charset, None, "root = true: nothing above /repo");
        assert_eq!((ts.insert_final_newline, ts.trim_trailing_whitespace), (Some(true), Some(true)));
        assert_eq!(ts.sources, vec![PathBuf::from("/repo/web/.editorconfig"), PathBuf::from("/repo/.editorconfig")]);
        let go = props(&files, "/repo/cmd/main.go");
        assert_eq!(go.indent_style, Some(IndentStyle::Tab), "the later section wins");
        assert_eq!(go.indent_size, Some(4));
        let other = props(&files, "/elsewhere/a.txt");
        assert_eq!((other.indent_size, other.charset), (Some(8), Some(Charset::Latin1)));
        assert!(props(&[], "/a/b.rs").is_empty());
    }

    #[test]
    fn sizes_and_widths_fill_each_other_in_and_unset_unsets() {
        let p = |text: &str| props(&[("/p/.editorconfig", text)], "/p/a.c");
        let tab = p("[*]\nindent_style = tab\ntab_width = 3\n");
        assert_eq!((tab.indent_size, tab.tab_width), (Some(3), Some(3)), "a level is a tab");
        let size = p("[*]\nindent_size = 2\n");
        assert_eq!(size.tab_width, Some(2), "a tab is a level wide");
        let size_tab = p("[*]\nindent_size = tab\ntab_width = 5\n");
        assert_eq!(size_tab.indent_size, Some(5));
        let unset = p("[*]\nindent_size = 2\n[*.c]\nindent_size = unset\n");
        assert_eq!(unset.indent_size, None);
        let upper = p("[*]\nINDENT_STYLE = Space\nEnd_Of_Line = CRLF\nindent_size = nonsense\n");
        assert_eq!((upper.indent_style, upper.end_of_line, upper.indent_size), (Some(IndentStyle::Space), Some(EndOfLine::Crlf), None));
    }

    #[test]
    fn saving_trims_ends_lines_and_adds_the_final_newline() {
        let props = Properties { trim_trailing_whitespace: Some(true), insert_final_newline: Some(true), end_of_line: Some(EndOfLine::Lf), ..Properties::default() };
        let text = "a  \r\nb\t\n  \nc ";
        assert_eq!(apply(text, &save_edits(text, &props, None)), "a\nb\n\nc\n");
        assert_eq!(apply(text, &save_edits(text, &props, Some(3))), "a\nb\n\nc \n", "the line being typed keeps its space");
        let crlf = Properties { end_of_line: Some(EndOfLine::Crlf), ..Properties::default() };
        assert_eq!(apply("x\ny\r\nz\n", &save_edits("x\ny\r\nz\n", &crlf, None)), "x\r\ny\r\nz\r\n");
        // The file's own ending when none is set; nothing to do, no edits.
        let final_only = Properties { insert_final_newline: Some(true), ..Properties::default() };
        assert_eq!(apply("p\r\nq", &save_edits("p\r\nq", &final_only, None)), "p\r\nq\r\n");
        assert!(save_edits("done\n", &final_only, None).is_empty());
        assert!(save_edits("", &final_only, None).is_empty(), "an empty file stays empty");
        let off = Properties { insert_final_newline: Some(false), ..Properties::default() };
        assert!(save_edits("open", &off, None).is_empty() && save_edits("closed\n", &off, None).is_empty(), "false leaves the end alone");
        assert!(save_edits("a  \n", &Properties::default(), None).is_empty());
    }
}
