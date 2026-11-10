//! Semantic tokens (`textDocument/semanticTokens/*`): what a server's
//! answer says, in terms Fenix draws by. Pure: no session, no drawing.
//!
//! A server sends tokens as runs of five numbers, each position relative
//! to the token before it, in UTF-16 units, with the type and modifiers
//! as indices into the legend it gave in its capabilities. Here they
//! become char ranges into the text the answer describes, each with a
//! [`Style`] Fenix looks up a theme colour for. They can then be moved
//! through an edit ([`shift`]) and looked up by window ([`within`]).

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensEdit, SemanticTokensFullOptions, SemanticTokensLegend, SemanticTokensServerCapabilities};

/// What a server offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Provider {
    pub legend: SemanticTokensLegend,
    /// `textDocument/semanticTokens/full`.
    pub full: bool,
    /// `textDocument/semanticTokens/full/delta`.
    pub delta: bool,
    /// `textDocument/semanticTokens/range`.
    pub range: bool,
}

/// The server's semantic tokens provider, if it offers either a whole
/// document or a range.
pub fn provider(capabilities: &lsp_types::ServerCapabilities) -> Option<Provider> {
    let options = match capabilities.semantic_tokens_provider.as_ref()? {
        SemanticTokensServerCapabilities::SemanticTokensOptions(o) => o,
        SemanticTokensServerCapabilities::SemanticTokensRegistrationOptions(r) => &r.semantic_tokens_options,
    };
    let (full, delta) = match &options.full {
        None | Some(SemanticTokensFullOptions::Bool(false)) => (false, false),
        Some(SemanticTokensFullOptions::Bool(true)) => (true, false),
        Some(SemanticTokensFullOptions::Delta { delta }) => (true, delta.unwrap_or(false)),
    };
    let range = options.range.unwrap_or(false);
    (full || range).then(|| Provider { legend: options.legend.clone(), full, delta, range })
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

/// How a token is drawn: its colour, and whether it's underlined
/// (`mutable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    pub slot: Slot,
    pub underline: bool,
}

/// The slot a token type takes, by name; `None` leaves the grammar's
/// colour (an unresolved reference, a type Fenix has no mapping for).
pub fn slot_for_type(name: &str) -> Option<Slot> {
    Some(match name {
        "namespace" | "type" | "class" | "enum" | "struct" | "typeParameter" | "builtinType" | "selfType" | "typeAlias" => Slot::Type,
        "interface" | "trait" => Slot::Interface,
        "parameter" => Slot::Parameter,
        "variable" | "property" | "event" | "label" => Slot::Variable,
        "enumMember" | "boolean" => Slot::Constant,
        "function" | "method" => Slot::Function,
        "macro" => Slot::Macro,
        "keyword" | "modifier" | "selfKeyword" | "lifetime" => Slot::Keyword,
        "comment" => Slot::Comment,
        "string" | "regexp" | "formatSpecifier" | "escapeSequence" => Slot::String,
        "number" => Slot::Number,
        "operator" => Slot::Operator,
        "decorator" | "attribute" | "derive" => Slot::Attribute,
        _ => return None,
    })
}

/// A server's legend, turned into what each type and modifier bit does.
#[derive(Debug, Clone, Default)]
pub struct Styles {
    types: Vec<Option<Slot>>,
    /// Bits that make a variable or property a constant.
    constant: u32,
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
            types: legend.token_types.iter().map(|t| slot_for_type(t.as_str())).collect(),
            constant: bit(&["readonly", "constant"]),
            deprecated: bit(&["deprecated"]),
            mutable: bit(&["mutable"]),
            variable_like: legend.token_types.iter().map(|t| matches!(t.as_str(), "variable" | "property")).collect(),
        }
    }

    /// How a token of type index `ty` with modifier bits `modifiers` is
    /// drawn; `None` leaves the grammar's colour.
    pub fn style(&self, ty: u32, modifiers: u32) -> Option<Style> {
        let mut slot = (*self.types.get(ty as usize)?)?;
        if modifiers & self.deprecated != 0 {
            slot = Slot::Comment;
        } else if modifiers & self.constant != 0 && self.variable_like.get(ty as usize).copied().unwrap_or(false) {
            slot = Slot::Constant;
        }
        Some(Style { slot, underline: modifiers & self.mutable != 0 })
    }
}

/// One token, as chars into the text it describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// `data` (relative, as sent) decoded against `text`: the tokens Fenix
/// draws, in order, each a char range. Those whose type maps to nothing
/// are left out; so is one past its line's end or empty. A token's
/// length stops at its line's end (Fenix doesn't ask for multi-line
/// tokens).
pub fn decode(data: &[SemanticToken], text: &str, styles: &Styles) -> Vec<Token> {
    let mut out = Vec::with_capacity(data.len());
    let mut lines = Lines::new(text);
    let (mut line, mut start) = (0u32, 0u32);
    for t in data {
        if t.delta_line > 0 {
            line = line.saturating_add(t.delta_line);
            start = t.delta_start;
        } else {
            start = start.saturating_add(t.delta_start);
        }
        let Some(style) = styles.style(t.token_type, t.token_modifiers_bitset) else { continue };
        let Some((from, to)) = lines.chars(line as usize, start as usize, start as usize + t.length as usize) else { continue };
        if from < to {
            out.push(Token { start: from, end: to, style });
        }
    }
    out
}

/// Walks `text` a line at a time, turning UTF-16 columns into chars.
/// Tokens come in order, so each line is read once (a server that goes
/// back on a line just costs a fresh walk of it).
struct Lines<'a> {
    /// The line being read, without its break, and the break's chars.
    current: &'a str,
    break_chars: usize,
    /// What follows the current line's break.
    rest: &'a str,
    line: usize,
    first_char: usize,
    /// How far along the current line the walk is: (UTF-16 units, chars, bytes).
    at: (usize, usize, usize),
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        let (current, break_chars, rest) = split_line(text);
        Lines { current, break_chars, rest, line: 0, first_char: 0, at: (0, 0, 0) }
    }

    /// Chars `[from, to)` of line `line`, given in UTF-16 units, the end
    /// clamped to the line's end; `None` past the text's last line or
    /// when the line is shorter than `from`.
    fn chars(&mut self, line: usize, from: usize, to: usize) -> Option<(usize, usize)> {
        while self.line < line {
            if self.break_chars == 0 {
                return None; // the last line: nothing below it
            }
            self.first_char += self.current.chars().count() + self.break_chars;
            (self.current, self.break_chars, self.rest) = split_line(self.rest);
            self.line += 1;
            self.at = (0, 0, 0);
        }
        if from < self.at.0 {
            self.at = (0, 0, 0);
        }
        let start = self.advance(from)?;
        let end = self.advance(to).unwrap_or(self.at.1);
        Some((self.first_char + start, self.first_char + end))
    }

    /// Walks on to UTF-16 column `column`, giving the chars from the
    /// line's start; `None` (left at the line's end) when it's shorter.
    fn advance(&mut self, column: usize) -> Option<usize> {
        let (mut units, mut chars, mut bytes) = self.at;
        for c in self.current[bytes..].chars() {
            if units >= column {
                break;
            }
            units += c.len_utf16();
            chars += 1;
            bytes += c.len_utf8();
        }
        self.at = (units, chars, bytes);
        (units >= column).then_some(chars)
    }
}

/// A line's text without its break, the break's length in chars (0 for
/// the last line), and what follows it.
fn split_line(text: &str) -> (&str, usize, &str) {
    match text.find('\n') {
        Some(i) => match text[..i].strip_suffix('\r') {
            Some(line) => (line, 2, &text[i + 1..]),
            None => (&text[..i], 1, &text[i + 1..]),
        },
        None => (text, 0, ""),
    }
}

/// `data` with a delta's `edits` applied. The edits' `start` and
/// `delete_count` count numbers (five to a token), against the array
/// before any of them, and are applied last first. `None` when one
/// doesn't land on a token's boundary or reaches past the end: ask for
/// the whole document again.
pub fn apply_edits(data: &[SemanticToken], edits: &[SemanticTokensEdit]) -> Option<Vec<SemanticToken>> {
    let mut edits: Vec<&SemanticTokensEdit> = edits.iter().collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
    let mut out = data.to_vec();
    let mut floor = usize::MAX;
    for edit in edits {
        if edit.start % 5 != 0 || edit.delete_count % 5 != 0 {
            return None;
        }
        let (start, count) = (edit.start as usize / 5, edit.delete_count as usize / 5);
        let end = start.checked_add(count)?;
        // Edits mustn't overlap, and each is against the original array.
        if end > out.len() || end > floor {
            return None;
        }
        out.splice(start..end, edit.data.clone().unwrap_or_default());
        floor = start;
    }
    Some(out)
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

    fn tok(delta_line: u32, delta_start: u32, length: u32, token_type: u32, token_modifiers_bitset: u32) -> SemanticToken {
        SemanticToken { delta_line, delta_start, length, token_type, token_modifiers_bitset }
    }

    fn style(slot: Slot) -> Style {
        Style { slot, underline: false }
    }

    fn caps(json: serde_json::Value) -> lsp_types::ServerCapabilities {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_server_offering_full_with_delta_and_range_is_read_as_such() {
        let p = provider(&caps(serde_json::json!({"semanticTokensProvider": {"legend": {"tokenTypes": ["function"], "tokenModifiers": []}, "full": {"delta": true}, "range": true}}))).unwrap();
        assert!(p.full && p.delta && p.range);
        assert_eq!(p.legend.token_types[0].as_str(), "function");
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
    fn the_advertised_capabilities_serialize_as_the_spec_names_them() {
        let v = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(v["requests"]["full"]["delta"], true);
        assert_eq!(v["requests"]["range"], true);
        assert_eq!(v["formats"][0], "relative");
        assert_eq!(v["augmentsSyntaxTokens"], true);
        assert!(v["tokenTypes"].as_array().unwrap().iter().any(|t| t == "typeParameter"));
    }

    #[test]
    fn standard_types_map_to_their_slots_and_unknown_ones_to_none() {
        assert_eq!(slot_for_type("parameter"), Some(Slot::Parameter));
        assert_eq!(slot_for_type("interface"), Some(Slot::Interface));
        assert_eq!(slot_for_type("macro"), Some(Slot::Macro));
        assert_eq!(slot_for_type("enumMember"), Some(Slot::Constant));
        assert_eq!(slot_for_type("decorator"), Some(Slot::Attribute));
        assert_eq!(slot_for_type("builtinType"), Some(Slot::Type));
        assert_eq!(slot_for_type("unresolvedReference"), None);
        assert_eq!(slot_for_type("generic"), None);
    }

    #[test]
    fn modifiers_make_constants_deprecations_and_underlines() {
        let s = Styles::new(&legend(&["variable", "function", "property"], &["declaration", "readonly", "deprecated", "mutable"]));
        assert_eq!(s.style(0, 0), Some(style(Slot::Variable)));
        assert_eq!(s.style(0, 0b0010), Some(style(Slot::Constant)), "a readonly variable");
        assert_eq!(s.style(2, 0b0010), Some(style(Slot::Constant)), "a readonly property");
        assert_eq!(s.style(1, 0b0010), Some(style(Slot::Function)), "readonly says nothing of a function");
        assert_eq!(s.style(1, 0b0100), Some(style(Slot::Comment)), "deprecated");
        assert_eq!(s.style(0, 0b1000), Some(Style { slot: Slot::Variable, underline: true }), "mutable");
        assert_eq!(s.style(0, 0b0001), Some(style(Slot::Variable)), "other modifiers are ignored");
        assert_eq!(s.style(9, 0), None, "a type index past the legend");
    }

    fn styles() -> Styles {
        Styles::new(&legend(&["function", "variable", "unresolvedReference"], &[]))
    }

    #[test]
    fn relative_tokens_decode_to_char_ranges() {
        let text = "fn main() {\n    let x = foo();\n}\n";
        // `main` on line 0 col 3; `x` line 1 col 8; `foo` line 1 col 12.
        let data = [tok(0, 3, 4, 0, 0), tok(1, 8, 1, 1, 0), tok(0, 4, 3, 0, 0)];
        let got = decode(&data, text, &styles());
        let spans: Vec<&str> = got.iter().map(|t| &text[t.start..t.end]).collect();
        assert_eq!(spans, ["main", "x", "foo"]);
        assert_eq!(got[1].style.slot, Slot::Variable);
    }

    #[test]
    fn a_type_mapped_to_nothing_is_left_out_but_still_counts_for_positions() {
        let text = "a b c";
        let data = [tok(0, 0, 1, 0, 0), tok(0, 2, 1, 2, 0), tok(0, 2, 1, 1, 0)];
        let got = decode(&data, text, &styles());
        assert_eq!(got.iter().map(|t| t.start).collect::<Vec<_>>(), [0, 4]);
    }

    #[test]
    fn utf16_columns_count_a_surrogate_pair_as_two() {
        // "😀" is one char, two UTF-16 units; `x` is at UTF-16 column 3.
        let text = "😀 x y";
        let data = [tok(0, 3, 1, 1, 0), tok(0, 2, 1, 1, 0)];
        let got = decode(&data, text, &styles());
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(got.iter().map(|t| chars[t.start]).collect::<Vec<_>>(), ['x', 'y']);
        assert_eq!((got[0].start, got[0].end), (2, 3));
    }

    #[test]
    fn crlf_lines_and_offsets_count_both_break_chars() {
        let text = "ab\r\ncd\r\n";
        let got = decode(&[tok(1, 0, 2, 0, 0)], text, &styles());
        assert_eq!((got[0].start, got[0].end), (4, 6));
    }

    #[test]
    fn a_token_past_its_line_or_the_text_is_dropped_and_one_too_long_is_cut() {
        let text = "abc\nde";
        let got = decode(&[tok(0, 1, 9, 0, 0), tok(0, 5, 1, 0, 0), tok(1, 0, 1, 0, 0), tok(5, 0, 1, 0, 0)], text, &styles());
        assert_eq!(got.iter().map(|t| (t.start, t.end)).collect::<Vec<_>>(), [(1, 3), (4, 5)]);
    }

    #[test]
    fn a_server_going_back_on_its_line_is_still_read_right() {
        let text = "abcdef";
        // Second token's start (2) is before the first's end (4): overlapping
        // input Fenix didn't ask for, decoded where it says all the same.
        let got = decode(&[tok(0, 0, 4, 0, 0), tok(0, 2, 1, 0, 0)], text, &styles());
        assert_eq!(got.iter().map(|t| (t.start, t.end)).collect::<Vec<_>>(), [(0, 4), (2, 3)]);
    }

    #[test]
    fn delta_edits_splice_the_array_last_first() {
        let data: Vec<SemanticToken> = (0..4).map(|i| tok(0, i, 1, 0, 0)).collect();
        let edits = [
            SemanticTokensEdit { start: 5, delete_count: 5, data: Some(vec![tok(9, 9, 9, 9, 9)]) },
            SemanticTokensEdit { start: 15, delete_count: 5, data: None },
        ];
        let got = apply_edits(&data, &edits).unwrap();
        assert_eq!(got, vec![tok(0, 0, 1, 0, 0), tok(9, 9, 9, 9, 9), tok(0, 2, 1, 0, 0)]);
    }

    #[test]
    fn an_edit_off_a_token_boundary_or_past_the_end_fails() {
        let data = vec![tok(0, 0, 1, 0, 0)];
        assert!(apply_edits(&data, &[SemanticTokensEdit { start: 3, delete_count: 0, data: None }]).is_none());
        assert!(apply_edits(&data, &[SemanticTokensEdit { start: 5, delete_count: 5, data: None }]).is_none());
        assert!(apply_edits(&data, &[SemanticTokensEdit { start: 5, delete_count: 0, data: Some(vec![tok(1, 1, 1, 1, 1)]) }]).is_some(), "appending at the end");
    }

    #[test]
    fn overlapping_edits_fail() {
        let data: Vec<SemanticToken> = (0..4).map(|i| tok(0, i, 1, 0, 0)).collect();
        let edits = [SemanticTokensEdit { start: 0, delete_count: 10, data: None }, SemanticTokensEdit { start: 5, delete_count: 5, data: None }];
        assert!(apply_edits(&data, &edits).is_none());
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

    #[test]
    fn changed_span_is_the_one_replaced_range() {
        assert_eq!(crate::changed_span("let x = 1;", "let xy = 1;"), (5, 5, 6));
        assert_eq!(crate::changed_span("abc", "abc"), (3, 3, 3));
        assert_eq!(crate::changed_span("a\r\nb", "a\nb"), (1, 3, 2), "the whole break is replaced, never half of it");
    }
}
