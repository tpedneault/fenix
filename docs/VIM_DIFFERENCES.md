# Differences from Vim

Fenix edits with Vim's grammar: its motions, operators, text objects,
counts, registers, `.` and macros. It isn't Vim, though, and this page
says honestly where it does something else or nothing at all. Each line
was checked against the code; if you find one that's wrong, or a key
that misbehaves and isn't here, please
[open an issue](https://github.com/tpedneault/fenix/issues).

How close it is gets checked: `crates/fenix-vim/tests/vim_conformance.txt`
is about two hundred cases (a text, the keys, what Vim leaves), which
`cargo test` holds Fenix to and `dev/vim-conformance/check.py` runs
through real Vim. A case that's a deliberate difference says so there.

Fenix's defaults are Vim's with `shiftwidth=4 expandtab autoindent
backspace=indent,eol,start nojoinspaces`.

## Deliberately different

- **Visual `>` and `<` keep the selection**, as the common `>gv` mapping
  does, so a second press shifts again. Vim drops back to Normal.
- **`Y` yanks to the line's end** (`y$`), as Neovim's does; Vim's `Y` is
  `yy`.
- **Marks are global.** `m{a-z}` and `m{A-Z}` both remember their file,
  and `` `a `` goes there from anywhere; Vim's lowercase marks belong to
  one buffer. An operator's mark (`d'a`) has to be in the buffer you're
  in.
- **One jumplist**, back and forward stacks across every pane and file,
  like a browser's history, not Vim's list per window. `n` and `N` don't
  add to it (a search typed with `/` and `*` do), nor do `{` and `}`.
- **Smart case is on** (`search.smart_case`): a lower-case search
  matches either case. Vim's default is to match case exactly.
- **`Esc` in Normal mode puts out the search highlight**, as Doom Emacs
  does; `:noh` does too.
- **Typing in Insert does more**: an opening bracket gets its closing one,
  typing a closing bracket over one steps past it, Backspace between an
  empty pair takes both, a `}` at a line's start dedents it, Enter after
  `{` indents a level (and by the language's own rules where a grammar
  says), and Enter on a list item (`- `, `1. `, `- [ ] `) starts the next
  one. `Tab` always
  inserts spaces, to the next multiple of `shiftwidth`.
- **`Ctrl-Y` is redo** (and `Ctrl-Z` undo, `Ctrl-S` save, `Ctrl-Q` quit)
  in both key schemes, so Vim's `Ctrl-Y` scroll isn't there.
- **Keys Fenix gives its own meaning**: `gd` and `gr` (definition and
  references, from the language server; Vim's `gr` is virtual replace),
  `K` (hover), `gh` (Home; Vim's is Select mode), `gt`/`gT` (the pane's
  tabs), `gf` (the file or link under the cursor, `path:line` too), `]t`
  `[t` `]h` `[h` `]d` `[d` `]a` `[a` `]c` `[c` (TODOs, changed hunks,
  problems, agent changes, side-by-side changes), `za` and the other
  folds (from the syntax tree), `do` `dp` (side by side only).
- **Plugins' keys are built in**: `gc`/`gcc` comment lines
  (vim-commentary), `ys` `ds` `cs` and Visual `S` surround
  (vim-surround).

## Not there

- **Scrolling keys in Normal mode**: `Ctrl-E`, `Ctrl-D`, `Ctrl-U`,
  `Ctrl-F` and `Ctrl-B` do nothing (`PageUp`/`PageDown` and `zz` `zt`
  `zb` scroll). `zh`/`zl` aren't there either.
- **Text objects**: `iw` `aw`, the quotes, `i(` `i{` `i[` (and `b`, `B`),
  `ip` `ap` and `it` `at` are there; `iW` `aW`, `is` `as` (sentences) and
  `i<` `a<` aren't. A count on a text object is ignored (`2diw` is
  `diw`).
- **A count on `D`, `C` and `Y`** is ignored (`3D` is `D`).
- **Registers**: the unnamed register (which is the clipboard), `a`-`z`
  (`A`-`Z` append), `"0` (the last yank) and `"_` (the black hole). There
  are no numbered `"1`-`"9`, no small-delete `"-`, and `"+`/`"*` can't be
  named before a command -- the unnamed register already is the
  clipboard. `Ctrl-R` in Insert also reads `.` (the last text typed),
  `/` (the last search), `:` (the last command line), and `+`/`*` (the
  clipboard), but not `-`, `%`, `#` or `=`.
- **Insert mode**: `Ctrl-R` puts a register's text in literally, as Vim's
  `Ctrl-R Ctrl-R` does (no indenting or brackets added). `Ctrl-V` (a
  literal character), `Ctrl-K` (digraphs), `Ctrl-E`/`Ctrl-Y` (the
  character below, above), `Ctrl-A` (the last text again), and
  `0 Ctrl-D` / `^ Ctrl-D` aren't there; `Ctrl-N`/`Ctrl-P` move through
  the completion popup when one is open, and aren't Vim's word
  completion. `Ctrl-W` and `Ctrl-U` delete
  past where this Insert began, as Vim does with `backspace=start`.
- **Replace mode**: `R` and `r` are there, Virtual Replace (`gR`, `gr`)
  isn't.
- **Normal commands** that aren't there: `=` (reindent is `SPC c f`),
  `gp` `gP` `]p`, `g;` `g,` (the change list), `gn`, `g*` `g#`, `ga`,
  `g8`, `gm`, `go`, `g Ctrl-G`, `Ctrl-G`, `q:` and `q/`, `ZZ` `ZQ`, and
  the undo tree (`g-`, `g+`, `:earlier`, `:later`).
- **`Ctrl-A`/`Ctrl-X`** count in decimal only: no hex, octal, binary or
  letters (`nrformats`).
- **Visual Block** `I` skips lines too short to reach the block (Vim
  does too); `A` pads them. Visual surround (`S`) doesn't work on a
  block.
- **Ex commands** are a subset: line ranges (`N`, `.`, `$`, `%`, `'<,'>`,
  `+N`/`-N`), `:d` `:y` `:m` `:t`/`:co` `:j` `:>` `:<` `:norm` `:sort`
  `:s` (with `c`), `:g`, `:noh`, `:{range}!`, `:e` `:sp` `:vs`,
  `:w` `:q` and their `!`, `a` and `x` forms, `:bd` `:bn` `:bp` `:on`,
  and `:set` for `shiftwidth`, `iskeyword` and `textwidth` only.
- **Patterns** are Vim's, translated to Rust's regular expressions, which
  have no back-references: `\(a\)\1` can't be searched for.

## Different in the details

- **`gq` and `gw`** wrap at `textwidth` (`:set tw=N`; 79 without one, as
  Vim's `gq` does when `tw` is 0), keeping the first line's indent and
  comment leader (`//`, `///`, `//!`, `#`, `--`, `;`, `>`, `%`) on every
  line, and lining a Markdown list item's lines up under its text.
  There's no `formatoptions` or `comments` to set, and a C comment's
  ` * ` lines are read as list items, each kept on its own. Typing in
  Insert never wraps. `textwidth` lasts until Fenix closes and is the
  same in every buffer.
- **`!{motion}`, `!!` and `:{range}!command`** run the command with
  `cmd /C` on Windows and `sh -c` elsewhere, in the project's folder, and
  wait up to 10 seconds for it. In a remote project the command still
  runs on this machine. `:!command` without a range, which in Vim runs a
  shell command and shows what it said, says to use Fenix's terminal
  instead.
- **`H`, `M` and `L`** go by the pane's lines as they're drawn; there's no
  `scrolloff`. Where a pane wraps long lines, its rows are counted as
  lines, so `L` can land on a line below the last one in view.
- **`U`** works on the line the cursor was on when the changes were made,
  and forgets it on any change that isn't on one line (`J`, `o`, `dd`).
  Vim's bookkeeping differs in corners: what `U` does after a `u`, say.
- **`.` with a count** (`3.`) takes the place of the count the change was
  typed with, as in Vim, for a count typed before the command or between
  an operator and its motion.
