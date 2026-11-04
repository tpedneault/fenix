//! Reading a `:` command: its line range and its name. Pure, so the odd
//! cases are tested here; `VimState` does what's read.

/// A line: a number, `.`, `$`, and any `+N`/`-N` after it. `here` is the
/// cursor's line, `last` the buffer's; both from 0, like the result. A
/// line is read as written, past `last` or not: what's out of range is
/// for the caller to say.
fn parse_line(text: &str, here: usize, last: usize) -> Option<(isize, &str)> {
    let digits = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (mut line, mut rest) = match text.chars().next()? {
        '.' => (here as isize, &text[1..]),
        '$' => (last as isize, &text[1..]),
        '+' | '-' => (here as isize, text),
        c if c.is_ascii_digit() => {
            let end = digits(text);
            (text[..end].parse::<isize>().ok()? - 1, &text[end..])
        }
        _ => return None,
    };
    while let Some(sign) = rest.chars().next().filter(|c| matches!(c, '+' | '-')) {
        let end = digits(&rest[1..]);
        // A bare `+` is one.
        let amount = if end == 0 {
            1
        } else {
            rest[1..=end].parse::<isize>().ok()?
        };
        line += if sign == '+' { amount } else { -amount };
        rest = &rest[1 + end..];
    }
    Some((line, rest))
}

/// Splits a leading range off `cmd`: `%`, a line, or `a,b` (or `a;b`), in
/// order. `None` when there's none, `cmd` untouched.
pub(crate) fn parse_range(cmd: &str, here: usize, last: usize) -> (Option<(isize, isize)>, &str) {
    if let Some(rest) = cmd.strip_prefix('%') {
        return (Some((0, last as isize)), rest);
    }
    let Some((first, rest)) = parse_line(cmd, here, last) else {
        return (None, cmd);
    };
    let Some(after) = rest.strip_prefix([',', ';']) else {
        return (Some((first, first)), rest);
    };
    match parse_line(after, here, last) {
        Some((second, rest)) => (Some((first.min(second), first.max(second))), rest),
        None => (Some((first, here as isize)), after),
    }
}

/// What the rest of a command line does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExCommand<'a> {
    Delete,
    Yank,
    /// To after this line (-1 is the top).
    Move(isize),
    Copy(isize),
    /// `>`s or `<`s, and how many.
    Shift {
        right: bool,
        by: usize,
    },
    Join,
    Normal(&'a str),
    Sort(SortBy),
    Edit {
        path: &'a str,
        force: bool,
    },
    Split {
        vertical: bool,
        path: &'a str,
    },
    /// One of Fenix's own commands.
    Run(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SortBy {
    pub reverse: bool,
    pub ignore_case: bool,
    pub numeric: bool,
    pub unique: bool,
}

/// The shortest name each command goes by, and its whole name.
const NAMES: &[(&str, &str)] = &[
    ("d", "delete"),
    ("y", "yank"),
    ("m", "move"),
    ("t", "t"),
    ("co", "copy"),
    ("j", "join"),
    ("norm", "normal"),
    ("sor", "sort"),
    ("e", "edit"),
    ("sp", "split"),
    ("vs", "vsplit"),
    ("bd", "bdelete"),
    ("bn", "bnext"),
    ("bp", "bprevious"),
    ("on", "only"),
];

fn full_name(word: &str) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(short, name)| word.len() >= short.len() && name.starts_with(word))
        .map(|(_, name)| *name)
}

/// `Err` says what's wrong; `Ok(None)` is a command that isn't read here
/// (`:s`, `:g`, `:set`, `:w`, ...).
pub(crate) fn parse_command(
    text: &str,
    here: usize,
    last: usize,
) -> Result<Option<ExCommand<'_>>, String> {
    let text = text.trim_start();
    if text.starts_with(['>', '<']) {
        let right = text.starts_with('>');
        let by = text
            .chars()
            .take_while(|&c| c == if right { '>' } else { '<' })
            .count();
        return Ok(Some(ExCommand::Shift { right, by }));
    }
    let end = text
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (word, rest) = text.split_at(end);
    let Some(name) = full_name(word) else {
        return Ok(None);
    };
    let (force, args) = match rest.strip_prefix('!') {
        Some(rest) => (true, rest.trim()),
        None => (false, rest.trim()),
    };
    // What follows a command's name is a space or nothing: `:d/x/` isn't
    // `:delete`, nor is `:e.` anything but an edit, which takes a path.
    if !force
        && !rest.is_empty()
        && !rest.starts_with(char::is_whitespace)
        && !matches!(name, "move" | "t" | "copy")
    {
        return Ok(None);
    }
    let destination = || match parse_line(args, here, last) {
        Some((line, "")) => Ok(line),
        _ => Err(format!(":{name} needs a line to go to")),
    };
    Ok(Some(match name {
        "delete" => ExCommand::Delete,
        "yank" => ExCommand::Yank,
        "move" => ExCommand::Move(destination()?),
        "t" | "copy" => ExCommand::Copy(destination()?),
        "join" => ExCommand::Join,
        "normal" => {
            if args.is_empty() {
                return Err("Argument required".to_string());
            }
            ExCommand::Normal(args)
        }
        "sort" => {
            let mut by = SortBy {
                reverse: force,
                ..SortBy::default()
            };
            for flag in args.chars() {
                match flag {
                    'i' => by.ignore_case = true,
                    'n' => by.numeric = true,
                    'u' => by.unique = true,
                    _ if flag.is_whitespace() => {}
                    _ => return Err(format!("Invalid argument: {args}")),
                }
            }
            ExCommand::Sort(by)
        }
        "edit" => ExCommand::Edit { path: args, force },
        "split" => ExCommand::Split {
            vertical: false,
            path: args,
        },
        "vsplit" => ExCommand::Split {
            vertical: true,
            path: args,
        },
        "bdelete" => ExCommand::Run(if force {
            "buffer.kill_force"
        } else {
            "buffer.kill"
        }),
        "bnext" => ExCommand::Run("buffer.next"),
        "bprevious" => ExCommand::Run("buffer.prev"),
        _ => ExCommand::Run("window.only"),
    }))
}

/// A pattern as Vim writes it, as the `regex` crate reads it. By default
/// `\(`, `\)`, `\|`, `\+`, `\?`, `\{` and `\}` are the operators and the
/// bare characters are literal, the other way round from `regex`; after
/// `\v` everything is an operator already. `\<` and `\>` are word edges.
pub(crate) fn pattern_to_regex(pattern: &str) -> String {
    let (very_magic, pattern) = match pattern.strip_prefix(r"\v") {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        if in_class {
            out.push(c);
            if c == '\\' {
                out.extend(chars.next());
            } else if c == ']' {
                in_class = false;
            }
            continue;
        }
        match c {
            '[' => {
                in_class = true;
                out.push(c);
                // `[]a]` and `[^]a]`: a `]` first is a member of the class.
                let first = chars.clone().next();
                if first == Some('^') {
                    out.extend(chars.next());
                }
                if chars.clone().next() == Some(']') {
                    chars.next();
                    out.push_str(r"\]");
                }
            }
            '\\' => match chars.next() {
                Some(o @ ('(' | ')' | '|' | '+' | '?' | '{' | '}')) if very_magic => {
                    out.push('\\');
                    out.push(o);
                }
                Some(o @ ('(' | ')' | '|' | '+' | '?' | '{' | '}')) => out.push(o),
                Some('=') if !very_magic => out.push('?'),
                Some('<' | '>') => out.push_str(r"\b"),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push_str(r"\\"),
            },
            '<' | '>' if very_magic => out.push_str(r"\b"),
            '=' if very_magic => out.push('?'),
            '(' | ')' | '|' | '+' | '?' | '{' | '}' if !very_magic => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_read_as_vim_reads_them() {
        let range = |text: &'static str| parse_range(text, 10, 99);
        assert_eq!(range("5"), (Some((4, 4)), ""));
        assert_eq!(range("2,3d"), (Some((1, 2)), "d"));
        assert_eq!(range("3,2d"), (Some((1, 2)), "d"), "either way round");
        assert_eq!(range("%d"), (Some((0, 99)), "d"));
        assert_eq!(range("."), (Some((10, 10)), ""));
        assert_eq!(range("$"), (Some((99, 99)), ""));
        assert_eq!(range(".+1d"), (Some((11, 11)), "d"));
        assert_eq!(range("+2"), (Some((12, 12)), ""));
        assert_eq!(range("-"), (Some((9, 9)), ""));
        assert_eq!(range(".,$-1y"), (Some((10, 98)), "y"));
        assert_eq!(range("d"), (None, "d"));
    }

    #[test]
    fn commands_go_by_their_short_names() {
        let read = |text: &'static str| parse_command(text, 4, 9);
        assert_eq!(read("d"), Ok(Some(ExCommand::Delete)));
        assert_eq!(read("del"), Ok(Some(ExCommand::Delete)));
        assert_eq!(read("delete"), Ok(Some(ExCommand::Delete)));
        assert_eq!(read("y"), Ok(Some(ExCommand::Yank)));
        assert_eq!(read("m3"), Ok(Some(ExCommand::Move(2))));
        assert_eq!(read("m 0"), Ok(Some(ExCommand::Move(-1))));
        assert_eq!(read("t$"), Ok(Some(ExCommand::Copy(9))));
        assert_eq!(read("co ."), Ok(Some(ExCommand::Copy(4))));
        assert_eq!(
            read(">>"),
            Ok(Some(ExCommand::Shift { right: true, by: 2 }))
        );
        assert_eq!(
            read("<"),
            Ok(Some(ExCommand::Shift {
                right: false,
                by: 1
            }))
        );
        assert_eq!(read("norm Ax"), Ok(Some(ExCommand::Normal("Ax"))));
        assert_eq!(
            read("sort! i"),
            Ok(Some(ExCommand::Sort(SortBy {
                reverse: true,
                ignore_case: true,
                ..SortBy::default()
            })))
        );
        assert_eq!(
            read("e src/a b.rs"),
            Ok(Some(ExCommand::Edit {
                path: "src/a b.rs",
                force: false
            }))
        );
        assert_eq!(
            read("vs"),
            Ok(Some(ExCommand::Split {
                vertical: true,
                path: ""
            }))
        );
        assert_eq!(read("bd!"), Ok(Some(ExCommand::Run("buffer.kill_force"))));
    }

    #[test]
    fn what_isnt_a_line_command_is_left_for_the_rest() {
        for text in ["s/a/b/", "set sw=2", "g/x/d", "nope", "w", "wq", "garbage"] {
            assert_eq!(parse_command(text, 0, 9), Ok(None), "{text}");
        }
        assert!(parse_command("m", 0, 9).is_err(), "nowhere to go");
        assert!(parse_command("norm", 0, 9).is_err());
    }

    #[test]
    fn patterns_read_as_vims() {
        assert_eq!(pattern_to_regex(r"\(a\)l"), "(a)l");
        assert_eq!(pattern_to_regex(r"a\|b"), "a|b");
        assert_eq!(pattern_to_regex(r"a\+"), "a+");
        assert_eq!(pattern_to_regex(r"\<word\>"), r"\bword\b");
        assert_eq!(
            pattern_to_regex("f(x)+1"),
            r"f\(x\)\+1",
            "bare, they're literal"
        );
        assert_eq!(pattern_to_regex(r"\v(t.)+"), "(t.)+");
        assert_eq!(pattern_to_regex(r"\v<word>"), r"\bword\b");
        assert_eq!(pattern_to_regex(r"\d\+\s*\w"), r"\d+\s*\w");
        assert_eq!(
            pattern_to_regex("[()+]x"),
            "[()+]x",
            "a class is left alone"
        );
        assert_eq!(pattern_to_regex("a.*b$"), "a.*b$");
    }
}
