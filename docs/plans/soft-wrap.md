# Plan: soft wrap

- **Status:** in progress, for the release after 1.5 (issue #107).
- **Scope:** a line wider than its pane goes on under itself, per buffer,
  with a default in settings; the agents' compose pane wraps. The row
  model it brings is also what code lens (#106) needs: rows in a pane
  that aren't lines of the buffer.
- **Lands as:** the stages in §4, each compiling and passing with wrap
  off behaving exactly as before.

## 1. What it is

- A line wider than its pane continues on the next rows, broken after
  the last whitespace that fits (in the middle of a word only when the
  word alone is wider), at the pane's real width: a resize or a split
  wraps again.
- `view.toggle_wrap` (`SPC t w`; Alt+Z with standard keys) turns it on
  or off for the buffer in front. `editor.wrap` is the default, with a
  list of languages that wrap whatever it says (Markdown, plain text)
  and one of those that never do.
- An agent's prompt (a `Prompt` buffer) wraps by default.
- A continuation row keeps the line's indent (at most half the pane);
  the gutter's number, signs and fold marker are on the line's first row
  only.
- Movement follows the screen where the scheme expects it: `gj`/`gk`
  (new) by row and `j`/`k` by line with Vim keys; Up/Down by row with
  standard keys and in Insert; Home and End go to the row's ends first,
  then the line's.
- Never wrapped: tables, diffs, merges, Git pages, the graph, terminals,
  pages and Home (already laid out to their pane), and a side-by-side
  pane (its filler rows need a row per line).

## 2. Where Fenix stands (1.5)

One screen row per display line. A pane's rows are
`pane_display_lines(pane, buffer) -> Vec<usize>` (`app/side_by_side.rs`):
the buffer's lines with folds applied (`folded_display_lines`,
`app/editor_ui.rs`) and, side by side, `FILLER` rows. `PaneState`
(`app.rs`) keeps `scroll_line` (an index into the display lines),
`rendered_scroll` (eased, in display lines) and `scroll_col`. Everything
that draws or hit-tests does its own `line - render_base_line` and
`col - scroll_col`:

- drawing: `content_spans_decorated` (gutter on every row, one string
  with `\n` between rows), `folded_content_spans`,
  `syntax_highlights_from_column`, the per-pane block of `redraw`
  (`col_maps` by row, indent guides, `remap_col` / `remap_segments`,
  three branches for overlays depending on folds, `render_base_override`);
- overlays: `visual_selection_segments`, `range_to_segments`,
  `bracket_match_segments`, `hlsearch_segments`, `focused_caret`,
  `pulse_overlay`, `change_overlays`, `beacon_row`, debug and Git marks,
  todo tints, `polish_pane` (problems, rainbow brackets, selection
  dots), sticky headers;
- hit-testing: `text_hit` (`app/mouse.rs`), `fold_marker_click`,
  `decoration_click`, `cell_at`, the layers' `GeoPane`;
- scrolling: `ensure_cursor_visible`, `reveal_cursor`, `search_reveal`,
  `reveal_lines`, `scroll_window_at` (`zt`/`zz`/`zb`),
  `scroll_unfocused_pane`, `scroll_focused_pane`, PageUp/PageDown,
  `update_rendered_scroll`, the overview ruler (`ruler_for`,
  `ruler_click`);
- motion: `fenix-vim`'s `motion::vertical`, `state/standard.rs`'s
  `Move::Up/Down/Home/End`, Insert's arrows, `Buffer::move_vertical`;
  the GUI only gets `VimEvent`s back, there is no callback into it.

A code pane is a grid of cells: a column per char after tabs are
expanded and decorations laid out, wide characters included. `redraw`
returns early with no GPU, so its per-pane block is tested through its
pieces.

There is no `gj`/`gk`, no `H`/`M`/`L`, no Ctrl+F/B/D/U, no scrollbar
beyond the overview ruler, no inlay hints from language servers, and no
per-language or per-buffer editor option yet.

## 3. Design

### Rows

`app/rows.rs`:

```rust
enum RowKind { Text, Filler, Virtual(u32) }
struct Row { line: usize, kind: RowKind, cols: Range<usize>, indent: usize, first: bool, last: bool }
```

`cols` are visual columns of the line's display text (tabs expanded,
decorations laid out); `indent` is what a continuation row is pushed in
by. A `RowMap` holds a pane's rows for a frame and is the only
translator between the text and the screen:

- `locate(line, col) -> Option<(row, x)>`: where a column is drawn;
- `segments(line, cols) -> [(row, x0, x1)]`: a range on a line, split
  across its rows;
- `at(row, x) -> (line, col)`: what's under a cell;
- `first_row(line)`, `rows_of(line)`.

With wrap off every line has one row with `cols = 0..` and the map is
what `line - render_base_line` was.

### Breaking

`wrap::break_line(display, width, indent) -> Vec<Range<usize>>`, pure:
break after the last whitespace that fits, hard-break a longer word; a
continuation's indent is `min(leading, width / 2)`. A column per char,
to agree with the caret. A line past the highlighter's long-line limit
breaks arithmetically (row k is `k*w..(k+1)*w`), so its rows cost
nothing to find.

### Scroll position

`scroll_line` stays an index into the display lines and gains
`scroll_row`: how many of that line's rows are above the top. Never a
global row index: that needs the whole file laid out and moves at every
resize. Layout is the visible rows and a bounded walk from the anchor;
nothing counts the rows of a whole buffer. The overview ruler stays in
lines. A wrapped pane's vertical scroll snaps (no easing) at first.

### Choke points

1. `App::wrap_cols(pane, buffer) -> Option<usize>`: `None` and every
   path is as before. It needs the pane's width in columns outside
   `redraw`: kept per pane from the last frame and the `*_at` functions,
   with a setter for tests.
2. `App::pane_rows(pane, buffer, top, count) -> RowMap`.
3. `App::row_step(pane, buffer, (line, row), ±n)` and
   `rows_from_top(pane, cursor, cap)`, for all scrolling.
4. `fenix_vim::ScreenRows` (`row_move`, `row_bounds`): `handle_key`
   delegates to `handle_key_with(…, &dyn ScreenRows)` with a no-wrap
   one; the GUI passes a snapshot (width, tab stops, decorations) that
   doesn't borrow `App`. A `goal_x` beside `sticky_col`, cleared by any
   other motion.

### What is hard

- A selection (Char, Line, Block) and every overlay split across rows.
- Following the cursor inside a line taller than the pane.
- A decoration that changes width when the cursor enters its line
  re-breaks the line under the caret.
- What is added at a line's end (a problem's message, `⋯ N lines`) goes
  on the last row; sticky headers are off in a wrapped pane at first.

## 4. Stages

1. **The row map, one row per line.** `rows.rs`; the segment functions
   return line-space segments, mapped once; `text_hit`,
   `fold_marker_click`, `decoration_click`, the overlays and
   `polish_pane` go through it. Selections, extra cursors and the change
   pulse are then drawn with a fold closed too.
2. **Breaking and drawing.** `break_line`, `wrap_cols`, a per-buffer
   flag and `view.toggle_wrap` (unbound); rows drawn with the gutter
   once, the indent, guides, the fold's and the problem's text on the
   last row; highlights by a row's columns; `scroll_col` held at 0;
   `scroll_row`.
3. **Everything mapped.** The caret and popups, selections, search hits,
   brackets, the pulse, underlines, decoration marks, tints, Git and
   debug marks (first row); click, drag and hover; the layers' geometry.
4. **Scrolling.** Follow-cursor, reveal, `zt`/`zz`/`zb`, the wheel on a
   focused and an unfocused pane, PageUp/PageDown by the pane's rows,
   drag-scroll, the reload clamp, the ruler's band.
5. **Motion.** `ScreenRows`; `gj`/`gk` with counts, operators and
   Visual; Up/Down by row and Home/End by row then line with standard
   keys and in Insert; the extra cursors' replay.
6. **Settings and keys.** `editor.wrap` and the per-language lists;
   `Prompt` buffers on; `SPC t w` and Alt+Z; CONFIGURATION, KEYBINDINGS,
   FEATURES.
7. **Hardening, and rows that aren't lines.** The break cache, the
   long-line path, a million-line file with wrap on; a
   `RowKind::Virtual` row above a line that scrolling, hit-testing and
   the caret step over, ready for code lens.

## 5. As built

Where the implementation differs from the plan above, and why. Kept up
stage by stage.

### Stage 1: the row map

- `app/rows.rs` has `RowKind`, `Row` and `RowMap` as §3 has them, and
  two more things the per-pane block of `redraw` was doing inline:
  `PaneLayout` (the `RowMap`, each shown line's char-to-visual column
  map, its decorations laid out, the indent guides), built by
  `App::pane_layout`, and `FocusedOverlays` (the caret, the extra
  cursors, the selections, the pulse, the bracket pair, the search's
  matches), worked out by `App::focused_overlays`. Both are plain
  functions of the app, so tests ask them what a frame would draw
  (`focused_layout`, `focused_overlays_now`) instead of going through
  `redraw`, which needs a GPU.
- `col_maps` is keyed by line, not row: a wrapped line's rows share one.
- The segment functions take a range of lines and return line-space
  segments (`LineSegments`: line, char columns); `PaneLayout::to_rows`
  maps them once. `render_base_override` and the three branches by
  folds are gone: there is one path. The overlays are worked out on
  each run of consecutive lines shown (`RowMap::runs`), so the search
  doesn't look inside a closed fold; `range_to_segments` looks only at
  the lines its range is on.
- With a fold closed, the extra cursors, their selections, the change
  pulse and the search preview's caret are now drawn (1.5 dropped them).
  Nothing else is different with wrap off.
- The gutter's debug marks and stripe take `RowMap::gutter_lines` (a
  line on its first row, `NO_LINE` elsewhere) and skip `NO_LINE`; in
  1.5 a side-by-side filler row (`usize::MAX`) overflowed `line + 1`
  there.
- `decoration_click` read the layers' `base_line` (a display row) as a
  buffer line, which was wrong with a fold closed above; it now goes
  through a `RowMap` from that display row. `layers::Geometry::resolve`
  (an extension's `Anchor::Text`) has the same mistake and is left for
  stage 3, with the layers' geometry.
- `fold_marker_click` counts from `scroll_line`, not `rendered_scroll`:
  the same as 1.5, noted for stage 4.
- Still per row, for stage 2: `content_spans_decorated` /
  `folded_content_spans`, the indent guides, rainbow brackets (by
  `scroll_col`), sticky headers, Home's and pages' backgrounds.

### Stage 2: breaking and drawing

- `wrap::break_line(display, width, indent)` and
  `wrap::continuation_indent(display, width)`, as §3 has them. Two
  rules the plan didn't have: a break never falls between a char and a
  combining mark, joiner or variation selector after it; and a line
  whose indent alone is as wide as the pane (a tab in a narrow pane)
  gets no continuation indent, its break being inside the indent.
- `App::wrap_cols(pane, buffer)` is `buffer_wraps` (the per-buffer
  `wrap_overrides`, default off until stage 6) and `pane_can_wrap`
  (Text and Prompt buffers, not a table, terminal, page, extension
  surface or side-by-side pane), at `pane_text_cols`: the text columns
  each frame finds for the pane, with `set_pane_text_cols` for tests.
- `pane_layout` takes `RowsWanted` (display lines from the top, the
  `skip` of `scroll_row`, how many rows, the sideways scroll, the wrap
  width) and builds the rows itself, breaking each shown line's display
  text (decorated, tab-expanded or plain). With wrap off it is stage
  1's map. `pane_rows` builds the same rows for the hit-tests.
- A wrapped pane is drawn by `row_content_spans`, a row at a time;
  `content_spans_decorated`'s gutter and line text became
  `push_gutter` and `push_line_text`, which both use. A row's text is
  its columns of the line, with the syntax's colours; a continuation
  row has a blank gutter and its indent; a closed fold's count follows
  its header's last row; the guides continue down a continuation row's
  indent; rainbow brackets are placed by the row's columns.
- `PaneState::scroll_row` exists and the rows honour it; nothing sets it
  yet (stage 4). A wrapped pane draws with no fractional scroll offset,
  and `scroll_col` is held at 0.
- The current-line highlight is on the caret's row of a wrapped line,
  not all its rows.
- Not yet: clicks in a wrapped pane (stage 3), following the cursor
  down the rows (stage 4), moving by rows (stage 5). Until then a
  wrapped pane can put the cursor below its bottom.
- After review: a row's text takes all its columns but the last, which
  is the caret's after the row's last char (so a line exactly as wide
  as the pane takes two rows, and a caret is never past the edge). A row
  ends after the last word that ends on it; the blanks after that word
  hang at its end (one in the kept column, any more past the edge, where
  `RowMap::locate` holds a caret to the last column), so no row starts
  with the blank of a break. A word after nothing but blanks on its row
  (an indent) goes whole to the next row when it fits there.
  `break_line_rows` finds only the rows a frame wants and reads only the
  chars they can hold; finding a break is linear in the row's width.
- A wrapped pane reads, expands and colours each line once a frame
  (`drawn_line`) and slices its rows from it in order (`push_drawn`).
  A very long line's highlights are asked for by the columns its rows
  show (`syntax_highlights_in_columns`).
- `RowMap::first_row` is a line's first row only when that row is shown;
  `rows_of` is the rows shown. `locate` says nowhere for a column below
  the pane's bottom, and `at` holds a cell right of a row the line goes
  on from to that row's last column. A problem's message and a fold's
  count go after the line's last row only when it's shown, cut to the
  room left on it. Guides are drawn on the columns each row shows of the
  indent.
- `frame_layout` builds what a frame lays out of a pane; `redraw`,
  `pane_rows` (the hit-tests) and the tests' `focused_layout` all use
  it. The `:s` preview is worked out once a frame, not per run of lines.
- Second review: blanks after a row's last word hang only as far as the
  row's last (kept) column; any more go on to the next rows like other
  chars, so a row, blanks and all, is never wider than the pane, every
  column of a wrapped line has a cell of its own, and `break_line_rows`'
  first k rows are the first k of the whole break whatever k is (a
  property test checks it over blank runs, tabs, wide chars and marks).
  A blank that would fill a line's last row goes to a row of its own,
  so the place after the line's last char always has a cell (a row of
  `abcd ` at five columns is `abcd` / ` `).
  `locate` and `at` are each other's inverse on every cell but two: a
  click in a continuation row's indent is its first column, and a click
  right of a row that the line goes on from is that row's last column
  (on a full row, the kept column's own column: the blank hanging there,
  or the row's last char). `segments` is held to the pane's edge as a
  guard. A very long line's highlights are asked for by the char columns
  its rows' visual ones come to.
- Left: `pane_text_cols` and `wrap_overrides` keep an entry for a pane
  or buffer that's gone (one small entry each).
