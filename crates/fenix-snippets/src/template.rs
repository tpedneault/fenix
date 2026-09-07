use std::{collections::BTreeMap, ops::Range, path::Path};

/// A snapshot: editing a header must not change its creation time.
#[derive(Clone, Debug, Default)]
pub struct Context(pub BTreeMap<String, String>);

impl Context {
    pub fn current(path: Option<&Path>) -> Self {
        let now = chrono::Local::now();
        let user = std::env::var("FENIX_SNIPPET_USER")
            .or_else(|_| std::env::var("USERNAME"))
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_default();
        Self(BTreeMap::from([
            ("DATE".into(), now.format("%Y-%m-%d").to_string()),
            ("TIME".into(), now.format("%H:%M:%S").to_string()),
            ("YEAR".into(), now.format("%Y").to_string()),
            ("DATETIME".into(), now.to_rfc3339()),
            ("USER_NAME".into(), user),
            (
                "FILENAME".into(),
                path.and_then(Path::file_name)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            (
                "FILE_STEM".into(),
                path.and_then(Path::file_stem)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            (
                "FILEPATH".into(),
                path.map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
            (
                "DIRECTORY".into(),
                path.and_then(Path::parent)
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            ),
        ]))
    }
}

#[derive(Clone, Debug)]
enum Transform {
    Trim,
    Upper,
    Lower,
    RemoveWhitespace,
    Center(usize),
}

impl Transform {
    fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "trim" => Self::Trim,
            "upper" => Self::Upper,
            "lower" => Self::Lower,
            "remove_whitespace" => Self::RemoveWhitespace,
            _ => {
                let width = s
                    .strip_prefix("center:")
                    .and_then(|n| n.parse::<usize>().ok())
                    .filter(|n| (1..=4096).contains(n))
                    .ok_or_else(|| format!("unknown transform or invalid width: {s}"))?;
                Self::Center(width)
            }
        })
    }

    fn apply(&self, value: String) -> String {
        match self {
            Self::Trim => value.trim().to_string(),
            Self::Upper => value.to_uppercase(),
            Self::Lower => value.to_lowercase(),
            Self::RemoveWhitespace => value.chars().filter(|c| !c.is_whitespace()).collect(),
            Self::Center(width) => value
                .split('\n')
                .map(|line| {
                    let padding = width.saturating_sub(line.chars().count());
                    format!(
                        "{}{}{}",
                        " ".repeat(padding / 2),
                        line,
                        " ".repeat(padding - padding / 2)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Clone, Debug)]
enum Part {
    Text(String),
    Field(u32, Vec<Transform>),
    Variable(String, Vec<Transform>),
}

#[derive(Clone, Debug)]
pub struct Template {
    parts: Vec<Part>,
    pub(crate) defaults: BTreeMap<u32, String>,
}

#[derive(Debug)]
pub struct Rendered {
    pub text: String,
    /// First untransformed occurrence of each editable field.
    pub fields: BTreeMap<u32, Range<usize>>,
    pub exit: usize,
}

fn unescape(s: &str) -> String {
    let mut chars = s.chars().peekable();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|c| matches!(c, '$' | '}' | '\\')) {
            result.push(chars.next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}

impl Template {
    pub fn parse(body: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut defaults = BTreeMap::new();
        let mut defined = std::collections::BTreeSet::new();
        let mut text = String::new();
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' && chars.peek().is_some_and(|c| matches!(c, '$' | '}' | '\\')) {
                text.push(chars.next().unwrap());
                continue;
            }
            if c != '$'
                || !chars
                    .peek()
                    .is_some_and(|c| *c == '{' || c.is_ascii_digit())
            {
                text.push(c);
                continue;
            }
            if !text.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut text)));
            }
            let expr = if chars.peek() == Some(&'{') {
                chars.next();
                let mut expr = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '}' {
                        closed = true;
                        break;
                    }
                    expr.push(c);
                    if c == '\\' {
                        if let Some(next) = chars.next() {
                            expr.push(next);
                        }
                    } else if c == '$' && chars.peek() == Some(&'{') {
                        return Err("nested placeholders are not supported".into());
                    }
                }
                if !closed {
                    return Err("unclosed ${ expression".into());
                }
                expr
            } else {
                let mut expr = String::new();
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    expr.push(chars.next().unwrap());
                }
                expr
            };
            // A colon immediately after the field number introduces literal
            // default text. Pipes within a default are ordinary text.
            let head_end = expr.find([':', '|']).unwrap_or(expr.len());
            let head = &expr[..head_end];
            let tail = &expr[head_end..];
            let transforms = if let Some(pipeline) = tail.strip_prefix('|') {
                pipeline
                    .split('|')
                    .map(Transform::parse)
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
                let id = head.parse::<u32>().map_err(|_| "field number too large")?;
                if id == 0 && !tail.is_empty() {
                    return Err("$0 cannot have a default or transform".into());
                }
                if let Some(default) = tail.strip_prefix(':') {
                    if !defined.insert(id) {
                        return Err(format!("duplicate default for field {id}"));
                    }
                    defaults.insert(id, unescape(default));
                }
                parts.push(Part::Field(id, transforms));
            } else {
                if !matches!(
                    head,
                    "DATE"
                        | "TIME"
                        | "YEAR"
                        | "DATETIME"
                        | "USER_NAME"
                        | "FILENAME"
                        | "FILE_STEM"
                        | "FILEPATH"
                        | "DIRECTORY"
                ) {
                    return Err(format!("unknown variable: {head}"));
                }
                if tail.starts_with(':') {
                    return Err("variables do not accept defaults".into());
                }
                parts.push(Part::Variable(head.to_string(), transforms));
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        let mut editable = std::collections::BTreeSet::new();
        let mut exits = 0;
        for part in &parts {
            if let Part::Field(id, transforms) = part {
                if *id == 0 {
                    exits += 1;
                } else if transforms.is_empty() {
                    editable.insert(*id);
                    defaults.entry(*id).or_default();
                }
            }
        }
        if exits > 1 {
            return Err("only one $0 is allowed".into());
        }
        for part in &parts {
            if let Part::Field(id, transforms) = part {
                if !transforms.is_empty() && !editable.contains(id) {
                    return Err(format!("transform refers to missing editable field {id}"));
                }
            }
        }
        Ok(Self { parts, defaults })
    }

    pub fn render(
        &self,
        values: &BTreeMap<u32, String>,
        context: &Context,
        indent: &str,
    ) -> Rendered {
        let mut text = String::new();
        let mut pos = 0;
        let mut fields = BTreeMap::new();
        let mut exit = None;
        for part in &self.parts {
            let (mut value, transforms) = match part {
                Part::Text(value) => (value.clone(), &[][..]),
                Part::Field(id, transforms) => (
                    values
                        .get(id)
                        .or_else(|| self.defaults.get(id))
                        .cloned()
                        .unwrap_or_default(),
                    transforms.as_slice(),
                ),
                Part::Variable(name, transforms) => (
                    context.0.get(name).cloned().unwrap_or_default(),
                    transforms.as_slice(),
                ),
            };
            for transform in transforms {
                value = transform.apply(value);
            }
            value = value.replace('\n', &format!("\n{indent}"));
            let end = pos + value.chars().count();
            if let Part::Field(id, transforms) = part {
                if *id == 0 {
                    exit = Some(pos);
                } else if transforms.is_empty() {
                    fields.entry(*id).or_insert(pos..end);
                }
            }
            text.push_str(&value);
            pos = end;
        }
        Rendered {
            text,
            fields,
            exit: exit.unwrap_or(pos),
        }
    }
}
