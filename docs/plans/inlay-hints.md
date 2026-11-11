# Plan: inlay hints

- **Status:** built, for the release after 1.5.2.
- **Scope:** `textDocument/inlayHint`: a variable's type after its name,
  a parameter's name before an argument, drawn dimmed in the line.
- **On by default** (`lsp.inlay_hints`), unlike code lens: hints are
  decorations in the line, not rows of their own, so a pane with them
  keeps easing and sticky scroll.

## Design

- **Drawn as decorations.** Fenix already lays out text that takes room
  in a line without being in it: an extension's inlays and the
  debugger's values (`crate::decorations`, `Look::Text`). Hints are one
  more set of those, owned by `fenix.inlay_hints`. So, with nothing new
  in the drawing code:
  - the caret, extra carets, Visual selections, search matches and
    problems are placed through the line's column map, past a hint;
  - soft wrap breaks the drawn line, hints included;
  - the mouse lands where the text is drawn;
  - nothing of a hint is in the buffer: not saved, yanked or searched.
- **Moving between answers.** Decorations follow the buffer's edits
  (`Buffer::track_anchors`). A type hint goes along with text typed at
  its place (`after: false`), so it stays after the name; a parameter
  hint stays put (`after: true`), so what's typed goes into the
  argument after it.
- **Which server.** The first of the buffer's servers whose
  `inlayHintProvider` is on (`fenix_lsp::inlay::offers`). Only a
  buffer's own sync asks, never an agent's look at a file.
- **What's asked.** The lines every pane shows of the buffer (noted as
  they're drawn, `note_inlay_view`), 60 lines above and below. Asked:
  - when the buffer is first seen with a server that has its text;
  - 300 ms after typing pauses;
  - at once when a view moves past the lines answered;
  - on `workspace/inlayHint/refresh`.
  One ask out per buffer. An answer for text since edited is let go
  and asked again. ContentModified, ServerCancelled and RequestCancelled
  are retried, backing off to 4 s, five times. A 10 s timeout sends
  `$/cancelRequest` and asks once more.
- **Settings and keys.** `lsp.inlay_hints` (on), `lsp.inlay_hints_off`
  (languages left out), `SPC t i` (`view.toggle_inlay_hints`, saved).
  The client capability and its refresh are advertised only when on, as
  for semantic tokens and lenses.

## Not done

- `inlayHint/resolve`, a hint's tooltip, its label parts' locations and
  commands, and its `textEdits` (accepting a hint into the text).
- Hints from more than one server on a file.
- Remote projects, which have no language servers yet.

## As built

- `fenix_lsp::inlay` (pure, tested): the provider, the capabilities, a
  label (parts joined, control characters made spaces, cut at 48 with
  `…`, padding), placing an answer by UTF-16 position, the request for a
  range of lines.
- `app/inlay_hints.rs`: `InlayDoc` per buffer; `inlay_targets` from
  `sync_lsp_for`, `ask_due_inlay_hints` each frame beside the lenses',
  `inlay_wake` for the loop, `inlay_hints_answered`, `refresh_inlay_hints`
  and `toggle_inlay_hints`. Tested against a fake session: asked only
  once a view is known, drawn where the server says, moved by typing,
  refreshed, a stale answer let go, and gone when turned off globally or
  for the language.
