# Keybindings

Fenix follows real Vim for editing and a Doom-Emacs-style `SPC` leader
for everything else. `SPC` starts a leader sequence from Normal mode; a
popup shows what keys continue it.

`Ctrl+Alt+Q` does everything `Esc` does, for when Escape doesn't reach
Fenix (some remote-desktop and UI-automation tools don't send it).

## Leader (`SPC ...`)

| Keys | Action |
|---|---|
| `SPC SPC` | Find file in project (same as `SPC p f`) |
| `SPC f s` | Save |
| `:w!` / `:e!` | Save over a file that changed on disk / re-read it, discarding your edits |
| `:lsp-restart` / `:lsp-restart NAME` | Restart this project's language servers / just the one called NAME |
| `SPC f v` | Recover unsaved work a previous session left behind |
| `SPC f j` | Open the file explorer at the current file's directory |
| `SPC f e` | Open the file explorer at your home directory; open a file directly, or `S` fuzzy-searches recursively from wherever you navigate to |
| `SPC f t` | Toggle the focused buffer between plain text and table view |
| `SPC f o` | Switch between an extension's page for a file (a picture, a PDF) and the file's text |
| `SPC f f` | Open a file by typing its path (bypasses `.gitignore`) |
| `SPC f n` | Create a new Arduino sketch (the Arduino extension) |
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
| `SPC t a` | Cycle motion: off, subtle, full |
| `SPC t d` | Cycle inline problems: all, errors only, off |
| `SPC e e` | Open the file explorer here |
| `SPC e H` | Open the file explorer at your home folder |
| `SPC e h` | Show or hide hidden files and folders (dot-names, and on Windows those marked hidden), in every explorer; remembered |
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
| `SPC e r` | Show the file being edited in the sidebar, and focus it |
| `SPC e R` | Recent directories |
| `SPC e m` | Bookmark the directory you are in |
| `SPC e t` | Toggle the file explorer sidebar |
| `SPC p f` | Find file in project |
| `SPC p s` | Search the project, on the Search page (as `SPC /`) |
| `SPC p n` / `SPC p N` | Next / previous match in the last search (quickfix) |
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
| `SPC u p` / `SPC u r` / `SPC u c` | Pause / restart the session / run on to the cursor's line |
| `SPC u d` | Show or put away the debug drawer |
| `SPC u f` | Go into the debug drawer: `h` `l` columns, `j` `k` rows, Enter selects a frame or opens a variable, `x` removes, `a` watches, `i` evaluates, `t` `e` `H` `L` a breakpoint's on/off, condition, hit count and message, Esc back |
| `SPC u e` | Evaluate an expression in the selected frame (the answer goes to the drawer's console) |
| `SPC u B` / `SPC u h` / `SPC u l` | Give the line's breakpoint a condition / a hit count / a log message (a logpoint) |
| `SPC u t` | Turn the line's breakpoint off, or on again, keeping it |
| `SPC u x` | The exceptions to stop on, in the drawer (`t` turns one on or off) |
| `SPC u P` | Pin the value under the cursor to its line, or unpin it |
| `SPC u U` | Start debugging with a launch picked again (`SPC u u` reuses the project's last) |
| `K` while stopped | The lens: the value of the name under the cursor, and what's in it |
| `[t` / `]t` while debugging | Back / on along the Ember Trail, the code and its values as they were; Enter returns to now |
| F5 / Shift-F5 / Ctrl-Shift-F5 / F6 | Start or continue / stop / restart / pause, in every mode |
| F9 / F10 / F11 / Shift-F11 | Breakpoint / step over / in / out, in every mode |
| `n` `s` `o` `c` `r` in DEBUG | Step over / in / out, continue, run to the cursor; Esc goes back to Normal, still stopped |
| `SPC l m` | Show LSP/DAP tool status (found on PATH, running, install hints) |
| `SPC s s` | Search this file, or the selection, on the Search page (a buffer that isn't a file: find a line) |
| `SPC s t` | List the TODO/FIXME/NOTE-style comments in the current buffer |
| `SPC s T` | List the TODO-style comments across the project (also becomes the quickfix list) |
| `SPC /` | Search the project, live, on the Search page |
| `SPC s r` | Replace in this file, or the Visual selection, on the Search page |
| `SPC s p` | Replace across the project, on the Search page |
| `SPC s l` | Fuzzy-find a line in the current buffer |
| `SPC s .` | The last search, as it was left |
| `SPC s u` | Undo the last replace |
| `SPC s *` | Search the project for the word under the cursor |
| `SPC o d` | Go to this workspace's Home, the start-up dashboard (same as `gh`) |
| `SPC o t` | Toggle the terminal panel |
| `SPC o T` | Open a shell in the focused pane |
| `SPC d d` | Containers (the `fenix.containers` extension, when installed) |
| `SPC d p` / `SPC d b` | Its performance page / build the project's `Dockerfile` |
| `SPC ,` | Every setting, on one page -- search, change, reset |
| `SPC :` / `Alt-x` | Run any command by name (the command palette) |
| `SPC h T` | The tutorial: Fenix's keys, a lesson at a time, each in a practice buffer |
| `SPC h x` / `SPC h w` | The Extensions page / the welcome's packs |
| `SPC h k` / `SPC h c` | Say what a key runs / what a command does |
| `SPC h b` / `SPC h K` | The keys bound here / where keys disagree |
| `SPC h e` / `SPC h l` | Your keybindings (`keybindings.toml`) / what the code extensions said |
| `SPC p ,` | This project's settings: overrides, kind, group, Jira key, tasks, launch |
| `SPC i S` / `SPC i n` | Manage snippets / make one from the last Visual selection |
| `SPC g g` | Open the Git status page (or the panel, with `[git] layout = panes`) |
| `SPC g l` | The Log page -- history with a menu on every commit (`a` for every branch) |
| `SPC g h` / `SPC g H` | This file's history / the history of the selected lines |
| `SPC g G` | Open the graph view (commit graph, refs, commit diff) |
| `SPC g c` | The Compare page: HEAD (or the working tree) against a branch, local or remote |
| `SPC g v` | This file beside its version on a branch, lined up (side by side) |
| `]c` / `[c` (side by side) | Next / previous change |
| `do` / `dp` (side by side) | Take the branch's version of the change under the cursor -- from your pane / from the branch's |
| `q` (side by side, left pane) | Close it; your buffer stays |
| `SPC g z` | The operation log -- undo what Fenix ran on the repository |
| `SPC g w` / `SPC g f` / `SPC g p` | Switch branch (most recently used first) / fetch all remotes and prune / pull with `--rebase` |
| `SPC g r` / `SPC g m` | Rebase onto / merge in a ref you pick |
| `SPC g P` | Open a pull request for this branch, prefilled from its commits (the Review extension) |
| `SPC g M` | The reviews: the pull and merge requests waiting (the Review extension) |
| `]h` / `[h` | Next / previous changed hunk in the file |
| `]d` / `[d` | Next / previous problem: a language server's, or a telecommand call checked against the MIB |
| `SPC g a` / `SPC g d` / `SPC g i` | Stage / discard / preview the hunk under the cursor |
| `SPC g B` / `SPC g e` | Blame beside the text / the commit behind this line |
| `SPC g x x` | Resolve conflicts side by side (the Merge view) |
| `SPC g x j` / `SPC g x k` | Next / previous conflict in the focused file |
| `SPC g x o` / `t` / `b` | Keep ours / theirs / both for the conflict under the cursor |
| `SPC g x s` | Stage the conflicted file as resolved |
| `SPC g x c` / `SPC g x a` | Continue / abort the suspended rebase, merge, cherry-pick or revert |
| `SPC g q` | Close the Git view in front -- the panel, graph or conflicts view, or a Git page |
| `o` / `M` (Git page) | This branch's pull request -- review it, or open one / the reviews (the Review extension) |
| `Enter` / `q` (Compose) | Send what's written / discard it (also used for commit messages) |
| `1` / `2` (Merge) | Jump to the Conflicts / Merge pane |
| `Enter` / `o` / `t` (Merge files) | Resolve line by line / take the whole file from the left / from the right |
| `n` / `p` / `u` (Merge) | Next / previous conflict / put the conflict back |
| `1` / `2` / `3` (History) | Jump to the Graph / Refs / Commit pane |
| `u` / `f` (History) | Refresh / fetch |
| `Tab` / `Enter` / `o` / `v` (Compare) | Open a file's diff or a commit's files / go to the file at that line / the target's version, read-only / side by side |
| `t` / `w` / `s` (Compare) | Since splitting vs every difference / bring in the working tree / turn the comparison round |
| `r` / `R` / `f` / `y` (Compare) | Compare with another ref / pick your own side / fetch the target's remote / copy the range |
| `x` (any Git view) | Show the keys the focused pane understands |
| `s` / `S` (working-tree diff) | Stage / unstage the hunk under the cursor |
| `d` (working-tree diff) | Discard the hunk under the cursor (confirms first) |
| `]` / `[` (any diff) | Next / previous hunk |
| `Tab` (any diff) | Fold the file under the cursor down to its header |
| `Enter` (any diff) | Open the real file at the line under the cursor |
| `SPC a a` | The agents' dock at the right, on the project's last thread or a new one, ready to write in; from the dock, put it away |
| `SPC a A` | Show or put away the agents' dock, wherever you are |
| `SPC a n` / `SPC a t` | A new thread with the default agent (`SPC a p` for another) / this project's threads |
| `SPC a p` | The agents: a thread with one, a login, a restart |
| `SPC a m` / `SPC a M` | The agent's mode / its model and other settings |
| `SPC a k` / `SPC a R` | Cancel the agent's turn / start its agent again |
| `SPC a i` | Continue a session the agent started outside Fenix |
| `SPC a r` | What the thread changed, hunk by hunk: keep or reject |
| `SPC a y` / `SPC a x` / `SPC a v` | Keep / reject / see what it replaced: the agent's change under the cursor |
| `]a` / `[a` | The next / previous change an agent made and you haven't kept, across files |
| `SPC a e` | Ask the agent about the selection (it goes to the prompt as `@selection`) |
| `SPC a d` | Ask the agent to fix the problem on the cursor's line |
| `SPC a c` | A commit message for what's staged, written by the agent, onto the clipboard |
| `SPC a h` | Hand the thread off to another agent: the conversation goes with it |
| `SPC a D` | Delete the thread (asks first) |
| `SPC a f` | Follow the agent: the pane beside its thread shows where it works |
| `SPC h a` | What each agent connection sent and was sent (ACP) |
| `SPC A a` | Open the agenda page where it was left |
| `SPC A b` / `SPC A l` / `SPC A r` | Open the agenda on its board / list / this week's time |
| `SPC A n` | Add a task |
| `SPC A h` | Add a task about the selection or line under the cursor |
| `SPC A /` | Search every task |
| `SPC A t` / `SPC A T` | Clock: stop, resume or switch / resume the last task |
| `SPC A i` / `SPC A s` / `SPC A w` | Bring in Jira issues / sync with Jira / review and send worklogs |
| `SPC n n` | The notebook page |
| `SPC n N` / `SPC n d` | A new note (from a template) / a new diagram |
| `SPC n j` | Today's journal day |
| `SPC n c` | Capture a line into the journal or the Inbox |
| `SPC n f` / `SPC n r` | Find a note or diagram by name / reopen the last one |
| `SPC n /` / `SPC n #` / `SPC n p` | Search the notebook / tags / this project's notes |
| `SPC n b` | What links to this note, beside it |
| `SPC n l` / `SPC n y` | Insert a link to a note / copy a link to this line |
| `SPC n v` | Paste the clipboard's picture into the note |
| `SPC n t` | Send the checkbox under the cursor to the agenda |
| `SPC n s` / `SPC n i` / `SPC n e` | Save the ```` ```mermaid ```` block here / import a file / export |
| `SPC i D` | Insert a notebook diagram (its block, or an SVG and a link) |
| `SPC m p` / `SPC m r` | Markdown: the reading view beside / in place; a diagram: its preview |
| `SPC m v` / `SPC m t` / `SPC m e` | A diagram: view it on its own / set its theme / export |
| `SPC m a` / `SPC m g` | A journal day: refresh its agenda list / add the day's commits |
| `SPC j j` | Open the Jira page |
| `SPC j /` | Search Jira (words, or JQL after `:`) |
| `SPC j g` | Open an issue by key |
| `SPC j n` | Create an issue |
| `SPC j r` | Run the Jira page's search again |
| `SPC k k` | The MIB page: the project's telecommands, TC and TM parameters, TM packets, calibrations (the MIB extension) |
| `SPC k /` | Open any MIB definition by name or description (the MIB extension) |
| `SPC k t` / `p` / `m` / `n` / `c` | Open a telecommand / TC parameter / TM packet / TM parameter / calibration (the MIB extension) |
| `SPC k i` | Insert a telecommand, from a form with every argument (the MIB extension) |
| `SPC k e` | Edit the telecommand call on this line in the form (the MIB extension) |
| `SPC k r` | Read the project's MIBs again (the MIB extension) |
| `SPC k ,` | The project's MIB settings (the MIB extension) |
| `SPC k d` | Decode the hex under the cursor, in the packet inspector (the CCSDS extension) |
| `SPC k b` | Copy a telecommand's packet bytes (the MIB extension) |
| `SPC k T` | Time converter (the CCSDS extension) |
| `SPC k f` / `SPC k l` | Open a recording / a live source (the CCSDS extension) |
| `SPC k g` | Generate from the MIB (XTCE, Wireshark, C, Python, ICD, test vectors) (the MIB extension) |
| `SPC k ?` | The standards library (the CCSDS extension) |
| `SPC f x` | Show a file as hex (the CCSDS extension) |
| `SPC v v` | Open (or switch to) a configured VNC session by name (the VNC extension) |
| `SPC v q` | Close the focused VNC session (the VNC extension) |
| `SPC v s` | Save the focused VNC session's current frame as a PNG (the VNC extension) |
| `SPC r n` / `SPC r p` | Next / previous page of the PDF read last (the PDF reader extension) |
| `SPC r g` | Prompt for a page number and jump to it |
| `SPC r [` / `SPC r ]` | Jump to the first / last page |
| `SPC r =` / `SPC r -` | Zoom in / out |
| `SPC r f` | Open a document from the shelf (`reader.documents`) |
| `SPC r 0` / `SPC r w` | Fit the page / its width to the pane |
| `SPC r o` | Open or close the outline (outline, matches, marks) |
| `SPC r t` | Go to a heading of the PDF, picked by name |
| `SPC r c` | Draw PDF pages as printed or in the theme's colours |
| `SPC r y` | Copy a link to the PDF page being read (`file.pdf#page=38`) |
| `SPC r /` | Search the document's text |
| wheel, `j` / `k`, `{n}j` | Scroll the column of pages; `Ctrl` + wheel zooms (PDF panes) |
| `Ctrl-d` / `Ctrl-u`, `Ctrl-f` / `Ctrl-b`, `PageDown` / `PageUp` | Half a screen / a screen (PDF panes) |
| `J` / `K`, `{n}J` | Next / previous page (PDF panes) |
| `gg` / `G` / `{n}G` | First / last / page n (PDF panes) |
| `h` / `l` | Pan sideways (PDF panes) |
| `+` / `-` / `=` / `zw` / `zp` / `z0` | Zoom in / out, fit width or page in turn, fit width, fit page, 100% (PDF panes) |
| `o` / `/` / `n` / `N` | Outline, search, next / previous match (PDF panes) |
| `m{a}` / `'{a}`, `Ctrl-o` / `Ctrl-i` | Set / go to a mark, back / forward through jumps (PDF panes) |
| `yp` | Copy a link to this page (PDF panes) |
| `v`, drag | Select text; `w` `b` `e` `0` `$` `j` `k` extend it, `y` copies (PDF panes) |
| `f`, click | Label the links in sight and follow one by typing it; a click follows a link (PDF panes) |
| `gd` | Go to definition (LSP) |
| `gf` | Open the file named under the cursor, in the preview tab -- a `spec.pdf#page=38` link opens the PDF at that page |
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
| `SPC m` (PDF) | The reader's commands: pages, zoom, outline, search, the shelf |
| `SPC m i` (Tcl) | Insert a telecommand from the MIB (`SPC k i`) (the MIB extension) |
| `SPC m e` (Tcl) | Edit the telecommand call on this line (`SPC k e`) (the MIB extension) |
| `SPC m t` (Tcl) | Open a MIB telecommand (`SPC k t`) (the MIB extension) |
| `SPC m k` (Tcl) | Open a MIB TM packet (`SPC k m`) (the MIB extension) |
| `SPC m p` (Tcl) | Open a MIB TM parameter (`SPC k n`) (the MIB extension) |
| `SPC m c` (Tcl) | Open a MIB calibration (`SPC k c`) (the MIB extension) |
| `SPC m m` (Tcl) | The MIB page (`SPC k k`) (the MIB extension) |
| `SPC m r` (Tcl) | Read the project's MIBs again (`SPC k r`) (the MIB extension) |
| `SPC m s` (Tcl) | Fuzzy-find a Tcl symbol by its fully-qualified name and jump to its definition (the Tcl extension) |
| `SPC m T` (Tcl) | Refresh completion tags: re-scans with ctags (the Tcl extension) |
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
| `SPC TAB f` | Open a workspace from the workspace shelf (`SPC ,` > Workspaces) -- switches to it if already open, otherwise creates it |
| `SPC TAB r` | Rename the active workspace |

## File explorer (`SPC e t`, `SPC f j`, the directory picker)

The sidebar, a listing in a pane and the directory picker are one page
reading one key table. `?` in any of them shows every key.

| Keys | Action |
|---|---|
| `j` / `k`, `↓` / `↑` | Move down / up |
| `g g` / `G` | First / last row |
| `Enter` / `l` | Open the file, or go into the folder, at point |
| `h` / `-` | Go to the folder above |
| `Tab` | Open / close a folder in place |
| `m` / `u` / `U` / `t` | Mark / unmark / unmark all / toggle all marks |
| `a` / `A` | New file / folder, its name typed in a row in place (a `/` makes the folders on the way) |
| `c` / `+` | The same: new file / folder |
| `R` | Rename in place, the name before its extension selected |
| `D` | Delete to the Recycle Bin (marked, or the row at point) |
| `C` / `M` | Copy / move to... (a menu beside the row asks before overwriting) |
| `z` / `x` | Pack the marked set into an archive / unpack the one at point |
| `i` | Properties (and, on a folder, count what is inside) |
| `w` | Toggle read-only |
| `/` / `f` | Filter the whole tree as you type (`Enter` keeps it, `Esc` clears it) |
| `n` / `N` | Next / previous match of the filter |
| `F` | Fuzzy-find by name through everything under here |
| `p` | A preview of the row at point beside the listing, following it (again to close) |
| `P` | Open the file at point in the editor's preview tab, beside |
| `J` / `K` | Scroll the preview |
| `.` | Show or hide hidden files and folders, as `SPC e h` |
| `o` / `O` | Cycle sort key (name/size/date/type) / reverse it |
| `r` / `g r` | Refresh |
| `?` | Every key |
| `S` | Pick the folder shown (when picking a folder) |
| `q` | Close a pane's listing or the picker; the sidebar hands the keyboard back |
| `Esc` | Stop waiting for a folder that isn't answering, then clear the filter, then (picker, sidebar) close |
| `SPC` / `:` | The leader and the command line, as anywhere |

Operations act on the marked set if there is one, and on the row at
point otherwise -- dired's own convention. The mouse: a click selects, a
second click opens, a chevron opens or closes its folder, a breadcrumb
segment goes there, the wheel scrolls, and the sidebar's edge drags.

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

## Search page (`SPC /`, `SPC s p`, `SPC s r`)

While a field has the keyboard:

| Keys | Action |
|---|---|
| typing | Searches as you type |
| `Enter` | To the results |
| `Tab` / `Shift-Tab` | Next / previous field (Search, Replace, files) |
| `Alt-c` / `Alt-w` / `Alt-r` | Match case / whole word / regex |
| `Alt-h` | Hidden and ignored files too |
| `Alt-s` | Next scope |
| `Alt-p` | Keep the match's case in a replace |
| `↑` / `↓` | This project's searches |
| `Esc` | Stop typing |

On the results:

| Keys | Action |
|---|---|
| `j` / `k`, `gg` / `G` | Next / previous match, first / last |
| `Enter` | Go to the match |
| `p` | Show the selection beside the page, or stop |
| `Tab` / `za`, `zM` / `zR` | Fold a file; fold / open them all |
| `/` / `i`, `r`, `f` | Edit the search, the replacement, the files |
| `x` / `X` | Leave out the match's line / its file (again: back in) |
| `R` | Replace what's in (asks first) |
| `+` / `-` | More / fewer lines around each match |
| `Esc` | Close Replace |
| `q` | Close the page (`SPC s .` brings it back) |

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

## A field being typed on a page (settings, filters, forms)

| Keys | |
|---|---|
| `←` `→` / `Ctrl-←` `Ctrl-→` | Move the caret a character / a word |
| `Home` `End` | To the start, the end |
| `Backspace` `Delete` / `Ctrl-Backspace` `Ctrl-W` | Delete a character / the word before the caret |
| `Ctrl-O` | Browse for the path being typed (a path setting, a list of paths, the project's program and cwd) |
| `Ctrl-V` | Paste at the caret |

In the explorer a setting opens, `Enter` picks a file and `S` the folder shown.

## MIB (`SPC k k`)

From the MIB extension (`fenix.mib`, in the welcome's Mission pack, or
`SPC h x`).

| Keys | MIB page |
|---|---|
| `1`-`5`, `Tab` | Telecommands, TC parameters, TM packets, TM parameters, calibrations |
| `/` | Search: words, and `field:value` filters (`Tab` completes a field); it narrows the services and the problems too |
| `Enter` / `i` / `y` | A definition's page / insert the telecommand / copy the name |
| `o` / `m` / `p` | Sort by the next column / one MIB of several / preview on and off |
| `!` / `R` / `a` `A` | Problems in the files / read again / the project's MIB settings, yours |
| `6` | Services: telecommands and packets by PUS service |

| Keys | A definition's page |
|---|---|
| `j` `k` / `h` `l` | The next, previous line with a link / link on the line |
| `Enter` | Follow the link |
| `H` `L` (or `Backspace`) | Back / forward |
| `u` / `i` / `y` | What uses it / insert the telecommand / copy the name |
| `t` | Follow the TM parameter through the recording or live source open |
| `/` / `Esc` | Only the rows with these words (parameters, points, what uses it) / every row again |
| `gf` / `r` / `m` | Its `.dat` line / raw fields / the MIB page |

| Keys | Insert form |
|---|---|
| `j` `k` `Tab` | Move between arguments |
| `Enter` / `c` | Type a value (or pick a status text) / clear and type |
| `h` `l` / `+` `-` | Previous, next status text / one more, one fewer repetition |
| `R` / `d` | The MIB's defaults / the telecommand's page |
| `b` / `s` `S` | Show the packet bytes / next sequence count, back to 0 |
| `y` / `D` | Copy the packet bytes / decode them in the inspector |
| `I` (`Esc` first while typing) / `q` | Insert / leave |

## CCSDS (`SPC k d`, `SPC k f`, `SPC k T`, `SPC f x`)

From the CCSDS extension (`fenix.ccsds`, in the welcome's Mission pack,
or `SPC h x`).

| Keys | Packet inspector |
|---|---|
| `j` `k` / `g` `G` | Fields |
| `Tab` / `za` | Fold |
| `/` / `n` `N` | Find fields by name, bytes, value or check / the next, previous one |
| `Enter` | Open the field's MIB definition or standard |
| `y` / `Y` / `b` | Copy the value / the whole decode / the bytes |
| `t` | Follow the TM parameter selected through the recording or live source open |
| `a` / `p` | Read as a packet, frame, CLTU, CFDP PDU, CLCW or time / the mission settings |

| Keys | Recording or live source |
|---|---|
| `1`-`4`, `Tab` | Packets, by APID, a parameter, problems |
| `/` / `i` | Filter every tab / idle packets shown or hidden |
| `]` `[` | Next, previous sequence gap |
| `Enter` / `x` | Inspect the packet / show it in the hex view |
| `f` | Inspect the CADU or CLTU it came in |
| `t` / `w` / `F` | Follow a TM parameter / write what's shown / read with another framing |
| `c` / `r` | On the parameter: its next limit crossing / the curve from raw or engineering values |

The filter takes words (found in a packet's name, SPID, time, APID,
service and check) and terms: `apid:` `type:` `stype:` `vc:` `seq:`
`len:` (a number, or a range like `apid:0x100..0x1FF`), `spid:` `name:`
`time:` `verifies:`, `dir:tc` or `dir:tm`, `check:bad` or `check:ok`,
and for the followed parameter `value:` `raw:` `limits:out` (`in`,
`soft`, `hard`). A `-` in front leaves out what a term matches.

| Keys | Hex view |
|---|---|
| `h` `j` `k` `l` / `d` `u` / `0` `$` / `gg` `G` | Move |
| `w` | The next packet |
| `]` `[` | The next, previous sync marker |
| `/` `n` `N` / `o` | Search hex or text / go to an offset |
| `x` `Enter` | Decode here |

| Keys | Time converter |
|---|---|
| `j` `k` | The formats |
| `Enter` `c` / `e` | Type a time / edit this one |
| `n` / `y` / `i` | Now / copy / insert the on-board time |

| Keys | Standards (`SPC k ?`) |
|---|---|
| `Enter` / `o` / `f` | Open the standard / where to get it / the folder setting |
| `/` | Search every standard's text; `Enter` on a match opens it at its page, `Esc` goes back |

## Notebook (`SPC n n`), reading view and diagrams

| Key | Where | What |
|---|---|---|
| `Enter` / `o` / `v` | notebook page | Edit / read / view a diagram |
| `N` / `d` / `i` | notebook page | New note / new diagram / import |
| `t` / `r` / `P` / `D` | notebook page | Tags / rename / pin / duplicate |
| `h` / `p` / `x x` / `e` / `T` | notebook page | Earlier versions / to the project / delete / export / a diagram's theme |
| `/` / `Tab` / `s` / `[` `]` | notebook page | Filter / chips / pin the search / journal days |
| `[[` | Markdown, Insert | Complete a note's name |
| `gd` / `gf` | on a `[[link]]` | Follow it (a missing note is made) |
| `K` | in a ```` ```mermaid ```` block | The reading view beside, on it |
| `j` `k` / `Tab` / `Enter` / `x` / `i` | reading view | Move / next link / follow / tick / edit here |
| `+` `-` `0` / `hjkl` / `Tab` / `/` / `Enter` | diagram preview | Zoom / pan / walk nodes / find one / its line |
| `t` / `T` / `e` | diagram preview | Set its theme / try one / export |
| `Tab` / `C-t` / `C-l` | capture prompt | Journal or Inbox / as a task / drop the link back |

## Reviews (`SPC g M`) and a review (the Review extension)

| Key | Where | What |
|---|---|---|
| `Enter` / `w` / `c` | reviews | Review it / in a worktree of its own / check it out here |
| `Tab` / `o` / `u` | reviews | Which requests / the browser / read again |
| `Tab` / `v` | review | Open a file's diff / mark it viewed |
| `C` / `S` | review | Comment on the line or lines marked / suggest a change, held for your review |
| `r` / `R` / `]t` | review | Reply / resolve or reopen / the next thread |
| `i` / `s` / `m` | review | Only what changed since your review / submit (`a` `r` `c`, `S S`) / merge (`m m`) |
| `S S` / `Enter` / `e` / `d` / `D` | new pull request | Open it / edit a field / write the description / toggle draft / the Compare page |

## An agent's thread (`SPC a a`)

| Key | Action |
|---|---|
| `i` / `a` / `c` / `Enter` | To the prompt under the thread, in Insert mode |
| `j` / `k`, `g` / `G` | The next / previous card, the first / last |
| `]` / `[` | The next / previous prompt of yours |
| `Tab` / `o` | Open or fold a card (thinking, a command's output, a diff) |
| `Enter` (a tool call) | Open the file it worked on, at its line |
| `y` / `Y` | Copy the card / the agent's answer (or the whole thread) |
| `v` | The card's text in a buffer of its own |
| `y` `Y` `n` `N` / `1`…`9` (a question) | Allow once, always / reject once, always / that option |
| `Ctrl-C` | Cancel the turn (or take back a queued prompt) |
| `/` | Filter the cards |
| `C` | Continue a read-only thread in a new session |
| `q` | Put the dock away (the thread is kept) |
| `Enter` (the prompt, Normal mode), `Ctrl-Enter` (Insert) | Send it |
| `Ctrl-C` (the prompt, Normal mode) | Cancel the turn |
| `↑` / `↓` (an empty prompt, or one brought back as it was) | The prompt sent before / after |
| `y` `Y` `n` `N` / `1`…`9` (an empty prompt, a question waiting) | Answer it, as in the thread |
| `@` (the prompt, Insert mode) | Mention a file, `@selection`, `@file`, `@diagnostics` or `@diff` |
| `/` (an empty prompt, Insert mode) | One of the agent's commands |
| `Ctrl-V` (the prompt) | Paste a picture, for an agent that takes them |

## Agent changes (`SPC a r`)

| Key | Action |
|---|---|
| `j` / `k`, `J` / `K` | The next / previous hunk, file |
| `a` / `x` | Keep / reject the hunk (on a file's row: the whole file) |
| `A` / `X` | Keep / reject the whole file |
| `Enter` | Open the file at the hunk |
| `r` | Read the files again |
| `q` | Close |

`agents.accept_all` and `agents.reject_all` (`SPC :`) keep or reject
everything the thread changed.

## Agenda (`SPC A a`) and Jira (`SPC j j`)

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

The VNC extension (`fenix.vnc`, from the registry: the welcome's Lab
pack, or `SPC h x`) embeds a live VNC (RFB) connection to a VM as a
surface in a pane -- configure hosts once in `vnc.hosts` (see
Configuration), then `SPC v v` fuzzy-picks one by name to open it, or
to bring it to the pane you're in. Each session connects the first
time you pick it and then stays live in the background, so switching
back later is instant, not a fresh handshake; a session in no pane
asks for updates at a much slower rate to stay cheap while you're not
looking at it, and a dropped connection retries automatically with
backoff before giving up and leaving the pane on its last frame.

| Keys | Action |
|---|---|
| `SPC v v` | Open or switch to a configured VNC session (picker by name) |
| `SPC v q` | Close the VNC session you're in (a picker when it can't tell) |
| `SPC v s` | Save its current frame as a timestamped PNG, in `vnc.screenshots` |
| `Ctrl-\` | Release keyboard capture back to the editor (same chord as the terminal panel) |

Clicking into a VNC pane both focuses it and starts sending your mouse
there; every other key while it's focused is forwarded to the VM
instead of Vim, exactly like the terminal panel, and the modeline says
`CAPTURE`. Clipboard content is mirrored in both directions: the VM's
clipboard always flows to yours as it changes, and yours flows to the
VM whenever its pane takes the keyboard.

Resizing a VNC pane asks the VM to match its own resolution to the
pane's, the same "remote resizing" real VNC clients offer, so the
picture renders at native size instead of a stretched/letterboxed
scale. Whether that request actually lands depends on the server: it
has to advertise support for it in the first place (confirmed
automatically per-connection, nothing to configure), and even then can
still decline a specific size (administrative policy, an unsupported
resolution, ...). Either way, the video keeps stretching to fill the
pane as a fallback -- a pane and a VM sitting at different resolutions
is never broken, just not pixel-native.

The connection is always made in the clear -- there's no encryption,
and VNC Authentication is answered with an empty password, matching
the assumption that every
configured host is on a trusted local network. Don't point this at
anything reachable over an untrusted network without your own tunnel
(SSH port-forwarding, a VPN) in front of it.

## PDF reader (`SPC r ...`)

The PDF reader is an extension (`fenix.pdf`, from the registry: the
welcome's Reading pack, or `SPC h x`). Its pages are rendered by
`fenix-pdfium`, a small program that comes with Fenix, beside
`fenix.exe` (see [Optional external
tools](BUILDING.md#optional-external-tools)); without it, or without
PDFium, opening a PDF says which is missing and where it goes.

A `.pdf` opens like any other file -- by typed path (`SPC f f`), the
explorer, a recent file, the shelf, a CLI argument, or a jump -- on a
surface in the focused pane. Opening a PDF that's already open brings
it to the pane you're in, where you were. The pages are laid out **one
under the other in a single column**, fitted to the pane's width the
first time: scrolling goes straight from one page onto the next, a
scroll draws only the strip that came into view, and a part not rendered
yet is blank paper. A zoom keeps the same place at the top of the pane.

The reader's keys are Vim's, with counts. Space (the leader), `:`,
`Ctrl-w` and the tab keys (`gt`, `gT`) are Fenix's; `q` closes the PDF.

| Keys | Action |
|---|---|
| mouse wheel, `j` / `k`, `Down` / `Up`, `{n}j` | Scroll the column of pages |
| `Ctrl` + wheel | Zoom in / out |
| `Ctrl-d` / `Ctrl-u` | Half a screen down / up |
| `Ctrl-f` / `Ctrl-b`, `PageDown` / `PageUp` | A screen down / up |
| `J` / `K`, `{n}J`, `SPC r n` / `SPC r p` | Next / previous page, at its top |
| `gg`, `G`, `{n}G`, `Home` / `End` | First page, last page, page n |
| `SPC r g` | Prompt for a page number and jump to it |
| `+` / `-`, `SPC r =` / `SPC r -` | Zoom in / out through 25% ... 400% |
| `=` | Fit width and fit page, in turn |
| `zw` / `zp` / `z0` | Fit width / fit page / 100% (the page's real size on this screen) |
| `h` / `l`, `Left` / `Right` | Pan sideways when the page is wider than the pane |
| `o`, `SPC r o` | Open or close the outline: outline, matches, marks |
| `SPC r t` | Go to a heading, picked by name |
| `/`, `SPC r /` | Search; `Enter` goes to the first match from here |
| `n` / `N` | The next / previous match, going round the ends |
| `Esc` | Clear a selection, hide the matches' highlights, or close the outline |
| `m{a}` / `'{a}` | Set a mark here / go back to it |
| `Ctrl-o` / `Ctrl-i` | Back / forward through jumps (`gg`, `G`, `{n}G`, headings, matches, marks, links) |
| `yp`, `SPC r y` | Copy a link to this page, `file.pdf#page=38` |
| `v` | Select text from the top line in sight; `w` `b` `e` `h` `l` `0` `$` `j` `k` extend it across pages, `y` copies, `Esc` stops |
| drag | Select text with the mouse; `y` copies |
| `f` | Label every link in sight; typing a label follows it, `Esc` cancels |
| click | Follow a link |
| `SPC r c` | Pages as printed, or in the theme's colours |

`SPC r ...` also works from another pane -- the outline, or the code
you're reading the document for -- and acts on the PDF read last. `SPC m`
on a PDF lists the reader's commands. The modeline shows `PDF p. 38/212`
and the zoom (and the search, `/packet 2/5`) for the PDF read last.

`SPC r f` opens the **shelf**: a picker over the documents you keep by
name in `reader.documents` (`SPC ,` > Extensions), or by hand in
`settings.toml`:

```toml
[reader.documents]
"Space Packet Protocol" = 'C:\refs\133x0b2e2.pdf'
"Time Code Formats" = 'C:\refs\301x0b4.pdf'
```

A project can keep a shelf of its own in its `.fenix/settings.toml`
(paths from the project), listed in place of yours; then come the PDFs
you've read lately (`recent · spec.pdf  p. 38 / 212`). A document is
listed once. An entry can point at any file Fenix opens, not just a PDF.

The **outline** (`o`) opens in a pane beside the pages -- or over them,
in their pane, when it's narrow (`reader.sidebar`) -- with three lists
that `Tab` goes between: the **outline**, the **matches** of the last
search and the document's **marks**. The keys stay on the PDF; `Ctrl-w`
gets you there. `j`/`k`/`gg`/`G` move, `Enter` goes there, `o` or `q`
close it. The outline is a tree: `za` folds an entry, `h` folds or goes
up to the parent, `l` unfolds, `zM`/`zR` fold and unfold everything, and
the section you're reading stays marked as you scroll.

**Search** (`/`) asks in the modeline -- a capital letter makes it match
case -- and highlights every match on the pages, the current one more
strongly. It goes to the first match from the page you're on (as soon as
it's found), and `n`/`N` step through the matches one by one, going round
the ends. A long document is searched a few pages at a time, so pages
keep rendering while it's searched.

**Colours**: `reader.colors` draws pages as printed -- `paper`, the
default -- or in the current `theme`'s colours: white becomes the
theme's background, black its text, and colours keep their hue, with the
gaps between pages a shade darker. `SPC r c` switches it.

**Where you were**: each PDF reopens at the place, zoom and marks you
left it with (`reader.remember`, on), and sessions bring PDF panes back
where they were. Home's **reading** lists the PDFs you read lately, with
the page you're on. `reader.zoom` sets how a PDF opens the first time
(fit width or fit page), `reader.page_gap` the space between pages.

**Links**: `yp` (or `SPC r y`) copies a link to the page you're on --
`specs/133x0b2e2.pdf#page=38`, from the project when the PDF is in it --
to the clipboard. `gf` on such a link, in any buffer (a Markdown note, a
task, a code comment), opens the PDF at that page; `gf` on any other
file name opens that file.

**Text and links**: dragging across a page selects its text, and so does
`v` from the keyboard. `y` copies it to the clipboard with the lines run
together and a word split by a hyphen at a line's end mended. A scanned
page has no text and says so. A link to another page is a jump (`Ctrl-o`
comes back); a web link opens in your browser -- any other kind is
refused, since it comes from the document.

**Rebuilt PDFs**: a PDF rewritten on disk -- LaTeX, Typst, Doxygen, a
datasheet downloaded again -- is read again, keeping its place.

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
