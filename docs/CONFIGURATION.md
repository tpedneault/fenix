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
| `editor.font_family` | text | the system's monospace font | A monospace font installed on this machine; h and l go through them. |
| `editor.font_size` | 6–48 | 16 | Text size, in points. |
| `editor.animations` | true / false | on | Smooth scrolling and the caret's fade. |
| `editor.preview_tab` | true / false | on | A jump (gd, a search result, a symbol) opens in one reusable tab, in italics, until you edit it or keep it with SPC b P. |
| **Files & explorer** | | | |
| `editor.watch_files` | true / false | on | Notice when an open file changes on disk, and reload it when you haven't edited it. |
| **Completion & LSP** | | | |
| `completion.symbols_file` | a path | – | A text file of words, one per line, offered by completion everywhere. |
| `snippets.builtin` | true / false | on | Offer the snippets that come with Fenix; yours and a project's always are. SPC i S manages them. |
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
| **Embedded & MIB** | | | |
| `embedded.arduino_cli` | a path | found on PATH | Where arduino-cli is, when it isn't found by itself. |
| `embedded.clangd` | a path | found on PATH | Where clangd is, when it isn't found by itself. |
| `embedded.arduino_language_server` | a path | downloaded when needed | Where arduino-language-server is, when it isn't found by itself. |
| `mib.roots` | name = folder | – | Folders holding a MIB database. |
| `mib.telecommand_template` | text | – | How a telecommand is written; {name} and {args} are filled in. |
| `mib.telecommand_argument_template` | text | – | How each argument is written; {name} and {value} are filled in. |
| `mib.telecommand_argument_separator` | text | – | What goes between arguments. |
| **VNC** | | | |
| `vnc.hosts` | [[tables]] of name, host, port | – | Machines SPC v connects to. No passwords: every host is taken to be on a trusted network. |
| **Documents & workspaces** | | | |
| `documents` | name = file | – | The SPC r f document index: a name and the file it opens. |
| `workspaces` | name = opens | – | SPC TAB f: git, jira, docker, vnc:HOST, project:PATH, or nothing. |
| **Windows & session** | | | |
| `session.restore_windows` | true / false | on | Put Fenix's windows back where they were, on the monitors they were on. *Needs a restart.* |
| `session.restore_session` | true / false | on | Reopen the workspaces and files you had open. *Needs a restart.* |
| `session.workspace_per_project` | true / false | on | Opening a project gives it a workspace of its own. |

**Shelves, not autostart.** VNC hosts, documents and the workspace
shelf make things selectable (`SPC v v`, `SPC r f`, `SPC TAB f`); none of
them opens or connects to anything at launch. The one thing Fenix does
by itself on startup is put its windows back (`session.restore_windows`)
and reopen your workspaces (`session.restore_session`).
