//! Pipelines: a prompt run through several agents in turn -- one plans,
//! another builds, a third reviews -- with a gate where you approve what
//! a step passes on, and a review step's findings sent back to be fixed.
//! Sans-I/O like the hub: a `Run` takes what happened (a step's turn
//! ended, you answered at a gate) and says what to do next. The editor
//! starts each step as a thread of its own and feeds its end back.
//!
//! A pipeline is data (`pipelines/<name>.toml` in Fenix's settings
//! folder, or a project's `.fenix/pipelines/`):
//!
//! ```toml
//! title = "Plan, build, review"
//!
//! [[steps]]
//! name = "plan"
//! mode = "plan"
//! prompt = "Plan this change. Don't edit files.\n\n{input}"
//! passes = "plan"
//! gate = true
//!
//! [[steps]]
//! name = "build"
//! prompt = "Carry out this plan.\n\n{previous}"
//! passes = "changes"
//! ```

use std::collections::BTreeMap;

use serde::Deserialize;

/// What a step hands the next.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Passes {
    /// Its last answer.
    #[default]
    Answer,
    /// Its plan, else its answer.
    Plan,
    /// The pipeline's changes so far, as a diff.
    Changes,
    /// Its whole conversation.
    Transcript,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct StepSpec {
    pub name: String,
    /// The agent; empty for the default.
    #[serde(default)]
    pub agent: Option<String>,
    /// A mode the agent offers (`plan`): its id, or a word in its name.
    #[serde(default)]
    pub mode: Option<String>,
    /// `{input}` (what you wrote), `{previous}` (what the step before
    /// passed), `{<step>}` (what that step passed).
    pub prompt: String,
    /// Fenix's toolsets this step's agent is shown; empty for the usual.
    #[serde(default)]
    pub toolsets: Vec<String>,
    #[serde(default)]
    pub passes: Passes,
    /// You approve what it passes on before the next step starts.
    #[serde(default)]
    pub gate: bool,
    /// Its answer is findings (a JSON list of `{path, line, severity,
    /// message}`), sent back to `on_findings` while there are any.
    #[serde(default)]
    pub findings: bool,
    #[serde(default)]
    pub on_findings: Option<String>,
    /// How many times findings go back, at most.
    #[serde(default = "two")]
    pub max_rounds: u32,
    /// What it changes on disk is put back at its end. A step in a plan
    /// mode is read-only whether it says so or not.
    #[serde(default)]
    pub read_only: bool,
}

fn two() -> u32 {
    2
}

impl StepSpec {
    pub fn is_read_only(&self) -> bool {
        self.read_only
            || self
                .mode
                .as_deref()
                .is_some_and(|m| m.contains("plan") || m.contains("read"))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PipelineSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title: String,
    pub steps: Vec<StepSpec>,
}

impl PipelineSpec {
    /// A pipeline file, read: its steps named and their references known.
    pub fn parse(name: &str, text: &str) -> Result<PipelineSpec, String> {
        let mut spec: PipelineSpec =
            toml::from_str(text).map_err(|e| format!("pipeline {name}: {e}"))?;
        spec.name = name.to_string();
        if spec.title.is_empty() {
            spec.title = name.to_string();
        }
        if spec.steps.is_empty() {
            return Err(format!("pipeline {name}: it has no steps"));
        }
        for step in &spec.steps {
            if let Some(back) = &step.on_findings {
                if !spec.steps.iter().any(|s| &s.name == back) {
                    return Err(format!("pipeline {name}: step {} sends findings to {back}, which isn't one of its steps", step.name));
                }
            }
        }
        Ok(spec)
    }

    /// The one Fenix ships: plan, build, review, findings back to build.
    pub fn review() -> PipelineSpec {
        PipelineSpec::parse("review", REVIEW).expect("the built-in pipeline reads")
    }
}

const REVIEW: &str = r#"
title = "Plan, build, review"

[[steps]]
name = "plan"
mode = "plan"
prompt = "Plan this change. Read what you need, but don't edit any files. Give the plan as numbered steps, each naming the files it touches.\n\n{input}"
passes = "plan"
gate = true

[[steps]]
name = "build"
prompt = "Carry out this plan, then run the project's tests.\n\n{previous}"
passes = "changes"

[[steps]]
name = "review"
mode = "plan"
prompt = "Review these changes against the plan. Don't edit any files. Answer with one JSON block, in ```json fences: a list of findings, each {\"path\", \"line\", \"severity\": \"error\" | \"warning\" | \"note\", \"message\"}; an empty list when there's nothing to fix.\n\nThe plan:\n{plan}\n\nThe changes:\n{previous}"
findings = true
on_findings = "build"
max_rounds = 2
"#;

/// How a step ended, as the editor read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepEnd {
    /// The agent's `stopReason` (`end_turn`, `cancelled`, `error`...).
    pub stop: String,
    pub answer: String,
    pub plan: Option<String>,
    pub changes: String,
    pub transcript: String,
}

/// What to do now.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Start step `step` with this prompt.
    Start {
        step: usize,
        prompt: String,
    },
    /// Ask you whether to go on, showing what step `step` passes.
    Gate {
        step: usize,
        passing: String,
    },
    Done {
        summary: String,
    },
    Stopped {
        why: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running(usize),
    AtGate(usize),
    Done,
    Stopped(String),
}

/// A finding a review step reported.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Finding {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub message: String,
}

/// A pipeline under way.
#[derive(Clone, Debug)]
pub struct Run {
    pub spec: PipelineSpec,
    pub input: String,
    pub state: State,
    /// Rounds of findings sent back so far.
    pub rounds: u32,
    /// What each step passed, by name.
    pub passed: BTreeMap<String, String>,
    previous: String,
    /// The last review's findings.
    pub findings: Vec<Finding>,
}

/// The JSON block in an answer: the first ```json fence, else the first
/// bracketed value that reads.
pub fn find_json(text: &str) -> Option<serde_json::Value> {
    if let Some(start) = text.find("```json") {
        let rest = &text[start + 7..];
        if let Some(end) = rest.find("```") {
            if let Ok(v) = serde_json::from_str(rest[..end].trim()) {
                return Some(v);
            }
        }
    }
    for (i, c) in text.char_indices() {
        if c == '[' || c == '{' {
            let mut stream =
                serde_json::Deserializer::from_str(&text[i..]).into_iter::<serde_json::Value>();
            if let Some(Ok(v)) = stream.next() {
                return Some(v);
            }
        }
    }
    None
}

/// A review's findings: a list, or `{findings: [...]}`.
pub fn findings_in(answer: &str) -> Option<Vec<Finding>> {
    let value = find_json(answer)?;
    let list = value.get("findings").cloned().unwrap_or(value);
    serde_json::from_value(list).ok()
}

fn findings_text(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            format!(
                "- {}{}: {}{}",
                f.path,
                f.line.map(|l| format!(":{l}")).unwrap_or_default(),
                f.severity
                    .as_deref()
                    .map(|s| format!("[{s}] "))
                    .unwrap_or_default(),
                f.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Run {
    /// A run of `spec` on what you wrote, and its first step.
    pub fn start(spec: PipelineSpec, input: &str) -> (Run, Next) {
        let mut run = Run {
            spec,
            input: input.to_string(),
            state: State::Running(0),
            rounds: 0,
            passed: BTreeMap::new(),
            previous: String::new(),
            findings: Vec::new(),
        };
        let prompt = run.prompt_for(0);
        run.state = State::Running(0);
        (run, Next::Start { step: 0, prompt })
    }

    /// Step `step`'s prompt, its template filled in.
    pub fn prompt_for(&self, step: usize) -> String {
        let mut text = self.spec.steps[step]
            .prompt
            .replace("{input}", &self.input)
            .replace("{previous}", &self.previous);
        for (name, passed) in &self.passed {
            text = text.replace(&format!("{{{name}}}"), passed);
        }
        text
    }

    pub fn step(&self) -> Option<usize> {
        match self.state {
            State::Running(i) | State::AtGate(i) => Some(i),
            _ => None,
        }
    }

    /// The steps as one line: `plan ✓ → build ● → review`.
    pub fn progress(&self) -> String {
        let at = self.step();
        self.spec
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| match at {
                Some(a) if i < a => format!("{} ✓", s.name),
                Some(a) if i == a => format!("{} ●", s.name),
                None if self.state == State::Done => format!("{} ✓", s.name),
                _ => s.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }

    /// The current step's turn ended.
    pub fn step_ended(&mut self, end: StepEnd) -> Next {
        let Some(i) = self.step() else {
            return Next::Stopped {
                why: "the pipeline isn't running".into(),
            };
        };
        if end.stop != "end_turn" {
            let why = format!("step {} ended: {}", self.spec.steps[i].name, end.stop);
            self.state = State::Stopped(why.clone());
            return Next::Stopped { why };
        }
        let step = self.spec.steps[i].clone();
        let passing = match step.passes {
            Passes::Answer => end.answer.clone(),
            Passes::Plan => end
                .plan
                .clone()
                .filter(|p| !p.trim().is_empty())
                .unwrap_or_else(|| end.answer.clone()),
            Passes::Changes => {
                if end.changes.trim().is_empty() {
                    "(no changes)".to_string()
                } else {
                    end.changes.clone()
                }
            }
            Passes::Transcript => end.transcript.clone(),
        };
        if step.findings {
            let Some(findings) = findings_in(&end.answer) else {
                let why = format!("step {}'s answer has no findings list", step.name);
                self.state = State::Stopped(why.clone());
                return Next::Stopped { why };
            };
            self.findings = findings.clone();
            if !findings.is_empty() {
                if let Some(back) = step
                    .on_findings
                    .as_ref()
                    .and_then(|b| self.spec.steps.iter().position(|s| &s.name == b))
                {
                    if self.rounds < step.max_rounds {
                        self.rounds += 1;
                        self.previous = format!(
                            "A review found these; fix them, then run the tests again:\n{}",
                            findings_text(&findings)
                        );
                        self.passed
                            .insert(step.name.clone(), findings_text(&findings));
                        self.state = State::Running(back);
                        return Next::Start {
                            step: back,
                            prompt: self.prompt_for(back),
                        };
                    }
                }
                self.state = State::Done;
                return Next::Done {
                    summary: format!(
                        "{} finding(s) left after {} round(s):\n{}",
                        findings.len(),
                        self.rounds,
                        findings_text(&findings)
                    ),
                };
            }
            self.passed
                .insert(step.name.clone(), "(no findings)".into());
        } else {
            self.passed.insert(step.name.clone(), passing.clone());
        }
        self.previous = passing.clone();
        if step.gate {
            self.state = State::AtGate(i);
            return Next::Gate { step: i, passing };
        }
        self.advance(i)
    }

    fn advance(&mut self, i: usize) -> Next {
        if i + 1 >= self.spec.steps.len() {
            self.state = State::Done;
            return Next::Done {
                summary: if self.spec.steps[i].findings {
                    "the review found nothing to fix".into()
                } else {
                    "every step is done".into()
                },
            };
        }
        self.state = State::Running(i + 1);
        Next::Start {
            step: i + 1,
            prompt: self.prompt_for(i + 1),
        }
    }

    /// You answered at the gate: go on (with what's passed, as you edited
    /// it), or stop.
    pub fn gate_answered(&mut self, go: bool, edited: Option<String>) -> Next {
        let State::AtGate(i) = self.state else {
            return Next::Stopped {
                why: "not at a gate".into(),
            };
        };
        if !go {
            let why = format!("stopped at {}'s gate", self.spec.steps[i].name);
            self.state = State::Stopped(why.clone());
            return Next::Stopped { why };
        }
        if let Some(text) = edited {
            self.passed
                .insert(self.spec.steps[i].name.clone(), text.clone());
            self.previous = text;
        }
        self.advance(i)
    }

    /// You stopped it.
    pub fn stop(&mut self, why: &str) {
        self.state = State::Stopped(why.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ended(answer: &str) -> StepEnd {
        StepEnd {
            stop: "end_turn".into(),
            answer: answer.into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_built_in_review_reads_and_a_bad_one_says_why() {
        let review = PipelineSpec::review();
        assert_eq!(
            review
                .steps
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["plan", "build", "review"]
        );
        assert!(review.steps[0].is_read_only() && !review.steps[1].is_read_only());
        assert!(PipelineSpec::parse("x", "steps = []")
            .unwrap_err()
            .contains("no steps"));
        assert!(PipelineSpec::parse(
            "x",
            "[[steps]]\nname = \"a\"\nprompt = \"p\"\non_findings = \"nope\"\n"
        )
        .unwrap_err()
        .contains("isn't one of its steps"));
    }

    #[test]
    fn a_gate_holds_the_plan_until_you_say_go() {
        let (mut run, next) = Run::start(PipelineSpec::review(), "Add a --verbose flag");
        let Next::Start { step: 0, prompt } = next else {
            panic!("{next:?}")
        };
        assert!(prompt.contains("Add a --verbose flag"));
        let next = run.step_ended(StepEnd {
            plan: Some("1. Parse it\n2. Print more".into()),
            ..ended("I'd do this")
        });
        assert_eq!(
            next,
            Next::Gate {
                step: 0,
                passing: "1. Parse it\n2. Print more".into()
            }
        );
        assert_eq!(run.progress(), "plan ● → build → review");
        let next = run.gate_answered(true, Some("1. Parse it".into()));
        let Next::Start { step: 1, prompt } = next else {
            panic!("{next:?}")
        };
        assert!(
            prompt.contains("1. Parse it") && !prompt.contains("Print more"),
            "your edit is what goes on"
        );
        assert_eq!(run.progress(), "plan ✓ → build ● → review");
    }

    #[test]
    fn findings_go_back_to_build_until_there_are_none_or_the_rounds_run_out() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        let next = run.step_ended(StepEnd {
            changes: "+a".into(),
            ..ended("done")
        });
        let Next::Start { step: 2, prompt } = next else {
            panic!("{next:?}")
        };
        assert!(
            prompt.contains("+a") && prompt.contains("The plan:\nplan"),
            "{prompt}"
        );
        let finding = "```json\n[{\"path\": \"a.rs\", \"line\": 3, \"severity\": \"error\", \"message\": \"off by one\"}]\n```";
        let Next::Start { step: 1, prompt } = run.step_ended(ended(finding)) else {
            panic!()
        };
        assert!(prompt.contains("a.rs:3: [error] off by one"), "{prompt}");
        run.step_ended(ended("fixed"));
        assert!(
            matches!(run.step_ended(ended(finding)), Next::Start { step: 1, .. }),
            "round two"
        );
        run.step_ended(ended("fixed again"));
        let Next::Done { summary } = run.step_ended(ended(finding)) else {
            panic!()
        };
        assert!(
            summary.contains("1 finding(s) left after 2 round(s)"),
            "{summary}"
        );
        assert_eq!(run.state, State::Done);
    }

    #[test]
    fn no_findings_ends_it_and_a_turn_that_fails_stops_it() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        run.step_ended(ended("built"));
        assert_eq!(
            run.step_ended(ended("Nothing to fix: ```json\n{\"findings\": []}\n```")),
            Next::Done {
                summary: "the review found nothing to fix".into()
            }
        );
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        assert!(matches!(
            run.step_ended(StepEnd {
                stop: "cancelled".into(),
                ..Default::default()
            }),
            Next::Stopped { .. }
        ));
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        assert!(matches!(
            run.gate_answered(false, None),
            Next::Stopped { .. }
        ));
    }

    #[test]
    fn a_review_without_a_list_stops_rather_than_guessing() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        run.step_ended(ended("built"));
        assert!(matches!(
            run.step_ended(ended("Looks fine to me")),
            Next::Stopped { .. }
        ));
    }

    #[test]
    fn json_is_found_fenced_or_bare() {
        assert_eq!(
            find_json("x ```json\n[1]\n``` y"),
            Some(serde_json::json!([1]))
        );
        assert_eq!(
            find_json("the answer is {\"a\": 2} ok"),
            Some(serde_json::json!({ "a": 2 }))
        );
        assert_eq!(find_json("none here"), None);
    }
}
