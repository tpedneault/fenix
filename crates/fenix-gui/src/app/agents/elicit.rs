//! What an agent asks the user for (`elicitation/create`): a form, filled
//! in in a compose pane as a few lines of TOML that say what each field
//! takes, or a link opened in the browser. The card in the thread answers
//! with `y` (fill it in, or open the link), `Y` (the defaults as they
//! are), `n` (decline) and `x` (cancel).

use fenix_acp::hub::ThreadId;
use fenix_acp::protocol::{ElicitMode, Elicitation, FieldKind};
use fenix_acp::thread::ElicitationState;
use serde_json::{Map, Value};

use super::*;

pub(crate) use crate::agent_page::ElicitAnswer;

fn toml_value(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{s:?}"),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(toml_value).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

/// The form as the compose pane opens with it: a line per field, its
/// default (or an empty value) to change, and a comment saying what it
/// takes.
pub(crate) fn form_template(e: &Elicitation) -> String {
    let mut out = String::new();
    for line in e.message.lines() {
        out.push_str(&format!("# {line}\n"));
    }
    out.push_str("# Change the values, then Esc and Enter to send; q declines.\n\n");
    for f in &e.fields {
        let mut about = vec![f.title.clone()];
        if let Some(d) = &f.description {
            about.push(d.clone());
        }
        let takes = match &f.kind {
            FieldKind::Text {
                min, max, format, ..
            } => {
                let mut t = "text".to_string();
                if let Some(format) = format {
                    t = format!("{format} text");
                }
                if min.is_some() || max.is_some() {
                    t.push_str(&format!(
                        ", {}..{} characters",
                        min.unwrap_or(0),
                        max.map(|m| m.to_string()).unwrap_or_default()
                    ));
                }
                t
            }
            FieldKind::Number { integer, min, max } => {
                let mut t = if *integer {
                    "a whole number".to_string()
                } else {
                    "a number".to_string()
                };
                if min.is_some() || max.is_some() {
                    t.push_str(&format!(
                        ", {}..{}",
                        min.map(|m| m.to_string()).unwrap_or_default(),
                        max.map(|m| m.to_string()).unwrap_or_default()
                    ));
                }
                t
            }
            FieldKind::Boolean => "true or false".to_string(),
            FieldKind::Choice { options, multi } => {
                let list = options
                    .iter()
                    .map(|(v, t)| {
                        if v == t {
                            format!("{v:?}")
                        } else {
                            format!("{v:?} ({t})")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                if *multi {
                    format!("a list of: {list}")
                } else {
                    format!("one of: {list}")
                }
            }
            FieldKind::Unknown(kind) => format!("a {kind}"),
        };
        about.push(takes);
        if f.required {
            about.push("required".into());
        }
        out.push_str(&format!("# {}\n", about.join(" -- ")));
        let value = match (&f.default, &f.kind) {
            (Some(d), _) => toml_value(d),
            (None, FieldKind::Boolean) => "false".into(),
            (None, FieldKind::Choice { multi: true, .. }) => "[]".into(),
            (None, FieldKind::Number { .. }) => String::new(),
            (None, _) => "\"\"".into(),
        };
        out.push_str(&format!("{} = {value}\n\n", f.name));
    }
    out
}

/// The form's defaults, as the answer would carry them.
pub(crate) fn defaults(e: &Elicitation) -> Map<String, Value> {
    e.fields
        .iter()
        .filter_map(|f| Some((f.name.clone(), f.default.clone()?)))
        .collect()
}

/// What was written in the compose pane, read and checked against the
/// form: the content to send, or what's wrong with it.
pub(crate) fn read_form(e: &Elicitation, text: &str) -> Result<Map<String, Value>, Vec<String>> {
    // A field left as `name =` is left out, not a TOML error.
    let cleaned: String = text
        .lines()
        .filter(|l| !l.trim_end().ends_with('='))
        .collect::<Vec<_>>()
        .join("\n");
    let table: toml::Table = cleaned
        .parse()
        .map_err(|e: toml::de::Error| vec![e.message().to_string()])?;
    let mut content = Map::new();
    for f in &e.fields {
        let Some(v) = table.get(&f.name) else {
            continue;
        };
        let v = serde_json::to_value(v).unwrap_or(Value::Null);
        // An empty text for a field that isn't required is no answer.
        if v.as_str() == Some("") && !f.required {
            continue;
        }
        content.insert(f.name.clone(), v);
    }
    let unknown: Vec<String> = table
        .keys()
        .filter(|k| !e.fields.iter().any(|f| &f.name == *k))
        .map(|k| format!("{k} isn't one of the form's fields"))
        .collect();
    let mut problems = e.problems(&content);
    problems.extend(unknown);
    if problems.is_empty() {
        Ok(content)
    } else {
        Err(problems)
    }
}

/// What a card says was sent.
fn summary(content: &Map<String, Value>) -> String {
    content
        .iter()
        .map(|(k, v)| format!("{k}: {}", toml_value(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

impl App {
    fn agents_elicitation(&self, thread: &ThreadId, key: &str) -> Option<Elicitation> {
        let view = self.agents.hub.view(thread)?;
        view.thread
            .elicitation(key)
            .filter(|c| c.state == ElicitationState::Pending)
            .map(|c| c.request.clone())
    }

    /// A card answered from the thread (or the empty prompt).
    pub(crate) fn agents_elicit(&mut self, thread: &ThreadId, key: &str, how: ElicitAnswer) {
        let Some(e) = self.agents_elicitation(thread, key) else {
            return;
        };
        let fx = match (how, e.mode) {
            (ElicitAnswer::Decline, _) => self
                .agents
                .hub
                .answer_elicitation(thread, key, "decline", None, ""),
            (ElicitAnswer::Cancel, _) => self
                .agents
                .hub
                .answer_elicitation(thread, key, "cancel", None, ""),
            (_, ElicitMode::Url) => {
                let url = e.url.clone().unwrap_or_default();
                // Only a web address is opened; anything else is refused.
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    self.set_error(format!("the agent's link isn't a web address: {url}"));
                    self.agents.hub.answer_elicitation(
                        thread,
                        key,
                        "decline",
                        None,
                        "not a web address",
                    )
                } else {
                    #[cfg(test)]
                    let opened: std::io::Result<()> = {
                        self.wasm.opened.push(url.clone());
                        Ok(())
                    };
                    #[cfg(not(test))]
                    let opened = super::super::open_in_browser(&url);
                    match opened {
                        Ok(()) => {
                            self.set_message(format!(
                                "opened {url} -- the agent hears when it's done"
                            ));
                            self.agents
                                .hub
                                .answer_elicitation(thread, key, "accept", None, &url)
                        }
                        Err(why) => {
                            self.set_error(format!("couldn't open {url}: {why}"));
                            return;
                        }
                    }
                }
            }
            (ElicitAnswer::Defaults, ElicitMode::Form) => {
                let content = defaults(&e);
                let problems = e.problems(&content);
                if !problems.is_empty() {
                    self.set_error(format!(
                        "the defaults won't do: {} -- y fills it in",
                        problems.join("; ")
                    ));
                    return;
                }
                let said = summary(&content);
                self.agents.hub.answer_elicitation(
                    thread,
                    key,
                    "accept",
                    Some(Value::Object(content)),
                    &said,
                )
            }
            (ElicitAnswer::Fill, ElicitMode::Form) => {
                let purpose = ComposePurpose::Elicitation {
                    thread: thread.0.clone(),
                    key: key.to_string(),
                };
                self.open_compose_seeded(purpose, &form_template(&e));
                return;
            }
        };
        self.agents_run(fx);
        self.agents_dock_titles();
    }

    /// The form written in the compose pane: sent when it reads and fits,
    /// else what's wrong is said and the pane stays.
    pub(crate) fn agents_elicit_submit(&mut self, thread: &str, key: &str, text: &str) -> bool {
        let thread = ThreadId(thread.to_string());
        let Some(e) = self.agents_elicitation(&thread, key) else {
            self.set_error("the agent isn't waiting for this any more".to_string());
            return true;
        };
        match read_form(&e, text) {
            Ok(content) => {
                let said = summary(&content);
                let fx = self.agents.hub.answer_elicitation(
                    &thread,
                    key,
                    "accept",
                    Some(Value::Object(content)),
                    &said,
                );
                self.agents_run(fx);
                self.agents_dock_titles();
                true
            }
            Err(problems) => {
                self.set_error(format!("not sent: {}", problems.join("; ")));
                false
            }
        }
    }

    /// The compose pane closed with `q`: the form is declined.
    pub(crate) fn agents_elicit_discarded(&mut self, thread: &str, key: &str) {
        self.agents_elicit(&ThreadId(thread.to_string()), key, ElicitAnswer::Decline);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn form() -> Elicitation {
        Elicitation::parse(&json!({ "mode": "form", "message": "How?", "requestedSchema": { "type": "object", "properties": {
            "strategy": { "type": "string", "title": "Strategy", "oneOf": [{ "const": "careful", "title": "Careful" }, { "const": "quick", "title": "Quick" }], "default": "careful" },
            "files": { "type": "integer", "minimum": 1, "maximum": 20 },
            "tests": { "type": "boolean", "default": true },
            "note": { "type": "string" }
        }, "required": ["strategy", "files"] } }))
        .unwrap()
    }

    #[test]
    fn the_template_says_what_each_field_takes_and_reads_back() {
        let e = form();
        let text = form_template(&e);
        assert!(text.contains("strategy = \"careful\""), "{text}");
        assert!(
            text.contains("one of: \"careful\" (Careful), \"quick\" (Quick)"),
            "{text}"
        );
        assert!(
            text.contains("files = \n") || text.contains("files =\n"),
            "{text}"
        );
        // As it opens, the required count is missing.
        assert_eq!(
            read_form(&e, &text).unwrap_err(),
            vec!["files is required".to_string()]
        );
        let filled = text.replace("files = ", "files = 3");
        let content = read_form(&e, &filled).unwrap();
        assert_eq!(
            Value::Object(content),
            json!({ "strategy": "careful", "files": 3, "tests": true }),
            "an empty optional text is left out"
        );
    }

    #[test]
    fn wrong_answers_say_why() {
        let e = form();
        let errs = read_form(&e, "strategy = \"wild\"\nfiles = 30\nextra = 1\n").unwrap_err();
        assert!(
            errs.iter().any(|p| p.contains("strategy is one of")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|p| p.contains("files is out of range")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|p| p.contains("extra isn't one of")),
            "{errs:?}"
        );
        assert!(read_form(&e, "strategy = ").is_err());
        assert!(read_form(&e, "not toml at all [").is_err());
    }

    #[test]
    fn the_defaults_are_what_the_schema_gives() {
        assert_eq!(
            Value::Object(defaults(&form())),
            json!({ "strategy": "careful", "tests": true })
        );
    }
}
