//! Semantic tokens (`textDocument/semanticTokens/*`): what a server's
//! answer says, in terms Fenix draws by. Pure: no session, no drawing.
//!
//! A server sends tokens as runs of five numbers, each position relative
//! to the token before it, in UTF-16 units, with the type and modifiers
//! as indices into the legend it gave in its capabilities. Here they
//! become char ranges into the text the answer describes, each with a
//! [`Style`] Fenix looks up a theme colour for. They can then be moved
//! through an edit ([`shift`]) and looked up by window ([`within`]).

use std::ops::Range;

use lsp_types::{SemanticTokenModifier, SemanticTokenType, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensServerCapabilities};

/// What a server offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// `textDocument/semanticTokens/full`.
    pub full: bool,
    /// `textDocument/semanticTokens/full/delta`.
    pub delta: bool,
    /// `textDocument/semanticTokens/range`.
    pub range: bool,
}

fn options(capabilities: &lsp_types::ServerCapabilities) -> Option<&lsp_types::SemanticTokensOptions> {
    Some(match capabilities.semantic_tokens_provider.as_ref()? {
        SemanticTokensServerCapabilities::SemanticTokensOptions(o) => o,
        SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(r) => &r.semantic_tokens_options,
    })
}

/// The server's semantic tokens provider, if it offers either a whole
/// document or a range. Copies nothing: cheap enough to ask each frame.
pub fn provider(capabilities: &lsp_types::ServerCapabilities) -> Option<Provider> {
    let options = options(capabilities)?;
    let (full, delta) = match &options.full {
        None | Some(SemanticTokensFullOptions::Bool(false)) => (false, false),
        Some(SemanticTokensFullOptions::Bool(true)) => (true, false),
        Some(SemanticTokensFullOptions::Delta { delta }) => (true, delta.unwrap_or(false)),
    };
    let range = options.range.unwrap_or(false);
    (full || range).then_some(Provider { full, delta, range })
}

/// The legend the server's tokens are in.
pub fn legend(capabilities: &lsp_types::ServerCapabilities) -> Option<&SemanticTokensLegend> {
    options(capabilities).map(|o| &o.legend)
}

/// An `initialize` result's JSON made readable by `lsp-types`: the spec
/// lets `semanticTokensProvider.range` be `{}` (and `full` too), which
/// `lsp-types` reads only as a boolean -- one server saying so would
/// otherwise make its whole capabilities unreadable.
pub fn normalize_initialize_result(value: &mut serde_json::Value) {
    let Some(provider) = value.get_mut("capabilities").and_then(|c| c.get_mut("semanticTokensProvider")).and_then(|p| p.as_object_mut()) else { return };
    if provider.get("range").is_some_and(serde_json::Value::is_object) {
        provider.insert("range".into(), serde_json::Value::Bool(true));
    }
}

/// What Fenix advertises: every standard type and modifier, relative
/// tokens, full with delta and range, no overlapping or multi-line
/// tokens, and that they're laid over the grammar's colours.
pub fn client_capabilities() -> lsp_types::SemanticTokensClientCapabilities {
    lsp_types::SemanticTokensClientCapabilities {
        dynamic_registration: Some(false),
        requests: lsp_types::SemanticTokensClientCapabilitiesRequests {
            range: Some(true),
            full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
        },
        token_types: STANDARD_TYPES.iter().map(|t| SemanticTokenType::new(t)).collect(),
        token_modifiers: STANDARD_MODIFIERS.iter().map(|m| SemanticTokenModifier::new(m)).collect(),
        formats: vec![lsp_types::TokenFormat::RELATIVE],
        overlapping_token_support: Some(false),
        multiline_token_support: Some(false),
        server_cancel_support: Some(true),
        augments_syntax_tokens: Some(true),
    }
}

const STANDARD_TYPES: &[&str] = &[
    "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable", "property", "enumMember", "event",
    "function", "method", "macro", "keyword", "modifier", "comment", "string", "number", "regexp", "operator", "decorator", "label",
];
const STANDARD_MODIFIERS: &[&str] =
    &["declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification", "documentation", "defaultLibrary"];

/// The theme colour a token takes. `Interface`, `Parameter` and `Macro`
/// are the slots the grammar has no word for; a theme that doesn't name
/// them falls back (to type, variable and function).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Type,
    Interface,
    Parameter,
    Variable,
    Constant,
    Function,
    Macro,
    Keyword,
    Comment,
    String,
    Number,
    Operator,
    Attribute,
}

/// How a token is laid over the grammar's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    /// What only the server knows (a parameter, a macro, a type): over
    /// the grammar's colour, but not inside its comments and strings.
    Over,
    /// What the grammar knows too (a keyword, a string, a number): only
    /// where the grammar coloured nothing, as in a language Fenix has no
    /// grammar for.
    Fill,
    /// A comment, which is how servers mark code they've left out (an
    /// inactive `#if`): over everything.
    Wins,
}

/// How a token is drawn: its colour (`None`, the grammar's), whether
/// it's underlined (`mutable`), and how it's laid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub slot: Option<Slot>,
    pub underline: bool,
    pub layer: Layer,
}

/// The colour a token type takes, by name, and how it's laid. A slot of
/// `None` leaves the grammar's colour: an unresolved reference, a type
/// Fenix has no mapping for, and what the grammar already colours as
/// well as the server does (a plain variable or field, a namespace, a
/// lifetime) unless a modifier says more (readonly, mutable).
pub fn kind_for_type(name: &str) -> (Option<Slot>, Layer) {
    use Layer::*;
    let slot = match name {
        "type" | "class" | "enum" | "struct" | "typeParameter" | "builtinType" | "selfType" | "typeAlias" => Slot::Type,
        "interface" | "trait" => Slot::Interface,
        "parameter" => Slot::Parameter,
        "enumMember" | "boolean" => Slot::Constant,
        "function" | "method" => Slot::Function,
        "macro" => Slot::Macro,
        "decorator" | "attribute" | "derive" => Slot::Attribute,
        "comment" => return (Some(Slot::Comment), Wins),
        "keyword" | "modifier" | "selfKeyword" => return (Some(Slot::Keyword), Fill),
        "string" | "regexp" | "formatSpecifier" | "escapeSequence" => return (Some(Slot::String), Fill),
        "number" => return (Some(Slot::Number), Fill),
        "operator" => return (Some(Slot::Operator), Fill),
        _ => return (None, Over),
    };
    (Some(slot), Over)
}

/// The slot a token type takes, by name (`kind_for_type`'s).
pub fn slot_for_type(name: &str) -> Option<Slot> {
    kind_for_type(name).0
}

/// A server's legend, turned into what each type and modifier bit does.
#[derive(Debug, Clone, Default)]
pub struct Styles {
    types: Vec<(Option<Slot>, Layer)>,
    /// Bits that make a variable or property a constant: `constant`
    /// (rust-analyzer's, on a `const` or `static` item) alone, or
    /// `readonly` with `static` (a `static final`, a C# `const`, a
    /// TypeScript `static readonly`).
    constant: u32,
    readonly: u32,
    statics: u32,
    deprecated: u32,
    mutable: u32,
    /// Which types `constant` applies to.
    variable_like: Vec<bool>,
}

impl Styles {
    pub fn new(legend: &SemanticTokensLegend) -> Self {
        let bit = |names: &[&str]| -> u32 {
            legend.token_modifiers.iter().enumerate().filter(|(i, m)| *i < 32 && names.contains(&m.as_str())).fold(0, |bits, (i, _)| bits | 1 << i)
        };
        Styles {
            types: legend.token_types.iter().map(|t| kind_for_type(t.as_str())).collect(),
            constant: bit(&["constant"]),
            readonly: bit(&["readonly"]),
            statics: bit(&["static"]),
            deprecated: bit(&["deprecated"]),
            mutable: bit(&["mutable"]),
            variable_like: legend.token_types.iter().map(|t| matches!(t.as_str(), "variable" | "property")).collect(),
        }
    }

    /// How a token of type index `ty` with modifier bits `modifiers` is
    /// drawn; `None` when it leaves the grammar's colour and isn't
    /// underlined either.
    pub fn style(&self, ty: u32, modifiers: u32) -> Option<Style> {
        let &(mut slot, layer) = self.types.get(ty as usize)?;
        let variable_like = self.variable_like.get(ty as usize).copied().unwrap_or(false);
        if modifiers & self.deprecated != 0 && (slot.is_some() || variable_like) {
            slot = Some(Slot::Comment);
        } else if variable_like && (modifiers & self.constant != 0 || (modifiers & self.readonly != 0 && modifiers & self.statics != 0)) {
            // A local that's only readonly (TypeScript's every `const`)
            // keeps the grammar's colour: a constant is a declaration
            // that's constant as an item, not any binding never assigned.
            slot = Some(Slot::Constant);
        }
        let underline = modifiers & self.mutable != 0;
        (slot.is_some() || underline).then_some(Style { slot, underline, layer })
    }
}

/// One token, as chars into the text it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// A server's answer to any of the three asks, read as sent: a whole
/// document's (`data`) or a delta's (`edits`), and its id. Kept as plain
/// numbers, since a delta's edits index the raw array, not tokens.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    #[serde(default)]
    pub result_id: Option<String>,
    #[serde(default)]
    pub data: Option<Vec<u32>>,
    #[serde(default)]
    pub edits: Option<Vec<Edit>>,
}

/// One edit of a delta: `delete_count` numbers from `start` replaced by
/// `data`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub start: u32,
    pub delete_count: u32,
    #[serde(default)]
    pub data: Option<Vec<u32>>,
}

/// An answer's JSON read; `None` for `null`.
pub fn read_answer(value: serde_json::Value) -> Result<Option<Answer>, serde_json::Error> {
    serde_json::from_value(value)
}

/// `data` (relative, five numbers to a token, as sent) decoded against
/// `text`: the tokens Fenix draws, in order and not overlapping, each a
/// char range. Lines are `text`'s as ropey and `position.rs` count them,
/// the same as the positions every `didChange` sends. Left out: a token
/// whose type maps to nothing, one past its line's end or empty, and
/// one overlapping a token before it (Fenix doesn't ask for those). A
/// token's length stops at its line's end (nor for multi-line ones).
pub fn decode(data: &[u32], text: &str, styles: &Styles) -> Vec<Token> {
    decode_in(data, &Rope::from_str(text), styles)
}

/// `decode`, against the text already in a rope.
pub fn decode_in(data: &[u32], rope: &Rope, styles: &Styles) -> Vec<Token> {
    let mut out = Vec::with_capacity(data.len() / 5);
    let mut lines = Lines::new(rope);
    let (mut line, mut start) = (0u32, 0u32);
    for t in data.as_chunks::<5>().0 {
        let [delta_line, delta_start, length, ty, modifiers] = *t;
        if delta_line > 0 {
            line = line.saturating_add(delta_line);
            start = delta_start;
        } else {
            start = start.saturating_add(delta_start);
        }
        let Some(style) = styles.style(ty, modifiers) else { continue };
        let Some((from, to)) = lines.chars(line as usize, start as usize, start as usize + length as usize) else { continue };
        if from < to {
            out.push(Token { start: from, end: to, style });
        }
    }
    // A server sending them out of order or overlapping: in order, the
    // first of those that overlap kept.
    if !out.is_sorted_by_key(|t| t.start) {
        out.sort_by_key(|t| t.start);
    }
    let mut end = 0;
    out.retain(|t| {
        let keep = t.start >= end;
        if keep {
            end = t.end;
        }
        keep
    });
    out
}

/// Walks `text` a line at a time, turning UTF-16 columns into chars.
/// Tokens come in order, so lines are read in order, once each (a server
/// that goes back on a line just costs a fresh walk to it).
struct Lines<'a> {
    rope: &'a ropey::Rope,
    iter: ropey::iter::Lines<'a>,
    /// The line `iter` gives next, and its first char's offset.
    next_line: usize,
    next_first: usize,
    /// The line read, its chars without its break, and its first char's
    /// offset.
    line: Option<usize>,
    chars: Vec<char>,
    first_char: usize,
    /// How far along the line the walk is: (UTF-16 units, chars).
    at: (usize, usize),
}

impl<'a> Lines<'a> {
    fn new(rope: &'a ropey::Rope) -> Self {
        Lines { rope, iter: rope.lines(), next_line: 0, next_first: 0, line: None, chars: Vec::new(), first_char: 0, at: (0, 0) }
    }

    /// Chars `[from, to)` of line `line`, given in UTF-16 units, the end
    /// clamped to the line's end; `None` past the text's last line or
    /// when the line is shorter than `from`.
    fn chars(&mut self, line: usize, from: usize, to: usize) -> Option<(usize, usize)> {
        if self.line != Some(line) {
            if line >= self.rope.len_lines() {
                return None;
            }
            if line < self.next_line {
                self.iter = self.rope.lines_at(line);
                self.next_line = line;
                self.next_first = self.rope.line_to_char(line);
            }
            while self.next_line < line {
                self.next_first += self.iter.next()?.len_chars();
                self.next_line += 1;
            }
            let slice = self.iter.next()?;
            self.first_char = self.next_first;
            self.next_first += slice.len_chars();
            self.next_line += 1;
            let len = crate::position::line_content_char_len(slice);
            self.chars.clear();
            match slice.as_str() {
                Some(text) => self.chars.extend(text.chars().take(len)),
                None => self.chars.extend(slice.chars().take(len)),
            }
            self.line = Some(line);
            self.at = (0, 0);
        }
        if from < self.at.0 {
            self.at = (0, 0);
        }
        let start = self.advance(from)?;
        let end = self.advance(to).unwrap_or(self.at.1);
        Some((self.first_char + start, self.first_char + end))
    }

    /// Walks on to UTF-16 column `column`, giving the chars from the
    /// line's start; `None` (left at the line's end) when it's shorter.
    fn advance(&mut self, column: usize) -> Option<usize> {
        let (mut units, mut chars) = self.at;
        while units < column {
            let Some(c) = self.chars.get(chars) else { break };
            units += c.len_utf16();
            chars += 1;
        }
        self.at = (units, chars);
        (units >= column).then_some(chars)
    }
}

/// `data` with a delta's `edits` applied. The edits' `start` and
/// `delete_count` index the raw numbers, against the array before any
/// of them, and are applied last first. `None` when one reaches past the
/// end, two overlap, or what's left isn't whole tokens: ask for the
/// whole document again.
pub fn apply_edits(data: &[u32], edits: &[Edit]) -> Option<Vec<u32>> {
    let mut edits: Vec<&Edit> = edits.iter().collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
    let mut out = data.to_vec();
    let mut floor = usize::MAX;
    for edit in edits {
        let start = edit.start as usize;
        let end = start.checked_add(edit.delete_count as usize)?;
        // Edits mustn't overlap, and each is against the original array.
        if end > out.len() || end > floor {
            return None;
        }
        out.splice(start..end, edit.data.iter().flatten().copied());
        floor = start;
    }
    out.len().is_multiple_of(5).then_some(out)
}

/// What a document's ask by range needs of its text, worked out once a
/// sync: how many lines it has (as ropey and every position sent count
/// them) and where it ends, as a position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    pub lines: usize,
    pub bytes: usize,
    pub end: lsp_types::Position,
}

/// The text as ropey holds it: what a document's extent, its lines and
/// its answers are worked out against, built once a sync.
pub use ropey::Rope;

pub fn extent(text: &str) -> Extent {
    extent_of(&Rope::from_str(text))
}

pub fn extent_of(rope: &Rope) -> Extent {
    Extent { lines: rope.len_lines(), bytes: rope.len_bytes(), end: crate::char_offset_to_position(rope, rope.len_chars()) }
}

/// The chars of `text`'s lines `lines` (a range of line numbers): from the
/// first's start to the start of the line after the last, or the end.
pub fn line_chars(text: &str, lines: Range<usize>) -> Range<usize> {
    line_chars_in(&Rope::from_str(text), lines)
}

pub fn line_chars_in(rope: &Rope, lines: Range<usize>) -> Range<usize> {
    let at = |line: usize| if line >= rope.len_lines() { rope.len_chars() } else { rope.line_to_char(line) };
    at(lines.start)..at(lines.end)
}

/// `tokens` with those reaching into chars `chars` replaced by `answered`
/// (what an ask for those lines found): the rest of the document's kept.
/// Both in order and not overlapping, and so is what's left.
pub fn merge(tokens: &mut Vec<Token>, answered: Vec<Token>, chars: Range<usize>) {
    tokens.retain(|t| t.end <= chars.start || t.start >= chars.end);
    let answered: Vec<Token> = answered.into_iter().filter(|t| t.start >= chars.start && t.end <= chars.end).collect();
    let at = tokens.partition_point(|t| t.start < chars.start);
    tokens.splice(at..at, answered);
}

/// `tokens` moved through one edit: chars `[start, old_end)` became
/// `[start, new_end)`. Those before stay, those after move by the
/// difference, those touching the replaced chars (or with an insertion
/// inside them) go: the grammar's colour shows there until the server
/// answers again.
pub fn shift(tokens: &mut Vec<Token>, start: usize, old_end: usize, new_end: usize) {
    tokens.retain_mut(|t| {
        if t.end <= start {
            return true;
        }
        if t.start >= old_end {
            t.start = t.start - old_end + new_end;
            t.end = t.end - old_end + new_end;
            return true;
        }
        false
    });
}

/// The tokens that reach into chars `[from, to)`. `tokens` are in order
/// and don't overlap, as [`decode`] gives them.
pub fn within(tokens: &[Token], from: usize, to: usize) -> &[Token] {
    let first = tokens.partition_point(|t| t.end <= from);
    let last = tokens.partition_point(|t| t.start < to).max(first);
    &tokens[first..last]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legend(types: &[&'static str], modifiers: &[&'static str]) -> SemanticTokensLegend {
        SemanticTokensLegend {
            token_types: types.iter().map(|t| SemanticTokenType::new(t)).collect(),
            token_modifiers: modifiers.iter().map(|m| SemanticTokenModifier::new(m)).collect(),
        }
    }

    fn tok(delta_line: u32, delta_start: u32, length: u32, token_type: u32, token_modifiers_bitset: u32) -> [u32; 5] {
        [delta_line, delta_start, length, token_type, token_modifiers_bitset]
    }

    fn style(slot: Slot) -> Style {
        Style { slot: Some(slot), underline: false, layer: Layer::Over }
    }

    fn caps(json: serde_json::Value) -> lsp_types::ServerCapabilities {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_server_offering_full_with_delta_and_range_is_read_as_such() {
        let c = caps(serde_json::json!({"semanticTokensProvider": {"legend": {"tokenTypes": ["function"], "tokenModifiers": []}, "full": {"delta": true}, "range": true}}));
        let p = provider(&c).unwrap();
        assert!(p.full && p.delta && p.range);
        assert_eq!(super::legend(&c).unwrap().token_types[0].as_str(), "function");
    }

    #[test]
    fn full_true_has_no_delta_and_a_range_only_server_still_counts() {
        let p = provider(&caps(serde_json::json!({"semanticTokensProvider": {"legend": {"tokenTypes": [], "tokenModifiers": []}, "full": true}}))).unwrap();
        assert!(p.full && !p.delta && !p.range);
        let p = provider(&caps(serde_json::json!({"semanticTokensProvider": {"legend": {"tokenTypes": [], "tokenModifiers": []}, "range": true}}))).unwrap();
        assert!(!p.full && p.range);
    }

    #[test]
    fn a_server_offering_neither_or_nothing_has_no_provider() {
        assert!(provider(&caps(serde_json::json!({}))).is_none());
        assert!(provider(&caps(serde_json::json!({"semanticTokensProvider": {"legend": {"tokenTypes": [], "tokenModifiers": []}, "full": false}}))).is_none());
    }

    #[test]
    fn registration_options_are_read_too() {
        let p = provider(&caps(serde_json::json!({"semanticTokensProvider": {"documentSelector": null, "legend": {"tokenTypes": [], "tokenModifiers": []}, "full": true, "id": "x"}})));
        assert!(p.is_some_and(|p| p.full));
    }

    #[test]
    fn a_range_given_as_an_object_still_reads() {
        let mut v = serde_json::json!({"capabilities": {"semanticTokensProvider": {"legend": {"tokenTypes": [], "tokenModifiers": []}, "full": {}, "range": {}}}});
        assert!(serde_json::from_value::<lsp_types::InitializeResult>(v.clone()).is_err(), "lsp-types alone can't");
        normalize_initialize_result(&mut v);
        let result: lsp_types::InitializeResult = serde_json::from_value(v).unwrap();
        let p = provider(&result.capabilities).unwrap();
        assert!(p.full && p.range && !p.delta);
    }

    #[test]
    fn the_advertised_capabilities_serialize_as_the_spec_names_them() {
        let v = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(v["requests"]["full"]["delta"], true);
        assert_eq!(v["requests"]["range"], true);
        assert_eq!(v["formats"][0], "relative");
        assert_eq!(v["augmentsSyntaxTokens"], true);
        assert!(v["tokenTypes"].as_array().unwrap().iter().any(|t| t == "typeParameter"));
    }

    #[test]
    fn types_map_to_their_slots_and_layers() {
        assert_eq!(slot_for_type("parameter"), Some(Slot::Parameter));
        assert_eq!(slot_for_type("interface"), Some(Slot::Interface));
        assert_eq!(slot_for_type("macro"), Some(Slot::Macro));
        assert_eq!(slot_for_type("enumMember"), Some(Slot::Constant));
        assert_eq!(slot_for_type("decorator"), Some(Slot::Attribute));
        assert_eq!(slot_for_type("builtinType"), Some(Slot::Type));
        // What the grammar colours as well: its colour stays.
        for name in ["variable", "property", "namespace", "lifetime", "unresolvedReference", "generic"] {
            assert_eq!(slot_for_type(name), None, "{name}");
        }
        assert_eq!(kind_for_type("function").1, Layer::Over);
        assert_eq!(kind_for_type("number"), (Some(Slot::Number), Layer::Fill), "only where the grammar coloured nothing");
        assert_eq!(kind_for_type("keyword").1, Layer::Fill);
        assert_eq!(kind_for_type("comment"), (Some(Slot::Comment), Layer::Wins), "an inactive region greyed whatever's in it");
    }

    #[test]
    fn modifiers_make_constants_deprecations_and_underlines() {
        let s = Styles::new(&legend(&["variable", "function", "property"], &["declaration", "readonly", "deprecated", "mutable", "static", "constant"]));
        assert_eq!(s.style(0, 0), None, "a plain variable: the grammar's");
        assert_eq!(s.style(0, 0b0010), None, "a readonly local (TypeScript's const): the grammar's");
        assert_eq!(s.style(0, 0b10_0000), Some(style(Slot::Constant)), "rust-analyzer's constant");
        assert_eq!(s.style(0, 0b01_0010), Some(style(Slot::Constant)), "static readonly");
        assert_eq!(s.style(2, 0b01_0010), Some(style(Slot::Constant)), "a static readonly property");
        assert_eq!(s.style(0, 0b01_0000), None, "static alone isn't constant");
        assert_eq!(s.style(1, 0b10_0010), Some(style(Slot::Function)), "says nothing of a function");
        assert_eq!(s.style(1, 0b0100), Some(style(Slot::Comment)), "deprecated");
        assert_eq!(s.style(0, 0b1000), Some(Style { slot: None, underline: true, layer: Layer::Over }), "mutable: underlined, the grammar's colour");
        assert_eq!(s.style(0, 0b0001), None, "other modifiers say nothing");
        assert_eq!(s.style(9, 0), None, "a type index past the legend");
    }

    fn styles() -> Styles {
        Styles::new(&legend(&["function", "macro", "unresolvedReference"], &[]))
    }

    #[test]
    fn relative_tokens_decode_to_char_ranges() {
        let text = "fn main() {\n    let x = foo();\n}\n";
        // `main` on line 0 col 3; `x` line 1 col 8; `foo` line 1 col 12.
        let data = [tok(0, 3, 4, 0, 0), tok(1, 8, 1, 1, 0), tok(0, 4, 3, 0, 0)].concat();
        let got = decode(&data, text, &styles());
        let spans: Vec<&str> = got.iter().map(|t| &text[t.start..t.end]).collect();
        assert_eq!(spans, ["main", "x", "foo"]);
        assert_eq!(got[1].style.slot, Some(Slot::Macro));
    }

    #[test]
    fn a_type_mapped_to_nothing_is_left_out_but_still_counts_for_positions() {
        let text = "a b c";
        let data = [tok(0, 0, 1, 0, 0), tok(0, 2, 1, 2, 0), tok(0, 2, 1, 1, 0)].concat();
        let got = decode(&data, text, &styles());
        assert_eq!(got.iter().map(|t| t.start).collect::<Vec<_>>(), [0, 4]);
    }

    #[test]
    fn utf16_columns_count_a_surrogate_pair_as_two() {
        // "😀" is one char, two UTF-16 units; `x` is at UTF-16 column 3.
        let text = "😀 x y";
        let data = [tok(0, 3, 1, 1, 0), tok(0, 2, 1, 1, 0)].concat();
        let got = decode(&data, text, &styles());
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(got.iter().map(|t| chars[t.start]).collect::<Vec<_>>(), ['x', 'y']);
        assert_eq!((got[0].start, got[0].end), (2, 3));
    }

    #[test]
    fn crlf_lines_and_offsets_count_both_break_chars() {
        let text = "ab\r\ncd\r\n";
        let got = decode(&tok(1, 0, 2, 0, 0), text, &styles());
        assert_eq!((got[0].start, got[0].end), (4, 6));
    }

    #[test]
    fn lines_are_counted_as_the_positions_sent_count_them() {
        // A lone CR breaks a line, for ropey and so for every position a
        // didChange sends: `foo` is line 1.
        let text = "a\rfoo\nbar";
        let got = decode(&tok(1, 0, 3, 0, 0), text, &styles());
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(chars[got[0].start..got[0].end].iter().collect::<String>(), "foo");
        let rope = ropey::Rope::from_str(text);
        assert_eq!(crate::position_to_char_offset(&rope, lsp_types::Position { line: 1, character: 0 }), got[0].start, "the same place a position names");
    }

    #[test]
    fn a_token_past_its_line_or_the_text_is_dropped_and_one_too_long_is_cut() {
        let text = "abc\nde";
        let got = decode(&[tok(0, 1, 9, 0, 0), tok(0, 5, 1, 0, 0), tok(1, 0, 1, 0, 0), tok(5, 0, 1, 0, 0)].concat(), text, &styles());
        assert_eq!(got.iter().map(|t| (t.start, t.end)).collect::<Vec<_>>(), [(1, 3), (4, 5)]);
    }

    #[test]
    fn overlapping_tokens_keep_the_first_and_come_out_in_order() {
        let text = "abcdef ghij";
        // The second (2..3) inside the first (0..4): Fenix didn't ask for
        // overlapping ones, so only the first is kept, and what's left is
        // in order for `within`'s search.
        let got = decode(&[tok(0, 0, 4, 0, 0), tok(0, 2, 1, 0, 0), tok(0, 5, 2, 0, 0)].concat(), text, &styles());
        assert_eq!(got.iter().map(|t| (t.start, t.end)).collect::<Vec<_>>(), [(0, 4), (7, 9)]);
        assert_eq!(within(&got, 5, 9).len(), 1);
    }

    #[test]
    fn delta_edits_splice_the_raw_array_last_first() {
        let data: Vec<u32> = (0..4).flat_map(|i| tok(0, i, 1, 0, 0)).collect();
        let edits = [Edit { start: 5, delete_count: 5, data: Some(tok(9, 9, 9, 9, 9).to_vec()) }, Edit { start: 15, delete_count: 5, data: None }];
        let got = apply_edits(&data, &edits).unwrap();
        assert_eq!(got, [tok(0, 0, 1, 0, 0), tok(9, 9, 9, 9, 9), tok(0, 2, 1, 0, 0)].concat());
    }

    #[test]
    fn an_edit_inside_a_token_s_numbers_is_applied_as_the_spec_says() {
        // Only `x`'s type (the 4th number of the 2nd token) changed.
        let data = [tok(0, 3, 4, 0, 0), tok(0, 13, 1, 1, 0)].concat();
        let got = apply_edits(&data, &[Edit { start: 8, delete_count: 1, data: Some(vec![0]) }]).unwrap();
        assert_eq!(got, [tok(0, 3, 4, 0, 0), tok(0, 13, 1, 0, 0)].concat());
    }

    #[test]
    fn an_edit_past_the_end_or_leaving_part_of_a_token_fails() {
        let data = tok(0, 0, 1, 0, 0).to_vec();
        assert!(apply_edits(&data, &[Edit { start: 3, delete_count: 0, data: Some(vec![1]) }]).is_none(), "six numbers");
        assert!(apply_edits(&data, &[Edit { start: 5, delete_count: 5, data: None }]).is_none());
        assert!(apply_edits(&data, &[Edit { start: 5, delete_count: 0, data: Some(tok(1, 1, 1, 1, 1).to_vec()) }]).is_some(), "appending at the end");
    }

    #[test]
    fn overlapping_edits_fail() {
        let data: Vec<u32> = (0..4).flat_map(|i| tok(0, i, 1, 0, 0)).collect();
        let edits = [Edit { start: 0, delete_count: 10, data: None }, Edit { start: 5, delete_count: 5, data: None }];
        assert!(apply_edits(&data, &edits).is_none());
    }

    #[test]
    fn answers_of_every_shape_are_read() {
        assert_eq!(read_answer(serde_json::Value::Null).unwrap(), None);
        let full = read_answer(serde_json::json!({"resultId": "1", "data": [0, 1, 2, 3, 4]})).unwrap().unwrap();
        assert_eq!((full.result_id.as_deref(), full.data, full.edits), (Some("1"), Some(vec![0, 1, 2, 3, 4]), None));
        let delta = read_answer(serde_json::json!({"resultId": "2", "edits": [{"start": 3, "deleteCount": 1, "data": [7]}]})).unwrap().unwrap();
        assert_eq!(delta.edits, Some(vec![Edit { start: 3, delete_count: 1, data: Some(vec![7]) }]));
    }

    fn t(start: usize, end: usize) -> Token {
        Token { start, end, style: style(Slot::Function) }
    }

    fn ranges(tokens: &[Token]) -> Vec<(usize, usize)> {
        tokens.iter().map(|t| (t.start, t.end)).collect()
    }

    #[test]
    fn shifting_keeps_what_is_before_moves_what_is_after_and_drops_what_it_touches() {
        let mut tokens = vec![t(0, 3), t(4, 7), t(8, 10), t(12, 14)];
        // Chars 5..9 replaced by two chars.
        shift(&mut tokens, 5, 9, 7);
        assert_eq!(ranges(&tokens), [(0, 3), (10, 12)]);
    }

    #[test]
    fn typing_at_a_tokens_end_or_before_its_start_keeps_it() {
        let mut tokens = vec![t(0, 3), t(4, 7)];
        shift(&mut tokens, 3, 3, 4); // one char typed right after `t(0,3)`, before the space
        assert_eq!(ranges(&tokens), [(0, 3), (5, 8)]);
        let mut tokens = vec![t(4, 7)];
        shift(&mut tokens, 4, 4, 5); // typed right before it
        assert_eq!(ranges(&tokens), [(5, 8)]);
    }

    #[test]
    fn typing_inside_a_token_drops_it() {
        let mut tokens = vec![t(0, 3), t(4, 7)];
        shift(&mut tokens, 5, 5, 6);
        assert_eq!(ranges(&tokens), [(0, 3)]);
    }

    #[test]
    fn deleting_up_to_a_tokens_start_moves_it() {
        let mut tokens = vec![t(4, 7)];
        shift(&mut tokens, 2, 4, 2);
        assert_eq!(ranges(&tokens), [(2, 5)]);
    }

    #[test]
    fn within_finds_the_tokens_reaching_into_a_window() {
        let tokens = vec![t(0, 3), t(4, 7), t(8, 10), t(12, 14)];
        assert_eq!(ranges(within(&tokens, 5, 9)), [(4, 7), (8, 10)]);
        assert_eq!(ranges(within(&tokens, 3, 4)), Vec::<(usize, usize)>::new());
        assert_eq!(ranges(within(&tokens, 0, 100)).len(), 4);
        assert!(within(&tokens, 20, 30).is_empty());
        assert!(within(&[], 0, 1).is_empty());
    }

    /// How long a large answer takes to read (`--ignored --nocapture`,
    /// in a release build): a million lines of five tokens each.
    #[test]
    #[ignore]
    fn measure_decoding_a_million_lines() {
        let text = "fn name(arg: Type) -> Out { body() }\n".repeat(1_000_000);
        let line = [tok(1, 3, 4, 0, 0), tok(0, 5, 3, 1, 0), tok(0, 5, 4, 0, 0), tok(0, 9, 3, 0, 0), tok(0, 6, 4, 1, 0)];
        let mut data: Vec<u32> = line.iter().cycle().take(5_000_000).flatten().copied().collect();
        data[0] = 0;
        let s = styles();
        let started = std::time::Instant::now();
        let got = decode(&data, &text, &s);
        let decoded = started.elapsed();
        let started = std::time::Instant::now();
        let mut shifted = got.clone();
        shift(&mut shifted, 1000, 1001, 1003);
        let moved = started.elapsed();
        let started = std::time::Instant::now();
        let n: usize = (0..1000).map(|i| within(&got, i * 37_000, i * 37_000 + 6000).len()).sum();
        let looked = started.elapsed();
        println!("decoded {} tokens in {decoded:?}; shifted in {moved:?}; 1000 window lookups ({n} tokens) in {looked:?}", got.len());
    }

    #[test]
    fn an_extent_and_line_chars_count_lines_as_positions_do() {
        let e = extent("ab\ncd\u{c}ef");
        assert_eq!(e.lines, 3, "a form feed breaks a line for ropey, so for every position sent");
        assert_eq!(e.end, lsp_types::Position { line: 2, character: 2 });
        assert_eq!(line_chars("ab\ncd\nef", 1..2), 3..6);
        assert_eq!(line_chars("ab\ncd", 1..9), 3..5);
    }

    #[test]
    fn a_range_s_answer_replaces_only_the_tokens_in_its_lines() {
        let mut tokens = vec![t(0, 2), t(5, 7), t(9, 10), t(20, 22)];
        merge(&mut tokens, vec![t(5, 6), t(8, 9), t(30, 31)], 4..12);
        assert_eq!(ranges(&tokens), [(0, 2), (5, 6), (8, 9), (20, 22)], "one outside the lines asked isn't taken");
    }

    #[test]
    fn changed_span_is_the_one_replaced_range() {
        assert_eq!(crate::changed_span("let x = 1;", "let xy = 1;"), (5, 5, 6));
        assert_eq!(crate::changed_span("abc", "abc"), (3, 3, 3));
        assert_eq!(crate::changed_span("a\r\nb", "a\nb"), (1, 3, 2), "the whole break is replaced, never half of it");
    }
}
