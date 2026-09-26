# Features

Everything Fenix does, by area. For the keys, see
[Keybindings](KEYBINDINGS.md); for settings, see
[Configuration](CONFIGURATION.md).

## Editing

- **Modal editing**: Normal/Insert/Visual (char/line/block)/Replace/Command
  modes, the standard motion set (`h j k l w b e 0 ^ $ gg G f F t T ; , % { }`),
  operators (`d c y` composing with motions and text objects, plus the
  doubled `dd`/`cc`/`yy` forms), `iw`/`aw` text objects, numeric counts
  (`3dw`, `2dd`, ...), yank/paste mirrored onto the OS clipboard (the
  unnamed register only -- `y`/`d`/`c` push to it, `p`/`P` pull from it
  first, so copying in Fenix and pasting elsewhere -- or vice versa --
  just works; in Visual mode `p` replaces the selection with it, leaving
  the replaced text on the clipboard as Vim does, and `P` replaces it
  while keeping the clipboard as it was), named registers (`"a`-`"z`/`"A`-`"Z` to select one for the
  next `y`/`d`/`c`/`x`/`s`/`p`/`P`, uppercase appends), undo/redo, search
  (`/`, `?`, `n`, `N`, `*`, `#`) with a live incsearch preview and
  persistent match highlighting while it's active, `:s` substitute with
  backreferences, indentation (`>>`/`<<`, auto-indent, `:set
  shiftwidth=N`). `'iskeyword'` is real Vim's own option, not a
  bespoke one: `w`/`e`/`b` and `iw`/`aw` treat `_` as part of a word by
  default (`testing_variables` is one word, real Vim's own factory
  default), same as `*`/`#`, which always search the whole
  underscore-inclusive identifier regardless of this setting (see the
  caveat below); `:set iskeyword-=_` narrows that to the snake_case-
  aware navigation some other editors (Doom Emacs's evil-mode among
  them) ship as their own default instead, so `e` on `testing_variables`
  stops at the end of `testing` -- `:set iskeyword+=X`/`iskeyword=X,Y`
  add to or replace the set outright. Persisted across restarts the
  same way `shiftwidth` already is.
- **Macros** (`q{a-zA-Z}` to record, a second bare `q` to stop, `@
  {register}` to replay, `@@` to repeat whichever was last played,
  `3@a`-style count prefix): real Vim's own model, not a separate
  storage -- a macro is just a named register's text (Vim's own keycode
  notation, `<Esc>`, `<C-r>`, ...), so `"ap` pastes a recorded macro's
  literal keystrokes as text and yanking text into a register makes it
  `@`-executable. Recording captures every keystroke, including
  `SPC`-leader sequences and prompts, not just what reaches Vim's own
  motion/operator dispatch. A self-referential macro is bailed out by a
  depth/key-count guard rather than actually hanging, unlike real Vim.
- **Jumplist** (`Ctrl-O`/`Ctrl-I`): back/forward through recent cursor
  positions, recorded on `gg`/`G`/`%`/a confirmed search/`*`/`#` and on
  jumping to a symbol definition, a grep match, a quickfix entry, or a
  mark -- so jumping to a Tcl definition (`SPC m s`), a search hit
  (`SPC p s`), or a mark (`` `a ``) and hitting `Ctrl-O` takes you right
  back, even across files. A single global back/forward pair of stacks
  (like a browser's history), not real Vim's per-window circular list --
  a disclosed simplification.
- **Marks** (`m{a-zA-Z}` to set, `` `{mark} ``/`'{mark}` to jump --
  exact position vs. the mark's line's first non-blank, real Vim's own
  split): named position bookmarks that work across files, unlike real
  Vim's lowercase-is-buffer-local/uppercase-is-global distinction, which
  Fenix doesn't replicate -- every mark can jump to wherever it was set,
  any file. Not composable with operators (no `` d`a `` / `d'a`) --
  jump-only.
- **`SPC`-leader menu** with a live which-key popup showing available
  continuations as you type a sequence -- reachable from Visual mode as
  well as Normal, so e.g. `SPC c f` (indent the selection) can act on an
  active selection without leaving it first.
- **TODO comments**: `TODO`, `FIXME`/`FIX`/`BUG`, `HACK`, `WARN`/`XXX`,
  `PERF`/`OPTIMIZE`, `NOTE`/`INFO` and `TEST` are highlighted inside real
  comments (tree-sitter decides what a comment is, so a `"TODO"` string
  literal never counts), each family in its own color with a soft tint
  behind it; an `(owner)` and trailing `:` are part of the marker
  (`TODO(tom):`). `TODO`, `FIXME`, `FIXIT`, `HACK`, `XXX` and `NOTE` count
  as the first word of a comment line; the rest need the colon, since
  `// INFO is logged` is prose. `]t`/`[t` step to the next/previous one
  (count-aware), `SPC s t` lists the buffer's, and `SPC s T` lists the
  whole project's -- `rg` narrows the project to files that mention a
  keyword, then each is parsed so only comments are listed (plain-text
  and Markdown files are read line by line). The project list also
  becomes the quickfix list, so `SPC p n`/`SPC p N` walk it.
- **XML**: highlighting for `.xml` and the formats built on it (`.svg`,
  `.xsd`, `.xsl`/`.xslt`, MSBuild `.csproj`/`.props`/`.targets`, `.resx`,
  `.xaml`, `.plist`, `.wxs`, `.nuspec`, feeds, and more), plus `.dtd`.
  Typing `>` to finish a start tag inserts its end tag after the cursor
  and typing `</` completes the innermost open element; `%` on a tag
  jumps to its partner (bracket matching as usual everywhere else);
  `it`/`at` select a tag's contents or the whole element (real Vim's
  text objects, textual, so they work in any file); `gcc` wraps lines in
  `<!-- -->` (Markdown too); multi-line elements fold (`SPC c z`), appear
  in the breadcrumbs (`SPC c b`); `SPC c o` is an element outline
  labeled with each element's `id`/`name`/`key`; `SPC c F`/`SPC c f`
  reindent by element depth; `SPC c v` checks well-formedness and jumps
  to the first error (saving a malformed XML file also says so); `SPC c
  y` copies the element's XPath (`/project/dependencies/dependency[2]`).
- **Syntax highlighting** via tree-sitter for Rust, TOML, Markdown, JSON,
  YAML, Python, JavaScript/TypeScript/TSX, C, C++, Bash, Tcl, Dockerfile/
  Containerfile, Batch (`.bat`/`.cmd`), XML, and DTD. Docker Compose files already
  get full highlighting for free via the existing YAML support -- no
  separate grammar needed. `Dockerfile`/`Containerfile` are detected by
  filename (they conventionally have no extension), including per-stage
  names like `Dockerfile.prod`. In Tcl, a bare word in command position
  is only colored as a command if it's actually known -- a built-in, a
  ctags-scanned project definition, or a symbols-file entry (the same
  three sources autocompletion draws from), matched against its
  fully-qualified path with an optional leading `::` -- not just any
  word that happens to be first on a line, and including the procs
  defined in the file you're looking at (which is the whole of what's
  known for a lone script with no project for `ctags` to scan). The word
  right after an ensemble command's own name -- `dict keys`, `string
  compare`, `info class methods` -- is colored the same way, restricted
  to the ensembles that are actually invoked as `command subcommand
  ...` (`after`'s one real exception, `after ms ?script?`, is excluded
  so a plain delay is never mistaken for one of its three real
  subcommands), including the handful that nest a second ensemble one
  level down (`string is alnum`, `binary encode hex`, `trace add
  variable`). An ordinary argument in the same position -- `dict set
  d k v`'s `k`/`v`, `info exists x`'s `x` -- is left alone. Markdown gets a real second
  pass beyond its own block structure (headings, lists, code fences):
  tree-sitter-md ships two grammars, and the block one only ever marks
  a span of prose with a bare `(inline)` node rather than parsing
  what's actually inside it -- bold, italic, inline code spans, and
  links are that second, inline grammar's own job, run as a genuine
  (if lightweight) language injection over each such span. Each reads
  as a distinctly colored token -- via color alone, not also a
  different font weight: making `**bold**` render in an actual bold
  typeface would mean threading a style flag through every function
  between a syntax capture and the glyph it becomes, several of them
  shared by every other language's own highlighting, for a purely
  cosmetic gain on one language -- more risk than the result was worth.
- **Markdown list continuation**: pressing Enter or `o` on a `-`/`*`/`+`
  bullet or a `1.`/`1)` ordered item continues it onto the next line at
  the same indent -- an ordered marker increments by one (not renumbered
  through the rest of the list; CommonMark renderers only look at a
  list's first number anyway, so a "wrong" one past that point is
  cosmetic in the source and invisible once rendered), and a `- [ ]`/
  `- [x]` task item continues *unchecked* regardless of whether the one
  you just finished was checked. Enter on an empty item (nothing typed
  after its marker yet) leaves the list instead of repeating the marker
  forever. No per-file-type gate -- a list shaped like a list continues
  wherever it appears, the same "the text shape alone is the signal"
  posture bracket-depth reindenting (`SPC c f`/`SPC c F`) already has.
  `O` (open line *above*) doesn't get this -- it would need renumbering
  every ordered item from there down to stay correct, real complexity
  for a much rarer motion than Enter/`o`. `SPC c x` toggles a GFM task
  checkbox (`- [ ]`/`- [x]`/`- [X]`) on the current line. `SPC c o` is a
  fuzzy picker over every heading in a Markdown buffer, indented by
  nesting depth -- confirming jumps straight to it, the same shape
  `SPC s s` (fuzzy-find any line) already has, just filtered to
  headings. Gated to a detected Markdown buffer specifically, unlike
  list continuation/checkbox toggling: `#` means "comment," not
  "heading," in half the languages this editor highlights.

## Files and projects

- **File explorer** (dired-style): `SPC f j` opens a real, Vim-navigable
  buffer (splittable, closable with `SPC b k`, listed in `SPC b b`) with
  the whole feature set -- marks, batch create/rename/copy/move/delete,
  git-status badges, subtree expansion, sorting by name/size/date/type;
  ordinary motions (`j k gg G /`) work for free since it's real text,
  and the cursor *is* the selection, so operations always act on the row
  you are looking at. A persistent sidebar (`SPC e t`) shows the same
  listing in a strip, reading the same key table, so a binding can never
  mean two different things in the two forms.

  `SPC e d` arranges the classic two-listings-side-by-side layout out of
  the window system rather than as a panel of its own, so both halves
  are ordinary buffers that split, close and switch like any other --
  and `C`/`M` seed their destination with the directory the other half
  is showing, which is the whole reason to arrange two listings.

  Reading a directory happens **off the main thread**, for all three
  forms of the listing -- the pane, the sidebar and the directory
  picker. A slow path --
  a network share, a disk waking up -- leaves the editor completely
  usable, and the pane keeps showing where you are until the new listing
  arrives; after a moment the header names what it is waiting on and
  `Esc` stops waiting and leaves you where you were. (Nothing tries to
  cancel the read itself: a blocked `read_dir` on a share that has gone
  away sits in the kernel until SMB gives up, minutes later, and no
  amount of asking shortens that. Its result simply arrives stale and is
  dropped.) Git badges follow as a separate pass, so they never hold up
  the listing, and are skipped for `\\server\share` paths where running
  `git status` across the wire would cost more than it is worth.

  **Getting somewhere** does not mean walking there. `SPC e p` is a path
  bar: type it, with `Tab` completing a directory at a time, `~` and
  `%APPDATA%` expanded, quotes stripped off anything pasted out of
  Explorer, and forward slashes accepted. Typing a *file* opens it
  rather than refusing. Typing `\server` with no share asks the server
  what it has and offers the answer as a list -- off the main thread,
  because a host that is not there takes tens of seconds to say so.
  `SPC e b` is everywhere else worth going in one list: bookmarks first,
  then the machine's drives with free space (a mapped drive shows the
  share it really points at, since `Z:` on its own tells you nothing),
  then the directories you have actually been to, then registered
  projects. `SPC e m` bookmarks where you are, named after the folder,
  with no prompt -- a bookmark you can add without stopping to think is
  one you will actually add. Bookmarks are kept with the rest of what
  Fenix remembers about this machine, in `state\bookmarks.json`.

  **Renaming in bulk** is the thing an editor can do that a file manager
  cannot. `SPC e w` re-renders the listing as one bare name per line and
  makes it editable, so `:%s/`, visual block, macros and counts become
  bulk-rename tools nobody had to build; `SPC e W` works out what the
  edit asks for and shows a real example before doing any of it. A name
  may contain `/`, so it reorganises as well as renames. Line position
  is identity -- line N is entry N -- which is why adding or removing a
  line is refused rather than guessed at: a deleted line is not a
  deleted file. Two names given the same value, or a name landing on an
  untouched file, are refused up front, so a rejected edit changes
  nothing and stays on screen to fix. Swapping two names works, because
  when a rename set has a cycle in it everything is moved aside first
  and then into place -- and if any part of that fails, all of it is put
  back, since a bulk rename is one edit and half of one is a directory
  nobody asked for.

  **Copying and moving run in the background**, with a live count of
  files and bytes in the modeline and `SPC e k` to stop. Cancelling
  stops between files -- a file already being written has to finish,
  since there is no way to abandon a copy part way through without
  leaving a truncated one behind -- and what has already been copied
  stays, because that is what actually happened. `z` packs the marked
  set into a `.zip` (or `.tar.gz`, by extension) and `x` unpacks the
  archive at point into a folder of its own; both go through the
  `bsdtar` Windows ships, named by absolute path, because the `tar` on
  `PATH` is often GNU tar and GNU tar answers a request for a `.zip`
  with an uncompressed tar under that name. `i` shows what a column
  cannot hold -- every timestamp, the attributes, where a link points --
  and on a folder counts what is inside it, which is the one number a
  listing cannot show without walking the whole tree. `w` flips
  read-only, the attribute that stops an ordinary save.

  **The last mile** is the set of things that make trusting all of this
  easy. `SPC e o` opens the entry with whatever the system associates
  with it, because the answer to a `.xlsx` is Excel and not a hex dump.
  `SPC e O` shows it in Explorer, selected -- being unable to leave is
  not the same as not needing to. `SPC e y` copies the full path (the
  path, not the name: a path is what you paste into a terminal or
  another program's open dialog). `SPC e T` opens a shell *here*, which
  is what the pane-terminal work made possible -- without a working
  directory the first thing anybody types is a `cd`. `SPC e g` searches
  this directory once, leaving the next unqualified search meaning the
  project again, and `SPC e G` makes this the project and opens the Git
  panel on it, so `SPC p f` and `SPC s p` follow rather than one panel
  pointing somewhere the rest of the editor is not.

  Links and junctions are shown as links (`name/@`) and coloured
  differently from real directories, because following one into a tree
  you did not expect to be in is exactly the surprise worth preventing;
  `i` says where one points.

  Deleting means the **Recycle Bin**, so a mistake is recoverable
  through Windows' own restore. Copying or moving onto something that
  already exists asks first -- overwrite, skip, or keep both -- rather
  than silently destroying it, and skipping a *move* leaves the source
  where it was.
  `SPC f e` starts that same fuller explorer at your home directory
  instead of the current file's -- for a file that isn't in any project
  and isn't worth typing an absolute path for (something in
  `~/Downloads`, say): navigate down with the usual `j k l h`/`Enter`
  and open it directly, or press `S` once you're close to fuzzy-search
  every file under wherever you've navigated to, recursively (the same
  candidate list `SPC p f` builds, just rooted there instead of the
  project).
- **Files menu** (`SPC f ...`): `SPC f f` opens a file by typing its
  path (`~` expands to home, a relative path resolves against the
  project root) -- unlike every fuzzy finder here, it doesn't enumerate
  anything, so a `.gitignore`'d file (`.env`, say) opens exactly like
  any other, and a path that doesn't exist yet opens an empty buffer to
  save later. `SPC f a` is `SPC p f`'s fuzzy-find sibling but *including*
  gitignored files, for when you want to search by name rather than type
  the exact path. `SPC f r` fuzzy-finds a recently-opened file. `SPC f
  R`/`SPC f D`/`SPC f y` rename/delete (with confirmation)/copy-the-path
  of the file currently open.
- **Project tooling**: fuzzy find-file (`SPC p f`), project-wide search via
  ripgrep (`SPC p s`) with the result list kept around afterward as a
  quickfix list -- `SPC p n`/`SPC p N` step to the next/previous match
  directly in the editor without reopening the picker or re-running the
  search, clamping (not wrapping) at either end.
- **Project hub** (`SPC p p`): every known project with its kind tag,
  branch, uncommitted changes and a health dot, pinned ones first, then
  by group. Typing filters (a folder path typed in adds it), `Tab`
  filters by kind, and the right column previews the selected project's
  health, git state, linked Jira work and tasks. `Enter` opens it, `P`
  pins, `g` sets its group, `d d` removes it from the list (the folder
  is untouched), `c` creates a new one, `h` and `,` open its doctor and
  settings. Opening a project gives it a workspace of its own and
  returns there next time -- `[windows] workspace_per_project = false`
  turns that off. `SPC p P` is the old path-only quick switcher.
- **New projects** (`SPC p c`, or Home's *new project* row): a four-step
  wizard -- template, location (typed, or `b` to pick the folder in the
  explorer), the template's own questions, then a review of every file it
  will write and every command it will run (literal arguments, no shell)
  before anything touches the disk. Built-in templates:
  - *Python* (uv): an app or library; data processing (pandas or polars,
    matplotlib or plotly, optional notebooks); a desktop GUI (PySide6 or
    tkinter); a command-line tool (typer, click or argparse); a FastAPI
    web API (optional Dockerfile).
  - *Systems*: a Rust crate (binary -- optionally a clap CLI -- or
    library); a Cargo workspace; a C or C++ CMake project -- executable,
    static or shared library -- with presets, `.clangd` and GoogleTest,
    Catch2 or plain CTest.
  - *Web*: a Vite app (React, Vue, Svelte, Preact, Solid, Lit or vanilla,
    all TypeScript).
  - *Embedded*: an Arduino sketch, or an Arduino library with an example.
  - *Mission*: an SCOS-2000 MIB -- only the chosen `.dat` tables,
    registered as a `[mib]` root.
  - *Scripting*: a Tcl package with a tcltest suite.
  - *Start from*: a monorepo (optionally a Cargo, uv and/or npm
    workspace at its root) or an empty folder.

  Each writes a `.fenix/tools.json` with its build/run/test tasks, and
  offers `git init` with a first commit -- off by default when the
  location is already inside a repository. A template is a folder --
  `template.toml` plus a `files/` tree -- so your own go in
  `<config dir>/fenix/templates/`, and a repository can offer its own in
  `.fenix/templates/`. Files can hold `{{#if key}}`/`{{else}}`/`{{/if}}`
  line blocks; a `when` (on a file, a command, a task or a question) is
  `key`, `!key`, `key == value`, `key != value` or a list of them that
  must all hold; `after = true` writes a file once the commands have run
  (for scaffolders like `npm create` that want an empty folder). A failed
  step stops the run and keeps what's done: `r` retries it, `s` skips
  it, `o` opens what's there.
  `:project-new [template] [name] [key=value ...]` skips the pages --
  `:project-new python-uv orbit python=3.13 dev=pytest,ruff` lands on the
  review.
- **Monorepos and nested projects**: a project inside another
  repository is its own project -- its git summary and the doctor are
  scoped to its folder ("part of the X repository"). Tasks and the
  modeline use the project; the language server starts at the enclosing
  Cargo/uv/npm/pnpm/Go workspace, so navigation crosses crates. The hub
  lists a project's subprojects (up to four levels down, skipping build
  and dependency folders) indented beneath it, and opening one doesn't
  add it to the list. A project created inside a Cargo or uv workspace
  joins it.
- **Project doctor** (`SPC p h`): every check the project's kind needs --
  uv and whether the venv matches `uv.lock`, the language server (found
  even when installed off PATH), debugpy; `arduino-cli`, the board's
  core and whether its port is connected; a MIB's tables, over-long rows
  and whether it's registered -- each with its fix. `f` runs the fix
  under the cursor, `F` every fix that's safe to run unasked (syncing a
  venv, `git init`; never an install), `Enter` opens a check's file, `y`
  copies a report.
- **Project settings** (`SPC p ,`): `.fenix/tools.json` as rows -- tasks,
  language servers, the debug launch and its environment -- plus the
  project's kind, group, pin and Jira key. Commands are typed as one line
  (quotes group, backslashes stay literal); every edit is validated
  before it's written, `t` runs a task, `e` opens the raw JSON.
- **Project identity**: every project has a kind (Python, Arduino, MIB,
  Rust, Tcl, ...), detected from its files or declared as `[project]
  kind = "..."` in `.fenix/settings.toml`. Its two- or three-letter tag
  leads the modeline (`PY orbit-tools · decoder.py`) and Home's project
  rows (with the doctor's health dot), and the window is titled after
  the project.
- **Files changing on disk**: every open file is checked against what's
  actually on disk a couple of times a second, and whenever the window
  regains focus. Both are needed -- focus catches editing in another
  application, and the timer catches Fenix's *own* terminal panel, which
  never takes focus away from the window at all. A clean buffer is
  re-read in place, keeping every pane on the line it was on. A buffer
  with unsaved edits is never touched: it's flagged instead, shown as
  `[disk]` in the modeline, and `:w` on it **refuses** rather than
  overwriting -- `:w!` keeps yours, `:e!` takes theirs. That refusal is
  the point of the whole feature. A plain write is a whole-file
  overwrite, so without it, running `git checkout` or a formatter in the
  terminal two splits away and then pressing `:w` silently discards it,
  with nothing to recover from.

  A deleted file never blanks its buffer -- a "safe write" in another
  editor removes and recreates the file, and an editor that empties your
  buffer for that moment is worse than one that says nothing. A file
  rewritten with identical content (a build, a formatter that changed
  nothing) says nothing either. The periodic check is a `stat` per open
  file, comparing modification time and length; the save guard reads the
  file outright, because that check can miss a same-length edit made
  inside the filesystem's timestamp granularity and a write is the one
  moment where being approximately right costs somebody their work.
  `[editor] watch_files = false` turns the whole thing off, for a
  working copy on a network share.
- **Safe saves**: documents, settings, and recovery snapshots are staged beside
  their destination, explicitly flushed and synced, then replaced. Windows
  replacement preserves destination ACLs and refuses sharing violations; failed
  saves leave buffers dirty. `:w <path>` names and saves an unnamed buffer
  without overwriting an existing file (spaces in the path are preserved).
- **Crash recovery**: every editable buffer with unsaved changes, including unnamed scratch work, is written to a
  snapshot a couple of times a second, so a crash, a power cut or a
  killed process costs at most a moment's typing instead of everything
  since the last `:w`. The snapshot is deleted the instant the buffer is
  saved or deliberately closed, so a normal session leaves nothing
  behind and only an abnormal exit leaves anything to find. On the next
  start, if anything survived, the modeline says which files and that
  `SPC f v` recovers them; recovering loads the text back as an
  *unsaved* edit, so `:w` accepts it and `:e!` throws it away -- the
  same pair of answers as any other two versions that disagree.
  Recovery continues when file watching is disabled. A failed snapshot displays
  `[recovery failed]` until recovery succeeds or the affected work is saved or
  discarded. Unnamed snapshots recover into unnamed, unsaved buffers.
  Snapshots older than two weeks are cleaned up on startup.

  Deliberately **not** Vim's swap files. Those live next to the file
  being edited, which means `.gitignore` entries and build tools
  tripping over them, and they carry a locking protocol for "another
  instance has this file open" that Fenix has no use for -- their
  famous failure mode, a stale `.swp` prompting about a file nobody is
  editing, is worse than the loss they prevent. Snapshots here live in
  one directory under Fenix's own config location, keyed by a hash of
  the file's path, and never touch the working tree. Nor is there a
  modal "recover?" prompt on launch: it says what's there and gets out
  of the way.
- **Search and replace** (`SPC s ...`): `SPC s s` fuzzy-finds any line in
  the current buffer (live-filtered as you type). `SPC s r` prompts for a
  pattern then a replacement and shows the match count before applying --
  a real UI over the same regex engine `:s` already uses, scoped to the
  current Visual selection's lines if invoked from Visual mode (mirroring
  real Vim's own `:'<,'>s`), the whole buffer otherwise. `SPC s p` does
  the same across the whole project: searches with ripgrep (respecting
  `.gitignore` by default, so build/generated files never show up),
  groups matches by file, and opens a real, navigable review buffer --
  toggle files out with `Space`, apply with `a`/`Enter` behind a y/n
  confirmation. An already-open file is edited in memory (left dirty, for
  you to review/save); anything else is edited on disk directly. One
  fresh regex pass per included file, not a snapshot from search time --
  a file that changed since the search is safely skipped rather than
  misapplied.

## Windows and the UI

- **Windows, buffers, workspaces**: splits (`SPC w v`/`SPC w s`) with each
  pane keeping its own independent cursor and scroll position, directional
  navigation, a buffer switcher (`SPC b b`), and Doom-Emacs-style
  workspaces (`SPC TAB`) -- name one (`SPC TAB r`), jump straight to it
  by name instead of only cycling (`SPC TAB TAB`), or define a shelf of
  them in `SPC ,` (Documents & workspaces) and open-or-switch-to one on
  demand (`SPC TAB f`). Every pane shows a small title bar naming its
  buffer (the filename, or a placeholder like `*dashboard*`/`*docker*`/
  a dired buffer's own directory for one with no path) -- with a split
  open, two different files are labeled at a glance, not just whichever
  one happens to be focused (the modeline only ever names that one).
  One editor can also drive several OS windows at once (`SPC w n`) --
  one per monitor, say. They are one process sharing one buffer list, one
  undo history and one PDF worker, so the same file can be open in both
  and edits show up in each; only the split layout is per window.
  Inside the `SPC w` group, lowercase acts on a split and uppercase on the
  whole OS window. `fenix --new-window <file>` adds one from a shortcut or
  the shell instead of handing the file to the window already open.
  Directional pane navigation ignores the boundary between them: walking
  off the left edge of the window on one monitor continues into the window
  on the monitor beside it, landing on whichever of its panes is actually
  adjacent (its rightmost, coming from that side -- not its first). It is
  one rule over one list of rectangles in desktop coordinates, so panes in
  the current window win simply by being nearer, and an open sidebar is
  reached before the next monitor for the same reason.
  The windows share one font database and one glyph atlas, so opening a
  second one doesn't re-scan your system fonts, and each scales text by
  its own monitor's DPI factor -- `font_size` is a logical size, so the
  same setting looks the same on a 100% screen and a 150% one.
  The focused pane's title is colored with an accent so it's obvious at
  a glance which one has focus, whether you're editing, or inside the
  Docker or Git panel. On the Docker and Git panels specifically, every
  title is also prefixed with a number (`1. Containers`, `2. Images`,
  ...) -- pressing that digit jumps focus straight to the matching pane.
- **Modeline**: mode badge, filename, and cursor position on the left;
  a live local date/time clock flush against the right edge, ticking in
  place as you work (omitted rather than overlapping anything if the
  window's too narrow to fit it).
- **Home** (the start-up dashboard; `gh` or `SPC o d` to go back to it):
  every workspace has one, pinned as the first tab of every pane under
  the workspace's name, and it never closes. A workspace opened from the
  project hub gets a Home of that project -- its recent files and TODOs
  only, with the project's name next to the date. The Fenix logo and
  the date, a find field (`SPC SPC`), then three columns --
  the file to resume and recent files; known projects with their git
  branch; today's agenda tasks (the one on the clock first, with its
  running time) and the project's TODO comments -- over a recovery
  notice and a key strip. `j`/`k` move within a column, `h`/`l` across,
  `Enter` opens, `1`-`9` open a numbered project or file directly. The
  columns reflow to two, then one, as the window narrows, and every
  colour comes from the active theme (the brand orange only marks a
  running timer).
- **Icon and logo**: drawn from one vector definition in `fenix-brand`
  (the "Forged F" mark and the Martian Mono wordmark, kept as outlines
  so no font ships). The build script generates the `.ico` embedded in
  every Windows `fenix.exe`, the window/taskbar icon is rendered from
  the same geometry at runtime, and the committed `fenix.ico` and
  `docs/brand/*.png` are regenerated with `cargo run -p fenix-brand
  --example write-icon` -- a test fails if they drift.

## Docker

- **Docker panel** (Lazydocker-style): `SPC d d` opens a real, six-pane
  workspace -- Containers/Images/Volumes/Networks on the left (each its
  own real, Vim-navigable buffer with a title bar), Status and Logs
  stacked on the right. Status live-updates to whatever's under the
  cursor in the focused left pane, including a selected container's
  CPU/MEM (which also ticks on its own every ~2s without a keypress);
  Logs is a dedicated pane for streamed log output. Each Containers row
  is just `[X] name`, prefixed with a one-letter, color-coded status
  badge (`R` running, `P` paused, `X` exited, etc.) instead of inline
  text that used to clip at small font sizes. `s`/`S`/`R` start/stop/
  restart the container under the cursor, `r` runs a new container from
  the image under the cursor, `d` removes the container/image/network
  under the cursor (with a `y`/`n` confirmation), `u` refreshes. `l`
  switches Logs into a live tail of that container's logs (`docker logs
  -f`), streaming new lines in and auto-scrolling to the bottom while
  you're already there -- scroll up to read earlier output and it leaves
  you alone until you navigate back to the end. Per-pane keybinding
  hints no longer clip inline either -- press `x` on a Containers/Images/
  Volumes/Networks pane for a Lazydocker-style contextual popup listing
  that pane's available keys; it's purely informational and dismisses on
  the very next keypress, which still does whatever it would normally
  do. A long Status value (a long bind-mount path, say) word-wraps onto
  indented continuation lines rather than running off the pane's right
  edge; the Containers/Images/Volumes/Networks list rows themselves
  never wrap, so they stay aligned. `SPC d b` builds an image from the
  current project root's `Dockerfile`; `SPC d q` closes the whole
  session.

## Git, GitHub and GitLab

- **Git status page** (`SPC g g`): one page for the repository the
  focused file is in. The header says where the branch stands -- its
  upstream (ahead/behind, when it was last fetched, or "not pushed yet")
  and its base branch (`[git] base_branch`, else `main`/`master`,
  preferring `origin`'s copy) -- and a suspended rebase or merge leads
  the page with its keys. Below: Conflicted, Untracked, Unstaged,
  Staged, Stashes, Unpulled, Unpushed and Recent commits, each folding
  with `Tab`. `Tab` on a file opens its diff inline, under its name;
  `s`/`S` stage and unstage a file, a hunk, or -- with `V` over some
  diff lines -- just those lines, and `d` discards (asking first; an
  untracked file is named as deleted). `Enter` opens the file at the
  line. Every verb with more than one form is a menu that opens under
  the row you're on: `c` commit (commit, amend, extend, reword, fixup,
  fixup-and-squash; flags `-a` all tracked, `-s` sign-off, `-n` skip
  hooks), `P` push (a new branch's first push sets its upstream; a push
  that would replace commits on the remote shows exactly which, and
  offers `--force-with-lease`), `p` pull (rebase, merge, fetch), `b`
  branch (switch, create, rename, merge in, rebase onto, delete branches
  whose upstream is gone), `z` stash (everything, with untracked, staged
  only, this file), `l` log, `r` rebase (continue/skip/abort while one
  is suspended). `Enter` on a commit is its own menu: fix it up with
  what's staged (folded in at once by an autosquash rebase), reword,
  revert, branch or tag there, reset to it. Operations run off the UI
  thread, one at a time; the page says what's running and how it ended,
  and `$` shows the full output. It refreshes itself every couple of
  seconds while it's visible.
  **Every operation is logged, and `U` takes the last one back** --
  after showing what it will do ("feature goes back to d42f6e1 ... brings
  back 3 commits and the uncommitted changes saved before it"). Before
  anything that throws work away runs (a discard, a hard reset, a
  dropped stash), Fenix saves that work as git objects first, so it
  can come back. Undoing a commit leaves its changes staged; undoing an
  undo redoes it. The log lives in the repository's own git dir
  (`.git/fenix/oplog`, never committed); `SPC g z` opens the page on
  its Operations section, where `U` on any entry undoes that one. A
  push can't be taken back, and says so.
- **Git in the file you're editing**: `]h`/`[h` move between the
  changed hunks the gutter marks, `SPC g a` stages the one under the
  cursor, `SPC g d` discards it (asking first; `U` brings it back), and
  `SPC g i` shows it in a popup. `SPC g B` puts blame beside the text --
  commit, author and age on the first line of each run from one commit,
  coloured by how recent the change is -- read from the buffer as it
  stands, so unsaved lines read "not committed yet" and it follows your
  edits; `SPC g e` shows the full commit behind the cursor's line.
  `SPC g w` switches branch, the one you left most recently first (from
  the reflog), with ahead/behind and age; a remote-only branch becomes a
  local one tracking it. When local changes are in the way it offers to
  stash them, and they come back when you switch back to that branch.
  The modeline carries the focused file's branch, how far it is ahead
  and behind its upstream, and how many files have changes
  (`feature/x ↑2 •3`) -- and while a rebase or merge is stopped, says so
  loudly, with its keys, in every window. `[git] auto_fetch = 5m` fetches
  in the background when the last fetch is older than that, so those
  numbers are true.
- **Git log** (`SPC g l`): history that acts. The current branch, or
  every branch as a graph (`a`); `SPC g h` is the focused file's history
  (following renames) and `SPC g H` the history of the lines last
  selected in Visual mode (`git log -L`), each commit with the patch that
  changed them. `Tab` opens a commit to its files and a file to its diff,
  inline; `/` filters by words in the message or `author:name`. `Enter`
  on a commit is its menu -- fix it up with what's staged, reword the
  last one, revert, cherry-pick one from another branch, check it out,
  branch or tag there, reset to it -- each logged, so `U` on the Git
  page takes it back. The graph view with its refs tree is `SPC g G`.
- **Interactive rebase** (`r i` on the Git page, from the upstream or
  base; `i` on a commit in the Log page, from that commit): the commits
  it would replay, newest first, with one key per verb -- `p` pick, `r`
  reword (the message is written in the compose buffer), `e` edit, `s`
  squash, `f` fixup, `d` drop -- and `J`/`K` to move one. `fixup!` and
  `squash!` commits start out next to the commit they name. Below the
  list, the branch as it will be: which commits survive, what folds into
  them, what's dropped, and a warning when some are already pushed.
  Nothing runs until `C-c C-c`; Fenix writes the todo list itself and
  git does the rest, and a stop for `edit` or a conflict hands over to
  the Git page's banner (`r c` / `r a`). The whole rebase is one entry
  in the operation log -- `U` takes it back.
- **Worktrees** (`w` on the Git page): `w a` checks a branch out --
  existing or new -- in a folder beside the repository
  (`fenix.hotfix` next to `fenix`) and opens it as a workspace of its
  own, so looking at another branch never means stashing your work.
  When there's more than one, the Git page lists them; `Enter` opens
  one, `w d` removes one (refused while it has uncommitted changes), `w
  p` forgets ones whose folder is gone. Switching to a branch that's
  checked out in another worktree opens that worktree instead of
  failing. `[git] layout = panes` keeps the older
  panel below.
- **Git panel** (Lazygit-style; `[git] layout = panes`): a seven-pane
  workspace -- Status/Staged/Unstaged/Branches/Commits/Stash stacked on
  the left (each its own real, Vim-navigable buffer with a title bar),
  Main on the right showing a diff of whatever's under the cursor in
  Staged/Unstaged/Commits/Stash. Main is a real diff *viewer*, not
  colored text: every row carries the file, hunk and old/new line
  numbers it came from, shown in two dim line-number columns beside the
  verbatim patch line. That's what makes hunk-level work possible --
  `s`/`S` stage/unstage just the hunk under the cursor (a patch built
  from that one hunk and piped through `git apply`, so the rest of the
  file stays exactly as it was), `d` discards it (with confirmation),
  `]`/`[` jump between hunks, `Tab` folds a file down to its header, and
  `Enter` opens the real file at the line under the cursor. Staged and Unstaged are two independent
  views of the same file list (a file that's both staged *and* further
  modified appears in both, since git tracks the two halves separately)
  -- `s`/`S` stage/unstage the file under the cursor from either pane,
  `a`/`A` stage/unstage everything, `c` commits (opening a compose
  buffer for the message -- a subject, a blank line and a body, which is
  the convention and which a one-line prompt cannot express),
  `d` on Unstaged discards the file under the cursor (`y`/`n` confirm,
  handling untracked files correctly via `git clean` rather than `git
  checkout`), `z` stashes every change, `P`/`p` push/pull. Status is a
  fixed repo-overview summary (branch, upstream, ahead/behind, staged/
  unstaged/untracked counts) that live-updates on its own every ~2s via
  a background poller, independent of cursor movement -- the inverse of
  the Docker panel's Status/Logs split, same pattern, roles swapped to
  match what's actually true of each domain. A long Status value word-
  wraps onto indented continuation lines, same as the Docker panel's own
  Status pane. On Branches: `c` checks out
  the branch under the cursor, `n` creates a new one (prompts for a
  name), `d` deletes it (confirm). On Stash: `a` applies the entry under
  the cursor, `g` pops it, `d` drops it (confirm). `u` refreshes the
  whole session from any pane; `x` on Staged/Unstaged/Branches/Commits/
  Stash shows a contextual popup of that pane's keys, same dismiss-on-
  next-keypress convention as Docker's. Real lazygit's own `<space>`
  stage-toggle isn't used here -- `SPC` is already Fenix's global
  leader-key trigger -- so Staged/Unstaged use separate `s`/`S` keys
  instead, matching the Docker panel's own `s`/`S`/`R` precedent.
  Main's diff fetch runs off the input thread (a background thread posts
  the result back when it lands, discarding a slow one a faster later
  selection already superseded) so scrolling through many files never
  blocks the UI waiting on a `git` subprocess. `SPC g q` closes the
  whole session.
- **History / commit graph** (`SPC g G`): a real commit DAG across
  *every* branch (`git log --all`), drawn the way `git log --graph`
  draws it -- two columns per lane, with a connector row below a merge
  (`|\`) and above the commit that branches converge on (`|/`), so the
  shape of the history is legible instead of crammed onto one line per
  commit. Each row carries its short hash (padded to a common width, as
  git abbreviates hashes to differing lengths), the refs pointing at it
  (`(HEAD -> develop)`, `(origin/main)`, tags), and its subject; moving
  down the graph shows that commit's author, date, full message and
  diff in the shared diff viewer -- with its files folded when it
  touched more than one, so a wide-ranging commit reads as a scannable
  list of what it touched and `Tab` opens whichever file you want to
  look at. `1`/`2`/`3` jump straight to the Graph, Refs and Commit
  panes, and `x` lists the keys the focused pane understands. Rails are ASCII by default because the
  box-drawing alternatives are missing from many monospace fonts, and
  the fallback font's different character width knocks every row out of
  alignment -- `[git] graph_style = unicode` opts in if yours has them. Beside it, a refs tree of Local / Remotes / Tags,
  where every local branch is badged with how it stands against its
  upstream -- `[=]` in sync, `[^2]` ahead, `[v3]` behind, `[^2 v3]`
  diverged, `[gone]` when the remote branch was deleted, `[--]` when
  there's no upstream at all -- led by how long ago you last fetched,
  since every one of those badges is only as current as that. `SPC g f`
  fetches (`--all --prune`, so deleted remote branches actually
  disappear and `[gone]` becomes true); `u` refreshes; `SPC g q` closes.
- **Compare refs** (`SPC g c`): pick any two refs -- branches, remote
  branches or tags -- and see what one adds over the other: the commits
  between them beside the full diff, hunk-navigable like every other
  diff here. Defaults to three-dot (`base...head`, measured from the
  merge base: "what does this branch actually do", the same thing a
  merge request shows), with `t` toggling two-dot (`base..head`, every
  difference between the two trees, including what the base gained
  meanwhile). `r` re-targets without closing, `u` refreshes, `SPC g q`
  closes. The base picker leads with the base branch from `SPC ,` (a
  project can set its own), falling back to whichever of `main`/`master` the repo
  actually has, so "how does this differ from the mainline" is two keys
  and an Enter; each step says which side it's asking for, and the
  second echoes the base you already chose (`master...?`).
- **Rebase, merge and conflicts**: `SPC g r` rebases the current branch
  onto a ref you pick, `SPC g m` merges one in, `SPC g p` pulls with
  `--rebase`, and a push from the Git page's push menu with `-f` is
  always `--force-with-lease`, never a bare `--force`, so a rebase-and-push
  can't silently discard what someone else pushed in between. When one
  of those stops, the Status pane leads with a banner naming what's
  suspended and how far it got (`REBASING 3/7 -- SPC g x c continue,
  SPC g x a abort`), and lists the conflicted files as their own section;
  `SPC g x c` and `SPC g x a` finish or undo whichever operation it is --
  rebase, merge, cherry-pick or revert -- so there's one pair of keys to
  remember rather than one per operation. In a conflicted file, `SPC g x j`
  and `SPC g x k` walk the markers and `SPC g x o`/`t`/`b` keep
  ours, theirs, or both for the conflict under the cursor, markers
  removed. Anything that rewrites the working tree also re-reads the
  files you already have open, so a buffer never keeps showing what a
  file said before the rebase -- saving that stale text would have
  written it straight back over git's conflict markers. A file with
  unsaved edits is left alone and counted in a message instead: your own
  work always outranks the refresh.
- **Resolving conflicts** (`SPC g x x`): the conflicted files on the
  left, the selected one shown as **two aligned columns** on the right
  -- your side and the incoming side on the same row, with shared text
  spanning the full width so it still reads as one file. `o` keeps the
  left, `t` the right, `b` both, `n`/`p` walk the conflicts, `s` stages
  the file once nothing is left (and refuses while markers remain, so
  they can't be committed), and `u` puts the conflict back if a choice
  went the wrong way. On the file list, `o`/`t` take a whole file from
  one side and stage it in one step.

  The reason this is a view and not just the raw markers: **both
  columns are labelled with the branch they actually came from**.
  During a rebase git replays your commits onto the target, so at every
  step `HEAD` -- what git calls "ours" -- is the branch you're rebasing
  *onto*, and "theirs" is your own work. Reading `<<<<<<< HEAD` as "my
  version" and keeping it deletes the commit you were rebasing. So
  nothing here says "ours" or "theirs": the columns, the keys, the
  status banner and the message after each choice all name the branch
  (`kept develop`, `o = keep myfeature`) and spell out its role. Opening
  a conflicted file directly still works, and its markers are colored
  with the same two colors the columns use.
- **Pull requests, on GitHub and GitLab**. Which forge a repository is
  on is read from its `origin` remote: `github.com` is GitHub, anything
  else the GitLab of `[gitlab] base_url`. GitHub signs in through the
  GitHub CLI (`gh auth login`) when it's installed, else `[github]
  token`; nothing is configured per repository.
  - **Opening one** (`SPC g P`, or `o` on the Git page): a page
    prefilled from the branch -- the title from its one commit or its
    name as a sentence, the description from the project's template --
    GitLab's default description template (Settings > Merge requests),
    else the repository's own (`.gitlab/merge_request_templates/`, GitHub's
    `pull_request_template.md` or `.github/PULL_REQUEST_TEMPLATE/`; with
    several, `Default` first and `h`/`l` on Template to switch) or else a
    summary of its commits, the base from `[git] base_branch`, you as the
    assignee, a Jira key in the branch name (`feature/FNX-58-...`) linked
    as `Refs FNX-58`, and the reviewers from `[git] reviewers` (or the
    project's own `.fenix/settings.toml`).
    `Enter` edits a field in place, `e` writes the description in a
    buffer, `d` toggles draft. Under the form are the commits it brings
    and what's worth knowing first: whether it's pushed, whether it
    merges into the base without conflicts, how far the base moved, and
    `fixup!` commits not folded in yet. `C-c C-c` opens it -- pushing the
    branch first when it isn't, and never asking you to review your own.
    From then on the Git page's header says how it stands: draft,
    checks, who approved, how many threads are still open.
  - **The inbox** (`SPC g M`): what's waiting on your review first, then
    yours and every open one (`Tab`), with a dot on the ones that moved
    since you last looked. `Enter` reviews one here, `w` in a worktree
    of its own beside the repository, so your own work stays where it
    is.
  - **The review page**: every changed file, with the ones you've
    marked viewed folded away until they change again. Comments are
    held as **pending** until you submit the review -- approve, request
    changes or just comment -- in one go, with a summary; a `suggestion`
    block proposes the exact lines. Replies and resolving a thread go
    straight away. `i` shows only what changed since your last review.
    The checks are listed with how long each took; `Enter` on a failed
    one opens its log and puts every `file:line` in it in the quickfix
    list, and `r` runs it again. `m` merges -- squashed, rebased, or
    once the checks pass.

  Tested against real forges: `cargo test -p fenix-github --test live
  -- --ignored` on a GitHub repository of your own (`FENIX_GITHUB_SANDBOX`)
  and the containerised GitLab below, and `cargo test -p fenix-gui
  live_ -- --ignored` drives the pages end to end on both.
- **GitLab merge requests, the older panel** (`SPC g M` with `[git]
  layout = panes`): the project's open merge
  requests on the left, the selected one in full on the right --
  author, source -> target, state, pipeline result, approvals, comment
  count, description, and the list of changed files. `f` cycles the
  filter (all open / mine / assigned to me), `Enter` shows one, `u`
  refreshes, and `c` checks one out locally: fetched from GitLab's own
  published `refs/merge-requests/N/head` on the project's remote, so a
  request opened from a fork needs no extra remote, and landing on
  `mr-42` rather than the source branch's name, which may not exist
  locally or may mean something else entirely. Each row's badge is
  colored by what would stop it merging -- failing CI or conflicts read
  as bad without reading a word of the titles. The two panes split the
  window evenly, and both wrap to their own real width -- long titles,
  paths, URLs and descriptions fold rather than running off the edge,
  with every line of a wrapped row still answering the action keys.
  Diffs and the merge view's two columns are deliberately never
  wrapped: a wrapped diff line no longer lines up with its neighbours,
  which is the whole point of showing it.
- **Reviewing a merge request**, in the third pane of the same view. The
  whole diff is reassembled from GitLab's per-file hunks and rendered by
  the same diff viewer everything else here goes through, so folding
  (`Tab`), hunk navigation (`]`/`[`) and "open the real file at this
  line" (`Enter`) work without the review knowing anything about them.
  Review threads are drawn **inline, under the line they hang on** --
  reading a comment about a line anywhere else means holding the line in
  your head while you look elsewhere for what was said about it. Every
  row a thread produces answers the same keys, so `r` (reply) and `R`
  (resolve / reopen) work with the cursor anywhere in it. `C` starts a
  new thread on the diff line under the cursor, anchored to the new side
  for an added or context line and the old side for a removed one, and
  quoting the merge request's own base/head/start SHAs so it can't land
  on a line of a version you weren't looking at -- if those SHAs are
  missing, commenting is refused rather than posted and lost. `A`
  approves or withdraws an approval, and `m` merges, twice: it's the one
  action here that can't be taken back, so it arms first and any other
  key backs out. Comments on the request as a whole (which hang on no
  line) appear in the detail pane instead, and the forge's own narration
  ("changed the description") is kept off the diff entirely.

  Comments are written in a **compose buffer**: a real scratch buffer in
  a strip under what you're reading, with ordinary Vim editing and undo,
  sent with `Enter` from Normal mode and abandoned with `q` -- the keys
  are in the pane title, since a buffer you type prose into has to say
  how to send it without pressing anything first. A send that fails
  leaves the draft where it is, which is the difference between retrying
  and retyping.

  Verified against a real GitLab, not just a stub: `dev/gitlab` brings
  up a containerized instance and seeds it with a project, two merge
  requests and a review thread, and `cargo test -p fenix-gitlab --test
  live -- --ignored` plus `cargo test -p fenix-gui live_ -- --ignored`
  run the whole integration against it. That is what caught the one bug
  a stub structurally cannot: a comment on an *unchanged* line needs
  both `old_line` and `new_line`, and GitLab rejects a position carrying
  only one of them.

  The only configuration is the GitLab server and token in `SPC ,`
  (Forges): the instance root, *not* `/api/v4`, and a personal access
  token with `api` scope. Which project a repo belongs to is read from
  its own `origin` remote -- SSH, `ssh://`, or HTTPS -- so one pair of
  values covers every repo on the instance, and nothing is configured
  per checkout. Each of the three ways that can fail says which one it
  was. The client is written against a `Forge` trait rather than
  GitLab's JSON, so a second forge would be a second client, not a
  second panel.

## Agenda and Jira

- **Agenda** (`SPC a a`): your own tasks on one page with four tabs --
  **Today** (what's running, in progress, due, next up, waiting, and
  what needs you, beside this week's hours), **Board**, **List** and
  **Time** (a week timesheet). A task has its own page: fields edited in
  place, a checklist, what it waits on, notes and Jira comments, and its
  time entries, which can be corrected. `/` searches, `f` filters, `P`
  keeps to the open project (a task filed under a category named like the
  project, or an issue in its Jira project). `n` adds a task in one line
  (`Fix login !high #ui due:fri PROJ-12`); `SPC a h` makes one from the
  selection or line under the cursor and remembers where it came from.
  `SPC a t` stops the clock, resumes the last task or switches; when the
  clock ran while you were away, Fenix asks what to keep. Tasks can be
  linked to Jira issues and stay in step both ways, due dates included;
  time on them is sent as worklogs from the Time tab (`W`). `?` on the
  page lists every key.
- **Jira** (`SPC j j`): a page of searches -- assigned to you, reported
  by you, watching, recently updated, each tracked project's current
  sprint and open issues, each tracked person's, and any saved JQL --
  with the selected search's issues and a preview. An issue answers the
  agenda's own keys (`s` status through its real transitions, `p`
  priority, `A` assign, `u` due, `C` comment, `T` log time, `a`/`t` add
  to the agenda). `n` creates an issue with the project's real issue
  types and the fields a type requires. `SPC j /` searches, `SPC j g`
  opens a key. Talks to a self-hosted Jira Server/Data Center through a
  personal access token (see [Configuration](CONFIGURATION.md)).

## VNC and PDF

- **VNC console panes** (`SPC v ...`): configure VM hosts by hand under
  `[vnc]`, then `SPC v v` fuzzy-picks one by name to open (or switch
  back to) a live VNC connection as an ordinary, splittable pane. Each
  session connects once and stays live in the background indefinitely
  (instant switching thereafter), auto-reconnects with backoff if it
  drops, and throttles its poll rate while unfocused. Mouse and keyboard
  forward straight to the VM while the pane is focused (`Ctrl-\` to
  release, same convention as the terminal panel); clipboard is
  mirrored both ways. `SPC v s` saves the current frame as a PNG.
  Remote resizing: resizing the pane asks the VM to match its own
  resolution, falling back to client-side scaling when the server
  doesn't support that or declines a particular size. No encryption or
  authentication at all -- trusted-network hosts only.
- **PDF viewer** (`SPC r ...`): open a `.pdf` file the same way you'd
  open any other file (typed path, the explorer, a CLI argument) and it
  renders as a scaled-to-fit page in an ordinary, splittable pane instead
  of loading as text. The mouse wheel, `j`/`k` and the arrow keys scroll
  the document continuously -- straight through page boundaries, so a
  scroll never dead-ends at the bottom of a page -- and `PageDown`/
  `PageUp`, `n`/`p`, `Home`/`End` turn/jump pages outright, all as bare
  single keystrokes while a PDF pane is focused. `SPC r g` jumps
  straight to a typed page number; `+`/`-`/`0`/`w` (or `SPC r =`/
  `SPC r -`/`SPC r 0`/`SPC r w`) zoom in/out and fit the page/width, with
  `h`/`l` panning sideways across whatever doesn't fit in the pane at the
  current zoom. The status line shows `Page N/M` and the current zoom in
  place of the line/column an ordinary buffer shows. The render
  re-fits automatically on window resize (except at a fixed percentage
  zoom, which stays put across a resize on purpose). `SPC r o` toggles a
  split-pane outline/bookmarks panel -- a real, Vim-navigable listing
  where `Enter` on an entry jumps the PDF straight to its page. `SPC r /`
  searches the whole document for a word or phrase and lists every match
  (page number plus surrounding context) in its own split pane, `Enter`
  jumping straight to that match's page the same way the outline does.
  Requires `pdfium.dll` (see
  [Optional external tools](BUILDING.md#optional-external-tools)) -- without it,
  opening a PDF shows an error instead of a blank pane.

## Code intelligence, tasks and debugging

- **Autocompletion**: a popup that's always available, sourced from
  whatever's already been typed in the current buffer (`<C-n>`/`<C-p>`-
  style buffer-word completion, any language) -- layered, for Tcl
  specifically, with a built-in keyword list,
  [Universal Ctags](https://ctags.io/)-scanned project definitions, and
  an optional external symbols file (see
  [Configuration](CONFIGURATION.md)). Namespaced procs show their fully-
  qualified path (`myns::subns::proc`, no leading `::`), not just the
  bare proc name.
- **Language servers (LSP)**: a real `lsp-types`/JSON-RPC client, spawned
  per-language on demand for whichever buffer you open, with a built-in
  default command for Python
  ([`pyright`](https://github.com/microsoft/pyright)'s
  `pyright-langserver`), Rust
  ([`rust-analyzer`](https://rust-analyzer.github.io/)), C/C++
  ([`clangd`](https://clangd.llvm.org/)), Bash
  ([`bash-language-server`](https://github.com/bash-lsp/bash-language-server)),
  and JavaScript/TypeScript/TSX
  ([`typescript-language-server`](https://github.com/typescript-language-server/typescript-language-server)) --
  anything else (or an override for one of these) via a `[lsp]` command
  you configure -- see [Configuration](CONFIGURATION.md). A Python server
  is pointed at the project's environment (uv's `.venv`, Poetry's, a
  plain `venv`, else the `python` on PATH) and told again whenever it
  changes -- `uv add`, `uv sync` or a venv created after the server
  started -- so new packages resolve without a restart. Live
  diagnostics (inline severity-colored markup, modeline error/warning
  counts), `gd` go-to-definition, `gr` find-references (populates the
  quickfix list -- `SPC p n`/`SPC p N` steps through it the same way a
  project grep does), `K` hover, `SPC c r` rename, `SPC c a` code
  actions, and `SPC c f`/`SPC c F` reach for the server's own formatter
  before falling back to the structural reindenter below. Completion
  candidates from the server merge straight into the same popup
  autocompletion already opens (`CompletionKind::Lsp`), rather than
  being a separate mechanism. Every one of these degrades gracefully to
  "not available" (not a hard error) when no server is attached, or the
  attached one doesn't advertise that capability. On Windows, a server
  installed as an npm-style `.cmd` shim (`typescript-language-server`,
  `bash-language-server`, and most other JS-ecosystem tooling) launches
  correctly despite `PATH` resolution quirks Rust's own process-spawning
  otherwise has no answer for. A server that asks for incremental sync
  gets each edit as one ranged change rather than the whole document
  (some, like `arduino-language-server`, accept nothing else). Inside an
  Arduino sketch, C/C++ files go to `arduino-language-server` instead of
  plain `clangd` (see the Arduino bullet).
- **Build/task runner**: `SPC p t` fuzzy-picks a task discovered from the
  focused buffer's project root -- built-in defaults for whichever
  ecosystem markers are present (`cargo build`/`test`/`clippy` for a
  `Cargo.toml`, `pytest`/`ruff check` for a `pyproject.toml`, CMake
  configure/build/`ctest` for a `CMakeLists.txt`, `npm run build`/`test`
  for a `package.json` -- every matching ecosystem contributes its own
  set, not just the first one found), plus the project's own
  tasks from `.fenix/tools.json` (`SPC p ,` edits them).
  Runs in a live-streamed single-pane Task Output panel (`SPC p T`
  reruns the last task, `SPC p k` ends it early); `cargo build`/`test`/
  `clippy` specifically run with `--message-format=json` so each
  diagnostic's recovered file/line/column feeds the same quickfix list
  a project grep or LSP references already populate (`SPC p n`/
  `SPC p N` steps through them), while the panel itself still shows
  cargo's own human-readable output (its `rendered` field), not raw
  JSON -- every other tool's plain `file:line:col: message` convention
  (gcc/clang/`pytest`/`ctest`) is recovered the same way without
  needing `--message-format=json` at all.
  Task cancellation terminates the process tree on Windows, including children
  holding output pipes. Closing or rerunning a task never waits for reader
  threads. Windows also cleans up tasks when the editor exits abruptly. Output
  from both streams is drained before the result appears; stale events from a
  previous run are ignored. Cleanup has a two-second deadline and reports errors
  rather than leaving the interface waiting indefinitely. A task's descendants
  are terminated when its main process exits, so use a separate terminal for
  background services intended to outlive a build.

- **Debugger (DAP)**: `SPC u u` starts a real Debug Adapter Protocol
  session for the focused buffer (Python via
  [`debugpy`](https://github.com/microsoft/debugpy)'s
  `python -m debugpy.adapter` -- `pip install debugpy` is the only setup
  needed; other languages have no built-in adapter yet, same "detect +
  guide, no wrong guesses" posture `lsp::default_server_command` already
  has), or continues one that's stopped at a breakpoint. `SPC u b`
  toggles a breakpoint on the current line (persists across
  buffers/sessions, resent live if a session is already running);
  `SPC u n`/`SPC u i`/`SPC u o` step over/into/out; `SPC u w` watches
  the identifier before the cursor; `SPC u q` ends the session. A
  four-pane Call Stack/Variables/Watches/Breakpoints panel (mirroring
  the Docker panel's own multi-pane shape) updates on every stop, and
  stopping moves the cursor straight to the current line -- opening the
  file in a fresh split if it wasn't already showing somewhere, without
  ever displacing whatever the debug panel's own panes are showing. A
  project's own launch target -- required for anything that isn't "the
  script I have open" -- comes from `.fenix/tools.json`'s `launch`
  (`program`, `args`, `cwd`, `env`; `SPC p ,` edits it).
- **Tool status**: `SPC l m` opens a single-pane listing of every
  language with a built-in LSP server or DAP adapter -- the exact
  command that would be launched (a `[lsp]` override if configured,
  else the built-in default), whether it's found on `PATH`, whether a
  session for it is running right now, and, for anything missing, the
  one-line command that installs it (`rustup component add
  rust-analyzer`, `uv tool install pyright`, `npm install -g
  typescript-language-server typescript`, ...). Detect and guide only --
  Fenix never downloads or manages a tool binary itself, the same
  posture the debugger bullet above already takes for adapters it
  doesn't have.
- **Symbol picker**: `SPC m s` in a Tcl file opens a fuzzy-find popup listing every
  known Tcl definition (`proc`/`namespace`) by its fully-qualified name,
  sourced from the same [Universal Ctags](https://ctags.io/) scan
  autocompletion draws on -- confirming a selection opens the file it's
  defined in (if not already open) and jumps straight to that line.
- **Indent region**: `SPC c f` reindents the active Visual selection,
  `SPC c F` the whole buffer -- a language server's own formatter first,
  if one's attached and advertises formatting support (see the LSP
  bullet above), falling back to a structural reindent from `{`/`(`/`[`
  nesting depth (Emacs' own `indent-region`, real Vim's `=` operator)
  otherwise, not by shelling out to a per-language external tool. Works
  on any buffer
  regardless of detected language; when one *is* detected, a fresh
  syntax parse excludes every string/comment span from the bracket scan
  so a stray brace inside a string or comment can't throw off the
  result. `SPC` reaches the leader menu from Visual mode as well as
  Normal for this reason, so `SPC c f` can act on a selection without
  leaving it first.
- **Mode menu** (`SPC m`): a menu whose contents depend on what you're
  working in, like Doom Emacs' local leader -- the focused buffer's
  project first, then its language, merged when both apply (the more
  specific one wins a shared key). Arduino sketches get the Arduino
  commands below; Tcl files get the MIB commands plus Tcl's symbol
  picker (`SPC m s`) and tag refresh (`SPC m T`). The status line names
  the menu that opened; a buffer with no menu says so. Every other
  `SPC` key is the same everywhere.

## Embedded and space systems

- **Arduino** (`SPC m ...` in a sketch): Arduino projects through
  [Arduino CLI](https://arduino.github.io/arduino-cli/), the engine the
  Arduino IDE uses. `SPC m b` builds and `SPC m u` uploads through the
  task runner, so compiler errors point at the real `.ino` line and land
  in the quickfix list. `SPC m m` opens a serial monitor in a terminal
  pane (type a line and `Enter` to send it); uploading stops it and
  restarts it afterwards, since only one program can hold the port, and
  `SPC m B` sets its speed. `SPC m s` / `SPC m o` / `SPC m p` pick the
  board, its options (a Nano's "Old Bootloader" processor, say) and the
  port -- a single recognized board is found and used on its own.
  `SPC m l` installs from the whole Arduino library index, `SPC m c`
  installs board packages, and `SPC m i` shows the board, port, speed
  and whether completion is ready. `SPC f n` creates a sketch from
  anywhere. The board and port live in the sketch's own `sketch.yaml`,
  so the Arduino IDE and `arduino-cli` agree with Fenix; the modeline
  shows them (`nano · COM4`). Completion, live errors and hover come
  from `arduino-language-server`, which understands `.ino` files the way
  the Arduino builder does and completes from the board's core and every
  included library. `SPC m d` debugs boards that support it (flashing a
  debug build and opening Arduino CLI's GDB console) and says why when
  one can't -- AVR boards (Uno, Nano, Mega) have no debug interface.
  Built behind a platform interface (`fenix-embedded`), so another
  family such as STM32 can be added with the same keys and panes.
- **SCOS-2000 MIB** (`SPC m ...` in a Tcl file): fuzzy-find and inspect telecommands
  (`SPC m t`), TM packets (`SPC m k`), TM parameters (`SPC m p`), and
  calibration definitions (`SPC m c`, numeric curves/status
  enumerations/range checks) from one or more configured MIB directories
  (see [Configuration](CONFIGURATION.md)) -- each opens a real, Vim-
  navigable buffer with the definition's summary, related rows (a
  telecommand's parameters with their calibration references, a TM
  packet's parameters, a TM parameter's packet occurrences), and raw
  fields. `SPC m i` builds and inserts a telecommand: pick one, build or
  skip its variable arguments (an argument with known engineering
  aliases offers a picker of them; one with a known numeric range warns,
  without blocking, if the typed value falls outside it), review the
  rendered command, confirm to insert at wherever the wizard started.
  `SPC m r` reparses the configured MIB directories from disk. `SPC m
  a` registers a new MIB directory without leaving the editor: browse
  to it in the file explorer, `S` to select it, then type a label --
  saved to `settings.toml` at once, same as everything else here.
  `SPC m d` fuzzy-finds a configured directory to remove the same way.
  Ported from an ICD 7.2 SCOS-2000 MIB workflow in the author's previous
  (Emacs) config -- see that config's own
  [MIB module](https://github.com/tpedneault/orbit-emacs/blob/master/modules/mod-mib.el)
  for the original.

## Themes, terminals and tables

- **Themes**: `Orbit Dark`, `TempleOS`, `Gruvbox Dark`, `Nord`, `Dracula`,
  `Solarized Dark`, and `One Dark`, jumped to directly by name with a
  fuzzy picker (`SPC t p`), persisted. Two rules hold across all of
  them, and are pinned by tests: comments sit closer to the background
  than body text does, so they read as quieter rather than as more
  code; and operators and punctuation share an accent that is neither
  the body-text color nor the comment color, so the structure of a line
  is visible. Both used to be violated -- punctuation was body-colored
  (invisible as a distinct thing) or comment-colored (actively
  de-emphasized) depending on the theme, which is most of why a
  brace-and-bracket language like Tcl looked flat.
- **Terminals**, in two shapes, because they answer two different
  questions. `SPC o t` is the **popup panel**: one shell for the whole
  application, a full-width strip along the bottom of the window under
  every existing split, that drops down over whatever you were looking
  at and follows you from workspace to workspace -- for the build you
  start, glance at, and dismiss. `SPC o T` puts a **shell in the focused
  pane** instead: it lives in that pane, in that workspace, beside the
  code it goes with, splits and resizes like any other pane, and is
  still there when you switch away and back. Open as many as you like
  (`*terminal 1*`, `*terminal 2*`, ... in `SPC b b`); numbers are never
  reused, since a recycled one would point at a different shell than the
  one you remembered.

  Both run `powershell.exe` on Windows, `$SHELL` (falling back to
  `/bin/sh`) elsewhere, with full ANSI color support (16-color,
  256-color, and RGB foreground/background). Neither is killed by
  looking away: hiding the panel, or pointing a pane at another buffer,
  leaves the shell and anything running in it going, and coming back
  shows it caught up to wherever it got to. Closing a terminal *buffer*
  (`:q`, `SPC b k`) is the one gesture that ends its shell -- that is
  the point at which nothing could show it again.

  `Ctrl-\` hands the keyboard back to the editor without closing or
  hiding anything (Neovim's own `:terminal` convention), so `SPC w ...`
  can move focus elsewhere while the shell stays on screen; `SPC o T`
  (or a click into the pane) takes it back, and running `exit` hands it
  back on its own. A pane terminal only receives what you type while its
  own pane is focused, so a keystroke meant for the file next door never
  lands in a shell.

  A terminal buffer holds no text of its own -- what you see is the
  shell's screen grid, rendered directly -- so there is nothing there
  for `:w` to write to a file and nothing Vim's editing commands can
  corrupt.

  v1 limitations: no mouse reporting, no bracketed paste, no F-keys, no
  application-cursor-mode variants -- covers ordinary shell/REPL/pager
  use, not a full terminfo-correct implementation. Terminal *queries*
  are answered, though (cursor position, device attributes), which is
  not optional on Windows: ConPTY opens every session by asking where
  the cursor is and produces no further output at all until it is told
  -- an emulator that only listens sits looking at a live shell and an
  empty screen forever.
- **Table/spreadsheet view**: `SPC f t` toggles the focused buffer
  between plain text and an elastic-column table view of its own,
  genuinely tab-separated content -- real elastic tabstops, not a
  padding trick: the renderer expands each real `\t` to the visual
  column its column needs (computed from the widest value currently in
  it, re-measured after every edit), so the file on disk stays exactly
  what you see, always genuinely tab-separated, and ordinary Vim editing
  (`i`, `cw`, ...) between two tabs just works. `]`/`[` jump to the
  next/previous column, `c` fuzzy-finds one by name, and `j`/`k` are
  reinterpreted to move a row while staying in the same visual column --
  plain char-based motion doesn't track "same column" once rows have
  different raw lengths up to it. Built for browsing MIB `.dat` files
  and any other TSV data, but general-purpose.

## Refactor previews

LSP rename and edit-based code actions open a read-only multi-file preview.
For a list of code actions, move to an action and press Enter to preview it.
Press `a` or Enter to apply every proposed text edit in memory, or `q` / Escape
to cancel. Files remain unsaved; use the usual save commands after review.
`:undo-refactor` reverses the latest refactor across all affected buffers, provided
none has since been edited, renamed, or closed. Normal `u` undoes only one file.

Edits validate document versions, exact UTF-16 ranges, overlapping ranges, and
buffer/disk changes before applying. Unopened targets are loaded only on apply.
File create/rename/delete operations, annotated edits, command-based actions, and
lazy action resolution are not supported; unsupported edits are rejected whole.

## Project tool environments

Language servers are scoped by language and project root. Configure literal
executables, argument arrays, working directories, and child environment values
in `.fenix/tools.json` for LSP, DAP, and tasks. `:lsp-restart` reloads language
services for the current project. Task rerun history is per project; debugger
and task controls guard against operating on another project's active process.
See [project tool settings](PROJECT_TOOLS.md) for examples and precedence.

## Session restoration

Fenix restores documents, unsaved buffers, workspaces, splits, focus, cursors, and
scroll positions at startup. Use `:session-save` to checkpoint immediately or
`:session-quit` to exit and resume unsaved work next time. Force quit still discards
unsaved work. Missing files and disk conflicts are reported without overwriting
files. See [session restoration](SESSION_RESTORATION.md) for configuration,
recovery behavior, and current limits.

## Native snippets

Insert-mode **Tab** expands `header`, `section`, and Tcl's `proc`, with editable
fields, live mirrors, transformations and date/user/file variables.

`SPC i S` manages them: every snippet by language, the one you're typing
in first, marked built-in, yours or the project's, with a preview of
what the selected one inserts. `n` makes one (its file opens with the
header written), `Enter` edits one, `c` copies a built-in one to yours to
change it, `p` copies one into the project, `d` deletes one to the
Recycle Bin and `t` tries it in the file you came from. `SPC i n` makes
one from the last Visual selection. Saving a `.snippet` file says whether
it works, or why it won't be offered.

They live one file each, in a folder per language: yours in
`%AppData%\fenix\snippets\<language>\`, a project's in
`.fenix\snippets\<language>\` (a project's wins over yours, yours over a
built-in one with the same trigger). See [the snippet guide](SNIPPETS.md)
for syntax, navigation, examples and design details.
