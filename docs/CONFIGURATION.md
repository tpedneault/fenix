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
| Your data: the agenda | `%AppData%\fenix\data` | `~/.local/share/fenix` |
| This machine's state: session, window placement, recent files, known projects, bookmarks | `%LocalAppData%\fenix\state` | `~/.local/state/fenix/state` |
| Unsaved buffers, downloaded tools, backups | `%LocalAppData%\fenix\recovery`, `\tools`, `\backup` | `~/.local/state/fenix/...` |

What you chose roams with your Windows profile; what Fenix noticed about
this machine, and what it downloaded, doesn't. `FENIX_HOME=<folder>`
puts all of it in one folder instead -- a portable install, or a test
run that mustn't touch your real settings.

An installation from before `settings.toml` moves over by itself the
first time Fenix starts: `config.ini` becomes `settings.toml`, the
other files go to their new places, and everything replaced is kept in `backup\` with a
`MIGRATION.txt` saying what moved.

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

[[vnc.hosts]]
name = "build-vm"
host = "10.0.0.5"         # port defaults to 5900

[documents]
"Space Packet Protocol" = 'C:\refs\133x0b2e2.pdf'
```

API tokens are kept in `settings.toml` with everything else -- mind that
before sharing the file. `SPC ,` sets them (typed masked), tests them
against their server (`t`) and clears them (`x`). `FENIX_GITLAB_TOKEN`,
`FENIX_JIRA_TOKEN` and `FENIX_GITHUB_TOKEN` override the file when
they're set, and are never written to it; GitHub also uses the GitHub
CLI's sign-in (`gh auth login`) when there's no token. (Keeping them in
the operating system's credential store instead is planned.)

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

## Every setting

The table is generated from the settings' own declarations (a test fails
when it's out of date).

| Setting | Takes | Default | What it does |
|---|---|---|---|
| **Editor** | | | |
| `editor.indent_width` | 1–16 | 4 | Spaces a Tab or >> inserts; Fenix always indents with spaces. *A project can set it.* |
| `editor.tab_width` | 1–16 | 8 | Columns a tab character already in a file takes up. *A project can set it.* |
| `editor.iskeyword_extra` | text | none extra | Characters besides letters, digits and _ that count as part of a word, for w, * and completion. *A project can set it.* |
| **Appearance** | | | |
| `editor.theme` | text | Orbit Dark | The colour theme; h and l preview each one. |
| `editor.font_family` | text | the system's monospace font | A monospace font installed on this machine; Enter lists them to pick from, h and l step through them. |
| `editor.font_size` | 6–48 | 16 | Text size, in points. |
| `appearance.corner_radius` | 0–16 | the theme's | Pixels of rounding on popups and floating boxes. 0 keeps them square. |
| `appearance.shadows` | true / false | on | Popups cast a soft shadow. |
| `appearance.indent_guides` | true / false | on | A thin line for each indent level. |
| `appearance.active_indent_guide` | true / false | on | The guide of the block the cursor is in is brighter. |
| `appearance.overview_ruler` | true / false | on | A track on each code pane's right edge showing the visible part, search matches, problems, changes and the cursor. Click it to jump. |
| `appearance.sticky_scroll` | true / false | on | The first lines of the function and class you're inside stay at the top of the pane. |
| `appearance.sticky_lines` | 1–6 | 3 | How many enclosing lines sticky scroll keeps at most. |
| `appearance.rounded_selection` | true / false | on | A Visual selection's outer corners are rounded, so it reads as one shape. |
| `appearance.selection_whitespace` | true / false | on | Spaces show as dots and tabs as arrows inside a Visual selection. |
| `appearance.dim_unfocused` | true / false | on | Panes that don't have the keyboard are drawn dimmer. |
| `appearance.dim_amount` | 5–80 | 30 | How much dimmer, in percent. |
| `which_key.delay_ms` | 0–2000 | 250 | Milliseconds a leader key (SPC, g, ...) waits before the menu of what comes next opens. Once open, deeper levels show at once. |
| `which_key.order` | key / label | key | The key menu's order: by key, so a key never moves, or by what it does. |
| `appearance.rainbow_brackets` | true / false | off | Brackets are coloured by how deeply they're nested. |
| `appearance.tabs` | auto / block / underline / off | auto | How each pane's tabs are drawn: the theme's own look (Visual Studio Dark uses blocks, the rest an underline), always blocks, always an underline, or no tab strip at all -- gt and gT still move between tabs then. |
| `editor.preview_tab` | true / false | on | A jump (gd, a search result, a symbol) opens in one reusable tab, in italics, until you edit it or keep it with SPC b P. |
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
| **PDF reader** | | | |
| `documents` | name = file | – | The SPC r f shelf: a name and the file it opens. A project's own shelf is listed first. *A project can set it.* |
| `reader.colors` | paper / theme | paper | Paper shows pages as printed. Theme draws them in the current theme's colours. SPC r c switches; open PDFs change at once. |
| `reader.zoom` | width / page | width | How a PDF opens the first time: its pages' width fills the pane, or a whole page fits. After that it opens as you left it. |
| `reader.page_gap` | 0–40 | 12 | Pixels between pages. |
| `reader.remember` | true / false | on | Reopen each PDF where you left it, with its zoom and marks. |
| `reader.sidebar` | auto / beside / over | auto | Where o opens the outline: beside the pages, over them, or beside them when the pane is wide enough. |
| **Files & explorer** | | | |
| `editor.watch_files` | true / false | on | Notice when an open file changes on disk, and reload it when you haven't edited it. |
| **Completion & LSP** | | | |
| `completion.symbols_file` | a path | – | A text file of words, one per line, offered by completion everywhere. |
| `snippets.builtin` | true / false | on | Offer the snippets that come with Fenix; yours and a project's always are. SPC i S manages them. |
| `diagnostics.inline` | all / errors / off | all | Problems from language servers are underlined in the text, with a dot in the gutter. SPC t d cycles it. |
| `diagnostics.message` | cursor / all / off | cursor | Which lines show their problem's message at the end: the cursor's line, every line, or none. |
| `diagnostics.delay_ms` | 0–5000 | 400 | Milliseconds after you stop typing in Insert before new problems are drawn. |
| `lsp.servers` | language = command | – | A language server to run for a language, as the command line that starts it. |
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
| `jira.sync_minutes` | minutes | 10 | How often linked agenda tasks are brought up to date, in minutes. |
| `jira.projects` | key = name | – | Jira projects the Jira page lists open issues and the current sprint for. |
| `jira.users` | username = name | – | People the Jira page lists issues assigned to. |
| `jira.queries` | name = jql | – | Searches saved on the Jira page, by name. |
| `jira.blocked` | project = meaning | – | Per project: flag, local, or the status to move to, as ID: Name. Learned the first time you block a task. |
| `jira.priorities` | jira priority = agenda priority | – | How a Jira priority maps onto the agenda's. |
| `agenda.categories` | a list | – | Categories offered when you file a task. |
| `agenda.worklog_round` | minutes | 15 | Time logged to Jira is rounded to this many minutes. |
| `agenda.idle_minutes` | minutes | 60 | With the clock running, how long without a key press before Fenix asks what time to keep. 0 never asks. |
| **Embedded** | | | |
| `embedded.arduino_cli` | a path | found on PATH | Where arduino-cli is, when it isn't found by itself. |
| `embedded.clangd` | a path | found on PATH | Where clangd is, when it isn't found by itself. |
| `embedded.arduino_language_server` | a path | downloaded when needed | Where arduino-language-server is, when it isn't found by itself. |
| **SCOS-2000 MIB** | | | |
| `mib.roots` | name = folder | – | The MIB folders: their .dat tables. A project's own list takes the place of yours in that project; a folder in a project can be relative to it. *A project can set it.* |
| `mib.default` | text | the first | The MIB whose definition is used when several define the same name. *A project can set it.* |
| `mib.include_yours` | true / false | off | In a project with MIBs of its own, use yours too. *A project can set it.* |
| `mib.telecommand_template` | text | telecommand_send PUS_T={type} PUS_ST={stype} APID={apid} MNEMO={mnemo} ARGUMENTS=[{arguments}] | How an inserted telecommand is written: {mnemo}, {type}, {stype}, {apid}, {description}, {mib} and {arguments} are filled in. *A project can set it.* |
| `mib.telecommand_argument_template` | text | {name}={value} | How each argument is written: {name} and {value} are filled in. *A project can set it.* |
| `mib.telecommand_argument_separator` | text | ", " | What goes between arguments. *A project can set it.* |
| `mib.editor_files` | a list | every file | Extensions of the files where K, gd and completion know MIB names (tcl, py). Empty: every file. *A project can set it.* |
| `mib.apid_format` | hex / decimal | hex | How APIDs are shown. |
| `mib.watch` | true / false | on | Reload a MIB when one of its .dat files changes on disk. |
| `mib.check_scripts` | true / false | on | Underline telecommand calls in scripts that the MIB disagrees with: unknown mnemonics, missing or out-of-range arguments. *A project can set it.* |
| **CCSDS & PUS** | | | |
| `ccsds.pus` | c / a / none | c | The packet utilization standard the mission's packets follow: ECSS-E-ST-70-41C, 70-41A, or plain space packets. *A project can set it.* |
| `ccsds.tm_time` | text | cuc 4.2 | The time in a TM secondary header: cuc 4.2 (4 coarse, 2 fine octets), cds 16, add p for a P-field, or none. *A project can set it.* |
| `ccsds.epoch` | text | 1958-01-01 TAI | What on-board times count from: a date and time, then TAI, UTC or GPS. *A project can set it.* |
| `ccsds.time_correlation` | text | – | An on-board time and the UTC it matched (814B878A.8000 = 2026-09-27 14:32:05.5): on-board times are read from the epoch this implies, everywhere, and the time converter shows them uncorrelated too. *A project can set it.* |
| `ccsds.clock` | true / false | off | The modeline shows the time now as the mission writes it on board, beside the clock. *A project can set it.* |
| `ccsds.crc` | ccitt16 / iso / none | ccitt16 | The check at the end of a packet, where the MIB doesn't say: CRC-16-CCITT, the ISO checksum, or none. *A project can set it.* |
| `ccsds.tc_source_id` | 0–65535 | 0 | The source ID written into telecommand packets Fenix builds. *A project can set it.* |
| `ccsds.plf_offset` | after-headers / packet-start | after-headers | Where the MIB's PLF_OFFBY counts from: after the packet's headers (PID_DFHSIZE), or its first octet. *A project can set it.* |
| `ccsds.frame_type` | tm / aos / uslp / tc | tm | The transfer frames recordings and live sources carry, when they carry frames. *A project can set it.* |
| `ccsds.frame_length` | 7–65535 | 1115 | Octets in a transfer frame, without its sync marker or Reed-Solomon check symbols. *A project can set it.* |
| `ccsds.frame_asm` | true / false | on | Frames are preceded by the 1ACFFC1D attached sync marker (CADUs). *A project can set it.* |
| `ccsds.frame_randomized` | true / false | off | Frames went through the CCSDS pseudo-randomizer. *A project can set it.* |
| `ccsds.frame_rs_depth` | 0–8 | 0 | Interleave depth of the (255,223) Reed-Solomon code; 0 when there's none. Code words are corrected. *A project can set it.* |
| `ccsds.frame_ocf` | true / false | on | TM or AOS frames end in an operational control field (the CLCW). *A project can set it.* |
| `ccsds.frame_fecf` | true / false | off | Frames end in a CRC-16 frame error control field. *A project can set it.* |
| `ccsds.tc_fecf` | true / false | on | Telecommand transfer frames (in CLTUs) end in a CRC-16 frame error control field. *A project can set it.* |
| `ccsds.tc_segment_header` | true / false | on | Telecommand transfer frames start their data with a segment header (MAP ID and sequence flags). *A project can set it.* |
| `ccsds.tc_randomized` | true / false | off | Telecommand transfer frames went through the CCSDS pseudo-randomizer before their CLTU was coded (optional in 231.0-B). *A project can set it.* |
| `ccsds.tc_bch` | correct / detect | correct | How CLTU code blocks are decoded, as the spacecraft does: correct puts one wrong bit per block right (error-correcting mode), detect turns the block away. *A project can set it.* |
| `ccsds.vc_names` | vc = name | – | A name for each virtual channel, shown wherever its frames are. *A project can set it.* |
| `ccsds.sources` | [[tables]] of name, address, subject, framing, header | – | Where live telemetry comes from: tcp://host:port, udp://:port, nats://host:port with a subject, or file://path of a recording being written. Framing, picked: guess, packets, frames (frames fecf or frames no-fecf when a link differs from the frame settings), cltus, or records with the record header's size in octets. Fenix only receives. *A project can set it.* |
| `ccsds.checks_off` | a list | – | Standards checks not to run on the MIB: apid, size, overlap, width, identification, pus, checksum, calibration, time. *A project can set it.* |
| `ccsds.library` | a path | – | The folder your CCSDS and ECSS standards' PDFs are in; SPC k ? lists them and a field's gd opens its heading. |
| `ccsds.leap_seconds` | a path | built in | A file of `YYYY-MM-DD N` lines (TAI - UTC from that date) to use instead of the table Fenix has. |
| **VNC** | | | |
| `vnc.hosts` | [[tables]] of name, host, port | – | Machines SPC v connects to. No passwords: every host is taken to be on a trusted network. |
| **Notebook & diagrams** | | | |
| `notebook.folder` | a path | Fenix's data folder | Where your notes, journal and diagrams are kept. Point it at a synced folder or an Obsidian vault to use that instead. |
| `notebook.history` | 1–500 | 50 | How many earlier versions of each note and diagram are kept; h on the notebook page lists them. |
| `notebook.journal` | true / false | on | Keep a note per day (SPC n j). Captures go to today's; turned off, they go to the Inbox note. |
| `notebook.show_project_files` | true / false | off | Also list the project's own .md and .mmd files at the bottom of the notebook page. |
| `diagrams.theme` | text | fenix | The theme a diagram is drawn in when it doesn't name one: fenix (follows the editor theme), default, neutral, dark, forest, base, or one of yours below. |
| `diagrams.export_theme` | text | same | The theme exports are drawn in: same (as shown), or a theme's name. |
| `diagrams.background` | theme / transparent / white | theme | What's behind an exported diagram. |
| `diagrams.themes` | [[tables]] of name, base, primaryColor, primaryBorderColor, primaryTextColor, lineColor, secondaryColor, background | – | Themes of your own: Mermaid theme variables over a theme to start from. Colours as #rrggbb; empty ones keep the starting theme's. |
| `diagrams.export_with` | fenix / mmdc | fenix | What draws exported files: Fenix, or mermaid-cli (mmdc, when it's installed) for output identical to mermaid.js. |
| `diagrams.font` | text | the renderer's | The font diagrams are drawn with. |
| **Workspaces** | | | |
| `workspaces` | name = opens | – | SPC TAB f: git, jira, docker, vnc:HOST, project:PATH, or nothing. |
| **Windows & session** | | | |
| `session.restore_windows` | true / false | on | Put Fenix's windows back where they were, on the monitors they were on. *Needs a restart.* |
| `session.restore_session` | true / false | on | Reopen the workspaces and files you had open. *Needs a restart.* |
| `session.workspace_per_project` | true / false | on | Opening a project gives it a workspace of its own. |
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 71 filtered out; finished in 0.00s
.
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 71 filtered out; finished in 0.00s

**Shelves, not autostart.** VNC hosts, documents and the workspace
shelf make things selectable (`SPC v v`, `SPC r f`, `SPC TAB f`); none of
them opens or connects to anything at launch. The one thing Fenix does
by itself on startup is put its windows back (`session.restore_windows`)
and reopen your workspaces (`session.restore_session`).
