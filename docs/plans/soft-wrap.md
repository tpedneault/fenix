# Plan: soft wrap

- **Status:** built, for the release after 1.5 (issue #107); see §5.
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

### Stage 3: everything mapped

- Most of the plan's list was mapped in stage 1, and lands on its row
  once stage 2's map has wrapped rows: the caret and the completion and
  hover cards anchored to it, selections, search matches, brackets, the
  pulse, underlines and decoration marks, todo tints, problems, and the
  Git and debug marks (`first_row`, `gutter_lines`). The debugger's
  stopped line is tinted on all its rows.
- `text_hit` in a wrapped pane (`wrapped_text_hit`) goes through the
  pane's rows: right of a row the line goes on from is that row's last
  char, inside a continuation row's indent its first, above or below
  the rows shown the line before or after. Click, drag, double and
  triple click, Alt+drag and the hover card all use it. The column
  maps it needs are `line_col_map`, shared with the unwrapped path.
- `fold_marker_click` takes a marker only on a line's first row.
- The layers' geometry: `PaneExtras::rows` carries the row map a code
  pane was drawn with into `GeoPane::map`; `Geometry::resolve` places an
  extension's `Anchor::Text` through it (keeping 1.5's bounds: off the
  rows that fit, or left of the sideways scroll, is nowhere), and
  `decoration_click` hits on it. This also fixes the fold mistake noted
  in stage 1.
- After review: the rule for a cell right of a short row is
  `RowMap::at`'s alone, which `wrapped_text_hit` and `decoration_click`
  both use; `decoration_click` without a drawn map builds one by
  `pane_rows`. Left: `Geometry::resolve` gives `locate` an extension's
  column as it is, a visual column, as 1.5 used it; on a line with a tab
  or a decoration a char column would be placed wrong, now perhaps on
  the wrong row of a wrapped line. Which the extension API means is for
  that API to say.
- Left as they are: the caret's glide counts rows from the top's
  display line, which in a wrapped pane is the real row on screen; the
  jump beacon flashes a line's first row; the unfold reveal still
  counts display lines (cosmetic, with wrap on).

### Stage 4: scrolling

- `app/row_scroll.rs`: a `RowWalk` over a pane's display lines breaks
  each line it passes once (`line_breaks`) and answers `step(pos, ±n)`,
  `distance(from, to, cap)`, `end()` and `row_of(line, col)`. This is
  the plan's `row_step` / `rows_from_top`: every walk starts from the
  view's top or the cursor and is bounded by the pane's rows (`cap`),
  so nothing breaks more lines than about two panes' worth.
- `follow` is `scroll_for_cursor` by rows: a cursor more than half a
  pane out of view, or one that jumped, is centred; one that stepped
  past an edge scrolls just far enough; the last row never goes above
  the bottom. `ensure_cursor_visible`, `reveal_cursor` (an unfocused
  pane's jump) and `search_reveal` use it when the pane wraps
  (`follow_cursor_by_rows`), and snap (`set_row_top`): no easing.
- `zt` / `zz` / `zb` (`scroll_window_at`) put the cursor's row, not its
  line's first, at the top, middle or bottom (`scroll_rows_to_cursor`).
- The wheel over a wrapped pane without the focus scrolls it by rows
  (`scroll_by_rows`). Over the focused pane it still moves the cursor
  by lines, as in 1.5, and the view follows by rows.
- A drag held above or below a wrapped pane moves the cursor a line at
  a time (stage 3's `wrapped_text_hit`), and the view follows.
- The reload clamp resets `scroll_row` with `scroll_line`. A resize
  needs nothing: a `scroll_row` past a line's rows is held to its last.
- The overview ruler's band is the lines shown in a wrapped pane.
- Moved to stage 5: PageUp/PageDown by the pane's rows. They move the
  cursor, and a cursor moving by rows is stage 5's `ScreenRows`.
- After review: a walk breaks a line only as far as the rows it needs
  (`rows_upto`, by `break_line_rows`, doubling when it needs more), so
  following a cursor down a long line, measuring from the top to it and
  checking the end of the file cost the rows walked, not the lines'
  lengths. Stepping back into a line from its end still breaks all of
  it (its rows are counted from its start); the cache is stage 7's.
- The top is held to its line's rows once, where it's read
  (`RowWalk::clamp`): a resize, an edit from another pane or a fold can
  leave `scroll_row` past them. `view.toggle_wrap` resets `scroll_row`
  in every pane showing the buffer.
- The wheel over a focused wrapped pane scrolls its view by rows, three
  a notch, and moves the cursor by rows only as far as keeps it in view,
  onto the first char of the top or bottom row (`scroll_focused_by_rows`);
  an unwrapped pane is as in 1.5.
- A cursor on a line inside a closed fold is on its header's row. The
  downward reach of "just far enough" is half a pane past the bottom,
  as upward. `search_reveal` in a wrapped pane may centre where 1.5
  scrolled just far enough. The ruler's band counts the line of the
  partly shown last row.
- Still O(lines) a frame, as in 1.5: `pane_display_lines`, and finding
  the cursor's display line in it (now a binary search).
- Still O(lines) a frame, as in 1.5 and not made worse:
  `pane_display_lines` builds a `Vec` of every display line, called a
  few times a frame. For stage 7.

### Stage 5: motion

- `fenix_vim::ScreenRows` (`row_move(buffer, at, delta, x)`,
  `row_bounds(buffer, at)`) and `LineRows`, the no-wrap one, in
  `fenix-vim/src/screen.rs`. Rather than `handle_key_with`, the GUI
  sets it before each key (`VimState::set_screen_rows`, from
  `dispatch_keypress` by `sync_screen_rows`): everything a key runs --
  the extra cursors' replay, `.`, a macro -- then goes by the same
  rows without another parameter through every key handler. The GUI's
  is `PaneRows`, a snapshot of the focused pane's width, tab stops and
  decorations that doesn't borrow `App`.
- The goal cell is kept for each cursor (the char it's on, and the
  cell), so extra cursors each keep their own, and only from one key to
  the next: `VimState::begin_key`, called for every key routed, makes
  the cells the last key kept the ones kept now, and a key that moved
  no cursor by row (`h`, `x`, an undo, a switch of buffer) forgets them
  -- unless it only began a command (`g`, a count), whose next key goes
  on with them.
- `Motion::ScreenDown` / `ScreenUp` (`gj`, `gk`, `g<Down>`, `g<Up>`) in
  Normal, Visual (Block too) and after an operator: charwise and
  exclusive, as Vim's, with a count; never onto the empty line after a
  last newline; without wrap, the line motions.
- Up and Down with standard keys and in Insert go by rows. Home and End
  go to the row's ends first, then the line's: End on a row before the
  line's last that ends in a blank goes after its last word (the blank's
  place, where typing goes on with that word); on a row ending inside a
  word broken by the row, to that row's last char, since the place after
  it is drawn at the next row's start. Shift+End selects to there.
- `gj` on the last row, or a count of them past it, stops on the last
  row there is, as `j` does: never onto the empty line after a last
  newline (a row back from it, at the same cell).
- The rows are set (`sync_screen_rows`) and the cells kept rotate
  (`begin_key`) where every key is routed, `route_keypress`: a macro or
  `.` that goes to another pane moves by that pane's rows. In a
  picker, the sidebar or a terminal they're a row a line. The rows'
  copy of the buffer's decorations is shared from key to key while
  `decorations_rev` says they haven't changed: it's bumped wherever an
  extension's, the debugger's or the agents' decorations are set, and
  by the shift ahead of a frame only when an edit moved them or a
  buffer went. A line is broken only as far as the rows a move needs.
- A move by row reads only the cells the last key kept, never this
  key's: with extra cursors, one moved first may land on another's
  char. A cursor put by a click or the wheel's hold forgets them.
- PageUp/PageDown in a wrapped pane move the cursor by the pane's rows
  less one (`VimState::move_rows`; twenty rows without a window).
- Adding a cursor above or below (Ctrl+Alt+Up/Down) stays by line; the
  extra cursors then each move by their own rows.
- One stage 2 test changed: End now stops at a row's end first, so the
  test presses it twice.
- The rows a key moves by can't go stale within it: `PaneRows` holds
  only the pane's width, tab stops and decorations, and lays each line
  out from the buffer as it is when asked, so a macro, `.`, an operator
  whose motion is `gj` after an edit, or the extra cursors' replay
  (each caret's edit moving the text under the next) see the text as
  edited. What it holds can lag within one key: decorations an edit
  moves are moved by their extension later, so a key that edits and
  then moves by row on a decorated line goes by where they were; and
  the cursor's line, whose decorations are laid out as on it, is the
  one the key started on.

### Stage 6: settings and keys

- `editor.wrap` (default off), `editor.wrap_languages` (default
  `markdown, text`; `text` is a `.txt` or `.text` file, any case -- a
  file of no language Fenix knows otherwise follows `editor.wrap`, as
  in 1.5 -- and an extension in capitals counts for wrap, `README.MD`
  being Markdown, though highlighting still goes by its own case) and `editor.nowrap_languages` (default none), read
  in that order of strength from the bottom: the nowrap list, then the
  wrap list, then `editor.wrap`. An agent's `Prompt` buffer wraps
  whatever they say; `view.toggle_wrap` for a buffer outranks them all.
  The default is a few map lookups and the buffer's language, nothing
  read from the file, so asking it every frame costs nothing.
- `wrap_refusal` says why a pane doesn't wrap (a terminal, a table, a
  page or Home, a side by side, a diff, a Git page), and gates the
  setting, a language's default and `Prompt` alike; `view.toggle_wrap`
  there changes nothing and says why.
- `SPC t w` and, with standard keys, `Alt+Z` (free in 1.5).
- Tested with real panes: a table (`table.toggle`), a page (the
  settings page) and a side by side refuse, and say so; a terminal's
  refusal is tested by its kind only (a real one starts a shell).
- Not covered by a test: `decoration_click` with a fold closed or at a
  break, the redraw half of debug marks and change seams, a decorated
  line wrapped, the blame column on continuation rows, Block-visual
  `gj`, `.` repeating `dgj`, PageDown with a real pane's height and at
  the buffer's ends, an unfocused pane's `reveal_cursor` and
  `search_reveal`, the reload clamp, folds, tabs, decorations and wide
  chars inside a move by row.
- After review: the settings page opens `editor.wrap_languages` with its
  default to edit, and emptied it stores an empty list rather than the
  default (`LISTED_DEFAULTS`); other list settings, whose unset is
  empty, are as they were. The per-buffer toggle isn't kept with the
  session and goes when its buffer closes.

### Stage 7: hardening, and rows that aren't lines

- Measured on a million lines of 50 chars at 20 columns, in a release
  build (`measure_a_million_lines`, ignored; run it with `--ignored
  --nocapture`): a frame's rows, follow included, 1.5 ms with wrap on
  and 1.6 ms with it off; `G` and ten `gk`, 13 ms against 8 ms. The
  `Vec` of display lines a frame builds a few times (as 1.5 does) is
  that cheap, so it stays.
- The row walk's breaks are kept (`break_cache`, by buffer and line,
  `App::line_breaks`). They're kept while these stay the same:
  - the buffer's edit count;
  - the width;
  - the tab stops;
  - the decorations revision, which is global, so an extension setting
    decorations on any buffer starts every line again;
  - on a decorated line, whether the cursor is on it.
  So a long line stepped back into is broken once per edit, not once per
  frame. A hit costs a few comparisons and an `Arc` clone of the rows.
  The cache holds at most 4096 lines, then starts again.
- Only the row walk reads the cache: scrolling, follow, `zt`/`zz`/`zb`,
  the wheel and the caret's row. Two others still break lines
  themselves:
  - drawing (`pane_layout`), which needs only the rows shown, so it
    breaks no further than the bottom;
  - Vim's motions by row (`PaneRows`), which take a snapshot per key
    without borrowing the app.
- No arithmetic long-line path: `break_line_rows` already reads only
  what the rows wanted can hold, and the cache covers stepping back. A
  very long line is still copied whole once a frame to be drawn, as it
  is with wrap off.
- `RowKind::Virtual` rows: `App::virtual_rows` (by buffer and line, a
  list of texts) puts rows above a line in a wrapped pane, drawn at the
  line's indent in the gutter's colour, the line's number on its text's
  first row. The row walk counts them as the line's first rows, so
  scrolling, `zt`/`zz`/`zb` and the wheel go by them; the caret, a
  selection and the gutter marks go by the text's rows; a click on one
  is the line's start, and a move by row (`gj`, the arrows) steps over
  them. Nothing fills them yet: code lens (#106) will. In a pane that
  doesn't wrap they aren't shown -- its scroll is by lines -- so a lens
  in such a pane needs it on the row path: the step that comes with
  code lens.
