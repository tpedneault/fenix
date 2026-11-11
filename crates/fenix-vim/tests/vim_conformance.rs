//! Every case in `vim_conformance.txt`, through Fenix's Vim engine: the
//! text, the cursor and the unnamed register must come out as Vim leaves
//! them. `dev/vim-conformance/check.py` checks the same file against real
//! Vim, so the two can't drift apart unnoticed.

use fenix_core::{Buffer, Cursor};
use fenix_keymap::{KeyPress, NamedKey};
use fenix_vim::VimState;

struct Case {
    name: String,
    line: usize,
    start: String,
    keys: String,
    end: String,
    reg: Option<String>,
}

/// A JSON string's text: `"a\nb"`.
fn unquote(value: &str, line: usize) -> String {
    let inner = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or_else(|| panic!("line {line}: {value} isn't a quoted string"));
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).expect("\\u escape")).expect("a char"));
            }
            other => panic!("line {line}: unknown escape \\{other:?}"),
        }
    }
    out
}

/// A case as read so far: its name, the line it's on, its fields.
type Partial = Option<(String, usize, Vec<(String, String)>)>;

fn cases() -> Vec<Case> {
    let text = include_str!("vim_conformance.txt");
    let mut cases = Vec::new();
    let mut current: Partial = None;
    let mut finish = |current: &mut Partial| {
        let Some((name, line, fields)) = current.take() else { return };
        let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        if get("differs").is_some() {
            return;
        }
        cases.push(Case {
            name,
            line,
            start: unquote(&get("start").expect("start"), line),
            keys: get("keys").expect("keys"),
            end: unquote(&get("end").expect("end"), line),
            reg: get("reg").map(|r| unquote(&r, line)),
        });
    };
    for (number, raw) in text.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            finish(&mut current);
        } else if trimmed.starts_with('#') {
        } else if let Some((_, _, fields)) = current.as_mut() {
            let (key, value) = trimmed.split_once(':').expect("a `key: value` line");
            fields.push((key.trim().to_string(), value.trim().to_string()));
        } else {
            current = Some((trimmed.to_string(), number + 1, Vec::new()));
        }
    }
    finish(&mut current);
    cases
}

/// Vim's key notation as key presses: `dw<Esc>`, `<C-w>`, `<lt>`.
fn keys(notation: &str) -> Vec<KeyPress> {
    let mut out = Vec::new();
    let mut rest = notation;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            if let Some(end) = rest.find('>').filter(|&e| e > 1) {
                let name = &rest[1..end];
                let key = match name.to_ascii_lowercase().as_str() {
                    "esc" => Some(KeyPress::named(NamedKey::Escape)),
                    "cr" | "enter" => Some(KeyPress::named(NamedKey::Enter)),
                    "bs" => Some(KeyPress::named(NamedKey::Backspace)),
                    "tab" => Some(KeyPress::named(NamedKey::Tab)),
                    "lt" => Some(KeyPress::char('<')),
                    "space" => Some(KeyPress::char(' ')),
                    lower => lower.strip_prefix("c-").and_then(|k| k.chars().next()).map(|k| KeyPress::char(k).with_ctrl()),
                };
                if let Some(key) = key {
                    out.push(key);
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        }
        out.push(KeyPress::char(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn buffer(text: &str) -> Buffer {
    let mut b = Buffer::empty();
    let mut c = Cursor::at_start();
    b.insert_str(&mut c, text);
    b
}

/// The text with `|` where the cursor is.
fn shown(buffer: &Buffer, cursor: &Cursor) -> String {
    let text = buffer.text();
    let at = cursor.char_idx.min(text.chars().count());
    let mut out: String = text.chars().take(at).collect();
    out.push('|');
    out.extend(text.chars().skip(at));
    out
}

#[test]
fn fenix_does_what_vim_does() {
    let cases = cases();
    assert!(cases.len() > 100, "the cases file reads ({} cases)", cases.len());
    let mut failures = Vec::new();
    for case in &cases {
        let at = case.start.find('|').unwrap_or_else(|| panic!("line {}: no | in start", case.line));
        let text = case.start.replacen('|', "", 1);
        let mut b = buffer(&text);
        let mut c = Cursor { char_idx: text[..at].chars().count(), sticky_col: 0 };
        c.sticky_col = b.line_col(&c).1;
        let mut vim = VimState::new();
        for key in keys(&case.keys) {
            vim.handle_key(&mut b, &mut c, key);
        }
        let got = shown(&b, &c);
        let reg = vim.register().0.to_string();
        if got != case.end || case.reg.as_ref().is_some_and(|r| *r != reg) {
            failures.push(format!("line {}: {}\n    keys: {}\n    want: {:?} reg {:?}\n    got:  {:?} reg {:?}", case.line, case.name, case.keys, case.end, case.reg, got, reg));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ from Vim:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
