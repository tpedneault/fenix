# Configuration

The easiest way to change a setting is **`SPC ,`**: every setting on one
page, by category, with what it does, its default, and a `•` on what
you've changed. `/` searches them all; `Space` flips a switch, `h`/`l`
step a number or cycle a choice (the theme previews as you go), `Enter`
types a value in place, and lists (VNC hosts, language servers, MIB
roots...) get rows you add, edit in a small form, move and delete. A
value is checked before it's kept -- a font size of 90 stays on its row
with the reason -- and a change applies at once and is saved at once.
`r` puts a setting back to its default, `e` opens the file at its line.

## Where things live

| What | Windows | Linux |
|---|---|---|
| Your settings, `settings.toml` | `%AppData%\fenix` | `~/.config/fenix` |
| Your snippets (a folder per language) and project templates | `%AppData%\fenix\snippets`, `\templates` | `~/.config/fenix/...` |
| Your themes, a `.toml` each | `%AppData%\fenix\themes` | `~/.config/fenix/themes` |
| Your language files, a `.toml` each | `%AppData%\fenix\languages` | `~/.config/fenix/languages` |
| Your data: the agenda | `%AppData%\fenix\data` | `~/.local/share/fenix` |
| This machine's state: session, window placement, recent files, known projects, bookmarks | `%LocalAppData%\fenix\state` | `~/.local/state/fenix/state` |
| Unsaved buffers, downloaded tools, backups | `%LocalAppData%\fenix\recovery`, `\tools`, `\backup` | `~/.local/state/fenix/...` |
| Extensions you've installed, and `extensions.toml` | `%LocalAppData%\fenix\extensions` | `~/.local/state/fenix/extensions` |
| Extensions an administrator installed for everyone, and `policy.toml` | `%ProgramData%\Fenix\extensions` | `/etc/fenix/extensions` |

What you chose roams with your Windows profile; what Fenix noticed about
this machine, and what it downloaded, doesn't. Since the agenda roams,
another machine can change `agenda.json` while Fenix is open here: Fenix
reads it again when it changes (a file it can't read is left alone, and
the agenda it has kept). Other tools change the agenda and notebook
through [the control channel](CONTROL.md), never by writing the files. `FENIX_HOME=<folder>`
puts all of it in one folder instead -- a portable install, or a test
run that mustn't touch your real settings. A second `fenix` normally
hands its files to the one already running with the same `FENIX_HOME`
(see [the control channel](CONTROL.md)), so a test run with its own
folder already runs beside your Fenix; `FENIX_STANDALONE=1` makes any
launch open its own window instead.

## settings.toml

Hand-editing is fine: Fenix notices when the file changes and uses the
new values at once. When it saves a setting itself, it changes only that
line -- your comments, the order of things and keys it doesn't know stay
as they are -- and a setting back at its default loses its line. A value
that's wrong costs only that setting; the status line and `SPC ,` name
the line and why, and the last good value stays in use. A file that
can't be parsed at all is never written over until it's fixed.

```toml
# Only what differs from the defaults needs to be here.
[editor]
theme = "Visual Studio Dark"
font_size = 18            # bigger on the laptop

[lsp.servers]
python = 'C:\Users\you\.local\bin\pyright-langserver.exe --stdio'

[git]
base_branch = "develop"
reviewers = ["alex", "sam"]
auto_fetch = 5

[gitlab]
base_url = "https://gitlab.example.com"
token = "glpat-..."      # SPC , > Forges types it masked

[[vnc.hosts]]            # the VNC extension's
name = "build-vm"
host = "10.0.0.5"         # port defaults to 5900

[reader.documents]       # the PDF reader extension's shelf
"Space Packet Protocol" = 'C:\refs\133x0b2e2.pdf'
```

API tokens are kept in `settings.toml` with everything else -- mind that
before sharing the file. `SPC ,` sets them (typed masked), tests them
against their server (`t`) and clears them (`x`). `FENIX_GITLAB_TOKEN`,
`FENIX_JIRA_TOKEN` and `FENIX_GITHUB_TOKEN` override the file when
they're set, and are never written to it; GitHub also uses the GitHub
CLI's sign-in (`gh auth login`) when there's no token. (Keeping them in
the operating system's credential store instead is planned.)

Coming from another editor? `SPC h i` reads VS Code's, Vim's or
Neovim's settings and shows what it would write here before it writes
anything -- see [Coming from another editor](COMING_FROM.md#bringing-your-settings-over).

## A project's own settings

A project can set some settings for itself in `.fenix/settings.toml`,
meant to be committed so the whole team shares them: indent and tab
width, word characters, the base branch and reviewers. `SPC p ,` opens
the settings page on the project: each row says whether it's set there,
comes from you, or is the default, `Enter` sets one for the project and
`r` goes back to yours. Its first section, "Project & tasks", holds
the rest of what's the project's own: its kind, group, pin and Jira key,
and its tasks, language servers and debug launch (`.fenix/tools.json`).
`p` switches the page between yours and the project's.

**A project's programs wait for you.** `.fenix/tools.json` names
programs: a language server, a debug adapter, tasks. So does
`.fenix/lsp/<server>.json`, since some servers' settings are commands.
Fenix starts a language server as soon as a file is opened, so for a
project you didn't write (a repository just cloned, an archive just
unpacked) those files would be a way to run something by opening a
file. Until you've allowed them, Fenix treats such a project as naming
nothing: your own settings and its defaults apply, and a notification
says the project is waiting. `SPC p y` (`project.trust`) opens the file
and asks; only a typed `yes` allows it, and `no` is remembered too. The
answer is kept in Fenix's own state folder, never in the project, and
for those files as they are: change them (a pull, an edit outside
Fenix) and it asks again. What Fenix writes there for you (the new
project wizard, the settings page, a doctor's fix) doesn't need
allowing again. The projects you already used before this question
existed go on working as they did.

A project's MIBs and how it writes a telecommand are its own too; they
take the place of yours there (`mib.include_yours` adds yours back), and
a folder is relative to the project:

```toml
[mib]
default = "OPS 7.2"
telecommand_template = "tc::send {mnemo} {arguments}"
telecommand_argument_template = "-{name} {value}"

[mib.roots]
"OPS 7.2" = "mib/ops"
SIM = "../simulator/mib"
```

A project from before `.fenix/settings.toml` kept these in
`.fenix/project.ini`; the first time it's opened, that file moves over
-- kind, Jira key, reviewers and serial monitor speed into
`settings.toml`, its old `[tasks]` and `[launch]` into `tools.json` --
and is deleted, so the change shows in `git status` for review.

## Your own themes

A theme is a `.toml` file in `themes\` beside `settings.toml`, in the
same format as the ones Fenix ships (`crates/fenix-gui/themes/`) -- the
quickest start is a copy of one of those. Themes are read when Fenix
starts; yours appear in `SPC t p` and the `editor.theme` setting after
the shipped ones, and one with the same `name` as a shipped theme takes
its place. A file that can't be read is named on the status line at
start-up, with the key that's wrong, and left out.

```toml
name = "My Theme"
tabs = "underline"        # or "block" (Visual Studio's filled tabs)
# font_family = "Consolas"
# border = "#0000aa"      # a frame around the window

[surfaces]
bg = "#1a1b26"
# sidebar_bg = "#1a1b26"  # the explorer's; bg when left out
# tray = "#12121a"        # between the panels; worked out when left out
# card = "#222430"        # a card in a panel (a tool call, a debugger column)
# input = "#292b39"       # an input or a key's cap
bg_modeline = "#24283b"
divider = "#565f89"
caret = "#e0af68"
hl_line = "#222534"
selection = { color = "#7aa2f7", alpha = 0.12 }
bracket_match = { color = "#2ac3de", alpha = 0.12 }
search_match = "#e0af681f"  # #rrggbbaa works too

[text]
fg = "#c0caf5"
# fg_modeline, caret_text: fg and the caret's colour when left out
gutter_fg = "#929bb9"

[modes]      # the modeline badge for each mode, and its lettering
normal = "#e0af68"
insert = "#7dcfff"
visual = "#f7768e"
replace = "#ff9e64"
command = "#7aa2f7"
explorer = "#7aa2f7"
picker = "#bb9af7"
text_dark = "#1a1b26"
# text_light: the Visual badge's lettering; text_dark when left out

[syntax]
keyword = "#bb9af7"
string = "#9ece6a"
comment = "#929bb9"
function = "#7aa2f7"
type = "#2ac3de"
number = "#ff9e64"
constant = "#ff9e64"
variable = "#c0caf5"
operator = "#89ddff"
punctuation = "#89ddff"
attribute = "#e0af68"
# What only a language server tells apart (lsp.semantic_highlighting);
# left out: type's, variable's and function's colours.
# interface = "#e0af68"   # an interface or trait
# parameter = "#ff9e64"   # a function's parameter
# macro = "#89ddff"       # a macro

[explorer]
folder = "#7aa2f7"
file = "#c0caf5"

[git]
modified = "#ff9e64"
staged = "#9ece6a"
untracked = "#7aa2f7"
ignored = "#929bb9"
conflicted = "#f7768e"

# [brackets]  # appearance.rainbow_brackets' colours, outermost first (one to six);
# colors = ["#e0af68", "#bb9af7", "#7aa2f7"]   # left out: syntax.function, keyword, type

# [terminal]  # a terminal's 16 ANSI colours; worked out from the above when left out
# ansi = ["#4c5064", "#f7768e", "#9ece6a", "#ff9e64", "#7aa2f7", "#bb9af7", "#2ac3de", "#a2aad0",
#         "#929bb9", "#ff97aa", "#b2e27f", "#ffb88d", "#9bbbff", "#d3bcfe", "#41d6f0", "#c0caf5"]
```

Colours are `#rrggbb`; a surface drawn over another (a highlight) can be
see-through, as `#rrggbbaa` or `{ color, alpha }`; text can't. Left out,
`tray` is a shade under `bg` -- towards black on a dark theme, towards
the text on a light one -- and `card` and `input` step from `bg` towards
`fg` only as far as keeps every text colour as readable on them as the
bars below (an input that can't step further sinks the other way). The
shipped themes keep body text, comments and line numbers at 4.5:1 against every surface
they're drawn on (accents 3:1 on a light theme),
comments quieter than code, and operators distinct from both -- worth
keeping in yours.

## Language files

A `.toml` in `languages/` gives a language Fenix highlights more file
types, and can name the language server to run for it:

```toml
language = "sql"              # rust, python, cpp, csharp, javascript, ...
extensions = ["pks", "pkb"]   # with or without the dot
filenames = ["schema.ddl"]    # whole names, like Dockerfile
server = "sqls"               # the command line that starts it
```

A file type in a language file wins over the built-in one, so a file can
also move `.inc` from one language to another. An installed extension's
language files are read first, then yours, and yours win. Your
`lsp.servers` setting wins over any language file. A language Fenix
has no grammar for needs code, which extensions can't carry yet. Fenix
reads language files when it starts.

## Extensions you install

An extension is a `.fenixext` file -- a zip with an `extension.toml` at
its root -- or, while you make one, the folder it's made from. For now
Fenix installs extensions of data: themes, snippets, project templates,
keys and language files, in `themes/`, `snippets/`, `templates/`,
`keymaps/` and `languages/` folders laid out like your own (a keymap is a `.toml` of `[[leader]]` and
`[[keys]]` entries, as in `keybindings.toml`: see [Your keys](KEYBINDINGS.md#your-keys-keybindingstoml)). They're read beside the built-in ones and
below yours: your theme, snippet, template or key of the same name wins.
A `launch/` folder holds launch configurations for the debugger
(`SPC u u` offers them in a project that has none of its own; see
[Language packs](WRITING_EXTENSIONS.md#language-packs)). A `tutorial/`
folder holds lessons for the tutorial (`SPC h T`), a `.toml` each in the
format of Fenix's own (`crates/fenix-gui/tutorial/`); they're listed
after Fenix's, under their `track`.

```toml
[extension]
id = "acme.dusk"            # publisher.name: lowercase, digits, - and _
name = "Dusk"
version = "1.0.0"
description = "A theme for late evenings"
runtime = "none"            # data only

[engine]
fenix = ">=1.0, <2"

[dependencies]              # other extensions it needs, by id
# "acme.base" = "^1"
```

`categories` (`["themes"]`) files it on the Extensions page: `languages`,
`themes`, `snippets`, `keymaps`, `templates`, `tools`, `data`, `web`,
`mission` or `fun`. A `screenshots/` folder of pictures is shown on its
details.

From a terminal (the window doesn't open):

```text
fenix --install-extension dusk-1.0.0.fenixext   # or the folder
fenix --install-extension acme.dusk             # from the registry
fenix --list-extensions
fenix --uninstall-extension acme.dusk
```

By id, an extension comes from the registry: `index.json` on the
`registry` branch of `tpedneault/fenix-extensions` (or the mirror
`extensions.registry` names), with the extensions it needs installed
first. The index must be signed with the registry key built into Fenix,
and each package must match the SHA-256 the index gives it; anything
else is refused before it's written. `extensions.store = false` keeps a
machine to files.

The Extensions page (`SPC h x`) does the same from inside Fenix, in
four tabs (`Tab`, or `1`-`4`): Installed, Updates (the installed ones the
registry has a newer version of: `u` updates one, `U` all), Browse (what
the registry has that you haven't installed, in a version for this
Fenix) and Bundles. Enter switches an installed one on or off -- at
once, in the running editor; only its themes and languages wait for the
next start (`SPC q r`) -- or installs one,
and `x x` removes an installed one. The categories down the left (`h`,
and `l` back to the list) keep a tab to one kind; `/` searches by name,
id and description, and `category:tools`, `runtime:data` (or `code`,
`bundle`) and `asks:network` (or `secrets`, `process`, `sockets`,
`files`, `folders`, `draw`, `editor`, `fonts`, `clipboard`, `settings`,
`nothing`) narrow it. `r` reads the registry again.

On a wide pane the selected one's details are beside the list. `d`
opens them whole in the reading view: its version, the Fenix it's for,
what it needs, the bundles it's in, the permissions it asks for (none,
for an extension of data), its versions, and its screenshots, README and
changelog -- read from its package, checked as an install is, before
it's installed. `q` goes back.

A bundle is extensions that go together, installed as one: a manifest
of `dependencies` alone, `runtime = "bundle"`. Installing it installs
its members; removing it removes the members it brought that nothing
else needs (one you installed yourself stays). The welcome's are
bundles in the registry -- Personal, Mission, Review, Jira, Lab and
Reading -- and Fenix knows them too, for a first start before the
registry can be read.

An install replaces any other version of the same extension, and
needs its dependencies installed first. From the Extensions page it's
used at once, but for its themes and languages, which come with the
next start; installed with `fenix --install-extension`, a running Fenix
uses it from its next start. A package is checked before anything is written: its manifest, the Fenix
it's for, and that none of its files would land outside its own folder.

### Extensions that run code

An extension can also carry code: `runtime = "wasm"`, with its
`extension.wasm` at the package's root. Its manifest says which version
of Fenix's extension API it was built against, when to load it, what it
may do, and the commands it adds:

```toml
[engine]
fenix = ">=1.0, <2"
api = "0.4"                  # Fenix's extension API

[activation]
on_startup = false           # or load it when Fenix starts
commands = ["hello.*"]

[permissions]                # nothing, unless it says; shown before it's installed
# network = ["api.example.com"]
# process = ["docker"]

[[contributes.commands]]
id = "hello.greet"
title = "Say hello"
```

Fenix compiles the code once, when the extension first loads after it's
installed or after Fenix is updated, and keeps the compiled copy with
its caches. Each extension runs on its own thread, in a sandbox with
nothing it didn't ask for:

- **Files:** its own folder (with your data, under `extensions/<id>`),
  and the open project only with `filesystem = "project-read"` or
  `"project-write"`.
- **Programs:** only those `process` names, by name, never by path.
- **HTTP:** only to the hosts `network` names: `api.example.com`,
  `*.example.com`, or `${setting:jira.base_url}` for the host in one of
  its settings.
- **Secrets:** only those `secrets` names, by key or by pattern
  (`http.secret.*`). A secret -- Fenix's tokens, an extension's `secret`
  settings, one an extension keeps -- is kept in the system's keychain:
  Windows' Credential Manager, the macOS Keychain, or on Linux the
  Secret Service (GNOME Keyring, KWallet) through `secret-tool`. One
  still in `settings.toml` is moved there when Fenix starts. Where
  there's no keychain, secrets stay in `settings.toml` as plain text. Its
  environment variable (`FENIX_` and its key, upper-case, with `_` for
  `.`: `FENIX_JIRA_TOKEN`) wins over both.
- No environment, and no settings but its own.

An extension can add settings of its own, which appear on the settings
page under Extensions and live in `settings.toml` like any other:

```toml
[[contributes.settings]]
key = "hello.url"            # its own name first
kind = "text"                # text, bool, int, path, list, choice or secret
description = "Where hello says it"
default = "https://example.com"
```

Its pages, pickers and prompts are Fenix's own. A page is laid out by the
extension, and the last one stays on screen while it works on the next.
`q` closes a page, as anywhere. A command that runs too long, endless
recursion, or memory past its limit (128 MiB) stops that extension, not
Fenix. `SPC h l` shows what each one said and why one stopped. A stopped
extension starts again with Fenix.

The API is 0.x while Fenix's own features move onto it, so an extension
built against 0.2 runs only on a Fenix that offers 0.2.
[Writing an extension](WRITING_EXTENSIONS.md) shows how to make one.

### For everyone on a machine

An administrator can install extensions for everyone on a machine by
adding `--machine` to the extension flags:

```text
fenix --install-extension dusk-1.0.0.fenixext --machine
fenix --list-extensions --machine
fenix --uninstall-extension acme.dusk --machine
```

They go in the machine's folder (`%ProgramData%\Fenix\extensions`,
`/Library/Application Support/Fenix/extensions` on macOS,
`/etc/fenix/extensions` on Linux; `FENIX_MANAGED_EXTENSIONS` names
another, and under `FENIX_HOME` it's `managed` in that folder). Create
the folder as an administrator, so that others can only read it.

A managed extension is pinned: it's used in place of anyone's own copy
of the same extension. Only the administrator switches, updates or
removes it, and the Extensions page says so. Nothing updates by itself.

A `policy.toml` in the same folder decides for everyone:

```toml
store = false                                # no registry here: files only
# registry = "https://mirror.example/index.json"   # or everyone uses this mirror
update_check = false                         # no one here looks for a newer Fenix
```

A `policy.toml` that can't be read keeps the store switched off, and
the update check too.

### Looking for a newer Fenix

Fenix doesn't go on the network unasked. With `updates.check = true`
(the Settings page's Windows & session) it asks GitHub once a day for
the latest release of `tpedneault/fenix` -- a request to
`api.github.com` with nothing about your files or settings in it -- and
when there's a newer one, Home says so in a line under the date. `app.check_for_updates`
(the palette) looks whenever you ask, the setting on or off. Nothing is
downloaded or installed: get the new one from the releases page as you
got this one. What it found last is kept in `updates.json` in Fenix's
state folder, so a restart doesn't ask again.

When an extension seems to be the trouble, `fenix --disable-extensions`
starts in safe mode: the core alone, with nothing read from an installed
extension, for that run only -- your settings,
themes and snippets are read as always and left as they are. It opens a
window of its own rather than handing over to a Fenix already running.

## Every setting

The table is generated from the settings' own declarations (a test fails
when it's out of date).

| Setting | Takes | Default | What it does |
|---|---|---|---|
| **Editor** | | | |
| `editor.indent_width` | 1–16 | 4 | Spaces a Tab or >> inserts; Fenix always indents with spaces. *A project can set it.* |
| `editor.tab_width` | 1–16 | 8 | Columns a tab character already in a file takes up. *A project can set it.* |
| `editor.wrap` | true / false | off | A line wider than its pane goes on under itself, at the pane's width; SPC t w (Alt+Z with standard keys) turns it on or off for one buffer until it's closed or Fenix restarts. Not tables, diffs, Git pages, terminals or a side by side. |
| `editor.wrap_languages` | a list | markdown, text | Languages whose lines wrap whatever Wrap long lines says (markdown; text for .txt and .text files). |
| `editor.nowrap_languages` | a list | none | Languages whose lines never wrap unless SPC t w says so: over Wrap long lines and the list of those that do. |
| `editor.line_numbers` | absolute / relative / off | absolute | The gutter's numbers: each line's own, how far each is from the cursor's line, or none. SPC t n cycles them until Fenix restarts. |
| `editor.fold_markers` | true / false | on | A column in the gutter of a file's lines: ▾ where a fold starts, ▸ where it's closed; a click opens or closes it, as za does. |
| `editor.mouse_hover` | true / false | on | The pointer resting on a symbol shows what K would -- its problems first -- under it; moving away or typing closes it. |
| `editor.hover_delay_ms` | 0–5000 | 500 | Milliseconds the pointer rests on a symbol before its hover shows; 0 never shows it. |
| `editor.keys` | vim / standard | vim | How you edit. Vim: Vim's modes and keys, and SPC for everything else. Standard: the keys of a traditional IDE -- typing at once, Shift+arrows to select, Ctrl+C/X/V, Alt+click for more cursors -- and Ctrl+K for the leader's menu. SPC t k switches. |
| `keys.leader_chord` | text | C-k | With standard keys: the chord that opens the leader's menu, as SPC does with Vim keys (C-k, C-;, F1...). |
| `keys.leader_hint` | true / false | on | With standard keys: the modeline says which chord opens the leader's menu, until you've used it a few times. |
| `editor.multi_cursor_modifier` | alt / ctrl | alt | With standard keys: the key held while clicking to add a cursor. Ctrl, for desktops that move windows with Alt+drag; go to definition then moves to Alt+click. |
| `editor.undo_pause_ms` | 0–10000 | 1000 | With standard keys: a pause this long, in milliseconds, between keys you type starts a new undo step; 0 never does. |
| `editor.iskeyword_extra` | text | none extra | Characters besides letters, digits and _ that count as part of a word, for w, * and completion. *A project can set it.* |
| `terminal.shell` | text | PowerShell 7, else Windows PowerShell; $SHELL | The shell a terminal starts, with its arguments: pwsh -NoLogo, bash -l; a path with spaces in double quotes. Empty: PowerShell 7 on Windows when it's installed, else Windows PowerShell; $SHELL elsewhere. |
| `search.smart_case` | true / false | on | A search in lower case matches any case; a capital letter makes it exact. For / and the Search page; Alt-c on the page, \C in /, match case anyway. |
| `search.regex` | true / false | off | Whether the Search page (SPC /) starts reading the query as a regex rather than literal text. Alt-r flips it there; / and :s are always regexes. |
| `search.exclude` | a list | – | Globs the Search page leaves out to begin with ("target", "*.lock"). Ignored files are left out anyway; f on the page edits the globs. |
| `search.context` | 0–9 | 0 | Lines the Search page shows around each match; + and - change it there. |
| `search.max_results` | 100–1000000 | 10000 | A search stops after this many matches and says so, rather than filling memory. |
| **Appearance** | | | |
| `editor.theme` | text | Orbit Dark | The colour theme; h and l preview each one. |
| `editor.font_family` | text | the system's monospace font | A monospace font installed on this machine; Enter lists them to pick from, h and l step through them. |
| `editor.font_size` | 6–48 | 16 | Text size, in points. |
| `appearance.corner_radius` | 0–16 | the theme's | Pixels of rounding on popups and floating boxes. 0 keeps them square. |
| `appearance.shadows` | true / false | on | Popups cast a soft shadow. |
| `appearance.indent_guides` | true / false | on | A thin line for each indent level. |
| `appearance.active_indent_guide` | true / false | on | The guide of the block the cursor is in is brighter. |
| `appearance.overview_ruler` | true / false | on | A track on each code pane's right edge showing the visible part, search matches, problems by severity, Git changes, the cursor and any extra cursors. A click on a mark goes to it; elsewhere, to that part of the file. |
| `appearance.sticky_scroll` | true / false | on | The first lines of the function and class you're inside stay at the top of the pane. |
| `appearance.sticky_lines` | 1–6 | 3 | How many enclosing lines sticky scroll keeps at most. |
| `appearance.rounded_selection` | true / false | on | A Visual selection's outer corners are rounded, so it reads as one shape. |
| `appearance.selection_whitespace` | true / false | on | Spaces show as dots and tabs as arrows inside a Visual selection. |
| `appearance.dim_unfocused` | true / false | on | Panes that don't have the keyboard are drawn dimmer. |
| `appearance.dim_amount` | 5–80 | 30 | How much dimmer, in percent. |
| `which_key.delay_ms` | 0–2000 | 250 | Milliseconds a leader key (SPC, g, ...) waits before the menu of what comes next opens. Once open, deeper levels show at once. |
| `which_key.order` | key / label | key | The key menu's order: by key, so a key never moves, or by what it does. |
| `appearance.rainbow_brackets` | true / false | off | Brackets are coloured by how deeply they're nested: the code's own, from its syntax tree, not those in strings and comments. The colours are the theme's [brackets] colors, or its function, keyword and type colours. |
| `appearance.tabs` | auto / block / underline / off | auto | How each pane's tabs are drawn: the theme's own look (Visual Studio Dark uses blocks, the rest an underline), always blocks, always an underline, or no tab strip at all -- gt and gT still move between tabs then. |
| `appearance.panels` | framed / flat | framed | How the window's panes, sidebar, terminal and docks are drawn: rounded panels on a darker tray a gap apart, with an edge on the one that has the keyboard, or flat and edge to edge with lines between them, as before 1.4. |
| `editor.preview_tab` | true / false | on | A jump (gd, a search result, a symbol) opens in one reusable tab, in italics, until you edit it or keep it with SPC b P. |
| `screenshot.style` | accent / solid / sunset / ocean / candy / forest / none | accent | What's behind a code screenshot's window: a gradient of the theme's accent, the accent flat, one of four gradients, or nothing (see-through). |
| `screenshot.title` | true / false | on | A code screenshot's window has a title bar with three dots and the file's name. |
| `screenshot.line_numbers` | true / false | off | A code screenshot shows each line's number in the file. |
| `screenshot.scale` | 1–4 | 2 | Pixels per point a code screenshot is drawn at; 2 is sharp on a retina screen. |
| `screenshot.padding` | 0–200 | 56 | Points of background around a code screenshot's window. |
| `screenshot.max_width` | 20–400 | 100 | Columns a code screenshot's lines take at most; a longer one wraps or is cut. |
| `screenshot.long_lines` | wrap / cut | wrap | A line wider than the screenshot's width goes on under itself, or stops with an ellipsis. |
| `screenshot.context` | 0–50 | 3 | Lines either side of the highlighted ones that SPC c S takes too. |
| `screenshot.folder` | a path | the Pictures folder | Where code screenshots are saved, besides the clipboard. |
| **Motion** | | | |
| `editor.animations` | true / false | on | Off stops every animation, whatever the settings below say. |
| `motion.level` | off / subtle / full | subtle | How much moves. Subtle: short animations that show what changed. Full adds the caret gliding, folds opening and panes moving. Each animation below can be turned on or off on its own. SPC t a cycles it. |
| `motion.speed` | 0.25–4 | 1 | Multiplies every animation's length: 2 is half as fast, 0.5 twice as fast. |
| `motion.caret_fade` | true / false | with subtle | The caret fades in and out when it blinks instead of switching. |
| `motion.smooth_scroll` | true / false | with subtle | The view eases to where it scrolls: up and down, sideways, and from one mouse-wheel notch to the next. |
| `motion.scroll_ms` | 20–1000 | 150 | Milliseconds a scroll takes. |
| `motion.yank_pulse` | true / false | with subtle | What you yank or paste flashes once. |
| `motion.beacon` | true / false | with subtle | After a jump (gd, a search, Ctrl-O, a picker, a mark) the line you land on flashes once. |
| `motion.beacon_ms` | 50–2000 | 300 | Milliseconds the jump beacon takes to fade. |
| `motion.beacon_on_focus` | true / false | with subtle | Moving to another pane with the keyboard flashes its cursor line. |
| `motion.change_pulse` | true / false | with subtle | Undo, redo, . and :s flash what they changed; a deletion flashes a bar where the text was. |
| `motion.popups` | true / false | with subtle | Which-key, completion, hover, prompts and menus fade in rising from where they open. |
| `motion.messages` | true / false | with subtle | Messages in the modeline fade in and out instead of appearing and cutting off. |
| `motion.error_flash` | true / false | with subtle | An error flashes the mode rail red once. |
| `motion.progress` | true / false | with subtle | Work in the background (a fetch, a sync, a language server starting) shows in the modeline with the Fenix mark turning. |
| `motion.mode_fade` | true / false | with subtle | Changing mode blends the rail and the caret to the new mode's colour. |
| `motion.tabs` | true / false | with subtle | The active tab's accent slides to the tab you move to. |
| `motion.caret_glide` | true / false | with full | The caret slides to where a motion takes it instead of jumping. Never while typing. |
| `motion.glide_ms` | 20–500 | 45 | Milliseconds the caret takes to glide. |
| `motion.folds` | true / false | with full | Rows revealed by unfolding (a folder, a code fold) open downwards. |
| `motion.layout` | true / false | with full | Splitting, closing and resizing panes move their edges instead of jumping. |
| `motion.theme_fade` | true / false | with subtle | Changing theme fades from the old colours to the new. |
| `motion.splash` | true / false | on | While Fenix starts, the mark and a line that fills as it loads show until the editor is ready. It moves unless animations are off; only this setting hides it. |
| **Files & explorer** | | | |
| `editor.watch_files` | true / false | on | Notice when an open file changes on disk, and reload it when you haven't edited it. |
| `editor.auto_save` | off / after_delay / on_focus_change | off | Save a file's edits for you: after a pause in editing (the delay below), or when you leave it for another buffer, pane or application. Never a file with no name yet or not yet on disk, one shown read-only, a remote project's, or one that changed on disk since it was read -- that, or a save that fails, waits until you save it yourself; it doesn't format. |
| `editor.auto_save_delay_ms` | 100–600000 | 1000 | With auto-save after a delay: milliseconds without an edit before the file is saved. |
| `editor.format_on_save` | true / false | off | Saving runs the formatter an extension gives for the file's language (a language pack's) first. |
| `explorer.follow` | true / false | on | The explorer's sidebar shows the file being edited: its project, the folders down to it, the file selected. Off pins it where it is; SPC e r still shows the file. |
| `explorer.width` | 120–2000 | 240 | The explorer sidebar's width in pixels. Dragging its edge sets it. |
| `explorer.hide` | a list | – | Names the explorer doesn't list, as globs: * is any run of characters, ? any one ("*.pyc", "node_modules"). Hidden files (. shows them) are separate. |
| `explorer.compact` | true / false | on | A folder holding only a folder shares its row with it (src/main/java), and opens down to where the tree branches. |
| `explorer.show_hidden` | true / false | off | List hidden files and folders: those whose name starts with a dot (.git), and on Windows those marked hidden or system. . in the explorer, or SPC e h, switches it. |
| **Completion & LSP** | | | |
| `completion.symbols_file` | a path | – | A text file of words, one per line, offered by completion everywhere. |
| `snippets.builtin` | true / false | on | Offer the snippets that come with Fenix; yours and a project's always are. SPC i S manages them. |
| `diagnostics.inline` | all / errors / off | all | Problems from language servers are underlined in the text, with a dot in the gutter. SPC t d cycles it. |
| `diagnostics.message` | cursor / all / off | cursor | Which lines show their problem's message at the end: the cursor's line, every line, or none. |
| `diagnostics.delay_ms` | 0–5000 | 400 | Milliseconds after you stop typing in Insert before new problems are drawn. |
| `lsp.semantic_highlighting` | true / false | on | Code is coloured by what its language server knows (a parameter apart from a local, a macro apart from a function), over the grammar's colours, where the server offers it. Applies to servers started after it's changed (SPC q r). |
| `lsp.semantic_highlighting_off` | a list | none | Languages whose files keep the grammar's colours alone, whatever Colours from language servers says. |
| `lsp.code_lens` | true / false | off | What a language server offers for a function or test, on a row of its own above it (Run, Debug, 3 references), dimmed. Off for now: while a file has lenses its pane snaps rather than easing, and sticky scroll is off. Turned off, every lens goes in the next frame. Turned on, each file is asked at its next sync; a server started while it was off learns that Fenix takes its refreshes only once it's restarted (SPC q r). |
| `lsp.code_lens_off` | a list | none | Languages whose files show no code lenses, whatever Code lens says. |
| `lsp.code_lens_tests` | true / false | on | With Code lens on: a ▶ Run test lens over each test Fenix can run alone (a Rust #[test], a pytest test, a Go TestXxx) where the language server offers none, with or without a language server; it runs only in a trusted project. So with Code lens on, a file of tests goes by rows (it snaps, and sticky scroll is off) though no server offers lenses. |
| `lsp.servers` | language = command | – | A language server to run for a language, as the command line that starts it. |
| `debug.trail_stops` | 1–10000 | 200 | How many of a debug session's stops the Ember Trail keeps, oldest first to go. Kept in memory only. |
| **Git** | | | |
| `git.base_branch` | text | main or master | The branch pull requests and comparisons start from. *A project can set it.* |
| `git.reviewers` | a list | – | Usernames asked to review a new pull request. *A project can set it.* |
| `git.auto_fetch` | minutes | never | Fetch the focused repository in the background this often, in minutes. |
| `git.layout` | page / panes | page | The Git status page, or the older seven-pane panel. |
| `git.graph_style` | ascii / unicode | ascii | Characters the commit graph is drawn with; unicode needs a font with box-drawing glyphs. |
| `git.graph_limit` | 10–100000 | 200 | How many commits the graph view reads. |
| **Forges** | | | |
| `gitlab.base_url` | text | – | The GitLab instance's address, like https://gitlab.example.com. |
| `gitlab.token` | text (a token) | – | A personal access token with the api scope. |
| `github.token` | text (a token) | – | Used when the GitHub CLI isn't signed in (gh auth login). |
| **Jira & agenda** | | | |
| `jira.base_url` | text | – | Your Jira Server or Data Center's address. |
| `jira.token` | text (a token) | – | A personal access token for the Jira server. |
| **Workspaces** | | | |
| `workspaces` | name = opens | – | SPC TAB f: git, jira, containers, vnc:HOST, project:PATH, or nothing. |
| **Windows & session** | | | |
| `session.restore_windows` | true / false | on | Put Fenix's windows back where they were, on the monitors they were on. *Needs a restart.* |
| `session.restore_session` | true / false | on | Reopen the workspaces and files you had open. *Needs a restart.* |
| `updates.check` | true / false | off | Once a day, ask GitHub whether there's a newer release of Fenix, and say so on Home. Nothing is downloaded. app.check_for_updates looks whenever you ask, this on or off. |
| `session.workspace_per_project` | true / false | on | Opening a project gives it a workspace of its own. |
| **Extensions** | | | |
| `extensions.store` | true / false | on | Whether Fenix may look up and download extensions from the registry. Off on a machine that installs only from files (fenix --install-extension x.fenixext). |
| `extensions.registry` | text | https://raw.githubusercontent.com/tpedneault/fenix-extensions/registry/index.json | Where the registry's index.json is: Fenix's own, or a mirror of it. The index must be signed with Fenix's registry key either way. |
| **Agents** | | | |
| `agents.default` | text | – | The agent a new thread talks to: claude, codex, devin, gemini, or one of yours. Empty: the first one found. *A project can set it.* |
| `agents.servers` | name = command | – | Agents to start, as the command line that runs each over ACP. A built-in's name (claude, codex, devin, gemini) replaces it. |
| `agents.env` | variable = value | – | Environment variables every agent gets: a proxy, a region. |
| `agents.secrets` | a list | – | Environment variables filled from the keychain for every agent (ANTHROPIC_API_KEY, OPENAI_API_KEY, WINDSURF_API_KEY): SPC a s sets one. |
| `agents.write` | save / buffer | save | An agent's edits made through Fenix are saved as they're made (save), so its builds see them, or left unsaved for you (buffer). |
| `agents.save_before_prompt` | ask / always / never | ask | Agents read files from disk: save the project's changed files before a prompt goes out. |
| `agents.follow` | true / false | off | The pane beside the agent's thread shows each file it works on. |
| `agents.panel_width` | 20–80 | 38 | How much of the width the agent's thread takes, in percent. |
| `agents.idle_minutes` | minutes | 10 | An agent with no thread open is stopped after this long. |
| `agents.notify` | true / false | on | When Fenix isn't in front: a permission question, or a turn ended, asks for your attention. |
| `agents.usage.modeline` | off / tokens / cost / all | tokens | What the modeline says each agent used this session: its tokens, its cost (where the agent says), or both with its turns and time. An agent that reports neither shows its turns and time. SPC a U has the days. |
| `agents.usage.daily_budget` | text | – | What the agents may use in a day before Fenix says so, at 80% and again at 100%: a cost ($5) or tokens (2M, 500k). Empty: no budget. |
| `agents.mcp.fenix` | true / false | on | Agents get Fenix as an MCP server (fenix --mcp): diagnostics, language servers' answers, open files as they stand, the debugger's stop. |
| `agents.mcp.fenix_brief` | true / false | on | Agents Fenix starts are told to prefer its tools, in a note before a new session's first prompt, which isn't shown in the thread. |
| `agents.mcp.allow` | class, toolset:name or tool = allow, ask or deny | – | Rules for the tools agents call through Fenix: a class (read, show, edit, run, external), toolset:<name>, or a tool's name, each allow, ask or deny; the most specific wins. By default reading and showing are allowed, edits and running are asked about, external is refused. client:<rule> is for MCP clients Fenix didn't start. A project's rules can only be stricter. *A project can set it.* |
| `agents.mcp.toolsets` | a list | – | Fenix's toolsets agents are shown, beyond those that fit the project: +name adds one (+commands, +debug, an extension's), -name takes one away. Agents can turn on the others themselves (fenix_toolsets). *A project can set it.* |
| `agents.mcp.http` | text | – | An address on this machine (127.0.0.1:7357) where Fenix serves its MCP tools over HTTP, for MCP clients that don't start fenix --mcp themselves. They present the token fenix mcp token prints, and follow the client: rules. Empty: off. *Needs a restart.* |
| `agents.mcp.max_answer_kib` | 4–1024 | 64 | The most a Fenix tool answers an agent, in KiB: a longer diff, log or output is cut, saying how to get the rest. |
| `agents.mcp.fenix_auto_read` | true / false | on | Fenix 1.0's switch, still read: off makes agents ask to read (read = ask in agents.mcp.allow). Use agents.mcp.allow instead. |
| `agents.mcp.fenix_write` | true / false | off | Fenix 1.0's switch, still read: on lets agents edit and run without asking (edit = allow, run = allow in agents.mcp.allow), and lets other control clients run tasks and commands. Use agents.mcp.allow instead. |
| `agents.mcp.servers` | name = command | – | Your own MCP servers, handed to every agent session: the command line that starts each, or the https:// address of one that's running (sse+https:// for SSE), with any headers after it as Name=value. |
| `agents.terminal.output_limit` | 16–65536 | 1024 | The most of a command's output an agent's terminal keeps, in KiB. |
| `agents.handoff_kib` | 4–1024 | 64 | How much of a thread's transcript goes with a hand-off to another agent, in KiB. |
| `agents.ask.agent` | text | – | The agent that answers an extension's question (agents.ask) when it doesn't name one: a commit message, a pull request's description. Empty: the default agent. *A project can set it.* |
| `agents.ask.model` | text | – | The model those questions go to, when the agent offers a choice (haiku, sonnet): a quick, cheap one for drafts. Empty: the agent's own. *A project can set it.* |
| `agents.ask.show` | thread / hidden | thread | Whether an extension's question to an agent is kept as a thread on SPC a t (thread), or gone once it's answered (hidden), when the extension doesn't say. |
| `agents.registry` | text | https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json | The ACP registry agents are installed from. Empty: none. |
| **Notifications** | | | |
| `notifications.enabled` | true / false | on | Toasts in a corner of the window for what finished or wants you: an agent's turn, a task, an extension's news. Off, they're only kept in the history (SPC o n). |
| `notifications.position` | top-right / bottom-right / top-left / bottom-left | top-right | The corner the toasts stack in, the newest nearest it. |
| `notifications.max_visible` | 1–8 | 4 | How many toasts show at once; the rest wait their turn. |
| `notifications.width` | 24–120 | 46 | A toast's width, in characters. |
| `notifications.timeout_low` | 1–600 | 4 | Seconds a low-urgency toast shows, counted only while Fenix is in front. |
| `notifications.timeout_normal` | 1–600 | 8 | Seconds a normal toast shows, counted only while Fenix is in front. |
| `notifications.timeout_critical` | 0–3600 | 0 | Seconds a critical toast shows; 0 keeps it until you dismiss it (SPC o x). |
| `notifications.system` | unfocused / always / never | unfocused | Also show each one as the system's own notification (notify-send on Linux, a Windows toast, macOS's): only while Fenix isn't in front, always, or never. |
| `notifications.rules` | source (agents, tasks, an extension's id, fenix.*) = mute, history, low, normal or critical | – | What to do with a source's notifications: mute them, keep them to the history, or show them at another urgency. A source ending in * matches every one it begins. |
| `notifications.agents` | all / unfocused / never | unfocused | Notify when an agent's turn ends or it asks you something: always, only when its thread isn't in front of you, or never. |
| `notifications.tasks` | all / long / failed / never | long | Notify when a task ends: every one, those that took 10 seconds or more, only those that failed, or never. |
test app::settings_host::tests::print_the_settings_table ... ok

**Shelves, not autostart.** VNC hosts, documents and the workspace
shelf make things selectable (`SPC v v`, `SPC r f`, `SPC TAB f`); none of
them opens or connects to anything at launch. The one thing Fenix does
by itself on startup is put its windows back (`session.restore_windows`)
and reopen your workspaces (`session.restore_session`).
