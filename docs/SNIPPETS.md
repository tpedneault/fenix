# Native snippets

In Insert mode, type an exact trigger and press **Tab** (or **Ctrl-J**).
Fenix ships `header` and `section` in every text language and `proc` in Tcl.
An exact snippet wins over the completion popup unless you explicitly select another suggestion. With no matching snippet,
Tab continues to accept completion or insert indentation as before.

The first field is highlighted. Typing replaces its default, then appends to
your new value; Backspace removes the last character. Tab/Ctrl-J selects the
next numbered field; Ctrl-K selects the previous one. Revisiting a field
selects its whole value for replacement. `$0` is the final cursor position;
without it, the cursor finishes at the end. Escape ends the session and
returns to Normal mode on the same keypress. Arrow keys or other editor
commands end the session and perform their usual action. There is one active
snippet at a time; nested expansion is not supported.

## Browse and complete

Press **SPC i s** in Normal mode to browse snippets available for the current
file. Type to search names, triggers, and body previews; use Up/Down to choose,
Enter to insert at the cursor, or Escape to cancel. Insertion enters Insert mode
and highlights the first field. Language-specific and user overrides appear once.

Snippets also appear in completion alongside LSP results, symbols, keywords, and
buffer words. Each row identifies its source. The selected entry shows a short
detail/body preview when space permits. Up/Down or Ctrl-N/Ctrl-P select;
PageUp/PageDown move a page; Tab/Enter or Ctrl-Y accept; Ctrl-E dismisses.
Ctrl-Space opens completion manually. Escape returns to Normal mode.

LSP plain-text insertion and replacement edits, including accompanying imports,
are applied together as one undoable change. Full LSP snippet syntax and
completion-item resolve requests are not supported.

## Your snippet files

Create a `snippets` directory beside Fenix's `config.ini`:

- Windows: `%APPDATA%\fenix\snippets`
- Linux: `~/.config/fenix/snippets` (or your platform's configured config directory)
- macOS: `~/Library/Application Support/fenix/snippets`

Each UTF-8 `.snippet` file contains one snippet. Files are read on expansion,
so saving an edit makes it available on the next trigger without a restart.
Fenix never writes these files. Copy the [bundled examples](../crates/fenix-snippets/examples)
as starting points. For example, `function.snippet`:

```text
# name: Rust function
# key: fn
# scope: rust
# --
fn ${1:name}(${2:args}) -> ${3:()} {
    ${4:todo!()}
}
$0
```

`# key:` and the `# --` separator are required. `# name:` is optional.
`# scope:` is a comma-separated list, defaulting to `*` (all text buffers).
Scope names are Fenix's detected languages in lowercase: `rust`, `toml`,
`markdown`, `json`, `yaml`, `python`, `javascript`, `typescript`, `tsx`, `c`,
`cpp`, `bash`, `tcl`, `dockerfile`, `batch`; `text` covers unnamed or unrecognized
files. The editor's existing extension/filename detection selects the scope.

Triggers cannot contain whitespace and match immediately before the cursor,
at the start of a line or after a non-word character. Language-specific entries
win over global ones, then the longest trigger wins. For equal scope and trigger,
user files override bundled snippets; later files in filename sort order win.
Invalid files are skipped and the first error appears in the modeline, including
its path. Valid files still work. Individual files are limited to 1 MiB.

The body is literal, including its final newline. CRLF files are normalized to
LF. Continuation lines inherit the current line's leading spaces or tabs;
indentation written in the snippet is added to that base. Expand `header` at
the top of a file when you want a file header. Bundled headers use `//` in Rust,
C, C++, JavaScript, TypeScript and TSX, and `#` elsewhere. Override them with a
language-specific file when you need another comment style.

## Fields, mirrors, and transforms

| Syntax | Meaning |
| --- | --- |
| `${1:default text}` | Editable field with a literal default |
| `$1` or `${1}` | Empty field, or a live mirror of field 1 |
| `$0` | Final cursor position, visited after all numbered fields |
| `${1\|upper}` | Computed mirror; edit the untransformed field to change it |
| `${FILENAME}` | Dynamic variable captured at expansion |
| `\$`, `\}`, `\\` | Literal dollar, closing brace, backslash |

Fields are visited in numeric order, regardless of position. The first
untransformed occurrence is editable; every other occurrence mirrors its value.
A default can appear after an earlier mirror, but each field has at most one
default. Defaults are literal: nested placeholders and variable interpolation
inside defaults are rejected. Literal pipes in defaults need no escaping.
Unbraced nonnumeric forms such as Tcl's `$name` remain literal.

Pipelines are evaluated left to right on each edit:

| Transform | Effect |
| --- | --- |
| `trim` | Remove leading and trailing Unicode whitespace |
| `remove_whitespace` | Remove all Unicode whitespace, including newlines |
| `upper`, `lower` | Unicode case conversion |
| `center:N` | Center each line to N characters using spaces (1–4096) |

For example, the bundled `section` snippet contains:

```text
# ${1:Section title}
# ========================================================================
# ${1|remove_whitespace|center:72}
# ========================================================================
$0
```

Type `Build steps` into the first comment line. The framed line updates to
`Buildsteps`, centered within 72 characters. Whitespace removal applies to the
source title before centering adds padding. The editable title stays visible
in the first comment line. Use `trim` instead when internal spaces should stay.
Centering does not truncate overlong text; odd padding puts the extra space
on the right. Width counts Unicode scalar values, not font pixels or terminal
display cells; wide CJK characters, combining marks and tabs can look uneven.

## Header variables

| Variable | Value |
| --- | --- |
| `DATE` | Local date, `YYYY-MM-DD` |
| `TIME` | Local time, `HH:MM:SS` |
| `YEAR` | Four-digit local year |
| `DATETIME` | Local RFC 3339 timestamp with UTC offset |
| `USER_NAME` | `FENIX_SNIPPET_USER`, then `USERNAME`, then `USER` |
| `FILENAME` | File name including extension |
| `FILE_STEM` | File name without extension |
| `FILEPATH` | Buffer's file path |
| `DIRECTORY` | Parent directory of that path |

Missing file/user values become empty strings. Variables are captured once
per expansion; time does not change while editing a field. Variables also
accept pipelines, for example `${FILE_STEM|upper}`. Unknown variables,
transforms, duplicate `$0` stops, and malformed placeholders are errors.
Snippets are data only: there is no shell, Lisp, JavaScript, or arbitrary code
evaluation. This is a native Fenix format inspired by yasnippet, not an import
implementation of every yasnippet or LSP snippet feature.

## Design and editing boundaries

`fenix-snippets` handles loading, parsing, rendering and sessions independently
of the GUI. It uses the same character offsets and `Buffer::replace_range`
as the editor. Expansion is one undo step. Each field keystroke updates all
mirrors in one atomic undo step; undo/redo works on ordinary buffer history.
The current implementation rerenders the snippet region per field edit, which
keeps mirrors and offsets consistent for the intended small templates.

The GUI adapter runs before Insert-mode completion, draws field selections
through the existing overlay, and checks buffer identity, pane identity, edit
revision and cursor position before every snippet key. Unexpected buffer edits
(including undo, reload and external replacements) invalidate the session.
Movement and other commands finish field editing, leaving expanded text intact.
Editing within a field with arrows, clipboard commands, choices, regex
substitution and nested snippets are outside this first version; normal editor
commands remain available after leaving the session.

Verification: `cargo test -p fenix-snippets` exercises the engine and loader;
`cargo test -p fenix-gui app::snippets` exercises the actual GUI adapter with
headless editor state, including completion priority and field highlighting.
