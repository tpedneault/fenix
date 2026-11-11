# Plan: cards that keep their rows, pipelines that can go on

Two things went wrong in one session of *Plan, build, review*:

1. The plan step asked to run a command that wrote the whole plan to a
   file (`cat > plan.md <<'EOF' …`). From that card down, the answers no
   longer sat on their backgrounds.
2. The command refused, the run was over. The thread was left as an
   ordinary thread, with no way to send what it had on to build.

Both are reproduced by a test each (under *Tests*), and both come from a
rule the code doesn't hold itself to. This plan fixes the two cases, then
the rules.

## 1. The card: a row is one line

### What happens

`Grid::put` and `Grid::write` (`fenix-gui/src/page.rs`) copy a string's
characters into a row as they are, and `Grid::finish` joins the rows with
`\n`. Everything else on a page -- colours, chips, panels, cards, the
focused row -- is addressed by row number. So a `\n` written *into* a row
makes it two lines of text, and every line under it is one lower than
the chip and panel meant for it.

The permission card writes the tool call's title with `fit`, which cuts
to a length and nothing else (`agent_page.rs`, `Entry::Permission`).
Claude Code's title for a shell command is the command, so a heredoc puts
a newline in the first hundred characters, and one more for each short
line of the plan after it. Three newlines, and the highlight for `y` is
three rows above the word *Yes*.

The same string breaks the page again once answered:

- the tool card's title (`Entry::Tool`) is written the same way, so the
  page stays broken for as long as the card is in view;
- the answered line (`✓ {title} — {answer}`) is cut as a whole, so a
  long title leaves no room for what you answered.

Others are open to it and haven't bitten yet: an answer's name, a plan
entry, an elicitation's field title and URL, a path, a line of tool
output with a `\r` or a tab in it. `command_shown` is the one place that
flattens its text first.

And the card shows one cut line of a command you're asked to allow. `v`
on it shows the title again, not the command (`entry_text`). What the
end of a long command does can't be read before saying yes.

### What changes

**The grid holds the rule, not each caller.** `put` and `write` store a
space for any control character (`\n`, `\r`, `\t`, the rest of
`char::is_control`). One cell in, one cell out, so the columns callers
worked out still hold. After this no page laid out on a `Grid` can
shift its own rows, whatever text it's handed. `finish` asserts in debug
builds that it gives as many lines as it had rows.

**Text for one row is made one line on purpose.** `page::one_line(text)`
-- runs of whitespace to one space, as `command_shown` does now -- for a
tool's title, an answer's name, a plan entry, a field's title, with `fit`
after it. The grid's rule is the net; this is what makes the row read
well rather than as a string of gaps.

**The question shows what's asked.** For a call with a command
(`ToolCall::command_line`, or the title when its kind is `execute`):

```text
 ? The agent asks to run a command (14 lines)
   $ cat > /tmp/plan.md <<'EOF'
     # Plan
     1. Change a.rs
     … 10 more lines · v shows it all …
     EOF
   y  Yes
   n  No
```

- its own lines, each wrapped to the card, the first five and the last
  one, with the count between: the start says what it is, the end is
  where a `&& rm -rf` would be;
- at most eight rows whatever the width, so the answers stay on screen
  in a short dock;
- `v` opens the whole command, then its input as the agent gave it.

For anything else the title is wrapped to three rows at most. The
answered line cuts the title, never the answer.

**The answers stay in view.** The card's focused row is its first
(`g.focus(top, …)`). With eight rows at most above the answers that's
enough in most docks; to be checked in a short one, and the focus moved
to the chosen answer's row if it isn't.

## 2. The pipeline: stopping isn't the end of it

### What happens

Refusing a permission ends Claude Code's turn as `cancelled` (the hub
already counts on this for a plan turned down, `hub.rs`, `turn_ended`).
`Run::step_ended` (`fenix-acp/src/pipeline.rs`) takes any stop but
`end_turn` as the end of the run:

```rust
let why = format!("step {} ended: {}", self.spec.steps[i].name, end.stop);
self.state = State::Stopped(why.clone());
```

and `pipeline_finished` (`fenix-gui/src/app/agent_pipelines.rs`) then
drops the run: its state, what each step passed, which threads were its
steps. Nothing is left to go on *from*. Write to the thread again and it
answers, but its turn's end is no step's end any more, so no gate comes.

The gate is the only way forward there is, and it's only asked after a
turn that ended well. Every other ending is a dead end the same way:

| What happened | Today |
|---|---|
| You refused a command the step asked for | run over |
| `SPC a k` on a step (the first round) | run over |
| The agent hit a token or request limit, or refused | run over |
| The agent crashed (it's started again; the run isn't) | run over |
| `n` at a gate, or `q` in the pane `e` opened | run over |
| A review's answer had no findings list | run over |
| A command step failed with its rounds used up | run over |
| Fenix was closed | "stopped: interrupted", run over |

### What changes

**A step that ends badly asks, it doesn't decide.** `step_ended` with
any stop but `end_turn` goes to the step's gate, saying why:

```text
 ? Fenix asks: Pipeline: plan stopped early (cancelled). Go on to build
   with what it has, or write to it for another round
   y  Go on to build
   e  Edit what's passed on, then go on
   r  Run plan again
   n  Stop
```

- What's passed is what the step has: the plan it offered, else its
  last answer. With neither, `y` and `e` aren't offered.
- Writing to the step is another round, as at any gate (that part works
  today: `gate_reopened`).
- `r` starts the step again on a thread of its own, with the prompt it
  had. It's offered at every gate, not only this one.
- The same for a review with no findings list, and a command out of
  rounds (`y` there is "go on anyway").
- `SPC a k` on a step stops the step and brings this gate; `n` stops the
  run. Today it stops both at once. This is the one change to something
  the docs promise, and it's what makes `SPC a k` safe to reach for.

In the engine: `Next::Gate` gains `trouble: Option<String>`, and
`Run::retry()` begins the step at the gate again. No new state: it's
`AtGate`, so the dock's title, `]s`, and the composer's hint all hold.

**A stopped run can be picked up.** `State::Stopped` keeps the step it
stopped at. `pipeline_finished` keeps a stopped run (its `Run`, its
threads, its ledger) rather than dropping it; a run that's *done* is
dropped as now. `SPC a g` (`agents.pipeline_go`, free today) on any of
its steps, or on its heading in `SPC a t`:

- stopped: the gate comes back on the step it stopped at, as above;
- at a gate: the same as `y`;
- a step at work: says so, and that `SPC a k` stops it first.

The notice a stopped run leaves says it: *Pipeline Plan, build, review:
stopped at plan's gate -- SPC a g picks it up*.

**It survives a restart.** The run is stored beside its threads (the
spec as it ran, what was asked, what each step passed, the round counts,
where it was), written when it moves. A run Fenix was closed on comes
back stopped at the step it was on, and `SPC a g` picks it up; its
threads resume the way any thread does. Kept until the run is archived
or deleted. An older Fenix ignores the file.

**The plan steps say where the plan goes.** The built-in prompts with
`mode = "plan"` gain: *Give the plan in your answer; don't write it to a
file.* It makes the card that started this rarer; it isn't what fixes it.

## Tests

Two that fail today, kept as the regressions:

- `agent_page`: a permission card titled with a heredoc. The row of the
  text *Yes* is the row of its chosen chip and of its panel.
- `agent_pipelines`: a step's turn ends `cancelled`. The run is still
  there, a gate is up, `y` starts the next step with the plan.

And for the rules:

- `page`: `put` and `write` with every control character; the page has
  as many lines as rows.
- `agent_page`: every kind of entry, in each of its states, given
  hostile text (newlines, tabs, `\r\n`, 10 000 characters, wide
  characters) in each of its strings, at 30, 60 and 110 columns: rows
  equal lines, `page::overflow` finds nothing, every chip sits on its
  own text. If wide characters fail it, `fit` counts cells (the page
  already knows how, `Page::cells`); that isn't checked yet.
- `pipeline`: each row of the table above ends at a gate or a stopped
  run that `resume` brings back; none ends with nothing to go on from.
  A run stored, read back, goes on from the same step with the same
  prompt.

## In what order

1. **The card** (1.5.1): the grid's rule, `one_line`, the command on
   the question, the tests. A fix only; nothing stored changes.
2. **The gate on a bad ending, `r`, `SPC a g`** (1.5.1 if `SPC a k`
   keeping the run counts as a fix, else 1.6.0), with
   [AGENTS.md](../AGENTS.md)'s *Pipelines* and
   [KEYBINDINGS.md](../KEYBINDINGS.md).
3. **Runs stored across a restart** (1.6.0): a new file in the state
   folder.

Nothing here is in fenix-extensions: the card and the pipeline are both
Fenix's own. Whether its `ui-kit` has the same hole (a newline in a
cell) is worth one look while doing step 1; it wasn't looked at here.
