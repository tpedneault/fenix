//! JSON with comments, as VS Code writes its `settings.json` and
//! `keybindings.json`: `//` and `/* */` comments, and a comma after the
//! last item of an object or a list. Both are taken out, keeping the
//! line breaks so a mistake still says its line, and what's left is
//! read as JSON.

/// `text` read as JSON with comments.
pub fn parse(text: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(&strip(text)).map_err(|e| format!("line {}: {e}", e.line()))
}

/// `text` without its comments and trailing commas. Strings are kept as
/// they are, `//` and all.
pub fn strip(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    // Where a comma was written, kept out until what follows it says
    // whether it ends a list (`]`, `}`: dropped) or goes on.
    let mut held_comma: Option<String> = None;
    let flush = |out: &mut String, held: &mut Option<String>, keep_comma: bool| {
        if let Some(run) = held.take() {
            if keep_comma {
                out.push(',');
            }
            out.push_str(&run);
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                flush(&mut out, &mut held_comma, true);
                out.push('"');
                while let Some(s) = chars.next() {
                    out.push(s);
                    match s {
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                out.push(escaped);
                            }
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => {
                // To the end of the line; the line break stays.
                for s in chars.by_ref() {
                    if s == '\n' {
                        push(&mut out, &mut held_comma, '\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = '\0';
                for s in chars.by_ref() {
                    if s == '\n' {
                        push(&mut out, &mut held_comma, '\n');
                    }
                    if last == '*' && s == '/' {
                        break;
                    }
                    last = s;
                }
            }
            ',' => {
                flush(&mut out, &mut held_comma, true);
                held_comma = Some(String::new());
            }
            ']' | '}' => {
                flush(&mut out, &mut held_comma, false);
                out.push(c);
            }
            c if c.is_whitespace() => push(&mut out, &mut held_comma, c),
            c => {
                flush(&mut out, &mut held_comma, true);
                out.push(c);
            }
        }
    }
    flush(&mut out, &mut held_comma, true);
    out
}

/// Whitespace: after a held comma it waits with it.
fn push(out: &mut String, held: &mut Option<String>, c: char) {
    match held {
        Some(run) => run.push(c),
        None => out.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas_go_and_strings_stay() {
        let text = "\u{feff}{\n  // the font\n  \"editor.fontFamily\": \"'Fira Code', monospace\", /* inline */\n  \"url\": \"http://x//y\",\n  \"a\": [1, 2,],\n  \"quote\": \"say \\\"hi\\\" // not a comment\",\n}\n";
        let value = parse(text).unwrap();
        assert_eq!(value["editor.fontFamily"], "'Fira Code', monospace");
        assert_eq!(value["url"], "http://x//y");
        assert_eq!(value["a"], serde_json::json!([1, 2]));
        assert_eq!(value["quote"], "say \"hi\" // not a comment");
    }

    #[test]
    fn a_mistake_says_its_line() {
        let err = parse("{\n// one\n\"a\": 1\n\"b\": 2\n}").unwrap_err();
        assert!(err.starts_with("line 4"), "{err}");
    }

    #[test]
    fn an_empty_keybindings_file_is_a_list() {
        assert_eq!(parse("// Place your key bindings in this file\n[\n]\n").unwrap(), serde_json::json!([]));
    }
}
