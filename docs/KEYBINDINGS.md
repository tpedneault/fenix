# Keybindings

Fenix follows real Vim for editing and a Doom-Emacs-style `SPC` leader
for everything else. `SPC` starts a leader sequence from Normal mode; a
popup shows what keys continue it.

## Leader (`SPC ...`)

| Keys | Action |
|---|---|
| `SPC SPC` | Find file in project (same as `SPC p f`) |
| `SPC f s` | Save |
| `:w!` / `:e!` | Save over a file that changed on disk / re-read it, discarding your edits |
| `SPC f v` | Recover unsaved work a previous session left behind |
| `SPC f j` | Open the file explorer at the current file's directory |
| `SPC f e` | Open the file explorer at your home directory; open a file directly, or `S` fuzzy-searches recursively from wherever you navigate to |
| `SPC f t` | Toggle the focused buffer between plain text and table view |
| `SPC f f` | Open a file by typing its path (bypasses `.gitignore`) |
| `SPC f n` | Create a new Arduino sketch |
| `SPC f a` | Fuzzy-find a file in the project, including gitignored ones |
| `SPC f r` | Fuzzy-find a recently-opened file |
| `SPC f R` | Rename the current file on disk |
| `SPC f D` | Delete the current file (with confirmation) |
| `SPC f y` | Copy the current file's path to the clipboard |
| `SPC q q` | Quit |
| `SPC t n` | Cycle line numbers (off / absolute / relative) |
| `SPC t p` | Pick a theme by name (fuzzy picker) |
| `SPC t =` / `SPC t -` / `SPC t 0` | Font size: increase / decrease / reset |
| `SPC t f` | Toggle fullscreen |
| `SPC t a` | Toggle caret-fade/scroll-ease/yank-pulse animations on/off |
| `SPC e e` | Open the file explorer here |
| `SPC e d` | Two listings side by side (copy/move default to the other one) |
| `SPC e o` / `SPC e O` | Open with the system's default app / show it in Explorer |
| `SPC e y` | Copy the full path |
| `SPC e T` | Open a shell in this directory |
| `SPC e g` | Search this directory |
| `SPC e G` | Make this the project and open the Git panel |
| `SPC e k` | Stop the running file operation |
| `SPC e w` | Edit the listing's names as text |
| `SPC e W` | Apply the edited names |
| `SPC e p` | Go to a path you type (Tab completes; `~`, `%VAR%` and `\server\share` all work) |
| `SPC e b` | Places: bookmarks, drives, recent directories, project roots |
| `SPC e r` | Recent directories |
| `SPC e m` | Bookmark the directory you are in |
| `SPC e t` | Toggle the file explorer sidebar |
| `SPC p f` | Find file in project |
| `SPC p s` | Search project (ripgrep) |
| `SPC p n` / `SPC p N` | Next / previous match in the last project search (quickfix) |
| `SPC p p` | Project hub |
| `SPC p P` | Quick switch project (path picker) |
| `SPC p c` | New project from a template |
| `SPC p h` | Project doctor |
| `SPC p ,` | Project settings |
| `SPC p a` / `SPC p d` | Add / remove a project from the known-projects list |
| `SPC p t` | Fuzzy-pick and run a discovered project task in the Task Output panel |
| `SPC p T` | Rerun the most recently run task |
| `SPC p k` | End the currently running task |
| `SPC u u` | Start a debug session, or continue one that's stopped |
| `SPC u b` | Toggle a breakpoint on the focused buffer's current line |
| `SPC u n` / `SPC u i` / `SPC u o` | Step over / into / out |
| `SPC u w` | Watch the identifier before the cursor |
| `SPC u q` | End the running debug session |
| `SPC l m` | Show LSP/DAP tool status (found on PATH, running, install hints) |
| `SPC s s` | Fuzzy-find a line in the current buffer |
| `SPC s t` | List the TODO/FIXME/NOTE-style comments in the current buffer |
| `SPC s T` | List the TODO-style comments across the project (also becomes the quickfix list) |
| `SPC s r` | Search and replace in the current buffer (Visual-scoped if invoked from Visual mode) |
| `SPC s p` | Search and replace across the project |
| `SPC o d` | Go to this workspace's Home, the start-up dashboard (same as `gh`) |
| `SPC o t` | Toggle the terminal panel |
| `SPC o T` | Open a shell in the focused pane |
| `SPC d d` | Open (or refocus/refresh) the Docker panel |
| `SPC d b` | Build an image from the current project's `Dockerfile` |
| `SPC d q` | Close the Docker panel session |
| `SPC ,` | Every setting, on one page -- search, change, reset |
| `SPC p ,` | This project's settings: overrides, kind, group, Jira key, tasks, launch |
| `SPC i S` / `SPC i n` | Manage snippets / make one from the last Visual selection |
| `SPC g g` | Open the Git status page (or the panel, with `[git] layout = panes`) |
| `SPC g l` | The Log page -- history with a menu on every commit (`a` for every branch) |
| `SPC g h` / `SPC g H` | This file's history / the history of the selected lines |
| `SPC g G` | Open the graph view (commit graph, refs, commit diff) |
| `SPC g c` | Compare two refs (pick base, then head) |
| `SPC g z` | The operation log -- undo what Fenix ran on the repository |
| `SPC g w` / `SPC g f` / `SPC g p` | Switch branch (most recently used first) / fetch all remotes and prune / pull with `--rebase` |
| `SPC g r` / `SPC g m` | Rebase onto / merge in a ref you pick |
| `SPC g P` | Open a pull request for this branch, prefilled from its commits |
| `SPC g M` | The review inbox (the older Merge Requests view with `[git] layout = panes`) |
| `]h` / `[h` | Next / previous changed hunk in the file |
| `SPC g a` / `SPC g d` / `SPC g i` | Stage / discard / preview the hunk under the cursor |
| `SPC g B` / `SPC g e` | Blame beside the text / the commit behind this line |
| `SPC g x x` | Resolve conflicts side by side (the Merge view) |
| `SPC g x j` / `SPC g x k` | Next / previous conflict in the focused file |
| `SPC g x o` / `t` / `b` | Keep ours / theirs / both for the conflict under the cursor |
| `SPC g x s` | Stage the conflicted file as resolved |
| `SPC g x c` / `SPC g x a` | Continue / abort the suspended rebase, merge, cherry-pick or revert |
| `SPC g q` | Close the Git view in front -- the panel, graph, comparison, conflicts or merge requests view, or a Git page |
| `o` / `M` (Git page) | This branch's pull request -- review it, or open one / the review inbox |
| `C-c C-c` / `Enter` / `e` / `d` (New pull request) | Open it (press twice) / edit a field / write the description / toggle draft |
| `1` / `2` (Merge Requests) | Jump to the list / detail pane |
| `Enter` / `f` / `c` / `u` (Merge Requests) | Show this one / cycle filter / check it out locally / refresh |
| `1` / `2` / `3` (Merge Requests) | Jump to the list / detail / review pane |
| `r` / `R` / `C` (Review pane) | Reply to this thread / resolve or reopen it / comment on this line |
| `A` / `m` (Merge Requests) | Approve or withdraw / merge (press twice) |
| `Enter` / `q` (Compose) | Send what's written / discard it (also used for commit messages) |
| `1` / `2` (Merge) | Jump to the Conflicts / Merge pane |
| `Enter` / `o` / `t` (Merge files) | Resolve line by line / take the whole file from the left / from the right |
| `n` / `p` / `u` (Merge) | Next / previous conflict / put the conflict back |
| `1` / `2` / `3` (History) | Jump to the Graph / Refs / Commit pane |
| `u` / `f` (History) | Refresh / fetch |
| `1` / `2` (Compare) | Jump to the Commits / Changes pane |
| `t` / `r` / `u` (Compare) | Toggle three-dot vs two-dot / re-target refs / refresh |
| `x` (any Git view) | Show the keys the focused pane understands |
| `s` / `S` (working-tree diff) | Stage / unstage the hunk under the cursor |
| `d` (working-tree diff) | Discard the hunk under the cursor (confirms first) |
| `]` / `[` (any diff) | Next / previous hunk |
| `Tab` (any diff) | Fold the file under the cursor down to its header |
| `Enter` (any diff) | Open the real file at the line under the cursor |
| `SPC a a` | Open the agenda page where it was left |
| `SPC a b` / `SPC a l` / `SPC a r` | Open the agenda on its board / list / this week's time (`SPC a k` still opens the board) |
| `SPC a n` | Add a task |
| `SPC a h` | Add a task about the selection or line under the cursor |
| `SPC a /` | Search every task |
| `SPC a t` / `SPC a T` | Clock: stop, resume or switch / resume the last task |
| `SPC a i` / `SPC a s` / `SPC a w` | Bring in Jira issues / sync with Jira / review and send worklogs |
| `SPC j j` | Open the Jira page |
| `SPC j /` | Search Jira (words, or JQL after `:`) |
| `SPC j g` | Open an issue by key |
| `SPC j n` | Create an issue |
| `SPC j r` | Run the Jira page's search again |
| `SPC v v` | Open (or switch to) a configured VNC session by name |
| `SPC v q` | Close the focused VNC session |
| `SPC v s` | Save the focused VNC session's current frame as a PNG |
| `SPC r n` | Turn the focused PDF session to the next page |
| `SPC r p` | Turn the focused PDF session to the previous page |
| `SPC r g` | Prompt for a page number and jump to it |
| `SPC r [` / `SPC r ]` | Jump to the first / last page |
| `SPC r =` / `SPC r -` | Zoom the focused PDF session in / out |
| `SPC r f` | Open a document from the document index (`SPC ,` > Documents) |
| `SPC r 0` | Fit the page to the pane |
| `SPC r w` | Fit the page's width to the pane |
| `SPC r o` | Toggle the focused PDF session's outline/bookmarks panel |
| `SPC r /` | Search the focused PDF session's text for a word or phrase |
| wheel, `j` / `k`, `Down` / `Up` | Scroll the document, continuing onto the next/previous page at an edge (PDF panes only) |
| `PageDown` / `PageUp`, `n` / `p` | Next / previous page (PDF panes only) |
| `Home` / `End`, `g` / `G` | First / last page (PDF panes only) |
| `h` / `l`, `Left` / `Right` | Pan sideways while the page is wider than the pane (PDF panes only) |
| `+` / `-` / `0` / `w` / `/` | Zoom in / out, fit page, fit width, search (PDF panes only) |
| `gd` | Go to definition (LSP) |
| `gr` | Find references (LSP) -- populates the quickfix list, `SPC p n` / `SPC p N` to step through |
| `K` | Show hover information for the symbol under the cursor (LSP) |
| `SPC c r` | Rename the symbol under the cursor across the project (LSP) |
| `SPC c a` | Choose a code action and preview its edits (LSP) |
| `SPC c f` | Indent region -- reindent the active Visual selection structurally, or (with an attached language server) reformat the whole document (LSP) |
| `SPC c F` | Indent region -- reindent the whole focused buffer structurally, or (with an attached language server) reformat it (LSP) |
| `SPC c x` | Toggle the GFM task checkbox (`- [ ]`/`- [x]`) on the current line |
| `SPC c o` | Fuzzy-find a Markdown heading, or an XML element, and jump to it |
| `SPC c v` | Check the current XML buffer is well-formed; jump to the first error |
| `SPC c y` | Copy the XPath of the XML element under the cursor |
| `SPC m` | The mode menu -- what's in it depends on the focused buffer (below) |
| `SPC m i` (Tcl) | Build and insert a telecommand from the MIB |
| `SPC m t` (Tcl) | Fuzzy-find a MIB telecommand and view its details |
| `SPC m k` (Tcl) | Fuzzy-find a MIB TM packet and view its details |
| `SPC m p` (Tcl) | Fuzzy-find a MIB TM parameter and view its details |
| `SPC m c` (Tcl) | Fuzzy-find a MIB calibration definition and view its details |
| `SPC m r` (Tcl) | Reparse the configured MIB directories from disk |
| `SPC m a` (Tcl) | Browse to and register a new MIB root directory |
| `SPC m d` (Tcl) | Fuzzy-find and remove a configured MIB root |
| `SPC m s` (Tcl) | Fuzzy-find a Tcl symbol by its fully-qualified name and jump to its definition |
| `SPC m T` (Tcl) | Refresh completion tags (re-scans with ctags, re-reads the symbols file) |
| `SPC m b` (Arduino) | Build (verify) the sketch |
| `SPC m u` (Arduino) | Build and upload to the board; the serial monitor steps aside and comes back |
| `SPC m m` (Arduino) | Open the serial monitor |
| `SPC m B` (Arduino) | Choose the serial monitor's speed |
| `SPC m p` (Arduino) | Choose the port |
| `SPC m s` (Arduino) | Choose the board |
| `SPC m o` (Arduino) | Choose the board's options (processor, clock, ...) |
| `SPC m l` (Arduino) | Install a library from the Arduino library index |
| `SPC m c` (Arduino) | Install a board package |
| `SPC m d` (Arduino) | Debug, on boards that support it |
| `SPC m n` (Arduino) | New sketch next to this one |
| `SPC m i` (Arduino) | Show the board, port, speed and whether completion is ready |
| `SPC w v` / `SPC w s` | Split window vertically / horizontally |
| `SPC w h/j/k/l` | Move focus between windows -- and across OS windows, by where they sit on the desktop |
| `SPC w w` | Cycle to the next window |
| `SPC w q` / `SPC w o` / `SPC w =` | Close window / close all others / balance splits |
| `SPC w n` | Open another OS window, on the next monitor with no Fenix window on it |
| `SPC w W` | Cycle to the next OS window |
| `SPC w Q` / `SPC w O` | Close this OS window / close all the others |
| `SPC b b` | Switch buffer |
| `SPC b n` / `SPC b p` | Next / previous buffer |
| `SPC b k` | Kill (close) the focused buffer -- its tab goes from every pane |
| `gt` / `gT` | Next / previous tab in the focused pane, wrapping round through Home |
| `{n}gt` | Tab *n* (Home isn't numbered, so `1gt` is the first file) |
| `g<Tab>` / `gh` | The tab you were on before / this workspace's Home |
| `Ctrl-PgDn` / `Ctrl-PgUp` / `Ctrl-Tab` | Next / previous / last tab, in any mode |
| `SPC b d` | Close the tab (the buffer stays open); the tab to its right takes its place, then the one to its left, then Home |
| `SPC b o` | Close every other tab in the pane |
| `SPC b u` | Reopen the tab this pane closed last, where it was |
| `SPC b P` | Keep the preview tab -- the italic one a jump (`gd`, a search result, a symbol, `Ctrl-O`) reuses; editing it or saving it keeps it too |
| `SPC b <` / `SPC b >` | Move the tab one place left / right (the mouse: drag it; a middle click closes it) |
| `SPC b X` | New scratch buffer |
| `SPC TAB n` | New workspace |
| `SPC TAB ]` / `SPC TAB [` | Next / previous workspace |
| `SPC TAB d` | Remove the active workspace |
| `SPC TAB TAB` | Switch to an open workspace by name |
| `SPC TAB f` | Open a workspace from the workspace shelf (`SPC ,` > Documents & workspaces) -- switches to it if already open, otherwise creates it |
| `SPC TAB r` | Rename the active workspace |

## File explorer sidebar (`SPC e t`)

| Keys | Action |
|---|---|
| `j` / `k` | Move down / up |
| `l` / `Enter` | Open |
| `h` / `-` | Go to parent directory |
| `Tab` | Expand / collapse a directory |
| `m` / `u` / `U` / `t` | Mark / unmark / unmark all / toggle all marks |
| `D` | Delete (marked, or entry under cursor) |
| `R` | Rename |
| `c` / `+` | Create file / directory |
| `C` / `M` | Copy / move to... |
| `.` | Toggle hidden files |
| `o` / `O` | Cycle sort key (name/size/date/type) / reverse it |
| `f` / `F` | Filter the listing / find by name under here |
| `r` / `g r` | Refresh |
| `S` | Select this directory (when picking a project root) |
| `q` / `Esc` | Quit |

## File explorer buffer (`SPC f j`)

A real buffer, so every ordinary Vim motion works (`j k gg G / n N ...`)
and the cursor is the selection. The keys below are the same table the
sidebar reads -- only `j`/`k` differ, because here they are the cursor.

| Keys | Action |
|---|---|
| `Enter` / `l` | Open the file, or navigate into the directory, at point |
| `-` | Go to the parent directory |
| `Tab` | Expand / collapse a directory in place |
| `m` / `u` / `U` / `t` | Mark / unmark / unmark all / toggle all marks |
| `D` | Delete to the Recycle Bin (marked, or entry under cursor) |
| `R` | Rename |
| `c` / `+` | Create file / directory (either may include `/`, and missing parents are created) |
| `C` / `M` | Copy / move to... |
| `.` | Toggle hidden files |
| `o` / `O` | Cycle sort key (name/size/date/type) / reverse it |
| `z` / `x` | Pack the marked set into an archive / unpack the one at point |
| `i` | Properties (and, on a folder, count what is inside) |
| `w` | Toggle read-only |
| `f` | Filter the listing as you type (`Esc` widens it back out) |
| `F` | Find by name through everything under here |
| `r` | Refresh |
| `Esc` | Stop waiting for a directory that isn't answering |

Operations act on the marked set if there is one, and on the row under
the cursor otherwise -- dired's own convention.

## Table view (`SPC f t`)

Toggles the focused buffer in place -- same file, same undo history,
just rendered with elastic-column alignment instead of plain text.
Every ordinary Vim motion and edit works (`j`/`k` and `c` are
reinterpreted, everything else is unchanged):

| Keys | Action |
|---|---|
| `]` / `[` | Jump to the start of the next / previous column |
| `j` / `k` | Move a row, staying in the same visual column |
| `c` | Fuzzy-find a column by name and jump to it |

## Search & replace review buffer (`SPC s p`)

A real, Vim-navigable buffer listing every file a pending project-wide
replace would touch, one row each (`j k gg G / n N ...` all work):

| Keys | Action |
|---|---|
| `Space` / `t` | Toggle the file under the cursor in/out of the replace |
| `a` / `Enter` | Arm the apply confirmation (`y`/`n`), or apply if already armed |
| `q` / `Esc` | Cancel -- closes the buffer, writes nothing |

## Docker panel (`SPC d d`)

Opens its own workspace with six real, titled panes -- Containers,
Images, Volumes, and Networks stacked on the left, Status and Logs
stacked on the right. Each is an ordinary Vim-navigable buffer (`j k gg
G / n N ...` all work); moving the cursor in a left pane live-updates
Status with that row's info. A Containers row is just a color-coded
status badge plus the container's name (`[R]` green for running, `[P]`
yellow for paused, `[X]` red for exited, etc.) -- press `x` on a pane for
a which-key-style popup of its available keys instead. Each title bar is
numbered (`1. Containers`, `2. Images`, ...) and the focused one is
shown in an accent color -- pressing that digit jumps straight to it.
Only these are special, and only on the pane named:

| Keys | Pane | Action |
|---|---|---|
| `1`-`6` | any | Jump to the pane numbered that in its title bar |
| `s` | Containers | Start the container under the cursor |
| `S` | Containers | Stop the container under the cursor |
| `R` | Containers | Restart the container under the cursor |
| `l` | Containers | Stream that container's logs live into the Logs pane (`docker logs -f`) |
| `r` | Images | Run a new detached container from the image under the cursor |
| `d` | Containers, Images, Networks | Remove the entry under the cursor (`y`/`n` to confirm) |
| `u` | any | Refresh the whole session |
| `x` | Containers, Images, Volumes, Networks | Show this pane's available keys |

## Git panel (`SPC g g` with `[git] layout = panes`)

Opens its own workspace with six real, titled panes -- Status, Files,
Branches, Commits, and Stash stacked on the left, Main on the right.
Each is an ordinary Vim-navigable buffer (`j k gg G / n N ...` all
work). Moving the cursor in Files, Commits, or Stash re-syncs Main to
that row's diff; Status doesn't follow the cursor -- it's a fixed
repo-overview summary that live-updates on its own every ~2s
regardless of where the cursor is. Each title bar is numbered (`1.
Status`, `2. Files`, ...) and the focused one is shown in an accent
color -- pressing that digit jumps straight to it. Only these are
special, and only on the pane named:

Files is a collapsible directory tree, not a flat list -- changed
paths are grouped by directory (`> src/` collapsed, `v src/` expanded),
so `Tab` on a directory reveals or hides its files, and every action
key (`s`/`S`/`d`) works on a directory the same way it works on a
single file: stage, unstage, or discard *everything underneath it* in
one keypress. Discarding a directory runs both `git checkout --` (for
tracked changes) and `git clean -fd --` (for untracked files) under it,
since a real directory routinely holds a mix of both at once. Every
directory starts collapsed; expansion state persists across `u`
refreshes within the session.

| Keys | Pane | Action |
|---|---|---|
| `1`-`6` | any | Jump to the pane numbered that in its title bar |
| `Tab` | Files | Expand/collapse the directory under the cursor |
| `s` | Files | Stage the file (or every file under the directory) under the cursor |
| `S` | Files | Unstage the file (or directory) under the cursor |
| `a` | Files | Stage every changed file |
| `A` | Files | Unstage every staged file |
| `c` | Files | Commit (prompts for a message) |
| `d` | Files | Discard the file (or directory) under the cursor (`y`/`n` to confirm) |
| `z` | Files | Stash every change |
| `P` / `p` | Files | Push / pull |
| `c` | Branches | Checkout the branch under the cursor |
| `n` | Branches | New branch (prompts for a name) |
| `d` | Branches | Delete the branch under the cursor (`y`/`n` to confirm) |
| `a` | Stash | Apply the entry under the cursor |
| `g` | Stash | Pop the entry under the cursor |
| `d` | Stash | Drop the entry under the cursor (`y`/`n` to confirm) |
| `u` | any | Refresh the whole session |
| `x` | Files, Branches, Commits, Stash | Show this pane's available keys |

Real lazygit's own `<space>` stage-toggle isn't used here, since `SPC`
is already Fenix's global leader-key trigger -- Files uses separate
`s`/`S` keys instead, the same distinct-keys-per-action convention the
Docker panel's own `s`/`S`/`R` already established.

## Agenda (`SPC a a`) and Jira (`SPC j j`)

Both are pages: a key strip at the bottom follows the row under the
cursor, choices open as a menu beside it, and `?` lists every key. A
task and an issue answer the same letters.

| Keys | On a task (row, card or its page) and on an issue |
|---|---|
| `s` / `p` / `u` | Status (a linked task or an issue: its real transitions) / priority / due date |
| `c` | Category (tasks; categories double as projects) |
| `e` / `E` | Title / description (a compose pane) |
| `N` / `C` | Private note (tasks) / Jira comment |
| `t` / `T` | Clock / log time (`1h 30m`, or `9:15-10:40`) |
| `I` / `A` | Link to an issue or unlink / assign |
| `a` | On an issue: add it to the agenda (`t` also starts the clock) |
| `y` / `o` / `r` | Copy the link / open it in the browser / retry a refused change |
| `x` / `D` `D` | Archive / delete (a task; the issue in Jira is never touched) |

| Keys | Agenda page |
|---|---|
| `1`-`4`, `Tab` | Today, Board, List, Time |
| `Enter` / `Esc` | A task's page / back |
| `n` | New task: `!high`, `#category`, `due:fri` and a Jira key work in the title |
| `/` / `f` / `F` / `P` | Search / filters / clear them / only the open project |
| `H` `L` / `J` `K` | Board: move the card / reorder |
| `a` / `d` `d` / `h` `l` | Task page: add to the section / delete the row / change a field |
| `[` `]` / `W` / `y` | Time: week before and after / review and send worklogs / copy the week |
| `R` | Sync with Jira |

| Keys | Jira page |
|---|---|
| `h` `l` / `j` `k` | Searches and issues / move |
| `Enter` / `Esc` | An issue's page (or its task, when it's in the agenda) / back |
| `a` / `d` `d` / `e` / `S` | Searches: add a project, person or saved search / remove / edit / save a typed one |
| `b` | On a project's search: what Blocked means in it |
| `n` | New issue, with the project's issue types and required fields |
| `/` / `f` | Filter the issues / hide statuses |
| `R` | Run the search again |

Tracked projects, people and saved searches are the `jira.projects`,
`jira.users` and `jira.queries` settings, so the settings page edits them
too. The agenda's categories are `agenda.categories`; `c` on a task
offers "+ new category".

## VNC console panes (`SPC v v`)

Embeds a live VNC (RFB) connection to a VM as an ordinary, splittable
pane -- configure hosts once under `[vnc]` (see Configuration below),
then `SPC v v` fuzzy-picks one by name to open or switch straight to
it. Each session connects the first time you pick it and then stays
live in the background indefinitely, so switching back later is
instant, not a fresh handshake; an unfocused/hidden session polls at a
much slower rate to stay cheap while you're not looking at it, and a
dropped connection retries automatically with backoff before giving up
and leaving the pane on its last frame.

| Keys | Action |
|---|---|
| `SPC v v` | Open or switch to a configured VNC session (picker by name) |
| `SPC v q` | Close the focused VNC session |
| `SPC v s` | Save the focused session's current frame as a timestamped PNG |
| `Ctrl-\` | Release keyboard capture back to the editor (same chord as the terminal panel) |

Clicking into a VNC pane both focuses it and starts sending your mouse
there; every other key while it's focused is forwarded to the VM
instead of Vim, exactly like the terminal panel. Clipboard content is
mirrored in both directions: the VM's clipboard always flows to yours
as it changes, and yours flows to the VM whenever you focus a session.

Resizing a VNC pane asks the VM to match its own resolution to the
pane's, the same "remote resizing" real VNC clients offer, so the
picture renders at native size instead of a stretched/letterboxed
scale. Whether that request actually lands depends on the server: it
has to advertise support for it in the first place (confirmed
automatically per-connection, nothing to configure), and even then can
still decline a specific size (administrative policy, an unsupported
resolution, ...). Either way, the video keeps scaling to fit the pane
as a fallback -- a pane and a VM sitting at different resolutions is
never broken, just not pixel-native.

The connection is always made in the clear -- there's no encryption or
authentication support at all, matching the assumption that every
configured host is on a trusted local network. Don't point this at
anything reachable over an untrusted network without your own tunnel
(SSH port-forwarding, a VPN) in front of it.

## PDF viewer (`SPC r ...`)

Opening a `.pdf` -- by typed path (`SPC f f`), the explorer, a recent
file, or a CLI argument -- renders it as a scaled-to-fit page in an
ordinary, splittable pane instead of loading its raw bytes as text.
Rendering happens on one shared background worker (every open PDF
shares it), so opening a document never blocks the editor and several
can be open at once.

Reading is done with bare single keystrokes while the PDF pane is
focused -- a three-key leader chord per page is not a page-turn gesture
anyone would use to read a 50-page document. The `SPC r ...` bindings all
still work (and are what the which-key menu discovers); they're the same
commands, just reachable in one keystroke here.

| Keys | Action |
|---|---|
| mouse wheel, `j` / `k`, `Down` / `Up` | Scroll the page; at the bottom/top edge, continue onto the next/previous page |
| `PageDown` / `PageUp`, `n` / `p`, `SPC r n` / `SPC r p` | Next / previous page |
| `Home` / `End`, `g` / `G`, `SPC r [` / `SPC r ]` | First / last page |
| `SPC r g` | Prompt for a page number and jump to it |
| `+` / `-`, `SPC r =` / `SPC r -` | Zoom in / out, in coarse 10% steps |
| `0`, `SPC r 0` | Fit the whole page to the pane (the default) |
| `w`, `SPC r w` | Fit the page's width to the pane -- a tall page then scrolls vertically instead of shrinking further |
| `h` / `l`, `Left` / `Right` | Pan sideways once the page is wider than the pane |
| `SPC r o` | Toggle the outline/bookmarks panel |
| `/`, `SPC r /` | Search the document's text |

`SPC r f` opens a fuzzy picker over a **document index** you keep in
`SPC ,` (Documents & workspaces), or by hand in `settings.toml`:

```toml
[documents]
"Space Packet Protocol" = 'C:\refs\133x0b2e2.pdf'
"Time Code Formats" = 'C:\refs\301x0b4.pdf'
```

Each entry is a display name and a path. The picker lists and
fuzzy-matches the *names*, so a reference you open constantly is two
keystrokes and a few characters away rather than a path to go hunting
for. Confirming opens that document **in the focused pane**, replacing
whatever it was showing -- unlike every other way of opening a PDF
(`SPC f f`, the explorer, a CLI argument), which gives the document its
own workspace. Picking a reference off a shelf means "show it to me
here", and if the pane already held a different PDF, that one (and its
outline/search companion panes) is retired first. An entry can point at
any file Fenix opens, not just a PDF -- a Markdown or plain-text
reference opens as ordinary editable text. A path that has since moved
is reported by name instead of opening an empty buffer, and an empty or
missing `[documents]` section says so rather than opening a picker over
nothing.

Scrolling is continuous across page boundaries in both directions:
scrolling past the bottom of a page turns to the next one at its top,
and scrolling back up past the top turns to the previous one at its
*bottom*, so scrolling back retraces exactly what scrolling forward
covered. Under the default fit-page zoom there is never anything to
scroll within a page, so every scroll gesture simply turns the page.

The status line shows `Page N/M` and the current zoom (`Fit page`,
`Fit width`, or a percentage) where an ordinary buffer shows `Ln`/`Col`
-- a PDF pane has no text and no cursor, so a line/column there would be
meaningless.

The outline panel (`SPC r o`) opens as a split next to the PDF pane,
listing the document's bookmark tree flattened into indented lines (a
nested bookmark just gets deeper indentation -- there's no tree widget,
so this is the whole tree in one flat, ordinary buffer). It's real,
Vim-navigable text: move around it with `j`/`k`/`gg`/`G`/`/` like
anything else, and press `Enter` on an entry to jump the PDF straight to
its page. `SPC r o` again -- from either the outline pane or the PDF
pane -- closes it. The outline is fetched once per document (a PDF's
bookmarks can't change while it's open) and cached, so reopening it is
instant after the first time; a PDF with no bookmarks at all shows a
single explanatory line instead of an empty pane.

`SPC r /` prompts for a search query and, once it comes back, opens (or
reuses, if one's already showing for this document) a results pane
listing every match in page order as `p.NNN  <context>` -- one line per
occurrence, anywhere in the document, not just the current page. It's
the same kind of real, Vim-navigable buffer the outline panel is;
`Enter` on a result jumps the PDF pane straight to that match's page. A
query with no hits shows an explanatory placeholder line rather than an
empty pane, same as the outline's no-bookmarks case. Search runs fresh
against the document each time rather than keeping the whole document's
text extracted in memory between searches -- there's no results cache to
go stale, just a brief "searching..." status message while it works.

The page re-renders to fit whenever its pane is resized, *except* at a
fixed zoom percentage (`SPC r =`/`SPC r -`), which stays exactly where
you left it across a resize instead of silently re-fitting -- panning
with `hjkl` then just shows a different part of the same render, no
fresh page turn needed. Fit-page/fit-width do still re-render on
resize, since what "fits" depends on the pane's own size by definition.

Pages are rasterized straight to BGRA and uploaded to a GPU texture. The
crop that's uploaded is only recomputed when the visible window actually
changes (a page turn, a resize, a zoom, a pan), the texture behind it is
only recreated when that crop's *size* changes, and a render that
already fits the pane exactly -- the fit-page default -- is uploaded
without being copied through a crop buffer at all.

A PDF pane's buffer is always empty and pathless -- the rendered page
lives in a GPU texture, not the buffer's own text -- so `:w`/`SPC f s`
on one is a no-op, same as every other generated panel in Fenix;
there's no risk of a stray save overwriting the real PDF file on disk.
Needs `pdfium.dll` (see [Optional external tools](BUILDING.md#optional-external-tools))
-- without it, opening a PDF reports the error in the status line
rather than rendering.

## Autocompletion popup (Tcl, Insert mode)

| Keys | Action |
|---|---|
| `Ctrl-Space` | Force-open the popup (even with no prefix typed) |
| `Up`/`Down` or `Ctrl-P`/`Ctrl-N` | Move selection |
| `Tab` / `Enter` / `Ctrl-Y` | Accept the selected candidate (stays in Insert mode) |
| `Ctrl-E` | Dismiss the popup, keep typing |
| `Esc` | Leave Insert mode, as it always does -- the popup closes with it |

`Esc` is deliberately not spent on the popup. It used to be, which meant
returning to Normal mode took two presses, and which one you needed
depended on whether a popup you may not have been looking at happened to
be open. That is the modeless-editor reflex: where `Esc` has nothing to
do *but* dismiss things, spending it on a popup costs nothing. Here it
costs the one key the whole grammar rests on. Vim itself agrees -- `:h
popupmenu-keys` lists every key with a special meaning while the menu is
up, and `Esc` is not among them; `Ctrl-E` is the dismiss key and
`Ctrl-Y` the accept key.
