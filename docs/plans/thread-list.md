# Plan: the thread list -- pipeline runs, archiving, by project (#109)

`SPC a t` lists the project's agent threads flat. A pipeline (`SPC a P`)
leaves its steps among the others with nothing tying them to what was
asked, and nothing puts a finished thread away. This reworks the list.

## The list

One row a line, left-aligned, so a narrow pane cuts the end of a row,
never its start (the state and age go last):

```text
▾ Fix the flaky login test · Review and fix · done · 2 h
    1 plan    claude  ✓
    2 build   codex   ✓
    3 review  claude  ● working
  Explain the parser · claude · 5 h
▸ Add CSV export · Build and review · stopped · 1 d       (2 steps)
```

- A **run** is a heading: what was asked (its first line, cut to fit),
  the pipeline's name, how it ended (`done`, `stopped`, or `running`),
  how long ago. Its steps go under it, in order, with their agent and
  state. A run under way is unfolded; a finished one folded.
- A thread started on its own is a row of its own, as today.
- Waiting on you first, then the most recently active.
- **By project** (`SPC a l`, or `Alt+P` in the list): every project's
  threads, each project a heading, the current one first, the same rows
  under each. `SPC a T` (at work, anywhere) is unchanged.
- **Archived** threads are left out; `Alt+H` shows them too, marked
  `archived`. Typing filters as in every picker; a run's heading matches
  its request, its pipeline and its steps.

## Keys

The list is a picker, and typing is its query, so its keys are the same
with Vim keys and standard keys:

| Key | Does |
|---|---|
| `Enter` on a step or thread | Opens it in the agents' panel |
| `Enter` on a run | A summary of the run: what was asked, each step and how it went, how it ended |
| `Tab` | Folds or unfolds the run |
| `Alt+R` on a run | Its changes, all steps', as one review (`SPC a r` from its summary too) |
| `Alt+A` | Archives the thread, or the whole run; on an archived one, brings it back |
| `Alt+H` | Shows archived threads too, or not |
| `Alt+P` | This project's threads / every project's |

Archiving never deletes: an archived thread is stored as it was, can be
searched (`Alt+H`), opened, continued and brought back.

## Stored

A thread's stored metadata says, when it's a pipeline's step, which run
and step: `run: { id, pipeline, request, step, stepName, started, ended,
ledger }` (`ledger`: the step whose changes the run's review shows), and
`archived: true` when it is. The index carries both, so the list never
reads a thread to lay itself out. A thread stored before has neither and
is a single, unarchived entry; an older Fenix reading a newer file skips
what it doesn't know.

## Where

The grouping, ordering and filtering are `fenix-acp`'s (`listing`),
tested without a display; the store's fields are `fenix-acp`'s `store`;
the picker, its keys and the summary are the GUI's.
