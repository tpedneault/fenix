//! Pipelines: a prompt run through several agents in turn -- one plans,
//! another builds, a third reviews -- with a gate where you approve what
//! a step passes on, and a review step's findings sent back to be fixed.
//! Sans-I/O like the hub: a `Run` takes what happened (a step's turn
//! ended, you answered at a gate) and says what to do next. The editor
//! starts each step as a thread of its own and feeds its end back. A step
//! at a gate stays open: write to it and that's another round of the same
//! step, and the gate comes back when it's done.
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
        self.read_only || self.mode.as_deref().is_some_and(|m| m.contains("plan") || m.contains("read"))
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
        let mut spec: PipelineSpec = toml::from_str(text).map_err(|e| format!("pipeline {name}: {e}"))?;
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

    /// The ones Fenix ships (`pipelines/*.toml` beside this file).
    pub fn built_in() -> Vec<PipelineSpec> {
        BUILT_IN.iter().map(|(name, text)| PipelineSpec::parse(name, &text.replace("{answer}", FINDINGS_ANSWER)).expect("a built-in pipeline reads")).collect()
    }

    /// Plan, build, review, findings back to build.
    pub fn review() -> PipelineSpec {
        Self::built_in().swap_remove(0)
    }
}

/// Fenix's own, in the order they're offered: name and file.
const BUILT_IN: &[(&str, &str)] = &[
    ("review", include_str!("pipelines/review.toml")),
    ("fix", include_str!("pipelines/fix.toml")),
    ("tdd", include_str!("pipelines/tdd.toml")),
    ("refactor", include_str!("pipelines/refactor.toml")),
    ("audit", include_str!("pipelines/audit.toml")),
];

/// What `{answer}` stands for in a built-in's prompt: how a review answers
/// so that `findings_in` can read it. As it's written in a TOML string.
const FINDINGS_ANSWER: &str = r#"Answer with one JSON block, in ```json fences: a list of findings, each {\"path\", \"line\", \"severity\": \"error\" | \"warning\" | \"note\", \"message\"}; an empty list when there's nothing to fix."#;

/// What a step that passes changes passes when it made none.
pub const NO_CHANGES: &str = "(no changes)";

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
    Start { step: usize, prompt: String },
    /// Ask you whether to go on, showing what step `step` passes.
    Gate { step: usize, passing: String },
    Done { summary: String },
    Stopped { why: String },
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
    /// The step is on another round, written to at its gate.
    follow_up: bool,
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
            let mut stream = serde_json::Deserializer::from_str(&text[i..]).into_iter::<serde_json::Value>();
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
        .map(|f| format!("- {}{}: {}{}", f.path, f.line.map(|l| format!(":{l}")).unwrap_or_default(), f.severity.as_deref().map(|s| format!("[{s}] ")).unwrap_or_default(), f.message))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Run {
    /// A run of `spec` on what you wrote, and its first step.
    pub fn start(spec: PipelineSpec, input: &str) -> (Run, Next) {
        let mut run = Run { spec, input: input.to_string(), state: State::Running(0), rounds: 0, passed: BTreeMap::new(), previous: String::new(), findings: Vec::new(), follow_up: false };
        let prompt = run.prompt_for(0);
        run.state = State::Running(0);
        (run, Next::Start { step: 0, prompt })
    }

    /// Step `step`'s prompt, its template filled in.
    pub fn prompt_for(&self, step: usize) -> String {
        let mut text = self.spec.steps[step].prompt.replace("{input}", &self.input).replace("{previous}", &self.previous);
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
        let Some(i) = self.step() else { return Next::Stopped { why: "the pipeline isn't running".into() } };
        let follow_up = std::mem::take(&mut self.follow_up);
        if end.stop != "end_turn" {
            // Another round given up on: what the step had still stands.
            if follow_up {
                self.state = State::AtGate(i);
                return Next::Gate { step: i, passing: self.previous.clone() };
            }
            let why = format!("step {} ended: {}", self.spec.steps[i].name, end.stop);
            self.state = State::Stopped(why.clone());
            return Next::Stopped { why };
        }
        let step = self.spec.steps[i].clone();
        let passing = match step.passes {
            Passes::Answer => end.answer.clone(),
            Passes::Plan => end.plan.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| end.answer.clone()),
            Passes::Changes => if end.changes.trim().is_empty() { NO_CHANGES.to_string() } else { end.changes.clone() },
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
                if let Some(back) = step.on_findings.as_ref().and_then(|b| self.spec.steps.iter().position(|s| &s.name == b)) {
                    if self.rounds < step.max_rounds {
                        self.rounds += 1;
                        self.previous = format!("A review found these; fix them, then run the tests again:\n{}", findings_text(&findings));
                        self.passed.insert(step.name.clone(), findings_text(&findings));
                        self.state = State::Running(back);
                        return Next::Start { step: back, prompt: self.prompt_for(back) };
                    }
                }
                self.state = State::Done;
                let left = if step.on_findings.is_some() { format!(" left after {} round(s)", self.rounds) } else { String::new() };
                return Next::Done { summary: format!("{} finding(s){left}:\n{}", findings.len(), findings_text(&findings)) };
            }
            self.passed.insert(step.name.clone(), "(no findings)".into());
        } else {
            self.passed.insert(step.name.clone(), passing.clone());
        }
        self.previous = passing.clone();
        // A step meant to change things that didn't: the next has nothing
        // to work on, so you say whether it's worth going on.
        let nothing_done = step.passes == Passes::Changes && end.changes.trim().is_empty() && i + 1 < self.spec.steps.len();
        if step.gate || nothing_done || follow_up {
            self.state = State::AtGate(i);
            return Next::Gate { step: i, passing };
        }
        self.advance(i)
    }

    fn advance(&mut self, i: usize) -> Next {
        if i + 1 >= self.spec.steps.len() {
            self.state = State::Done;
            return Next::Done { summary: if self.spec.steps[i].findings { "the review found nothing to fix".into() } else { "every step is done".into() } };
        }
        self.state = State::Running(i + 1);
        Next::Start { step: i + 1, prompt: self.prompt_for(i + 1) }
    }

    /// You wrote to the step at its gate: it's working again, and its end
    /// brings the gate back.
    pub fn gate_reopened(&mut self) {
        if let State::AtGate(i) = self.state {
            self.state = State::Running(i);
            self.follow_up = true;
        }
    }

    /// You answered at the gate: go on (with what's passed, as you edited
    /// it), or stop.
    pub fn gate_answered(&mut self, go: bool, edited: Option<String>) -> Next {
        let State::AtGate(i) = self.state else { return Next::Stopped { why: "not at a gate".into() } };
        if !go {
            let why = format!("stopped at {}'s gate", self.spec.steps[i].name);
            self.state = State::Stopped(why.clone());
            return Next::Stopped { why };
        }
        if let Some(text) = edited {
            self.passed.insert(self.spec.steps[i].name.clone(), text.clone());
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
        StepEnd { stop: "end_turn".into(), answer: answer.into(), changes: "+a".into(), ..Default::default() }
    }

    #[test]
    fn the_built_in_review_reads_and_a_bad_one_says_why() {
        let review = PipelineSpec::review();
        assert_eq!(review.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["plan", "build", "review"]);
        assert!(review.steps[0].is_read_only() && !review.steps[1].is_read_only());
        assert!(PipelineSpec::parse("x", "steps = []").unwrap_err().contains("no steps"));
        assert!(PipelineSpec::parse("x", "[[steps]]\nname = \"a\"\nprompt = \"p\"\non_findings = \"nope\"\n").unwrap_err().contains("isn't one of its steps"));
    }

    /// Run through as if everything went well: each step has its turn, each
    /// gate is passed, and it ends. Only names that mean something in a
    /// prompt, and only steps that have run by then.
    #[test]
    fn every_built_in_reads_runs_and_names_only_what_it_has() {
        let specs = PipelineSpec::built_in();
        assert_eq!(specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["review", "fix", "tdd", "refactor", "audit"]);
        for spec in specs {
            for (i, step) in spec.steps.iter().enumerate() {
                let known: Vec<&str> = ["input", "previous"].into_iter().chain(spec.steps[..i].iter().map(|s| s.name.as_str())).collect();
                for piece in step.prompt.split('{').skip(1) {
                    let Some(word) = piece.split('}').next().filter(|w| !w.is_empty() && w.chars().all(|c| c.is_alphanumeric() || c == '_')) else { continue };
                    assert!(known.contains(&word), "{}: {} names {{{word}}}", spec.name, step.name);
                }
                if step.findings {
                    assert!(step.prompt.contains("```json") && step.is_read_only(), "{}: {}", spec.name, step.name);
                }
            }
            let name = spec.name.clone();
            let steps = spec.steps.len();
            let (mut run, first) = Run::start(spec, "the task");
            assert!(matches!(first, Next::Start { step: 0, .. }));
            let mut next = run.step_ended(ended("```json\n[]\n```"));
            for _ in 0..steps * 2 {
                next = match next {
                    Next::Gate { .. } => run.gate_answered(true, None),
                    Next::Start { prompt, .. } => {
                        assert!(prompt.contains("the task") || !prompt.contains("{input}"), "{name}");
                        run.step_ended(ended("```json\n[]\n```"))
                    }
                    done => {
                        assert!(matches!(done, Next::Done { .. }), "{name}: {done:?}");
                        break;
                    }
                };
            }
            assert_eq!(run.state, State::Done, "{name}");
        }
    }

    /// A review on its own has nowhere to send what it finds: it says.
    #[test]
    fn a_review_alone_reports_its_findings() {
        let audit = PipelineSpec::built_in().into_iter().find(|s| s.name == "audit").unwrap();
        let (mut run, _) = Run::start(audit, "");
        let finding = "```json\n[{\"path\": \"a.rs\", \"line\": 3, \"severity\": \"error\", \"message\": \"off by one\"}]\n```";
        let Next::Done { summary } = run.step_ended(ended(finding)) else { panic!() };
        assert_eq!(summary, "1 finding(s):\n- a.rs:3: [error] off by one");
    }

    #[test]
    fn a_gate_holds_the_plan_until_you_say_go() {
        let (mut run, next) = Run::start(PipelineSpec::review(), "Add a --verbose flag");
        let Next::Start { step: 0, prompt } = next else { panic!("{next:?}") };
        assert!(prompt.contains("Add a --verbose flag"));
        let next = run.step_ended(StepEnd { plan: Some("1. Parse it\n2. Print more".into()), ..ended("I'd do this") });
        assert_eq!(next, Next::Gate { step: 0, passing: "1. Parse it\n2. Print more".into() });
        assert_eq!(run.progress(), "plan ● → build → review");
        let next = run.gate_answered(true, Some("1. Parse it".into()));
        let Next::Start { step: 1, prompt } = next else { panic!("{next:?}") };
        assert!(prompt.contains("1. Parse it") && !prompt.contains("Print more"), "your edit is what goes on");
        assert_eq!(run.progress(), "plan ✓ → build ● → review");
    }

    #[test]
    fn findings_go_back_to_build_until_there_are_none_or_the_rounds_run_out() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        let next = run.step_ended(ended("done"));
        assert!(matches!(next, Next::Gate { step: 1, .. }), "{next:?}");
        let next = run.gate_answered(true, None);
        let Next::Start { step: 2, prompt } = next else { panic!("{next:?}") };
        assert!(prompt.contains("+a") && prompt.contains("The plan:\nplan"), "{prompt}");
        let finding = "```json\n[{\"path\": \"a.rs\", \"line\": 3, \"severity\": \"error\", \"message\": \"off by one\"}]\n```";
        let Next::Start { step: 1, prompt } = run.step_ended(ended(finding)) else { panic!() };
        assert!(prompt.contains("a.rs:3: [error] off by one"), "{prompt}");
        assert!(matches!(run.step_ended(ended("fixed")), Next::Gate { step: 1, .. }), "build waits every round");
        run.gate_answered(true, None);
        assert!(matches!(run.step_ended(ended(finding)), Next::Start { step: 1, .. }), "round two");
        run.step_ended(ended("fixed again"));
        run.gate_answered(true, None);
        let Next::Done { summary } = run.step_ended(ended(finding)) else { panic!() };
        assert!(summary.contains("1 finding(s) left after 2 round(s)"), "{summary}");
        assert_eq!(run.state, State::Done);
    }

    #[test]
    fn a_step_at_its_gate_takes_as_many_rounds_as_you_want() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan one"));
        run.gate_reopened();
        assert_eq!(run.state, State::Running(0));
        assert_eq!(run.step_ended(ended("plan two")), Next::Gate { step: 0, passing: "plan two".into() });
        let Next::Start { step: 1, prompt } = run.gate_answered(true, None) else { panic!() };
        assert!(prompt.contains("plan two") && !prompt.contains("plan one"), "{prompt}");
        // Build: a second round's changes are what's passed, and one you
        // cancel leaves the gate as it was.
        run.step_ended(StepEnd { changes: "+one".into(), ..ended("built") });
        run.gate_reopened();
        assert_eq!(run.step_ended(StepEnd { changes: "+one\n+two".into(), ..ended("more") }), Next::Gate { step: 1, passing: "+one\n+two".into() });
        run.gate_reopened();
        assert_eq!(run.step_ended(StepEnd { stop: "cancelled".into(), ..Default::default() }), Next::Gate { step: 1, passing: "+one\n+two".into() });
        let Next::Start { step: 2, prompt } = run.gate_answered(true, None) else { panic!() };
        assert!(prompt.contains("+one\n+two"), "{prompt}");
    }

    #[test]
    fn a_build_that_changed_nothing_waits_at_a_gate() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        let next = run.step_ended(StepEnd { changes: String::new(), ..ended("I updated the plan") });
        assert_eq!(next, Next::Gate { step: 1, passing: NO_CHANGES.to_string() });
        assert_eq!(run.state, State::AtGate(1));
        assert!(matches!(run.gate_answered(false, None), Next::Stopped { .. }), "stopped, not reviewed");
    }

    #[test]
    fn no_findings_ends_it_and_a_turn_that_fails_stops_it() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        run.step_ended(ended("built"));
        run.gate_answered(true, None);
        assert_eq!(run.step_ended(ended("Nothing to fix: ```json\n{\"findings\": []}\n```")), Next::Done { summary: "the review found nothing to fix".into() });
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        assert!(matches!(run.step_ended(StepEnd { stop: "cancelled".into(), ..Default::default() }), Next::Stopped { .. }));
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        assert!(matches!(run.gate_answered(false, None), Next::Stopped { .. }));
    }

    #[test]
    fn a_review_without_a_list_stops_rather_than_guessing() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        run.step_ended(ended("built"));
        run.gate_answered(true, None);
        assert!(matches!(run.step_ended(ended("Looks fine to me")), Next::Stopped { .. }));
    }

    #[test]
    fn json_is_found_fenced_or_bare() {
        assert_eq!(find_json("x ```json\n[1]\n``` y"), Some(serde_json::json!([1])));
        assert_eq!(find_json("the answer is {\"a\": 2} ok"), Some(serde_json::json!({ "a": 2 })));
        assert_eq!(find_json("none here"), None);
    }
}
