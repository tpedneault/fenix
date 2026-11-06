//! What an agent asks the user for (`elicitation/create`): a form, or a
//! link opened in the browser. The card in the thread answers with `y`
//! (answer it, or open the link), `Y` (the defaults as they are), `n`
//! (decline) and `x` (cancel). A form is answered a field at a time: a
//! picker for what has choices, the prompt line for what's typed.

use fenix_acp::hub::ThreadId;
use fenix_acp::protocol::{ElicitMode, Elicitation, Field, FieldKind};
use fenix_acp::thread::ElicitationState;
use serde_json::{Map, Value};

use super::*;

pub(crate) use crate::agent_page::ElicitAnswer;

/// `CodePrompt::ext` of a form's field being typed.
pub(crate) const FORM_PROMPT: &str = "fenix:agents.form";

/// A form part way through: Esc anywhere leaves the card waiting, and `y`
/// starts it again.
pub(crate) struct FormFill {
    thread: ThreadId,
    key: String,
    /// The field being asked.
    at: usize,
    content: Map<String, Value>,
    /// A many-choice field's answers so far.
    ticked: Vec<String>,
}

/// What a row of a form's picker does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FormPick {
    /// This is the field's answer.
    Value(Value),
    /// Ticks or unticks one of many.
    Toggle(String),
    /// The ticked ones are the answer.
    Done,
    /// The answer isn't listed: type it.
    Own,
    /// No answer, for a field that doesn't need one.
    Skip,
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(plain).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// What a many-choice field starts with ticked.
fn ticked_at_first(f: Option<&Field>) -> Vec<String> {
    match f.and_then(|f| f.default.as_ref()) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// A field's rows, and the one to start on.
fn rows(f: &Field, ticked: &[String]) -> (Vec<(String, FormPick)>, usize) {
    let mut rows = Vec::new();
    let mut start = 0;
    let row = |what: &str| format!("{}: {what}", f.title);
    match &f.kind {
        FieldKind::Choice { options, multi: false } => {
            for (value, title) in options {
                if f.default.as_ref().and_then(Value::as_str) == Some(value) {
                    start = rows.len();
                }
                rows.push((row(title), FormPick::Value(Value::String(value.clone()))));
            }
            rows.push((row("something else…"), FormPick::Own));
        }
        FieldKind::Choice { options, multi: true } => {
            let own = ticked.iter().filter(|t| !options.iter().any(|(v, _)| v == *t)).map(|t| (t.clone(), t.clone()));
            for (value, title) in options.iter().cloned().chain(own) {
                let mark = if ticked.contains(&value) { "[x]" } else { "[ ]" };
                rows.push((row(&format!("{mark} {title}")), FormPick::Toggle(value)));
            }
            rows.push((row("something else…"), FormPick::Own));
            rows.push((row("that's all"), FormPick::Done));
        }
        _ => {
            for (title, value) in [("yes", true), ("no", false)] {
                if f.default.as_ref().and_then(Value::as_bool) == Some(value) {
                    start = rows.len();
                }
                rows.push((row(title), FormPick::Value(Value::Bool(value))));
            }
        }
    }
    if !f.required && !matches!(f.kind, FieldKind::Choice { multi: true, .. }) {
        rows.push((row("leave it out"), FormPick::Skip));
    }
    (rows, start)
}

/// What was typed for a field, as its value.
fn typed(f: &Field, input: &str) -> Result<Value, String> {
    let v = match &f.kind {
        FieldKind::Number { integer, .. } => {
            let bad = || format!("{} is {}", f.title, if *integer { "a whole number" } else { "a number" });
            if *integer {
                input.parse::<i64>().map(Value::from).map_err(|_| bad())?
            } else {
                input.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).ok_or_else(bad)?
            }
        }
        // What's typed over a list of choices is the point of typing it.
        FieldKind::Choice { .. } => return Ok(Value::String(input.to_string())),
        _ => Value::String(input.to_string()),
    };
    f.problem(&v).map_or(Ok(v), Err)
}

/// What a card says was sent.
fn summary(content: &Map<String, Value>) -> String {
    content.iter().map(|(k, v)| format!("{k}: {}", plain(v))).collect::<Vec<_>>().join(", ")
}

impl App {
    fn agents_elicitation(&self, thread: &ThreadId, key: &str) -> Option<Elicitation> {
        let view = self.agents.hub.view(thread)?;
        view.thread.elicitation(key).filter(|c| c.state == ElicitationState::Pending).map(|c| c.request.clone())
    }

    /// A card answered from the thread (or the empty prompt).
    pub(crate) fn agents_elicit(&mut self, thread: &ThreadId, key: &str, how: ElicitAnswer) {
        let Some(e) = self.agents_elicitation(thread, key) else { return };
        let fx = match (how, e.mode) {
            (ElicitAnswer::Decline, _) => self.agents.hub.answer_elicitation(thread, key, "decline", None, ""),
            (ElicitAnswer::Cancel, _) => self.agents.hub.answer_elicitation(thread, key, "cancel", None, ""),
            (_, ElicitMode::Url) => {
                let url = e.url.clone().unwrap_or_default();
                // Only a web address is opened; anything else is refused.
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    self.set_error(format!("the agent's link isn't a web address: {url}"));
                    self.agents.hub.answer_elicitation(thread, key, "decline", None, "not a web address")
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
                            self.set_message(format!("opened {url} -- the agent hears when it's done"));
                            self.agents.hub.answer_elicitation(thread, key, "accept", None, &url)
                        }
                        Err(why) => {
                            self.set_error(format!("couldn't open {url}: {why}"));
                            return;
                        }
                    }
                }
            }
            (ElicitAnswer::Defaults, ElicitMode::Form) => {
                let content = e.defaults();
                let problems = e.problems(&content);
                if !problems.is_empty() {
                    self.set_error(format!("the defaults won't do: {} -- y answers it", problems.join("; ")));
                    return;
                }
                let said = summary(&content);
                self.agents.hub.answer_elicitation(thread, key, "accept", Some(Value::Object(content)), &said)
            }
            (ElicitAnswer::Fill, ElicitMode::Form) => {
                self.agents.form = Some(FormFill { thread: thread.clone(), key: key.to_string(), at: 0, content: Map::new(), ticked: ticked_at_first(e.fields.first()) });
                self.agents_form_ask(0);
                return;
            }
        };
        self.agents_run(fx);
        self.agents_dock_titles();
    }

    /// The form being answered, and the field it's at: `None` inside when
    /// there's no more to ask. A form the agent gave up on ends here.
    fn agents_form_field(&mut self) -> Option<Option<Field>> {
        let form = self.agents.form.as_ref()?;
        let (thread, key, at) = (form.thread.clone(), form.key.clone(), form.at);
        let Some(e) = self.agents_elicitation(&thread, &key) else {
            self.agents.form = None;
            self.set_error("the agent isn't waiting for this any more".to_string());
            return None;
        };
        Some(e.fields.get(at).cloned())
    }

    /// Asks the field the form is at, or sends the form if that was the
    /// last. `row`: where a picker asked again starts.
    fn agents_form_ask(&mut self, row: usize) {
        let Some(field) = self.agents_form_field() else { return };
        let Some(f) = field else { return self.agents_form_send() };
        match &f.kind {
            FieldKind::Choice { .. } | FieldKind::Boolean => {
                let ticked = self.agents.form.as_ref().map(|form| form.ticked.clone()).unwrap_or_default();
                let (rows, start) = rows(&f, &ticked);
                let candidates = rows.into_iter().map(|(label, pick)| fenix_picker::Candidate::new(label, Pick::Form(pick))).collect();
                let mut picker = fenix_picker::PickerState::new(candidates);
                picker.move_selection(row.max(start) as isize);
                self.enter_picker(ActivePicker::Agents(picker));
                if let Some(about) = &f.description {
                    self.set_message(about.clone());
                }
            }
            _ => self.agents_form_type(&f, f.default.as_ref().map(plain).unwrap_or_default()),
        }
    }

    fn agents_form_type(&mut self, f: &Field, input: String) {
        let label = f.description.as_ref().map_or_else(|| f.title.clone(), |d| format!("{} ({d})", f.title));
        self.wasm.prompt = Some(crate::app::wasm_host::CodePrompt { ext: FORM_PROMPT.into(), id: 0, label, input, masked: false });
        self.wake_caret();
    }

    /// On to the next field, this one answered with `value`.
    fn agents_form_next(&mut self, f: &Field, value: Option<Value>) {
        let Some(form) = self.agents.form.as_mut() else { return };
        if let Some(value) = value {
            form.content.insert(f.name.clone(), value);
        }
        form.at += 1;
        let (thread, key, at) = (form.thread.clone(), form.key.clone(), form.at);
        let ticked = self.agents_elicitation(&thread, &key).map(|e| ticked_at_first(e.fields.get(at))).unwrap_or_default();
        if let Some(form) = self.agents.form.as_mut() {
            form.ticked = ticked;
        }
        self.agents_form_ask(0);
    }

    /// A row of a form's picker.
    pub(super) fn agents_form_picked(&mut self, pick: FormPick) {
        let Some(Some(f)) = self.agents_form_field() else { return };
        match pick {
            FormPick::Value(value) => self.agents_form_next(&f, Some(value)),
            FormPick::Skip => self.agents_form_next(&f, None),
            FormPick::Own => self.agents_form_type(&f, String::new()),
            FormPick::Toggle(value) => {
                let Some(form) = self.agents.form.as_mut() else { return };
                match form.ticked.iter().position(|t| *t == value) {
                    Some(i) => drop(form.ticked.remove(i)),
                    None => form.ticked.push(value.clone()),
                }
                let ticked = form.ticked.clone();
                let row = rows(&f, &ticked).0.iter().position(|(_, p)| *p == FormPick::Toggle(value.clone())).unwrap_or(0);
                self.agents_form_ask(row);
            }
            FormPick::Done => {
                let ticked = self.agents.form.as_ref().map(|form| form.ticked.clone()).unwrap_or_default();
                if ticked.is_empty() && f.required {
                    self.set_error(format!("{} needs at least one", f.title));
                    return self.agents_form_ask(0);
                }
                let value = (!ticked.is_empty()).then(|| Value::Array(ticked.into_iter().map(Value::String).collect()));
                self.agents_form_next(&f, value);
            }
        }
    }

    /// Enter on a form's prompt line.
    pub(crate) fn agents_form_typed(&mut self, input: &str) {
        let Some(Some(f)) = self.agents_form_field() else { return };
        let input = input.trim();
        let choice = matches!(f.kind, FieldKind::Choice { .. });
        if input.is_empty() {
            return match (choice, f.required) {
                // Back to the list.
                (true, _) => self.agents_form_ask(0),
                (false, false) => self.agents_form_next(&f, None),
                (false, true) => {
                    self.set_error(format!("{} is required", f.title));
                    self.agents_form_type(&f, String::new());
                }
            };
        }
        if matches!(f.kind, FieldKind::Choice { multi: true, .. }) {
            if let Some(form) = self.agents.form.as_mut() {
                if !form.ticked.iter().any(|t| t == input) {
                    form.ticked.push(input.to_string());
                }
            }
            return self.agents_form_ask(0);
        }
        match typed(&f, input) {
            Ok(value) => self.agents_form_next(&f, Some(value)),
            Err(why) => {
                self.set_error(why);
                self.agents_form_type(&f, input.to_string());
            }
        }
    }

    fn agents_form_send(&mut self) {
        let Some(form) = self.agents.form.take() else { return };
        let said = summary(&form.content);
        let fx = self.agents.hub.answer_elicitation(&form.thread, &form.key, "accept", Some(Value::Object(form.content)), &said);
        self.agents_run(fx);
        self.agents_dock_titles();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn form() -> Elicitation {
        Elicitation::parse(&json!({ "mode": "form", "message": "How?", "requestedSchema": { "type": "object", "properties": {
            "Data scope": { "type": "string", "enum": ["all", "some"], "default": "some" },
            "strategy": { "type": "array", "items": { "enum": ["a", "b"] }, "default": ["b"] },
            "files": { "type": "integer", "minimum": 1, "maximum": 20 },
            "ratio": { "type": "number" },
            "tests": { "type": "boolean", "default": false }
        }, "required": ["Data scope", "files"] } }))
        .unwrap()
    }

    fn field(name: &str) -> Field {
        form().fields.into_iter().find(|f| f.name == name).unwrap()
    }

    fn labels(rows: &[(String, FormPick)]) -> Vec<&str> {
        rows.iter().map(|(l, _)| l.as_str()).collect()
    }

    #[test]
    fn a_choice_lists_its_options_starts_on_the_default_and_offers_your_own() {
        let (one, start) = rows(&field("Data scope"), &[]);
        assert_eq!(labels(&one), ["Data scope: all", "Data scope: some", "Data scope: something else…"]);
        assert_eq!(start, 1);
        assert_eq!(one[2].1, FormPick::Own);
    }

    #[test]
    fn a_field_that_isnt_required_can_be_left_out() {
        let (yes_no, start) = rows(&field("tests"), &[]);
        assert_eq!(labels(&yes_no), ["tests: yes", "tests: no", "tests: leave it out"]);
        assert_eq!(start, 1);
    }

    #[test]
    fn many_choices_show_whats_ticked_your_own_included() {
        let f = field("strategy");
        assert_eq!(ticked_at_first(Some(&f)), ["b"]);
        let (many, _) = rows(&f, &["b".to_string(), "mine".to_string()]);
        assert_eq!(labels(&many), ["strategy: [ ] a", "strategy: [x] b", "strategy: [x] mine", "strategy: something else…", "strategy: that's all"]);
    }

    #[test]
    fn what_is_typed_is_read_as_the_field_takes_it() {
        assert_eq!(typed(&field("files"), "4"), Ok(json!(4)));
        assert_eq!(typed(&field("files"), "four"), Err("files is a whole number".to_string()));
        assert_eq!(typed(&field("files"), "30"), Err("files is out of range".to_string()));
        assert_eq!(typed(&field("ratio"), "0.5"), Ok(json!(0.5)));
        assert_eq!(typed(&field("Data scope"), "only the last hour"), Ok(json!("only the last hour")), "not one of the list: still your answer");
    }

    #[test]
    fn the_defaults_are_what_the_schema_gives() {
        assert_eq!(Value::Object(form().defaults()), json!({ "Data scope": "some", "strategy": ["b"], "tests": false }));
    }
}
