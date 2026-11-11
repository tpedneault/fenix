# Plan: code lens

- **Status:** planned, for the release after 1.5 (issue #106).
- **Scope:** what a language server offers for an item, drawn on a row
  of its own above it, dimmed: "▶ Run | Debug", "3 references",
  "2 implementations". Each one runs with a click, or by key in both key
  schemes. Fenix offers its own "▶ Run test" where the server gives none.
- **Lands as:** the stages in §4. Each compiles and passes. With
  `lsp.code_lens` off, or a file with no lenses, every pane behaves
  exactly as in 1.5. `lsp.code_lens` is **off by default** for now; see
  "Why off" in §3.

## 1. What it is

- A server that offers `codeLensProvider` is asked for a document's
  lenses (`textDocument/codeLens`):
  - when the document opens;
  - after typing pauses;
  - when the server asks (`workspace/codeLens/refresh`).
  A lens that comes without its command is resolved
  (`codeLens/resolve`) once its line is in view.
- The lenses of a line are drawn as one row above it: their titles
  joined by `|`, at the line's indent, in the gutter's colour. That row
  is a `RowKind::Virtual` row (soft wrap, stage 7). The caret and
  selections step over it; scrolling counts it.
- A click on a title runs that lens. `SPC c l` (`code.lens`; the same
  leader with standard keys, `Ctrl+K c l`) lists the lenses of the
  cursor's line in a picker: Enter runs one. With one lens, it runs
  directly.
- Running a lens:
  - A command the server says it executes (its
    `executeCommandProvider.commands`) is sent back as
    `workspace/executeCommand`.
  - Commands that clients are expected to run themselves are mapped to
    Fenix's own actions where their arguments say enough:
    `editor.action.showReferences` and the like open the references
    list; rust-analyzer's `runSingle` and `debugSingle` run in Task
    Output or start the debugger.
  - Anything else says plainly that Fenix can't run it.
- `lsp.code_lens` (off for now) turns it all on. `lsp.code_lens_off`
  lists languages left out.

## 2. Where Fenix stands

- **Virtual rows** exist but are only laid out in wrapped panes.
  `App::virtual_rows` is a `BTreeMap` of line → row texts per buffer,
  and stays empty in 1.5. Wrapped panes go by rows: `RowWalk` for
  scrolling (`app/row_scroll.rs`), `frame_layout` and
  `row_content_spans` for drawing, `wrapped_text_hit` for the mouse,
  `PaneRows` for Vim's motions by row. Virtual rows are already counted
  and stepped over on all of those paths.
- **A pane that doesn't wrap** goes by lines: `scroll_line` and an
  eased `rendered_scroll`, one row per display line (`RowMap::lines`),
  drawn by `content_spans_decorated` or `folded_content_spans`,
  scrolled sideways by `scroll_col`. It has no room for a row that
  isn't a line.
- **LSP:** `LspSession` holds each server's capabilities, its pending
  asks and what its documents were last sent. `semantic.rs` is the
  model for per-document asks: a pause, one ask out at a time, a stale
  answer let go, a timeout, refresh.

## 3. Design

### The row path for panes that don't wrap

The deciding question is how an unwrapped pane gets rows that aren't
lines. A second scroll model, with lines plus offsets, would touch every
place a pane's top line is read. Instead, **a pane whose buffer has
lenses goes down the row path**, as a wrapped pane does, at an unbounded
width:

- `App::row_cols(pane, buffer)` replaces the row path's use of
  `wrap_cols`:
  - `Some(width)` for a wrapped pane;
  - `Some(UNBOUNDED)` for a pane that doesn't wrap but whose buffer has
    virtual rows;
  - `None` otherwise, which is exactly 1.5's line path.
  `wrap_cols` keeps its meaning (the lines break) for everything that
  asks it.
- At `UNBOUNDED`, a line is one row (`break_line_rows` never breaks it).
  The row walk, `zt`/`zz`/`zb`, the wheel and following the cursor are
  unchanged.
- **Sideways scroll stays.** In row mode without wrap, `frame_layout`
  makes each text row's columns `scroll_col..` and records `scroll_col`
  in the `RowMap`. Drawing, the caret, selections, problems, the mouse
  and the semantic windows all go through the map, so they move with
  it. `ensure_cursor_visible_horizontally` runs as it does now.
- **Drawn by `row_content_spans`.** It learns to start a row at its
  columns' start through the line's display text, as it does for a
  continuation row. Folds work as they do in a wrapped pane.
- **What differs from an unwrapped pane without lenses**, while a buffer
  has lenses:
  - the view snaps a row at a time instead of easing;
  - sticky headers are off.
  Both are what a wrapped pane does today. rust-analyzer, tsserver and
  clangd send lenses for nearly every source file. With lenses on by
  default, almost every code buffer would lose both, compared with 1.5.
  That's why `lsp.code_lens` is off by default until "Easing and sticky
  headers in the row path" (§4, stage 6) lands.
- **Everything else as in 1.5.** A buffer with lenses and wrap off keeps
  1.5's behaviour for:
  - motion: `j`/`k` by line, and `gj`/`gk` the same as `j`/`k`;
  - hit-testing;
  - sideways scroll;
  - selections;
  - overlays: the caret, extra carets, brackets, search, problems,
    tokens and decorations.
  Only rows are offset, by the lens rows above. **How it's checked:** a
  test seam (`App::force_row_path`) sends a pane down the row path with
  no virtual rows at all. Its tests run the existing unwrapped-pane
  scenarios both ways (moves, clicks and drags, sideways scroll past a
  long line, Visual selections, search highlights, problems, folds, tabs
  and wide chars) and assert the same results. Then the same scenarios
  run with a lens row on some lines, asserting the same results apart
  from those rows' offset.
- **Lens rows going away.** The server stops, restarts, or answers with
  no lenses, or the setting is turned off. The rows go, and the view
  keeps its top text row where it was: a top inside the rows above a
  line (`scroll_row < above`) moves to the line's first text row (row
  0), and `scroll_row` drops by the rows removed. A pane left with no
  virtual rows leaves the row path with `scroll_line` unchanged. The
  same holds when rows appear: the top text row doesn't move, and the
  new rows go above it, out of view, until scrolled to.
- **Vim's motions by row** (`PaneRows`) get the same width, so `gj`/`gk`
  in such a pane are `j`/`k` with the lens rows stepped over.
  `PageUp`/`PageDown` go by rows.
- **Side by side, diffs, tables, terminals, pages:** never row mode,
  since they never have lenses.

### Asking

- **Which servers.** Every one of the buffer's servers that offers
  `codeLensProvider`. Unlike semantic tokens, lenses from two servers
  sit side by side, the primary's first. Only a buffer's own sync asks
  (`App::semantic_of`'s model), never an agent's look at a file.
- **When.** On open; 500 ms after typing pauses (lenses are slower to
  compute than tokens, and less urgent); on
  `workspace/codeLens/refresh` for documents drawn now, the rest when
  next drawn. One ask out per server and document. An answer for text
  since edited is let go and asked again. ContentModified,
  ServerCancelled and RequestCancelled are retried. A 10 s timeout
  sends `$/cancelRequest`.
- **Resolve.** A lens without a command is resolved
  (`codeLens/resolve`) when its line is within the rows drawn, at most
  16 at a time per server. Until it's resolved, its title is drawn as
  `…`.
- **Between answers** lenses move by line with each change sent
  (`changed_span`, turned into lines). Those whose line was replaced
  stay on the line their range now starts on: they're redrawn when the
  answer comes. That keeps the rows from jumping while typing.

### Drawing

- `App::virtual_rows` becomes the lenses' layout: a row's text, plus for
  each lens its column range in the row and what runs it
  (`LensHit { server, index }`), for the mouse.
- **The text** is the titles joined by ` | `, cut to the pane's width,
  at the line's display indent.
- **Colour:** gutter colour, with the title under the pointer in the
  text colour (a hover affordance).
- **A row per line, not per lens:** five lenses on one function are one
  row.
- **Folds:** a closed fold's header keeps its lenses; lines hidden in it
  show none.
- **Long titles** are cut, keeping whole titles where they fit, then
  `…`.

### Running

`fenix_lsp::code_lens::action` classifies a resolved lens's command:

| Command | Fenix does |
|---|---|
| Listed in the server's `executeCommandProvider.commands` | `workspace/executeCommand` with its arguments; the server's edits (`workspace/applyEdit`) are refused as now; a message if the server answers with an error |
| `editor.action.showReferences`, `rust-analyzer.showReferences` (uri, position, locations) | the references list, as `gr` shows it |
| `rust-analyzer.runSingle` (a cargo runnable) | `cargo <cargoArgs> -- <executableArgs>` in Task Output, in the runnable's workspace root |
| `rust-analyzer.debugSingle` | the debugger on that test, as `SPC p r`'s debug path starts one (cargo build with `--no-run`, then the test binary) |
| Fenix's own (below) | the action it names |
| Anything else | "Fenix can't run *title* (*command*)" |

### Fenix's own lenses

For a file whose servers give no lenses, or no run lens on a line,
Fenix offers "▶ Run test" over each test `fenix_tasks::test_at` finds:
Rust `#[test]`, pytest and Go `TestXxx`. It runs as `SPC p r` would on
that line. `lsp.code_lens_tests` (on) turns these off on their own.

### Settings and keys

| | Default | |
|---|---|---|
| `lsp.code_lens` | off | Lenses from language servers, and Fenix's own. Off: none asked or drawn, and every pane as in 1.5. |
| `lsp.code_lens_off` | none | Languages left out. |
| `lsp.code_lens_tests` | on | Fenix's "▶ Run test" lenses. |
| `SPC c l` (`code.lens`) | | The cursor line's lenses: one runs, several list. |

**Why off, for now:** most code files would get lenses. In the row path
their panes snap and lose sticky headers, so lenses on by default would
take eased scrolling and sticky headers away from almost every code
buffer. They default on once stage 6 keeps both. Until then they're
there for anyone who wants them.

### Not done

- Lenses on a line in the middle of a wrapped line's rows: a lens row
  is always above the line's first row.
- `$/progress` while resolving.
- Clients' commands beyond those in the table.

## 4. Stages

1. **Plan and model.** This document, and `fenix_lsp::code_lens`
   (pure, tested):
   - reading the provider and the executable commands;
   - the client capabilities;
   - turning a list of lenses into rows (titles, columns, per-line
     order across servers);
   - moving lenses by line through a change;
   - `action` (the command classification in the table above).
2. **The row path for panes that don't wrap.**
   - `row_cols`, and `UNBOUNDED` rows with sideways scroll through
     `frame_layout`, `row_content_spans`, `RowWalk`, the mouse and
     `PaneRows`.
   - Tested through `virtual_rows` filled by hand: scroll, keys, click,
     selection, sideways scroll, folds.
   - With no virtual rows, nothing changes.
3. **Asking and drawing.**
   - The settings, and the client capability advertised when on.
   - Asks per buffer and server, resolve as lines come into view,
     refresh, following edits, the rows filled from the answers.
   - The hover colour.
4. **Running.** Click on a title, `SPC c l` and its picker,
   `executeCommand`, references, rust-analyzer's run and debug, and the
   "can't run" message.
5. **Fenix's own test lenses**, a measurement on a large file,
   CHANGELOG, FEATURES, KEYBINDINGS and "As built".
6. **Easing and sticky headers in the row path**, before the default
   turns on. In a pane that doesn't wrap, every line has exactly one
   text row plus its lens rows.
   - `rendered_scroll` becomes a fractional offset over rows there,
     eased as lines are now. The absolute row of a line is its index
     plus the lens rows above it, counted from `virtual_rows`' `BTreeMap`.
   - Sticky headers are worked out from the top text row's line, as
     today.
   - Tests check that a lensed unwrapped pane eases and shows sticky
     headers the same as an unlensed one.
   - Then `lsp.code_lens` defaults on. Wrapped panes keep snapping.

## 5. As built

### Stage 1

`fenix_lsp::code_lens` as planned. The plan was revised after review:
lenses are off by default until stage 6.

### Stage 2

- **`App::row_cols(pane, buffer)`** decides the row path: `wrap_cols`, else `rows::UNBOUNDED` when the
  buffer has virtual rows (or `App::force_row_path`, a test seam) and
  the pane could wrap. Its users:
  - `frame_layout`;
  - `row_walk` (follow, `zt`/`zz`/`zb`, and the wheel on an unfocused
    pane);
  - the mouse's hit test;
  - PageUp/PageDown;
  - and in `redraw`: the drawer choice, the snapping and sticky
    headers.
- **What still asks `wrap_cols`**, meaning "the lines really break":
  - the reset of sideways scroll;
  - `ensure_cursor_visible_horizontally`;
  - the eased sideways scroll;
  - the per-line highlight windows;
  - Vim's motions by row: at `UNBOUNDED`, `sync_screen_rows` gives
    `LineRows`, so `gj`/`gk` are `j`/`k` with the sticky column;
  - the focused wheel, which at `UNBOUNDED` moves the cursor by lines
    as 1.5 did while the view follows.
- **Sideways scroll.** `frame_layout` keeps `scroll_col` when the rows
  are `UNBOUNDED`, and `RowMap` already offsets by it.
  `row_content_spans` starts a row at the scroll and draws at most
  `MAX_ROW_CHARS` of it, as the line path does. Lens rows don't scroll.
- **`App::set_virtual_rows(buffer, rows)`** replaces a buffer's rows
  and keeps each pane's top text row in place.
- **Tested.**
  - `force_row_path` against the line path: frame by frame, the drawn
    chars and their colours, the overlays, every cell's hit, the
    scroll and the cursor match. Covered: moves, `gj`/`gk`, `$`/`0`/`w`/`%`,
    a long line with tabs, `é` and `漢` scrolled sideways, Visual,
    search, folds, the wheel, a click.
  - The line path's frames are compared once settled, since it eases
    and the row path snaps.
  - With lens rows on lines 1 and 3: the same moves, and the caret,
    the selection and the search's matches shifted by exactly the lens
    rows above them.
  - Rows appearing and going keep the top text row in place, and with
    none left the pane is on the line path again.
- **After review.**
  - Panes in every workspace and frame keep their top through
    `set_virtual_rows`, not just the live workspace's.
  - A top at its line's very first row stays there when rows come
    above that line, so a lens arriving on the top line shows (on line
    0 it couldn't be reached otherwise). `follow` shows a line's lens
    when the cursor is on the top line's first text row with the lens
    hidden, so `gg`, `k` and the wheel reach it.
  - Off the rows (above, below, right of a line's end, a drag past an
    edge), a hit is where the line path has it, through the sideways
    scroll. A lens row is no place in the text: a click, drag or hover
    there moves no cursor and scrolls nothing.
  - PageUp and PageDown in an unwrapped pane with lens rows go by rows,
    lens rows counted (`page_by_rows`), as in a wrapped pane.
