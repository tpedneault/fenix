# Vim conformance

`crates/fenix-vim/tests/vim_conformance.txt` is a list of cases: a start
text with `|` for the cursor, the keys typed (Vim's notation), and what
Vim leaves -- the text and cursor, and sometimes the unnamed register.

- `cargo test -p fenix-vim --test vim_conformance` holds Fenix to every
  case. It runs on every platform, without Vim.
- `python3 dev/vim-conformance/check.py` runs every case through real Vim
  and says where a case's expectation isn't what Vim does. It needs `vim`
  and util-linux's `script`, so Linux; it was last run against
  Vim 9.1.

To add a case, write its name, `start:` and `keys:` with an empty
`end: ""` (and `reg: ""` to check the register), then run
`check.py --write`: Vim's results go into the file. If Fenix then
disagrees, either fix Fenix or, for a deliberate difference, give the
case a `differs:` line saying why, put Fenix's own result in `end:`, and
add it to [Differences from Vim](../../docs/VIM_DIFFERENCES.md).

Vim runs with Fenix's defaults: `shiftwidth=4 expandtab autoindent
backspace=indent,eol,start nojoinspaces`, a window as tall as every case,
and the keys fed as if typed (`feedkeys(…, 'tx')`), so undo steps fall
where they would.
