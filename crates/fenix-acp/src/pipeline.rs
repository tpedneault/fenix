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
//!
//! [[steps]]
//! name = "test"
//! run = "cargo test"
//! on_failure = "build"
//! ```
//!
//! A step with `run` is a command, no agent in it: its exit code says
//! whether the run goes on, and what it printed goes back to `on_failure`.
//!
//! A step that ends badly -- cancelled, refused, cut off, a review with no
//! findings list, a command out of rounds -- asks at its gate rather than
//! ending the run, saying why. A run stopped keeps the step it stopped at,
//! and `resume` brings that gate back; `retry` runs the step again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What a step hands the next.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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
    #[serde(default)]
    pub prompt: String,
    /// A command to run in the project, in place of a prompt, taken as
    /// written. It passes what came before it on untouched; `{<step>}` is
    /// what it printed.
    #[serde(default)]
    pub run: Option<String>,
    /// Where a failed command's output goes to be fixed.
    #[serde(default)]
    pub on_failure: Option<String>,
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
    /// How many times findings or a failure go back, at most.
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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
            let say = |what: &str| format!("pipeline {name}: step {} {what}", step.name);
            let Some(command) = &step.run else {
                if step.prompt.trim().is_empty() {
                    return Err(say("needs a prompt, or a command in `run`"));
                }
                if step.on_failure.is_some() {
                    return Err(say("has on_failure, which is for a step with `run`"));
                }
                continue;
            };
            if command.trim().is_empty() || !step.prompt.is_empty() {
                return Err(say("needs either a prompt or a command in `run`, not both"));
            }
            if step.gate || step.findings || step.on_findings.is_some() || step.agent.is_some() || step.mode.is_some() || !step.toolsets.is_empty() {
                return Err(say("is a command: gate, findings, on_findings, agent, mode and toolsets are for agents"));
            }
            if let Some(back) = &step.on_failure {
                if !spec.steps.iter().any(|s| &s.name == back && s.run.is_none()) {
                    return Err(say(&format!("sends failures to {back}, which isn't an agent's step")));
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
    /// Run step `step`'s command, and say how it went (`command_ended`).
    Run { step: usize, command: String },
    /// Ask you whether to go on, showing what step `step` passes (empty:
    /// it has nothing to pass, so only running it again or stopping are
    /// offered). `trouble`: why it's asked though the step didn't end
    /// well.
    Gate { step: usize, passing: String, trouble: Option<String> },
    Done { summary: String },
    Stopped { why: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum State {
    Running(usize),
    AtGate(usize),
    Done,
    /// Stopped at step `step`, and why: `resume` brings its gate back.
    Stopped { step: usize, why: String },
}

/// The gate up at a step, kept while the run is stopped there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GateUp {
    pub passing: String,
    pub trouble: Option<String>,
}

/// A finding a review step reported.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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

/// A pipeline under way. Kept as it is (`store::SavedRun`), so a field
/// added later needs a default.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    /// Times each command step's failure went back.
    failures: BTreeMap<String, u32>,
    /// What the step under way was started with: its prompt, or its
    /// command. `retry` starts it with the same.
    #[serde(default)]
    prompt: String,
    /// The gate up, or the one the run stopped at.
    #[serde(default)]
    pub gate: Option<GateUp>,
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

/// How much of a command's output an agent is given: the end of it, where
/// a failure is.
const OUTPUT_KEPT: usize = 12_000;

fn output_kept(output: &str) -> &str {
    let mut start = output.len().saturating_sub(OUTPUT_KEPT);
    while !output.is_char_boundary(start) {
        start += 1;
    }
    &output[start..]
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
        let mut run = Run { spec, input: input.to_string(), state: State::Running(0), rounds: 0, passed: BTreeMap::new(), previous: String::new(), findings: Vec::new(), follow_up: false, failures: BTreeMap::new(), prompt: String::new(), gate: None };
        let next = run.begin(0);
        (run, next)
    }

    /// Step `i` is next: its prompt to send, or its command to run.
    fn begin(&mut self, i: usize) -> Next {
        self.prompt = match &self.spec.steps[i].run {
            Some(command) => command.clone(),
            None => self.prompt_for(i),
        };
        self.again(i)
    }

    /// Step `i` started with what it was started with last.
    fn again(&mut self, i: usize) -> Next {
        self.state = State::Running(i);
        self.gate = None;
        self.follow_up = false;
        match &self.spec.steps[i].run {
            Some(_) => Next::Run { step: i, command: self.prompt.clone() },
            None => Next::Start { step: i, prompt: self.prompt.clone() },
        }
    }

    /// The gate at step `i`, up: what it passes, and why it's asked when
    /// the step didn't end well.
    fn gate_at(&mut self, i: usize, passing: String, trouble: Option<String>) -> Next {
        self.state = State::AtGate(i);
        self.gate = Some(GateUp { passing: passing.clone(), trouble: trouble.clone() });
        Next::Gate { step: i, passing, trouble }
    }

    /// What step `i` has to pass on that went badly: kept for the next
    /// step if you go on, unless there's nothing.
    fn gate_on_trouble(&mut self, i: usize, passing: String, trouble: String) -> Next {
        if !passing.trim().is_empty() {
            self.passed.insert(self.spec.steps[i].name.clone(), passing.clone());
            self.previous = passing.clone();
        }
        let passing = if passing.trim().is_empty() { String::new() } else { passing };
        self.gate_at(i, passing, Some(trouble))
    }

    /// The step a stopped run stopped at.
    pub fn stopped_at(&self) -> Option<usize> {
        match self.state {
            State::Stopped { step, .. } => Some(step),
            _ => None,
        }
    }

    /// It's under way: a step at work or at its gate.
    pub fn is_live(&self) -> bool {
        matches!(self.state, State::Running(_) | State::AtGate(_))
    }

    /// The step under way is a command, so no agent's turn is its end.
    pub fn at_command(&self) -> bool {
        matches!(self.state, State::Running(i) if self.spec.steps[i].run.is_some())
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
            .map(|(i, s)| match (at, self.stopped_at()) {
                (Some(a), _) | (None, Some(a)) if i < a => format!("{} ✓", s.name),
                (Some(a), _) if i == a => format!("{} ●", s.name),
                (None, Some(a)) if i == a => format!("{} ■", s.name),
                (None, None) if self.state == State::Done => format!("{} ✓", s.name),
                _ => s.name.clone(),
            })
            .collect::<Vec<_>>()
            .join(" → ")
    }

    /// The current step's turn ended.
    pub fn step_ended(&mut self, end: StepEnd) -> Next {
        let Some(i) = self.step() else { return Next::Stopped { why: "the pipeline isn't running".into() } };
        if !matches!(self.state, State::Running(_)) {
            return Next::Stopped { why: "the pipeline isn't running".into() };
        }
        let follow_up = std::mem::take(&mut self.follow_up);
        let step = self.spec.steps[i].clone();
        let passing = match step.passes {
            Passes::Answer => end.answer.clone(),
            Passes::Plan => end.plan.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| end.answer.clone()),
            Passes::Changes => if end.changes.trim().is_empty() { NO_CHANGES.to_string() } else { end.changes.clone() },
            Passes::Transcript => end.transcript.clone(),
        };
        if end.stop != "end_turn" {
            // Another round given up on: what the step had still stands.
            if follow_up {
                let previous = self.previous.clone();
                return self.gate_at(i, previous, None);
            }
            // Refused, cancelled, cut off: you say whether what it has
            // goes on, or it runs again.
            return self.gate_on_trouble(i, passing, format!("{} stopped early ({})", step.name, end.stop));
        }
        if step.findings {
            let Some(findings) = findings_in(&end.answer) else {
                return self.gate_on_trouble(i, end.answer.clone(), format!("{}'s answer has no findings list", step.name));
            };
            self.findings = findings.clone();
            if !findings.is_empty() {
                if let Some(back) = step.on_findings.as_ref().and_then(|b| self.spec.steps.iter().position(|s| &s.name == b)) {
                    if self.rounds < step.max_rounds {
                        self.rounds += 1;
                        self.previous = format!("A review found these; fix them, then run the tests again:\n{}", findings_text(&findings));
                        self.passed.insert(step.name.clone(), findings_text(&findings));
                        return self.begin(back);
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
            return self.gate_at(i, passing, None);
        }
        self.advance(i)
    }

    fn advance(&mut self, i: usize) -> Next {
        let troubled = self.gate.take().is_some_and(|g| g.trouble.is_some());
        if i + 1 >= self.spec.steps.len() {
            self.state = State::Done;
            let found_nothing = self.spec.steps[i].findings && !troubled;
            return Next::Done { summary: if found_nothing { "the review found nothing to fix".into() } else { "every step is done".into() } };
        }
        self.begin(i + 1)
    }

    /// The current step's command ended: `exit` is `None` when it was
    /// stopped. Passed, what the step before gave goes on to the next.
    /// Failed, its output goes to `on_failure` while rounds are left.
    pub fn command_ended(&mut self, exit: Option<i32>, output: &str) -> Next {
        let Some(i) = self.step().filter(|_| self.at_command()) else { return Next::Stopped { why: "the pipeline isn't running".into() } };
        let step = self.spec.steps[i].clone();
        let command = step.run.clone().unwrap_or_default();
        let kept = output_kept(output);
        self.passed.insert(step.name.clone(), kept.to_string());
        // At its gate, what came before it is what goes on: the command
        // passes it on untouched.
        let before = self.previous.clone();
        let failure = match exit {
            Some(0) => return self.advance(i),
            Some(code) => code,
            None => return self.gate_at(i, before, Some(format!("`{command}` was stopped"))),
        };
        let back = step.on_failure.as_ref().and_then(|b| self.spec.steps.iter().position(|s| &s.name == b));
        let used = self.failures.entry(step.name.clone()).or_default();
        let why = match back {
            Some(back) if *used < step.max_rounds => {
                *used += 1;
                self.previous = format!("`{command}` failed (exit {failure}). Fix what's wrong; it runs again after you.\n\n{kept}");
                return self.begin(back);
            }
            Some(_) => format!("`{command}` still fails (exit {failure}) after {used} round(s)"),
            None => format!("`{command}` failed (exit {failure})"),
        };
        self.gate_at(i, before, Some(why))
    }

    /// You wrote to the step at its gate: it's working again, and its end
    /// brings the gate back.
    pub fn gate_reopened(&mut self) {
        if let State::AtGate(i) = self.state {
            self.state = State::Running(i);
            self.follow_up = true;
            self.gate = None;
        }
    }

    /// You answered at the gate: go on (with what's passed, as you edited
    /// it), or stop.
    pub fn gate_answered(&mut self, go: bool, edited: Option<String>) -> Next {
        let State::AtGate(i) = self.state else { return Next::Stopped { why: "not at a gate".into() } };
        if !go {
            // The gate is kept: `resume` brings it back as it was.
            let why = format!("stopped at {}'s gate", self.spec.steps[i].name);
            self.state = State::Stopped { step: i, why: why.clone() };
            return Next::Stopped { why };
        }
        if let Some(text) = edited {
            self.passed.insert(self.spec.steps[i].name.clone(), text.clone());
            self.previous = text;
        }
        self.advance(i)
    }

    /// It stops where it is, for `why`: a gate up is kept.
    pub fn stop(&mut self, why: &str) {
        let step = match self.state {
            State::Running(i) | State::AtGate(i) => i,
            State::Stopped { step, .. } => step,
            State::Done => return,
        };
        if matches!(self.state, State::Running(_)) {
            self.gate = None;
        }
        self.follow_up = false;
        self.state = State::Stopped { step, why: why.to_string() };
    }

    /// A stopped run picked up: the gate of the step it stopped at, as it
    /// was -- or, when it stopped at work, asking with why it stopped and
    /// nothing to pass on yet. `None` unless it's stopped.
    pub fn resume(&mut self) -> Option<Next> {
        let State::Stopped { step, why } = self.state.clone() else { return None };
        let gate = self.gate.clone().unwrap_or(GateUp { passing: String::new(), trouble: Some(why) });
        Some(self.gate_at(step, gate.passing, gate.trouble))
    }

    /// `r` at a gate (or on a stopped run): the step starts again, with
    /// the prompt it had.
    pub fn retry(&mut self) -> Next {
        let i = match self.state {
            State::AtGate(i) => i,
            State::Stopped { step, .. } => step,
            _ => return Next::Stopped { why: "not at a gate".into() },
        };
        if self.prompt.is_empty() {
            return self.begin(i);
        }
        self.again(i)
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

    const TESTED: &str = "[[steps]]\nname = \"build\"\nprompt = \"{input} {previous}\"\npasses = \"changes\"\n[[steps]]\nname = \"test\"\nrun = \"cargo test\"\non_failure = \"build\"\nmax_rounds = 1\n[[steps]]\nname = \"review\"\nprompt = \"{previous} / {test}\"\n";

    #[test]
    fn a_command_that_passes_hands_on_what_came_before_it() {
        let (mut run, _) = Run::start(PipelineSpec::parse("p", TESTED).unwrap(), "x");
        assert_eq!(run.step_ended(ended("built")), Next::Run { step: 1, command: "cargo test".into() });
        assert!(run.at_command());
        let Next::Start { step: 2, prompt } = run.command_ended(Some(0), "ok") else { panic!() };
        assert_eq!(prompt, "+a / ok", "the changes, and what it printed");
        assert!(!run.at_command());
    }

    #[test]
    fn a_failed_command_goes_back_until_its_rounds_run_out() {
        let (mut run, _) = Run::start(PipelineSpec::parse("p", TESTED).unwrap(), "x");
        run.step_ended(ended("built"));
        let Next::Start { step: 0, prompt } = run.command_ended(Some(101), "assertion failed") else { panic!() };
        assert!(prompt.contains("`cargo test` failed (exit 101)") && prompt.contains("assertion failed"), "{prompt}");
        assert!(matches!(run.step_ended(ended("fixed")), Next::Run { step: 1, .. }), "run again");
        let next = run.command_ended(Some(101), "assertion failed");
        assert_eq!(next, Next::Gate { step: 1, passing: "+a".into(), trouble: Some("`cargo test` still fails (exit 101) after 1 round(s)".into()) }, "out of rounds, it asks");
        let Next::Start { step: 2, prompt } = run.gate_answered(true, None) else { panic!() };
        assert!(prompt.starts_with("+a / "), "y goes on anyway: {prompt}");
    }

    #[test]
    fn a_command_with_nowhere_to_go_asks_and_one_stopped_asks() {
        let spec = "[[steps]]\nname = \"test\"\nrun = \"make\"\n";
        let (mut run, first) = Run::start(PipelineSpec::parse("p", spec).unwrap(), "x");
        assert_eq!(first, Next::Run { step: 0, command: "make".into() }, "it can come first");
        assert_eq!(run.command_ended(Some(2), ""), Next::Gate { step: 0, passing: String::new(), trouble: Some("`make` failed (exit 2)".into()) });
        assert_eq!(run.retry(), Next::Run { step: 0, command: "make".into() }, "r runs it again");
        let (mut run, _) = Run::start(PipelineSpec::parse("p", spec).unwrap(), "x");
        assert_eq!(run.command_ended(None, ""), Next::Gate { step: 0, passing: String::new(), trouble: Some("`make` was stopped".into()) });
        let (mut run, _) = Run::start(PipelineSpec::parse("p", spec).unwrap(), "x");
        assert_eq!(run.command_ended(Some(0), ""), Next::Done { summary: "every step is done".into() });
    }

    #[test]
    fn only_the_end_of_a_long_output_is_kept_on_a_character() {
        let long = format!("é{}END", "x".repeat(OUTPUT_KEPT - 3));
        let kept = output_kept(&long);
        assert!(kept.ends_with("END") && kept.len() <= OUTPUT_KEPT, "{}", kept.len());
    }

    #[test]
    fn a_step_is_an_agents_or_a_command_not_both_or_neither() {
        let bad = |text: &str| PipelineSpec::parse("p", text).unwrap_err();
        assert!(bad("[[steps]]\nname = \"a\"\n").contains("needs a prompt"));
        assert!(bad("[[steps]]\nname = \"a\"\nprompt = \"p\"\nrun = \"r\"\n").contains("not both"));
        assert!(bad("[[steps]]\nname = \"a\"\nrun = \"r\"\ngate = true\n").contains("is a command"));
        assert!(bad("[[steps]]\nname = \"a\"\nprompt = \"p\"\non_failure = \"a\"\n").contains("step with `run`"));
        assert!(bad("[[steps]]\nname = \"a\"\nrun = \"r\"\non_failure = \"a\"\n").contains("isn't an agent's step"));
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
        assert_eq!(next, Next::Gate { step: 0, passing: "1. Parse it\n2. Print more".into(), trouble: None });
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
        assert_eq!(run.step_ended(ended("plan two")), Next::Gate { step: 0, passing: "plan two".into(), trouble: None });
        let Next::Start { step: 1, prompt } = run.gate_answered(true, None) else { panic!() };
        assert!(prompt.contains("plan two") && !prompt.contains("plan one"), "{prompt}");
        // Build: a second round's changes are what's passed, and one you
        // cancel leaves the gate as it was.
        run.step_ended(StepEnd { changes: "+one".into(), ..ended("built") });
        run.gate_reopened();
        assert_eq!(run.step_ended(StepEnd { changes: "+one\n+two".into(), ..ended("more") }), Next::Gate { step: 1, passing: "+one\n+two".into(), trouble: None });
        run.gate_reopened();
        assert_eq!(run.step_ended(StepEnd { stop: "cancelled".into(), ..Default::default() }), Next::Gate { step: 1, passing: "+one\n+two".into(), trouble: None });
        let Next::Start { step: 2, prompt } = run.gate_answered(true, None) else { panic!() };
        assert!(prompt.contains("+one\n+two"), "{prompt}");
    }

    #[test]
    fn a_build_that_changed_nothing_waits_at_a_gate() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        let next = run.step_ended(StepEnd { changes: String::new(), ..ended("I updated the plan") });
        assert_eq!(next, Next::Gate { step: 1, passing: NO_CHANGES.to_string(), trouble: None });
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
        assert_eq!(run.step_ended(StepEnd { stop: "cancelled".into(), ..Default::default() }), Next::Gate { step: 0, passing: String::new(), trouble: Some("plan stopped early (cancelled)".into()) });
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        assert!(matches!(run.gate_answered(false, None), Next::Stopped { .. }));
        assert_eq!(run.stopped_at(), Some(0));
    }

    #[test]
    fn a_review_without_a_list_stops_rather_than_guessing() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "x");
        run.step_ended(ended("plan"));
        run.gate_answered(true, None);
        run.step_ended(ended("built"));
        run.gate_answered(true, None);
        assert_eq!(run.step_ended(ended("Looks fine to me")), Next::Gate { step: 2, passing: "Looks fine to me".into(), trouble: Some("review's answer has no findings list".into()) });
        assert_eq!(run.gate_answered(true, None), Next::Done { summary: "every step is done".into() }, "y goes on anyway, and says nothing it doesn't know");
    }

    /// Every way a step can end badly comes to a gate, or a stopped run
    /// that `resume` brings back to one: none leaves nothing to go on from.
    #[test]
    fn every_bad_ending_asks_or_can_be_picked_up() {
        let cancelled = |stop: &str| StepEnd { stop: stop.into(), plan: Some("1. Change a.rs".into()), ..Default::default() };
        let at_plan = || Run::start(PipelineSpec::review(), "x").0;
        // A command refused, `SPC a k`, a token or request limit, a refusal, a crash.
        for stop in ["cancelled", "max_tokens", "max_turn_requests", "refusal", "error"] {
            let mut run = at_plan();
            let next = run.step_ended(cancelled(stop));
            assert_eq!(next, Next::Gate { step: 0, passing: "1. Change a.rs".into(), trouble: Some(format!("plan stopped early ({stop})")) }, "{stop}");
            let Next::Start { step: 1, prompt } = run.gate_answered(true, None) else { panic!("{stop}") };
            assert!(prompt.contains("1. Change a.rs"), "y goes on with what it had: {prompt}");
        }
        // Nothing to pass: `y` isn't offered (the passing is empty), `r` is.
        let mut run = at_plan();
        let Next::Gate { passing, .. } = run.step_ended(StepEnd { stop: "cancelled".into(), ..Default::default() }) else { panic!() };
        assert!(passing.is_empty());
        let Next::Start { step: 0, prompt } = run.retry() else { panic!() };
        assert!(prompt.contains("Plan this change") && prompt.contains("x"), "the prompt it had: {prompt}");
        // `n` at a gate, `q` in the pane `e` opened (the same answer), the
        // gate taken back when its agent stopped: stopped there, and
        // brought back as it was.
        let mut run = at_plan();
        run.step_ended(StepEnd { plan: Some("the plan".into()), ..ended("") });
        assert_eq!(run.gate_answered(false, None), Next::Stopped { why: "stopped at plan's gate".into() });
        assert_eq!(run.progress(), "plan ■ → build → review");
        assert_eq!(run.resume(), Some(Next::Gate { step: 0, passing: "the plan".into(), trouble: None }));
        assert!(matches!(run.gate_answered(true, None), Next::Start { step: 1, .. }));
        // Stopped at work (its agent couldn't take the prompt; Fenix was
        // closed): its gate, saying why, with nothing to pass yet.
        let mut run = at_plan();
        run.stop("interrupted, when Fenix closed");
        assert!(!run.is_live());
        assert_eq!(run.resume(), Some(Next::Gate { step: 0, passing: String::new(), trouble: Some("interrupted, when Fenix closed".into()) }));
        assert!(matches!(run.retry(), Next::Start { step: 0, .. }));
        // `r` offered at every gate: the step again, as it was asked.
        let mut run = at_plan();
        run.step_ended(ended("a plan"));
        let Next::Start { step: 0, prompt } = run.retry() else { panic!() };
        assert!(prompt.contains("Plan this change"), "{prompt}");
        // A step that's done isn't picked up: there's nothing to.
        let (mut run, _) = Run::start(PipelineSpec::parse("p", "[[steps]]\nname = \"a\"\nprompt = \"p\"\n").unwrap(), "x");
        assert!(matches!(run.step_ended(ended("ok")), Next::Done { .. }));
        assert_eq!(run.resume(), None);
    }

    /// A run read back goes on from the same step, with the same prompt.
    #[test]
    fn a_run_kept_and_read_back_goes_on_where_it_was() {
        let (mut run, _) = Run::start(PipelineSpec::review(), "Add a flag");
        run.step_ended(StepEnd { plan: Some("1. Parse it".into()), ..ended("") });
        run.gate_answered(true, None);
        let text = serde_json::to_string(&run).unwrap();
        let mut back: Run = serde_json::from_str(&text).unwrap();
        assert_eq!(back.state, State::Running(1));
        assert_eq!(back.spec, run.spec);
        back.stop("interrupted, when Fenix closed");
        assert!(matches!(back.resume(), Some(Next::Gate { step: 1, .. })));
        let (Next::Start { step: 1, prompt }, Next::Start { step: 1, prompt: was }) = (back.retry(), run.retry_for_test()) else { panic!() };
        assert_eq!(prompt, was);
        assert!(prompt.contains("1. Parse it"), "{prompt}");
    }

    impl Run {
        /// Step under way, started again.
        fn retry_for_test(&mut self) -> Next {
            let i = self.step().unwrap();
            self.again(i)
        }
    }

    /// The steps that plan say where the plan goes: in the answer, not a
    /// file (a file's command is a card that can't be read whole).
    #[test]
    fn a_plan_step_gives_its_plan_in_its_answer() {
        for spec in PipelineSpec::built_in() {
            for step in spec.steps.iter().filter(|s| s.mode.as_deref() == Some("plan") && s.passes == Passes::Plan) {
                assert!(step.prompt.contains("Give the plan in your answer; don't write it to a file."), "{}: {}", spec.name, step.name);
            }
        }
    }

    #[test]
    fn json_is_found_fenced_or_bare() {
        assert_eq!(find_json("x ```json\n[1]\n``` y"), Some(serde_json::json!([1])));
        assert_eq!(find_json("the answer is {\"a\": 2} ok"), Some(serde_json::json!({ "a": 2 })));
        assert_eq!(find_json("none here"), None);
    }
}
