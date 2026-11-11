# Coming from another editor

Fenix is a Vim-keyed editor with a `SPC` leader for everything else.
These pages are for the first day when you come from somewhere else:
what carries over, what's different, the ten keys to learn first, and
how to bring your settings with you.

- [Fenix for Neovim and Vim users](#fenix-for-neovim-and-vim-users)
- [Fenix for Helix users](#fenix-for-helix-users)
- [Fenix for VS Code users](#fenix-for-vs-code-users)
- [Bringing your settings over](#bringing-your-settings-over)

Whichever you come from, `SPC h T` is the tutorial (a lesson at a time,
in a real buffer), `SPC h ?` lists the keys in use, and `SPC h k` says
what a key does.

## Fenix for Neovim and Vim users

**What carries over.** Vim keys are the default, and they're Vim's:
modes, motions, operators with text objects and counts, registers
(`"a`, `"0`, `"_`), macros (`q`, `@`), marks, the jumplist (`Ctrl-O` /
`Ctrl-I`), Visual block, folds (`za`, `zR`, `zM`), `gcc` / `gc{motion}`
to comment, `]d` / `[d` between problems, and the `:` line: `:w`, `:e`,
`:sp`, `:vs`, `:bd`, `:s` and `:g` with Vim's patterns, ranges,
`:norm`, `:sort`, `:set shiftwidth=N` and `iskeyword`. Language servers
and tree-sitter are built in: `gd`, `gr` and `K` work without a plugin
for Python, Rust, C and C++, Bash, JavaScript and TypeScript, Go and Lua,
and a language pack (`SPC h x`) or `lsp.servers` adds the rest.

**What's different.**

- **No Vimscript, no Lua, no Vim plugins.** Settings live in
  `settings.toml` (every one on `SPC ,`), keys in `keybindings.toml`
  (`SPC h e`); Fenix's [extensions](WRITING_EXTENSIONS.md) are sandboxed
  WebAssembly, from its own store (`SPC h x`).
- **The leader is `SPC`, and it's a menu**: `SPC f` files, `SPC p`
  projects, `SPC g` Git, `SPC c` code, `SPC w` windows, `SPC b` buffers,
  with which-key showing what comes next. Your own leader keys go in
  `keybindings.toml` as `[[leader]]` entries (`keys = "SPC o s"`).
- **`:q` closes the buffer**, not the window; `SPC w q` closes a pane,
  `SPC q q` quits.
- **Marks and the jumplist are global**: every mark jumps to wherever it
  was set, in any file, and `Ctrl-O` goes back across files.
- **One cursor.** Extra cursors are a standard-keys feature; Vim keys
  have Visual block, `:s`, `:g`, macros and `:norm` instead.

**The ten keys to learn first.**

| Keys | What |
|---|---|
| `SPC` | The leader's menu: wait a moment and it shows what comes next |
| `SPC SPC` | Find a file in the project |
| `SPC /` | Search the project, live, on the Search page |
| `SPC b b` | Switch buffer |
| `SPC w v` / `SPC w h j k l` | Split / move between panes |
| `gd` / `gr` / `K` | Definition / references / hover, from the language server |
| `SPC c a` / `SPC c r` | Code actions / rename |
| `SPC g g` | The Git status page |
| `SPC o t` | The terminal panel (`Ctrl-\` gives the keyboard back) |
| `SPC :` | Any command by name |

**Bringing your settings over.** `SPC h i` reads your `init.lua`,
`init.vim` or `.vimrc` -- without running it -- and shows what it would
write: [below](#bringing-your-settings-over).

## Fenix for Helix users

**What carries over.** Language servers and tree-sitter are built in, as
in Helix: `gd` and `gr`, `]d` / `[d` between problems, highlighting and
folding from the grammar. The space bar opens a menu with a popup of
what comes next, and much of Helix's space mode is a key or two away:

| Helix | Fenix |
|---|---|
| `space f` (file picker) | `SPC SPC` |
| `space b` (buffer picker) | `SPC b b` |
| `space /` (global search) | `SPC /` |
| `space k` (hover) | `K` |
| `space a` / `space r` (code action / rename) | `SPC c a` / `SPC c r` |
| `space d` (diagnostics) | `SPC c p` |
| `space ?` (command palette) | `SPC :` |
| `space w` (window mode) | `SPC w` |
| `:w`, `:q` | `:w`, `:q` (`:q` closes the buffer) |

**What's different.**

- **Action, then motion.** Fenix's grammar is Vim's: `dw` deletes a
  word, where Helix selects first (`wd`). To select first, use Visual
  mode: `v` (or `V` for lines), move, then `d`, `c`, `y` or `>`.
- **Vim's keys where Helix moved them**: `x` deletes a character (`V`
  selects the line), `%` goes to the matching bracket, `0` / `$` are the
  line's start and end (Helix's `gh` / `gl`), `G` the last line (`ge`),
  and redo is `Ctrl-R` (`U`).
- **One cursor with Vim keys.** Helix's `C` and `s` have no equivalent;
  Visual block, `:s`, `:g`, macros and `:norm` do that work. Extra
  cursors are a standard-keys feature (`Ctrl+D`, `Alt`+click).
- **Settings are on a page** (`SPC ,`) and in `settings.toml`, not
  `config.toml` and `languages.toml`.

**The ten keys to learn first.**

| Keys | What |
|---|---|
| `SPC SPC` | Find a file in the project |
| `SPC /` | Search the project |
| `SPC b b` | Switch buffer |
| `SPC w` | Windows: `v` and `s` split, `h j k l` move, `q` closes |
| `gd` / `gr` / `K` | Definition / references / hover |
| `SPC c a` / `SPC c r` | Code actions / rename |
| `v` / `V`, then `d` `c` `y` | Select, then act on it |
| `u` / `Ctrl-R` | Undo / redo |
| `SPC :` | Any command by name |
| `SPC h T` | The tutorial, for Vim's grammar a lesson at a time |

**Bringing your settings over.** The importer doesn't read Helix's
files; these are the settings to set by hand (`SPC ,`, or
`settings.toml`):

| Helix | Fenix |
|---|---|
| `theme` | `editor.theme` (`SPC t p` lists them) |
| `[editor] line-number = "relative"` | `editor.line_numbers = "relative"` |
| `[editor.soft-wrap] enable` | `editor.wrap` |
| `indent` in `languages.toml` | `editor.indent_width` (a project can set its own) |
| `language-server` in `languages.toml` | `lsp.servers`, or a language pack (`SPC h x`) |
| `auto-format` | `editor.format_on_save` (the formatter a language pack gives) |

## Fenix for VS Code users

**What carries over.** Choose **standard keys** (the welcome's `s`,
`SPC t k`, or `editor.keys = "standard"`) and VS Code's Windows and
Linux shortcuts work for every command Fenix has: you type at once,
Shift with the arrows selects, `Ctrl+C` / `Ctrl+V` / `Ctrl+Z`, `Ctrl+P`,
`Ctrl+Shift+P`, `Ctrl+Shift+F`, `F12`, `F2`, `Ctrl+.`, `Ctrl+/`,
`Alt+↑` / `Alt+↓`, `Ctrl+D` and `Alt`+click for more cursors,
``Ctrl+` `` for the terminal. Language servers are built in for a few
languages, and the store's language packs (`Ctrl+K h x`) bring the rest
with their debuggers, formatters and tasks. Visual Studio Dark and Light
are among the themes.

**What's different.**

- **`Ctrl+K` opens Fenix's menu of everything else** -- the same leader
  menu Vim keys open with `SPC`: `Ctrl+K g g` is Git's page, `Ctrl+K w`
  the panes, `Ctrl+K h` help. Wherever these docs say `SPC …`, read
  `Ctrl+K …`. VS Code's own two-key chords, `Ctrl+K Ctrl+I` among them,
  are leader keys too.
- **Finding is the Search page**, not a bar over the editor: `Ctrl+F`
  and `Ctrl+H` open it on the file, `Ctrl+Shift+F` on the project, `F3`
  steps through the matches.
- **Panes are split from the leader** (`Ctrl+K w v`, `Ctrl+K w s`, or
  `Ctrl+\`), and closing a split's last tab closes the pane.
- **No menu bar.** The palette (`Ctrl+Shift+P`) finds any command, and
  `Ctrl+K h ?` lists every key in use.
- **No `:` line**, `.` repeat or macros -- those are Vim keys'. You can
  switch at any time (`Ctrl+K t k`), and the tutorial teaches either.
- **Extensions are Fenix's own**, from its store; VS Code's don't run
  in it.

**The ten keys to learn first.**

| Keys | What |
|---|---|
| `Ctrl+K` | The leader's menu: everything Fenix has, a key at a time |
| `Ctrl+Shift+P` | Any command by name |
| `Ctrl+P` | Find a file in the project |
| `Ctrl+Shift+F` / `Ctrl+F` | Search the project / this file, on the Search page |
| `F12` / `Shift+F12` | Go to definition / references |
| `Ctrl+.` / `F2` | Code actions / rename |
| ``Ctrl+` `` / `Ctrl+J` | To the terminal panel and back / show or hide it |
| `Ctrl+Shift+G` | Git |
| `Ctrl+,` | Settings |
| `Ctrl+K h ?` | Every key in use, on one page |

Coming from JetBrains' shortcuts instead? The store has **JetBrains
keys**, a keymap over standard keys (`Ctrl+K h x`).

**Bringing your settings over.** `Ctrl+K h i` (or the welcome's `i`)
reads your `settings.json` and `keybindings.json`: [below](#bringing-your-settings-over).
If you used VSCodeVim, it proposes Vim keys instead.

## Bringing your settings over

`SPC h i` (`Ctrl+K h i` with standard keys), or `i` on the welcome's
first step, lists the configs it finds and reads the one you pick. It
shows what it would write -- each setting and key with the line it came
from, `+` for what's new, `~` for one of yours that differs, `=` for
what you already have -- and what it couldn't bring over, with why.
**Nothing is written until you press `y`**, and `y` leaves your own
settings and keys as they are; `R` replaces those too, when you ask it
to. Settings go into `settings.toml`, keys into `keybindings.toml` (each
with a `# from …` comment), and both are in use at once.

**Where it looks**

| Editor | Windows | Linux |
|---|---|---|
| VS Code, VS Code Insiders, VSCodium, Code - OSS, Cursor, Windsurf | `%AppData%\<name>\User\settings.json`, `keybindings.json` | `~/.config/<name>/User/...` (and Flatpak's) |
| Neovim | `%LocalAppData%\nvim\init.lua` or `init.vim` | `~/.config/nvim/init.lua` or `init.vim` |
| Vim | `~\_vimrc`, `~\vimfiles\vimrc`, `~\.vimrc` | `~/.vimrc`, `~/.vim/vimrc` |

**What comes over**

| | From VS Code | From Vim and Neovim |
|---|---|---|
| Font and size | `editor.fontFamily` (the first font), `editor.fontSize` | `guifont` (`:h12` is in points) |
| Theme | `workbench.colorTheme`: the same theme, or the nearest of its family | `colorscheme`, with `background` choosing light or dark |
| Indent | `editor.tabSize`, `editor.indentSize` | `tabstop`, `shiftwidth` |
| Wrap | `editor.wordWrap` | `wrap` / `nowrap` |
| Line numbers | `editor.lineNumbers` | `number`, `relativenumber` |
| Keys | standard keys (Vim keys with VSCodeVim) | Vim keys |
| Also | format on save, auto-save, sticky scroll, bracket colours, preview tabs, hover, files and search excludes, Git auto-fetch, and more | smart case |
| Key bindings | `keybindings.json` entries whose command Fenix has, as `[[keys]]` for standard keys | `map`, `nnoremap`, `inoremap` and `vim.keymap.set` of one key with Ctrl, Alt or an F-key to one Ex command (`:w<CR>`, `<cmd>Telescope find_files<CR>`, `vim.lsp.buf.rename()`) or window key, as `[[keys]]` for Vim keys |

Vim and Neovim configs are **read, never run**. A plain option line
(`set tabstop=4`, `vim.opt.number = true`), a `colorscheme` and a simple
map are understood; anything inside an `if`, a function or an
autocommand, plugin managers and their setup, `<leader>` maps and maps
for Visual mode only are listed as not brought over, a run of lines at a
time. From VS Code, a setting for one language (`"[python]"`), a key
sequence (`ctrl+k ctrl+c`) and a binding whose `when` Fenix can't tell
are listed the same way.
