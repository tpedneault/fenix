# Plan: semantic highlighting

- **Status:** built, for the release after 1.5 (issue #105); see §5.
- **Scope:** code coloured by what its language server knows
  (`textDocument/semanticTokens`), on top of tree-sitter's colours:
  a parameter apart from a local, a macro apart from a function, an
  interface apart from a type, a mutable variable apart from an
  immutable one.
- **Lands as:** the stages in §4. Each compiles and passes. With the
  setting off, Fenix behaves exactly as before: nothing is advertised,
  asked for or drawn.

## 1. What it is

- A server that offers semantic tokens is asked for a document's tokens
  when it opens, after typing pauses, and when the server asks for a
  refresh. Its tokens are drawn over the grammar's colours. Tree-sitter
  colours a file at once; the server's colours land on top when they
  arrive, and only where it named something.
- Each token type maps to a theme colour. Themes may name the new
  colours, and a theme that doesn't falls back to the nearest one it
  has. A token Fenix has no mapping for leaves the grammar's colour.
- While typing, the tokens Fenix holds move with each edit until the
  server's next answer, so colours don't flicker or slide.
- `lsp.semantic_highlighting` turns it on or off. A language can be
  left out with `lsp.semantic_highlighting_off`.

## 2. Where Fenix stands (1.5)

- **Highlighting.** `App::syntax_highlights_in_columns` (`app.rs`)
  returns `(byte range, colour)` for the rows being drawn. It drains the
  buffer's edits into tree-sitter, asks the grammar for captures in the
  visible byte windows, and maps each capture name to one of the theme's
  eleven `syntax_*` colours (`Theme::syntax_color`). Two limits keep it
  bounded:
  - `MAX_HIGHLIGHT_CHARS` (64 Ki chars) on the visible span;
  - per line, `MAX_ROW_CHARS` (4096) columns on very long lines. In a
    wrapped pane this is the columns its rows show.
  The drawers (`row_content_spans` for wrapped panes, the
  line-per-row path otherwise) cut lines by these ranges with
  `split_line_by_highlights`. So anything that ends up in that `Vec` is
  drawn in wrapped and unwrapped panes alike, by rows.
- **LSP.** One `LspSession` per server (`app.rs`). It holds:
  - the capabilities;
  - `open_documents` (edit count and version per path);
  - `synced_text`, the text last sent;
  - `pending`, for routing answers;
  - `pulls`, for pulled diagnostics. Pulled diagnostics are the model
    to copy: one ask out at a time per document, a second marked due,
    and `workspace/diagnostic/refresh` asks again.
  `sync_lsp_document` sends `didOpen`/`didChange`. It works out one
  replaced range per change by comparing the text last sent with the
  text now (`fenix_lsp::change_event`), whatever sync kind the server
  asked for. Several servers can serve one buffer
  (`cached_lsp_keys`).
- **Positions** are UTF-16 (`fenix_lsp::position`). Fenix doesn't
  advertise `general.positionEncodings`, so every server uses UTF-16.
- **Capabilities** are fixed at connect time
  (`crate::lsp::client_capabilities`), with no dynamic registration.

## 3. Design

### Requests: full, delta, range

- **`full`** when a document opens and after each pause in typing. This
  is the baseline: every server that offers tokens answers it.
- **`full/delta`** in place of `full` when the server offers it and an
  earlier answer gave a `resultId`. A delta's edits apply to the raw
  `u32` array of the answer it's relative to, so that array is kept
  (5 × tokens × 4 bytes; 400 KB for 20 000 tokens). On a typical
  keystroke the answer is a handful of numbers instead of the whole
  file. If a delta can't be applied (indices out of range), Fenix asks
  `full` again.
- **`range`** only for a large document (more than 10 000 lines, or
  more than 1 MiB). It asks for the lines shown in each pane on that
  document, with 100 lines of margin, when the view moves past the
  margin or after an edit. It is used when the server offers it;
  otherwise `full` is used up to 50 000 lines, and the document gets
  none above that. Range answers aren't kept beyond what they cover: a
  new range answer replaces the last.
- One ask out per server and document at a time. An ask made while one
  is out is marked due and sent when the answer lands, as pulled
  diagnostics already do.
- `workspace/semanticTokens/refresh` asks again for every open document
  of that server. It's already accepted with a null reply by
  `server_request_reply`.
- An answer is decoded against the text it was asked for. If the
  document was edited after the ask, its edit count no longer matches,
  so the answer is thrown away and the ask is made again. The old,
  shifted tokens stay drawn meanwhile. A delta's `resultId` is only
  taken from an answer that was kept.
- `ContentModified` (-32801) and `ServerCancelled` (-32802) mean "ask
  again later". They're treated like a thrown-away answer.

### Debounce and what's drawn meanwhile

- The ask after an edit waits until typing has paused for
  `SEMANTIC_PAUSE` (250 ms), measured from the last `didChange` sent.
  The frame loop checks what's due, and a wake is set for the deadline,
  so an idle editor still asks.
- Tokens are kept as char offsets (`start`, `end`, type, modifiers)
  against the text they describe, stamped with its edit count.
- Every `didChange` `sync_lsp_document` sends is one replaced range
  `[start, old_end) → new_end`, in chars. Tokens are moved with it:
  - those before `start` stay;
  - those after `old_end` shift by `new_end − old_end`;
  - those touching the range are dropped (the grammar's colour shows
    there until the answer).
  The stamp then moves to the new edit count. This is the same range
  that's sent, so the two can't drift apart. It's done for every
  server whatever sync kind it asked for: the range is worked out
  anyway.
- Tokens are drawn only when their stamp equals the buffer's edit count.
  A buffer edited by something that didn't go through
  `sync_lsp_document` yet (another pane's buffer, an agent) shows
  tree-sitter alone until the next sync, rather than tokens in the
  wrong place.

### Legend → theme

- Each server's legend (token types and modifiers by index) is read
  once from its capabilities. Types index into a table built once per
  session that maps each index to a `Style`:
  - a theme colour slot;
  - or "none", which leaves the grammar's colour.
- Standard types and their slots. A new slot falls back to the slot
  after the arrow when the theme doesn't name it:

  | Token type | Slot |
  |---|---|
  | `namespace`, `type`, `class`, `enum`, `struct`, `typeParameter` | type |
  | `interface` | **interface** → type |
  | `parameter` | **parameter** → variable |
  | `variable`, `property`, `event`, `label` | variable |
  | `enumMember` | constant |
  | `function`, `method` | function |
  | `macro` | **macro** → function |
  | `keyword`, `modifier` | keyword |
  | `comment` | comment |
  | `string`, `regexp` | string |
  | `number` | number |
  | `operator` | operator |
  | `decorator` | attribute |

- Common non-standard types are mapped too:
  - rust-analyzer's `builtinType`, `selfType` and `typeAlias` to type, `trait` to interface;
  - `selfKeyword` and `lifetime` to keyword;
  - `boolean` to constant;
  - `attribute` and `derive` to attribute;
  - `formatSpecifier` and `escapeSequence` to string.
  Any other type (`unresolvedReference`, `generic`, ...) maps to "none".
- **Modifiers:**
  - `mutable` (rust-analyzer) draws a thin underline under the token, in
    the token's colour, as VS Code does;
  - `readonly` or `constant` on a `variable` or `property` takes the
    constant slot;
  - `deprecated` takes the comment slot;
  - every other modifier is ignored.
- The new theme keys are optional, under `[syntax]`: `interface`,
  `parameter` and `macro`. The built-in themes get values chosen from
  their own palettes, so the distinctions the issue asks for show out of
  the box. A user theme without them still loads, and falls back as
  above.

### Layering over tree-sitter

- `syntax_highlights_in_columns` produces the grammar's ranges as now.
  When the buffer has drawable tokens, those inside the same byte
  windows are converted to byte ranges and laid over them:
  - the grammar's ranges are cut where a token starts and ends;
  - the token's colour wins inside it;
  - the grammar's colour stays outside.
- A token mapped to "none" isn't laid at all.
- Tokens never colour inside a comment or string the grammar found.
  Servers sometimes send a token for a doc comment's code. Laying it
  would split a comment into colours the grammar already got right, so
  the grammar wins there.
- TODO captures and conflict markers are untouched: conflict markers
  return before syntax runs, and TODO spans are taken from the grammar's
  captures.
- Because the result is the same `Vec` of byte ranges, both drawers
  cut by row columns as they do now. That covers wrapped panes, tabs and
  the long-line column windows.

### Servers, buffers, sizes

- **Servers that don't offer it:** none of the buffer's servers has
  `semanticTokensProvider` (or it offers neither `full` nor `range`).
  Nothing is asked, and the buffer stays tree-sitter only.
- **Multi-server buffers:** the first of the buffer's servers, in
  `cached_lsp_keys` order, that offers tokens is the one asked. Tokens
  from two servers are not merged: two legends' opinions on the same
  word would fight.
- **Large files and long lines:**
  - Decoding happens once per answer, not per frame. Each frame only
    looks up the tokens inside the visible windows (a binary search on
    `start`) and converts those to bytes.
  - A line beyond `MAX_ROW_CHARS` only gets tokens in the columns shown,
    since the windows are those columns already.
  - Range requests (above) keep a huge file from producing a huge answer.
- **Position encoding:** UTF-16 stays. A token's line and UTF-16 start
  and length are turned into char offsets through the text the ask was
  made against (`synced_text`), one line at a time while decoding, so
  a non-BMP character before a token on its line doesn't shift it.
  Advertising `utf-8` too would save that walk, but it adds a second
  path everywhere else positions cross. Not worth it for this alone.

### Settings

| Setting | Default | What it does |
|---|---|---|
| `lsp.semantic_highlighting` | on | Ask servers for semantic tokens and draw them. Off: the capability isn't advertised, and nothing is asked or drawn. A change applies to servers started after it (`SPC q r` or reopening). |
| `lsp.semantic_highlighting_off` | none | Languages whose files are never asked, by the same names `editor.wrap_languages` uses. Applies at once. |

**Why on:** with the fallbacks, a token in a theme that doesn't name the
new slots takes the colour the grammar would most likely have given it.
Where they differ, it's because the server knows better (a call the
grammar took for a variable, a type it took for a function). The
setting is there for anyone who prefers the grammar alone. The
per-language list covers a server whose tokens are poor.

### Revised after review

These supersede the table and rules above where they differ.

- **Plain names keep the grammar's colour.** `variable`, `property`,
  `namespace` and `lifetime` map to none, so the grammar's colour stays.
  In themes where the variable colour isn't the text colour, colouring
  every local and field would change most of a file. A path like
  `std::collections` isn't a type. A modifier still counts:
  - a constant is a `variable` or `property` with rust-analyzer's
    `constant` modifier (its `const` and `static` items), or with
    `readonly` together with `static` (a `static final`, a C# `const`,
    a TypeScript `static readonly`). `readonly` alone (every TypeScript
    `const` local) keeps the grammar's colour;
  - deprecated makes a comment;
  - mutable underlines the name in its grammar colour.
- **Layers.** Each type says how it's laid.
  - `Over`, for what only the server knows (types, parameters,
    functions, macros, enum members and the like): over the grammar,
    but never inside its comments and strings.
  - `Fill`, for what the grammar knows as well (keywords, strings,
    numbers, operators): only where the grammar coloured nothing, as in
    a language with a server but no grammar.
  - `Wins`, for `comment`: over everything. clangd marks an inactive
    `#if` region with comment tokens, and that region greys whole,
    literals and all.
- **Enum members take the constant colour.** That's intended: the
  grammar's `constructor` guess gave them the attribute colour.
- **Numbers are Fill**, so the grammar's colour for a literal stays.
- **Lines in `decode`** are ropey's, measured with
  `position::line_content_char_len`. That's the same pair every position
  sent with `didChange` uses: LF, CR and CRLF, plus VT, FF, NEL, LS and
  PS with ropey's default features. So the two can't disagree.
- **Overlap.** Tokens out of order are sorted, and of two that overlap,
  the first is kept.
- **Answers stay raw.** They're read as plain numbers
  (`semantic::Answer`, `read_answer`). A delta's edits apply to that
  array at any index, and only the result must be whole tokens.
- **`provider` copies no legend**, and `legend` borrows it.
- **`range: {}`** (allowed by the spec, refused by lsp-types) is
  rewritten to `true` before an `initialize` result is read
  (`normalize_initialize_result`).

### What's not done

- Partial results (`partialResultToken`): answers are taken whole.
- Dynamic registration of the provider (Fenix has none for anything).
- Token modifiers beyond the four above: bold and italic would need
  font variants per span, which the text drawer doesn't have.

## 4. Stages

1. **Plan and decoding.** This document, and `fenix_lsp::semantic`
   (pure, tested):
   - reading the provider from capabilities (`full`, `delta`, `range`);
   - the legend → `Style` table;
   - decoding relative tokens into char-offset tokens against a text;
   - applying a delta's edits to the raw array;
   - shifting tokens through one replaced range;
   - finding the tokens inside a char window.
   No GUI change.
2. **Asking and drawing (`full`).**
   - The settings, and the client capability advertised when on.
   - Session state per document; asks on open, after the pause and on
     refresh; answers decoded and stamped.
   - Shifting on each `didChange`.
   - Layering in `syntax_highlights_in_columns`; the multi-server choice;
     clearing on close and disconnect.
3. **Theme.**
   - The `interface`, `parameter` and `macro` keys, with values in the
     built-in themes.
   - The modifier rules, including the `mutable` underline in both
     drawers (in a wrapped pane, an underline split at a row break goes
     on each row).
   - Settings page and docs.
4. **Delta and range, and hardening.**
   - `full/delta` with the kept array.
   - `range` for large documents, re-asked as the view moves.
   - A measurement on a large file, like soft wrap's.
   - CHANGELOG, FEATURES and "As built".

## 5. As built

### Stage 1

- `fenix_lsp::semantic`, as planned. Two additions to the type table:
  `typeAlias` maps to type, and `trait` to interface.
- `fenix_lsp::changed_span` is factored out of the ranged `didChange`,
  with the same result.

### Stage 2

- **Settings.** `lsp.semantic_highlighting` (on) and
  `lsp.semantic_highlighting_off` (none).
  - `crate::lsp::client_capabilities(semantic)` advertises the
    capability, and refresh support, only when the setting is on.
  - Language names in the list are matched without regard to case.
- **Where the code is.** `app/semantic.rs`:
  - `SemanticDoc` per server and document (`LspSession::semantic`),
    holding the tokens, their stamp, an ask that's out and when the next
    ask is due;
  - `after_sync`, called by `sync_lsp_document` after each `didOpen`
    or `didChange`;
  - `ask_due_semantic_tokens`, run each frame after the LSP sync;
  - `semantic_wake`, which wakes the loop when the next ask is due;
  - `semantic_tokens_answered`;
  - `refresh_semantic_tokens`;
  - `with_semantic_tokens`, the last step of
    `syntax_highlights_in_columns`.
- **Which server is asked.** `semantic_server` picks the first of the
  path's servers whose capabilities offer `full`. `sync_lsp_for` passes
  each server whether it's that one:
  - the chosen server's document gets an ask due;
  - any other server's tokens for it are let go.
  A document already in sync is picked up, or let go, when the choice
  changes. That happens on the next sync, which runs when a server
  becomes ready or the buffer is edited.
- **Moving with edits.** On `didChange`, the tokens move through
  `changed_span(old synced text, new text)`. That's one more char walk
  of the text, on top of the one the ranged change already does. It
  runs only when the chosen server holds tokens with the old stamp;
  otherwise the stamp is cleared, so nothing is drawn until the answer.
- **Answers.**
  - An answer for an edit count the server has moved past is thrown
    away, and the ask is made due again.
  - `ContentModified` and `ServerCancelled` are asked again after the
    pause.
  - Any other error is left until the next edit.
  - `null` means no tokens.
- **Drawing.**
  - The grammar's comments and strings (captures `comment`, `string`
    and `spell`) are protected.
  - `lay_over` cuts the grammar's ranges by the tokens, keeping the
    order and non-overlap that `split_line_by_highlights` and the tab
    remap need.
  - Paths are canonicalized once per buffer path (`App::lsp_canonical`),
    not each frame.
  - Nothing is looked up while no server holds any tokens.
- **Changed test.** `fenix-config`'s
  `every_setting_round_trips_through_the_file` asserted that no `[lsp]`
  header is ever written. `[lsp]` now has settings of its own, so the
  empty-header check covers `[vnc]` only.
- **Revised after review.** These supersede the stage-2 notes above
  where they differ.
  - **Only buffers ask.** `sync_lsp_for` takes the buffer shown, which
    is none for an agent's look at a file. For a buffer it resolves the
    tokens' server and path once and keeps them in `App::semantic_of`,
    so drawing touches nothing on disk and copies no legend.
    `lsp_canonical` is gone. An agent's sync only moves tokens already
    held (`SemanticSync::Follow`).
  - **Tokens belong to a buffer.** `SemanticDoc::buffer` records which
    one, and tokens are drawn only for that buffer at the stamped edit
    count. A document whose buffer closed is dropped, along with its
    `semantic_of` entry, on the next frame or wake.
  - **A reopened buffer is synced.** Before, it started its edit count
    at 0 again, and the "already in sync" check let a file changed on
    disk go unsent: the server, diagnostics included, kept the old
    text. That was a sync bug from before this feature.
    `sync_lsp_document` now sends a `didChange` whenever the text
    differs from what was last sent, not only when the edit count
    moved. A document shown for the first time, or in a new buffer, is
    asked at once. Fenix still sends no `didClose`; that's unchanged.
  - **Asks go out from the wake.** `about_to_wait` calls
    `ask_due_semantic_tokens` (so an ask isn't held for an unrelated
    frame), and `semantic_wake` returns the due time itself.
  - **Timeout.** An ask unanswered for 10 s is cancelled with
    `$/cancelRequest` and asked once more; a second timeout waits for
    an edit. RequestCancelled (-32800) is retried, like the other two.
  - **Refresh** marks a server's documents stale. Each is asked when
    it's next drawn, not all at once.
  - **Nothing is spent on a buffer with no tokens to draw.**
    `semantic_to_draw` runs before any window copy or comment-and-string
    list is built.
  - **No grammar.** A buffer with no grammar gets the server's tokens
    over nothing.
  - **The language checked** is `buffer_language_name`, the same one
    `editor.wrap_languages` uses.
  - **Redraw.** An answer redraws every window.
  - **`lay_over` follows each token's layer.**
- **Second review.**
  - An agent's sync that moves held tokens also makes an ask due.
  - A refresh that comes while an ask is out stays pending until an ask
    is sent after it.
  - An answer counts only if its request is the document's current ask;
    any other is ignored.
  - Refusals (cancelled, modified) back off from 250 ms, doubling each
    time up to 4 s.
  - Documents are dropped when their buffer closes, is renamed or saved
    under another name, or is asked of another server, and all of them
    when the setting goes off.
  - A buffer with no grammar works out its windows only when it has
    tokens to lay.
  - **One writer.** Every writer of `synced_text` and `open_documents`
    goes through `record_lsp_sync`, which moves held tokens through the
    change sent. There were two: `sync_lsp_document` and a refactor's
    own sync (`sync_refactor_buffers`), which used to skip it.
- **Not yet:** delta and range (stage 4).

### Stage 3

- **Theme keys.** `[syntax]` takes optional `interface`, `parameter`
  and `macro` keys (`Theme::syntax_interface`, `syntax_parameter`,
  `syntax_macro`). Left out, they're type, variable and function.
- **Built-in themes.** Every built-in theme names all three.
  - Each must differ from variable, constant, type, function, keyword,
    string, comment and the text colour, and from the other two. "Differ"
    means a distance of at least 0.15 in RGB (0 to 1 per channel), not
    only a different hex value.
  - Each must keep at least 4:1 contrast with the background.
  - Each is the first that qualifies of the theme's own attribute,
    number and operator colours, then its other syntax, mode, Git and
    explorer colours. Where none qualifies, it's the fallback colour
    blended toward one of those.
  - A test checks that rule as stated: the 0.15 distance from each
    other and from the eight colours above, and 4:1 contrast. The lowest
    contrast is 4.07:1 (everforest-dark); the smallest distance is 0.152
    (catppuccin-frappe's interface against its parameter).
- **The `mutable` underline.** `App::underlined_tokens` gives the
  mutable tokens on the lines shown. `polish_pane` turns them into row
  segments (`RowMap::segments`, so one split by a row break is
  underlined on each row). The segments are drawn as underlines,
  through the path extensions' decorations already use, in the token's
  colour at 0.8 alpha. None is drawn inside the grammar's comments and
  strings, where a token's colour isn't laid either. Those ranges are
  `App::semantic_protected`, kept only from a pane's own highlight pass
  (`viewport`), not sticky headers or one-off lookups, and dropped with
  the buffer's tokens.
- **Stage 2's tests changed.** A macro token is now drawn in the macro
  colour, not the function colour.

### Stage 4

- **Delta.** `full/delta` is asked once a whole-document answer gave a
  `resultId`. The answer's raw array and id (`SemanticDoc::data`,
  `result_id`) are kept as the server's last answer even when its text
  has since moved on; only decoding waits for text that matches.
  - A delta that doesn't apply (edits off a token boundary, past the
    end, or overlapping) clears the id, and the whole document is asked
    again.
  - An error on a delta does the same after the pause.
  - A delta answered with whole tokens is taken as a whole answer.
- **Range.** A document is asked by range when its server offers range
  and either it has more than 10 000 lines or 1 MiB, or the server
  offers no full. `with_semantic_tokens` notes the lines its windows
  show (`note_semantic_view`). When they aren't all covered, the next
  ask is for them with 100 lines of margin, made due at once. A range
  answer replaces the tokens and records the lines it covers. One
  refused with an error counts as covered, so it isn't asked again
  every frame. Edits don't move `covered`; the ask after the pause uses
  the last lines in view.
- **Revised after the third review.** These supersede the range notes
  above where they differ.
  - **Several answered ranges.** A document keeps up to 8 ranges of
    answered lines (`covered`), joined where they touch. When there are
    more, those no view reaches go first. A range's answer is merged
    into the held tokens (`fenix_lsp::semantic::merge`), replacing only
    those in its lines. So two panes on one file, or a view and sticky
    scroll's header, don't take turns evicting each other.
  - **Only a pane's own view asks.** `note_semantic_view` runs from the
    frame for each pane, with the lines it shows. Sticky scroll's
    headers and any line looked up on its own use what's held. Views are
    kept per pane and dropped when the pane no longer shows the buffer,
    in any frame.
  - **One line count.** Range or full is decided from a
    `fenix_lsp::semantic::Extent` (ropey's lines, the bytes, the end
    position) worked out once per sync, the same for asks and views.
    Nothing scans the text per frame.
  - **Tried and failed.** A range that's refused, doesn't parse or times
    out counts as covered until the next edit or refresh. An edit or a
    refresh clears `covered`, so the lines shown are asked again.
  - **The document's end.** A range ask ends at the document's last
    position, not a line past it.
- **No range and over 50 000 lines:** not asked, and no wake is set.
- **Wake.** `semantic_wake` gives the next due time, or now for one
  already due (a draw just made it due). `ask` clears `due` on every
  path that doesn't send, so a due that can't be sent doesn't keep
  waking the loop.
- **Measured** (`measure_decoding_a_million_lines` in `fenix-lsp`,
  ignored; run with `--release -- --ignored --nocapture`), on 1M lines
  with 5M tokens:
  - decode: 340 ms (158 ms before decoding went through ropey's lines,
    so its lines match the positions `didChange` sends);
  - moving all tokens through one edit: 86 ms;
  - 1000 window lookups: 1.8 ms.
  The whole-document path stops at 50 000 lines (about 250 000 tokens),
  so an answer there decodes in about 17 ms and an edit moves tokens in
  about 4 ms. A larger document is
  asked by range and holds only what's in view. Moving tokens is still
  O(tokens) per edit; a tree or gap structure would fix that if it
  shows up.
- **Not done:**
  - partial results;
  - the theme colours picked by hand rather than by rule.
