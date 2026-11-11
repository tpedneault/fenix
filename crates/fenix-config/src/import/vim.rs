//! A Vim (`.vimrc`, `init.vim`) or Neovim (`init.lua`) config, as Fenix
//! settings and `[[keys]]`. Nothing is run: only lines that are plainly
//! an option (`set tabstop=4`, `vim.opt.number = true`), a colour scheme
//! or a map of one chord to one command are read. Everything else --
//! plugins, functions, autocommands, conditions, Lua that does anything
//! -- is listed as not brought over.

use super::{theme_for, Catalog, Proposal};
use crate::Value;

const NO_SETTING: &str = "Fenix has no setting for this";

/// What the options said, settled once the whole file is read (the last
/// word wins, and some depend on others: `shiftwidth=0`, `background`).
#[derive(Default)]
struct Options {
    number: Option<(bool, String)>,
    relative: Option<(bool, String)>,
    tabstop: Option<(i64, String)>,
    shiftwidth: Option<(i64, String)>,
    wrap: Option<(bool, String)>,
    background: Option<(bool, String)>,
    colorscheme: Option<(String, String)>,
    guifont: Option<(String, String)>,
    ignorecase: Option<(bool, String)>,
    smartcase: Option<(bool, String)>,
}

impl Options {
    fn settle(self, catalog: &Catalog, out: &mut Proposal) {
        match (&self.number, &self.relative) {
            (_, Some((true, from))) => {
                let note = matches!(self.number, Some((true, _))).then(|| "the cursor's line shows 0 rather than its own number".to_string());
                out.set("editor.line_numbers", Value::Text("relative".into()), from.clone(), note);
            }
            (Some((true, from)), _) => out.set("editor.line_numbers", Value::Text("absolute".into()), from.clone(), None),
            (Some((false, from)), _) => out.set("editor.line_numbers", Value::Text("off".into()), from.clone(), None),
            _ => {}
        }
        if let Some((n, from)) = &self.tabstop {
            match n {
                1..=16 => out.set("editor.tab_width", Value::Int(*n), from.clone(), None),
                _ => out.unmapped(from.clone(), "Fenix's tab width is from 1 to 16"),
            }
        }
        match (&self.shiftwidth, &self.tabstop) {
            (Some((0, from)), Some((ts @ 1..=16, _))) => out.set("editor.indent_width", Value::Int(*ts), from.clone(), Some("0 is the tabstop's".into())),
            (Some((n @ 1..=16, from)), _) => out.set("editor.indent_width", Value::Int(*n), from.clone(), None),
            (Some((_, from)), _) => out.unmapped(from.clone(), "Fenix's indent width is from 1 to 16"),
            _ => {}
        }
        if let Some((on, from)) = self.wrap {
            out.set("editor.wrap", Value::Bool(on), from, None);
        }
        let light = self.background.as_ref().map(|(l, _)| *l);
        match (&self.colorscheme, &self.background) {
            (Some((name, from)), _) => match theme_for(name, light, catalog) {
                Some((theme, note)) => out.set("editor.theme", Value::Text(theme), from.clone(), note),
                None => out.unmapped(from.clone(), "Fenix has no theme like it (SPC t p lists them)"),
            },
            (None, Some((true, from))) => {
                if let Some((theme, _)) = theme_for("default", Some(true), catalog) {
                    out.set("editor.theme", Value::Text(theme), from.clone(), Some("a light theme, as Vim's was".into()));
                }
            }
            _ => {}
        }
        if let Some((font, from)) = self.guifont {
            guifont(&font, &from, out);
        }
        match (&self.ignorecase, &self.smartcase) {
            (Some((true, _)), Some((true, from))) => out.set("search.smart_case", Value::Bool(true), from.clone(), None),
            (Some((true, from)), _) => out.unmapped(from.clone(), "Fenix has no search that always ignores case: smart case (on by default) is the nearest"),
            (Some((false, from)), _) => out.set("search.smart_case", Value::Bool(false), from.clone(), Some("every search matches case, as Vim's did".into())),
            _ => {}
        }
    }
}

/// `guifont=JetBrains\ Mono:h12`: the font, and its size from points.
fn guifont(value: &str, from: &str, out: &mut Proposal) {
    let first = value.split(',').next().unwrap_or("").replace("\\ ", " ");
    let mut parts = first.split(':');
    let name = parts.next().unwrap_or("").replace('_', " ").trim().to_string();
    if !name.is_empty() && name != "*" {
        out.set("editor.font_family", Value::Text(name), from, value.contains(',').then(|| "the first of the list".to_string()));
    }
    for part in parts {
        if let Some(points) = part.strip_prefix('h').and_then(|p| p.parse::<f64>().ok()) {
            // Points, at 96 dots an inch: Fenix's size is in the pixels
            // VS Code's is.
            let size = (points * 4.0 / 3.0 * 2.0).round() / 2.0;
            if (6.0..=48.0).contains(&size) {
                out.set("editor.font_size", Value::Float(size), from, Some(format!("{points} points")));
            }
        }
    }
}

/// One line's `set` arguments.
fn set_options(args: &str, from: &str, opts: &mut Options, out: &mut Proposal) {
    for token in split_set_args(args) {
        let (name, value) = match token.split_once(['=', ':']) {
            Some((n, v)) => (n.trim_end_matches(['+', '-', '^']), Some(v)),
            None => (token.as_str(), None),
        };
        let appended = token.contains("+=") || token.contains("-=") || token.contains("^=");
        let from_tok = format!("{from}: set {token}");
        let int = || value.and_then(|v| v.parse::<i64>().ok());
        if appended {
            out.unmapped(from_tok, NO_SETTING);
            continue;
        }
        let name = name.trim_end_matches('!');
        match name {
            "number" | "nu" => opts.number = Some((true, from_tok)),
            "nonumber" | "nonu" => opts.number = Some((false, from_tok)),
            "relativenumber" | "rnu" => opts.relative = Some((true, from_tok)),
            "norelativenumber" | "nornu" => opts.relative = Some((false, from_tok)),
            "tabstop" | "ts" => match int() {
                Some(n) => opts.tabstop = Some((n, from_tok)),
                None => out.unmapped(from_tok, "isn't a number"),
            },
            "shiftwidth" | "sw" => match int() {
                Some(n) => opts.shiftwidth = Some((n, from_tok)),
                None => out.unmapped(from_tok, "isn't a number"),
            },
            "expandtab" | "et" => {}
            "noexpandtab" | "noet" => out.unmapped(from_tok, "Fenix always indents with spaces"),
            "wrap" => opts.wrap = Some((true, from_tok)),
            "nowrap" => opts.wrap = Some((false, from_tok)),
            "background" | "bg" => match value {
                Some("light") => opts.background = Some((true, from_tok)),
                Some("dark") => opts.background = Some((false, from_tok)),
                _ => out.unmapped(from_tok, "isn't light or dark"),
            },
            "guifont" | "gfn" => match value {
                Some(v) if !v.is_empty() => opts.guifont = Some((v.to_string(), from_tok)),
                _ => {}
            },
            "ignorecase" | "ic" => opts.ignorecase = Some((true, from_tok)),
            "noignorecase" | "noic" => opts.ignorecase = Some((false, from_tok)),
            "smartcase" | "scs" => opts.smartcase = Some((true, from_tok)),
            "nosmartcase" | "noscs" => opts.smartcase = Some((false, from_tok)),
            _ => out.unmapped(from_tok, NO_SETTING),
        }
    }
}

/// `set` arguments, split on spaces that aren't escaped (`guifont=A\ B`).
fn split_set_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = args.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                cur.push('\\');
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A line of Vimscript Fenix can read: an option, a colour scheme, the
/// leader, a map. `false` when it's none of those.
fn vim_command(line: &str, from: &str, opts: &mut Options, out: &mut Proposal) -> bool {
    let line = line.trim().trim_start_matches(':').trim();
    let (cmd, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
    match cmd {
        "set" | "se" | "setglobal" | "setg" | "setlocal" | "setl" => {
            set_options(strip_comment(rest), from, opts, out);
            true
        }
        "colorscheme" | "colo" => {
            let name = strip_comment(rest).trim();
            if !name.is_empty() {
                opts.colorscheme = Some((name.to_string(), format!("{from}: colorscheme {name}")));
            }
            true
        }
        "let" if rest.starts_with("mapleader") || rest.starts_with("g:mapleader") => {
            let value = rest.split_once('=').map(|(_, v)| quoted(v.trim())).unwrap_or_default();
            let why = if value == " " || value.eq_ignore_ascii_case("<space>") { "Fenix's leader is the space bar already, opening its menu of everything else" } else { "Fenix's leader is the space bar, opening its menu of everything else" };
            out.unmapped(format!("{from}: let mapleader = \"{value}\""), why);
            true
        }
        "Plug" | "Plugin" | "Bundle" | "NeoBundle" | "packadd" | "packadd!" => {
            out.unmapped(format!("{from}: {}", short(line)), "a plugin: Fenix doesn't run Vim's plugins (it has extensions of its own: SPC h x)");
            true
        }
        _ => match map_command(cmd) {
            Some(mode) => {
                parse_map(mode, rest, &format!("{from}: {}", short(line)), out);
                true
            }
            None => false,
        },
    }
}

/// The first 60 characters of a line, for saying which.
pub(super) fn short(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= 60 {
        line.to_string()
    } else {
        format!("{}…", line.chars().take(59).collect::<String>())
    }
}

/// A Vimscript string's text (`" "`, `'\\'`), or the word as written.
fn quoted(v: &str) -> String {
    match v.chars().next() {
        Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or("").to_string(),
        _ => v.split_whitespace().next().unwrap_or("").to_string(),
    }
}

/// A trailing `" comment` off an option line.
fn strip_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        if c == '"' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
            return text[..i].trim_end();
        }
    }
    text
}

/// The modes a map command is for (`nnoremap`: `n`), or `None` when it
/// isn't one. `map` is Normal, Visual and Operator-pending (`nvo`), `map!`
/// Insert and the command line (`ic`).
fn map_command(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "map" | "noremap" | "no" => "nvo",
        "map!" | "noremap!" | "no!" => "ic",
        "nmap" | "nnoremap" | "nn" | "nno" | "nm" => "n",
        "vmap" | "vnoremap" | "vn" | "vno" | "vm" => "v",
        "xmap" | "xnoremap" | "xn" | "xno" | "xm" => "x",
        "smap" | "snoremap" => "s",
        "omap" | "onoremap" | "ono" => "o",
        "imap" | "inoremap" | "ino" | "im" => "i",
        "cmap" | "cnoremap" | "cno" | "cm" => "c",
        "tmap" | "tnoremap" | "tno" => "t",
        "lmap" | "lnoremap" => "l",
        _ => return None,
    })
}

/// `<silent> <C-s> :w<CR>` after a map command.
fn parse_map(mode: &str, rest: &str, from: &str, out: &mut Proposal) {
    let mut rest = rest.trim();
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(arg) = ["<silent>", "<nowait>", "<unique>", "<special>", "<script>", "<buffer>", "<expr>"].into_iter().find(|a| lower.starts_with(a)) else { break };
        if arg == "<buffer>" || arg == "<expr>" {
            out.unmapped(from, if arg == "<buffer>" { "for one buffer only" } else { "works out its keys as Vim runs it" });
            return;
        }
        rest = rest[arg.len()..].trim_start();
    }
    let (lhs, rhs) = rest.split_once(char::is_whitespace).map_or((rest, ""), |(l, r)| (l, r.trim()));
    map(mode, lhs, rhs, from, out);
}

/// One map: a chord for a command Fenix has, in a mode its chords can
/// tell (Normal: `not_typing`; Insert: `typing`).
fn map(mode: &str, lhs: &str, rhs: &str, from: &str, out: &mut Proposal) {
    let when = match mode {
        "n" | "nvo" => "not_typing",
        "i" => "typing",
        "v" | "x" => {
            out.unmapped(from, "only in Visual mode, which Fenix's chords don't tell from Normal");
            return;
        }
        _ => {
            out.unmapped(from, "only in a mode Fenix's chords don't tell apart");
            return;
        }
    };
    let keys = match vim_chord(lhs) {
        Ok(keys) => keys,
        Err(why) => {
            out.unmapped(from, why);
            return;
        }
    };
    let Some(command) = rhs_command(rhs, mode == "i") else {
        out.unmapped(from, "does something Fenix has no command for, or plays keys");
        return;
    };
    out.key(keys, command, "vim", Some(when), from);
}

/// Vim's `<C-s>` as a Fenix chord, `C-s`. A plain key, a sequence or a
/// `<leader>` key isn't one.
fn vim_chord(lhs: &str) -> Result<String, String> {
    let lower = lhs.to_ascii_lowercase();
    if lower.contains("<leader>") || lower.contains("<localleader>") {
        return Err("a leader key: Fenix's leader (SPC) opens its own menu -- give it a [[leader]] entry in keybindings.toml".into());
    }
    let Some(inner) = lhs.strip_prefix('<').and_then(|l| l.strip_suffix('>')).filter(|i| !i.contains('<') && !i.contains('>')) else {
        return Err(if lhs.chars().count() == 1 { "a plain key: Vim's own keys are Fenix's" } else { "a sequence of keys: Fenix's chords are one key with its modifiers" }.into());
    };
    let parts: Vec<&str> = inner.split('-').collect();
    // `<C-->`: the key is `-`.
    let (name, mods) = if inner.ends_with("--") { ("-", &parts[..parts.len() - 2]) } else { parts.split_last().map(|(n, m)| (*n, m)).ok_or("no key")? };
    let (mut ctrl, mut alt, mut shift, mut sup) = (false, false, false, false);
    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "c" => ctrl = true,
            "a" | "m" => alt = true,
            "s" => shift = true,
            "d" => sup = true,
            other => return Err(format!("<{other}-…> isn't a modifier Fenix knows")),
        }
    }
    let named = match name.to_ascii_lowercase().as_str() {
        "up" => Some("Up"),
        "down" => Some("Down"),
        "left" => Some("Left"),
        "right" => Some("Right"),
        "tab" => Some("Tab"),
        "cr" | "enter" | "return" => Some("Enter"),
        "bs" | "backspace" => Some("Backspace"),
        "del" | "delete" => Some("Delete"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "pageup" => Some("PgUp"),
        "pagedown" => Some("PgDn"),
        "space" => Some("SPC"),
        "esc" => Some("Esc"),
        "bslash" => Some("\\"),
        "bar" => Some("|"),
        _ => None,
    };
    let lowered = name.to_ascii_lowercase();
    let fkey = lowered.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=12).contains(n));
    let key = match (named, fkey) {
        (Some(n), _) => n.to_string(),
        (None, Some(n)) => format!("F{n}"),
        // Vim reads `<C-S>` as `<C-s>`.
        (None, None) if name.chars().count() == 1 => if ctrl && !shift { name.to_ascii_lowercase() } else { name.to_string() },
        _ => return Err(format!("<{inner}> isn't a key Fenix knows")),
    };
    if !(ctrl || alt || sup) && fkey.is_none() {
        return Err("a key without Ctrl or Alt: Vim's own keys are Fenix's".into());
    }
    let mut out = String::new();
    for (on, m) in [(ctrl, "C-"), (alt, "M-"), (sup, "s-"), (shift, "S-")] {
        if on {
            out.push_str(m);
        }
    }
    out.push_str(&key);
    Ok(out)
}

/// What a map's right-hand side runs, when it's one Ex command (`:w<CR>`,
/// `<cmd>Telescope find_files<cr>`) or a window key (`<C-w>h`) Fenix
/// has a command for.
fn rhs_command(rhs: &str, insert: bool) -> Option<&'static str> {
    let mut s = rhs.trim().to_string();
    let lower = |s: &str| s.to_ascii_lowercase();
    if insert {
        for prefix in ["<c-\\><c-n>", "<c-\\><c-o>", "<esc>", "<c-o>"] {
            if lower(&s).starts_with(prefix) {
                s = s[prefix.len()..].to_string();
                break;
            }
        }
        // Back to Insert after it.
        for suffix in ["gi", "a", "i", "li"] {
            if lower(&s).ends_with(&format!("<cr>{suffix}")) {
                s.truncate(s.len() - suffix.len());
                break;
            }
        }
    }
    let l = lower(&s);
    let body = if let Some(rest) = l.strip_prefix("<cmd>") {
        rest.strip_suffix("<cr>")?.to_string()
    } else if let Some(rest) = l.strip_prefix(':') {
        let rest = rest.strip_prefix("<c-u>").unwrap_or(rest);
        rest.strip_suffix("<cr>").or_else(|| rest.strip_suffix("<enter>"))?.to_string()
    } else {
        return WINDOW_KEYS.iter().find(|(k, _)| *k == l).map(|(_, c)| *c);
    };
    let body = body.trim().trim_start_matches("silent!").trim_start_matches("silent ").trim();
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = body.trim_end_matches(';');
    EX_COMMANDS.iter().find(|(ex, _)| ex.eq_ignore_ascii_case(body)).map(|(_, c)| *c)
}

/// Window keys a map may play, and Fenix's command for each.
pub const WINDOW_KEYS: &[(&str, &str)] = &[
    ("<c-w>h", "window.navigate_left"),
    ("<c-w>j", "window.navigate_down"),
    ("<c-w>k", "window.navigate_up"),
    ("<c-w>l", "window.navigate_right"),
    ("<c-w><c-h>", "window.navigate_left"),
    ("<c-w><c-j>", "window.navigate_down"),
    ("<c-w><c-k>", "window.navigate_up"),
    ("<c-w><c-l>", "window.navigate_right"),
    ("<c-w>v", "window.split_vertical"),
    ("<c-w>s", "window.split_horizontal"),
    ("<c-w>c", "window.close"),
    ("<c-w>q", "window.close"),
    ("<c-w>o", "window.only"),
    ("<c-w>=", "window.balance"),
    ("<c-w>w", "window.cycle"),
    ("<c-o>", "nav.jump_back"),
    ("<c-i>", "nav.jump_forward"),
];

/// Ex commands (and plugins' commands) Fenix has a command for. Matched
/// whole, any case. (fenix-gui's tests check every one is a command.)
pub const EX_COMMANDS: &[(&str, &str)] = &[
    ("w", "file.save"),
    ("write", "file.save"),
    ("up", "file.save"),
    ("update", "file.save"),
    ("w!", "file.save_force"),
    ("qa", "app.quit"),
    ("qall", "app.quit"),
    ("qa!", "app.quit_force"),
    ("wqa", "app.save_all_and_quit"),
    ("xa", "app.save_all_and_quit"),
    ("xall", "app.save_all_and_quit"),
    ("bn", "buffer.next"),
    ("bnext", "buffer.next"),
    ("bp", "buffer.prev"),
    ("bprev", "buffer.prev"),
    ("bprevious", "buffer.prev"),
    ("bd", "buffer.kill"),
    ("bdelete", "buffer.kill"),
    ("bd!", "buffer.kill_force"),
    ("tabnext", "tab.next"),
    ("tabn", "tab.next"),
    ("tabprevious", "tab.prev"),
    ("tabprev", "tab.prev"),
    ("tabp", "tab.prev"),
    ("vs", "window.split_vertical"),
    ("vsp", "window.split_vertical"),
    ("vsplit", "window.split_vertical"),
    ("sp", "window.split_horizontal"),
    ("split", "window.split_horizontal"),
    ("close", "window.close"),
    ("clo", "window.close"),
    ("only", "window.only"),
    ("on", "window.only"),
    ("sort", "edit.sort_lines"),
    ("e!", "file.reload"),
    ("edit!", "file.reload"),
    ("NERDTreeToggle", "explorer.toggle_sidebar"),
    ("NvimTreeToggle", "explorer.toggle_sidebar"),
    ("Neotree toggle", "explorer.toggle_sidebar"),
    ("Lexplore", "explorer.toggle_sidebar"),
    ("NERDTreeFind", "explorer.reveal_current"),
    ("NvimTreeFindFile", "explorer.reveal_current"),
    ("Neotree reveal", "explorer.reveal_current"),
    ("Telescope find_files", "project.find_file"),
    ("Telescope git_files", "project.find_file"),
    ("Files", "project.find_file"),
    ("GFiles", "project.find_file"),
    ("FZF", "project.find_file"),
    ("CtrlP", "project.find_file"),
    ("FzfLua files", "project.find_file"),
    ("Telescope live_grep", "search.project"),
    ("Rg", "search.project"),
    ("Ag", "search.project"),
    ("FzfLua live_grep", "search.project"),
    ("Telescope oldfiles", "file.recent"),
    ("History", "file.recent"),
    ("Telescope commands", "commands.palette"),
    ("Commands", "commands.palette"),
    ("Telescope current_buffer_fuzzy_find", "search.file"),
    ("Telescope lsp_document_symbols", "code.outline"),
    ("Telescope lsp_dynamic_workspace_symbols", "code.project_symbols"),
    ("Telescope lsp_workspace_symbols", "code.project_symbols"),
    ("Telescope diagnostics", "code.problems"),
    ("Telescope lsp_references", "code.references"),
    ("Telescope lsp_definitions", "code.goto_definition"),
    ("Telescope lsp_implementations", "code.implementations"),
    ("Telescope keymaps", "keys.bindings"),
    ("Git", "git.open"),
    ("G", "git.open"),
    ("Neogit", "git.open"),
    ("LazyGit", "git.open"),
    ("Git blame", "git.blame"),
    ("ToggleTerm", "terminal.toggle"),
    ("lua vim.lsp.buf.definition()", "code.goto_definition"),
    ("lua vim.lsp.buf.references()", "code.references"),
    ("lua vim.lsp.buf.implementation()", "code.implementations"),
    ("lua vim.lsp.buf.hover()", "code.hover"),
    ("lua vim.lsp.buf.rename()", "code.lsp_rename"),
    ("lua vim.lsp.buf.code_action()", "code.lsp_code_action"),
    ("lua vim.lsp.buf.format()", "code.format_buffer"),
    ("lua vim.lsp.buf.formatting()", "code.format_buffer"),
    ("lua vim.lsp.buf.document_symbol()", "code.outline"),
    ("lua vim.lsp.buf.workspace_symbol()", "code.project_symbols"),
    ("lua vim.diagnostic.goto_next()", "nav.next_problem"),
    ("lua vim.diagnostic.goto_prev()", "nav.prev_problem"),
    ("lua vim.diagnostic.setloclist()", "code.problems"),
];

/// A run of lines Fenix can't read, said once.
struct Skipped {
    first: usize,
    last: usize,
    text: String,
    why: &'static str,
}

fn flush(skipped: &mut Option<Skipped>, out: &mut Proposal) {
    if let Some(s) = skipped.take() {
        let lines = if s.first == s.last { format!("line {}", s.first) } else { format!("lines {}-{}", s.first, s.last) };
        out.unmapped(format!("{lines}: {}", s.text), s.why);
    }
}

fn skip(skipped: &mut Option<Skipped>, out: &mut Proposal, n: usize, line: &str, why: &'static str) {
    match skipped {
        Some(s) if s.last + 1 >= n && s.why == why => s.last = n,
        _ => {
            flush(skipped, out);
            *skipped = Some(Skipped { first: n, last: n, text: short(line), why });
        }
    }
}

const NOT_RUN: &str = "Vimscript the importer doesn't run (a condition, a function, an autocommand...)";
const NOT_RUN_LUA: &str = "Lua the importer doesn't run: only plain options, colorschemes and simple maps are read";

pub(super) fn vimscript(text: &str, catalog: &Catalog, out: &mut Proposal) {
    let mut opts = Options::default();
    let mut skipped: Option<Skipped> = None;
    // Lines inside an `if`, a function or an augroup are what Vim runs
    // under a condition: not read.
    let mut depth = 0usize;
    // `\` at the start of a line goes on from the line before.
    let mut lines: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        match (trimmed.strip_prefix('\\'), lines.last_mut()) {
            (Some(more), Some((_, last))) => last.push_str(more),
            _ => lines.push((i + 1, raw.to_string())),
        }
    }
    for (n, line) in lines {
        let t = line.trim();
        if t.is_empty() || t.starts_with('"') {
            continue;
        }
        let first = t.trim_start_matches(':').split_whitespace().next().unwrap_or("");
        let opens = matches!(first, "if" | "function" | "function!" | "fu" | "fun" | "fun!" | "for" | "while" | "try") || (first == "augroup" && !t.ends_with(" END") && !t.ends_with(" end"));
        let closes = matches!(first, "endif" | "en" | "endfunction" | "endf" | "endfun" | "endfor" | "endfo" | "endwhile" | "endw" | "endtry") || (first == "augroup" && (t.ends_with(" END") || t.ends_with(" end")));
        if depth > 0 || opens || closes {
            if opens {
                depth += 1;
            }
            if closes {
                depth = depth.saturating_sub(1);
            }
            skip(&mut skipped, out, n, t, NOT_RUN);
            // A block said whole, on its own.
            if depth == 0 {
                flush(&mut skipped, out);
            }
            continue;
        }
        // `a | b`: each, unless it's a map (whose `|` is its own).
        let from = format!("line {n}");
        let parts: Vec<&str> = if map_command(first).is_some() { vec![t] } else { t.split(" | ").collect() };
        for part in parts {
            if !vim_command(part, &from, &mut opts, out) {
                let why = if part.trim_start().starts_with("autocmd") || part.trim_start().starts_with("au ") { NOT_RUN } else { NO_SETTING };
                skip(&mut skipped, out, n, part, why);
            } else {
                flush(&mut skipped, out);
            }
        }
    }
    flush(&mut skipped, out);
    opts.settle(catalog, out);
}

/// A Lua string literal at the start of `s` (`"x"`, `'x'`, `[[x]]`), and
/// what follows it.
fn lua_string(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix("[[") {
        let end = rest.find("]]")?;
        return Some((rest[..end].to_string(), &rest[end + 2..]));
    }
    let quote = s.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, e)) => out.push(e),
                None => return None,
            },
            c if c == quote => return Some((out, &s[1 + i + 1..])),
            c => out.push(c),
        }
    }
    None
}

/// `x = value` in Lua, as `set`'s word for it: `true` is `x`, `false`
/// `nox`, anything else `x=value`. `None` for a value that isn't a plain
/// literal.
fn lua_option(name: &str, value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches(';').trim();
    let value = value.split_once("--").map_or(value, |(v, _)| v.trim());
    match value {
        "true" => Some(name.to_string()),
        "false" => Some(format!("no{name}")),
        v if v.parse::<f64>().is_ok() => Some(format!("{name}={v}")),
        v => {
            let (text, rest) = lua_string(v)?;
            rest.trim().is_empty().then(|| format!("{name}={}", text.replace(' ', "\\ ")))
        }
    }
}

/// How much a line of Lua opens (`function`, `if`, `do`, `{`, `(`) less
/// what it closes (`end`, `}`, `)`), outside its strings and comments.
fn lua_depth(line: &str) -> isize {
    let mut depth = 0isize;
    let mut word = String::new();
    let mut chars = line.chars().peekable();
    let count = |word: &mut String, depth: &mut isize| {
        match word.as_str() {
            "function" | "do" | "if" | "repeat" => *depth += 1,
            "end" | "until" => *depth -= 1,
            _ => {}
        }
        word.clear();
    };
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                count(&mut word, &mut depth);
                while let Some(s) = chars.next() {
                    if s == '\\' {
                        chars.next();
                    } else if s == c {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => break,
            '[' if chars.peek() == Some(&'[') => {
                // A long string: to its `]]`, on this line.
                count(&mut word, &mut depth);
                chars.next();
                let mut last = '\0';
                for s in chars.by_ref() {
                    if last == ']' && s == ']' {
                        break;
                    }
                    last = s;
                }
            }
            '{' | '(' => {
                count(&mut word, &mut depth);
                depth += 1;
            }
            '}' | ')' => {
                count(&mut word, &mut depth);
                depth -= 1;
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' => word.push(c),
            _ => count(&mut word, &mut depth),
        }
    }
    count(&mut word, &mut depth);
    depth
}

pub(super) fn lua(text: &str, catalog: &Catalog, out: &mut Proposal) {
    let mut opts = Options::default();
    let mut skipped: Option<Skipped> = None;
    let mut depth = 0isize;
    let mut in_comment = false;
    // `local opt = vim.opt`: what `opt.` stands for.
    let mut aliases: Vec<String> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let t = raw.trim();
        if in_comment {
            if t.contains("]]") {
                in_comment = false;
            }
            continue;
        }
        if t.starts_with("--[[") || t.starts_with("--[=[") {
            in_comment = !t.contains("]]");
            continue;
        }
        if t.is_empty() || t.starts_with("--") {
            continue;
        }
        let change = lua_depth(t);
        if depth > 0 || change != 0 {
            depth = (depth + change).max(0);
            skip(&mut skipped, out, n, t, NOT_RUN_LUA);
            if depth == 0 {
                flush(&mut skipped, out);
            }
            continue;
        }
        let from = format!("line {n}");
        if lua_line(t, &from, &mut aliases, &mut opts, out) {
            flush(&mut skipped, out);
        } else {
            skip(&mut skipped, out, n, t, NOT_RUN_LUA);
        }
    }
    flush(&mut skipped, out);
    opts.settle(catalog, out);
}

/// One whole statement of Lua: whether it was one the importer reads.
fn lua_line(t: &str, from: &str, aliases: &mut Vec<String>, opts: &mut Options, out: &mut Proposal) -> bool {
    let t = t.trim_end_matches(';').trim();
    // `local opt = vim.opt`
    if let Some(rest) = t.strip_prefix("local ") {
        if let Some((name, value)) = rest.split_once('=') {
            if matches!(value.trim(), "vim.opt" | "vim.o" | "vim.wo" | "vim.bo" | "vim.go") {
                aliases.push(format!("{}.", name.trim()));
                return true;
            }
        }
        return false;
    }
    // `vim.g.mapleader = " "`
    if let Some(rest) = t.strip_prefix("vim.g.mapleader").or_else(|| t.strip_prefix("vim.g[\"mapleader\"]")) {
        let value = rest.split_once('=').and_then(|(_, v)| lua_string(v)).map(|(s, _)| s).unwrap_or_default();
        return vim_command(&format!("let mapleader = \"{value}\""), from, opts, out);
    }
    // `vim.opt.number = true`, `vim.o.tabstop = 4`, `opt.wrap = false`
    let prefixes = ["vim.opt.", "vim.o.", "vim.wo.", "vim.bo.", "vim.go.", "vim.opt_global.", "vim.opt_local."];
    let prefix = prefixes.iter().map(|p| p.to_string()).chain(aliases.iter().cloned()).find(|p| t.starts_with(p.as_str()));
    if let Some(prefix) = prefix {
        let rest = &t[prefix.len()..];
        let Some((name, value)) = rest.split_once('=') else { return false };
        let name = name.trim();
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        let Some(word) = lua_option(name, value) else { return false };
        set_options(&word, from, opts, out);
        return true;
    }
    // `vim.cmd("colorscheme x")`, `vim.cmd [[set nu]]`, `vim.cmd.colorscheme("x")`
    if let Some(rest) = t.strip_prefix("vim.cmd.colorscheme") {
        let inner = rest.trim().trim_start_matches('(').trim_end_matches(')');
        let Some((name, after)) = lua_string(inner) else { return false };
        return after.trim().is_empty() && vim_command(&format!("colorscheme {name}"), from, opts, out);
    }
    for call in ["vim.cmd", "vim.api.nvim_command", "vim.api.nvim_exec2", "vim.api.nvim_exec"] {
        if let Some(rest) = t.strip_prefix(call) {
            let inner = rest.trim();
            let inner = inner.strip_prefix('(').map(|i| i.trim()).unwrap_or(inner);
            let Some((command, _)) = lua_string(inner) else { return false };
            if command.contains('\n') {
                return false;
            }
            return vim_command(&command, from, opts, out);
        }
    }
    // `vim.keymap.set("n", "<C-s>", "<cmd>w<cr>", { silent = true })`
    for call in ["vim.keymap.set", "vim.api.nvim_set_keymap"] {
        if let Some(rest) = t.strip_prefix(call) {
            let Some(args) = rest.trim().strip_prefix('(') else { return false };
            let (modes, after) = if let Some(list) = args.trim_start().strip_prefix('{') {
                let Some((inside, after)) = list.split_once('}') else { return false };
                let mut modes = Vec::new();
                let mut s = inside;
                while let Some((m, rest)) = lua_string(s) {
                    modes.push(m);
                    s = rest.trim_start().trim_start_matches(',');
                }
                (modes, after)
            } else {
                let Some((m, after)) = lua_string(args) else { return false };
                (vec![m], after)
            };
            let after = after.trim_start().trim_start_matches(',');
            let Some((lhs, after)) = lua_string(after) else { return false };
            let after = after.trim_start().trim_start_matches(',');
            let shown = format!("{from}: {}", short(t));
            let Some((rhs, _)) = lua_string(after) else {
                out.unmapped(shown, "runs a Lua function, which the importer doesn't run");
                return true;
            };
            for mode in modes {
                let mode = if mode.is_empty() { "nvo".to_string() } else { mode };
                map(&mode, &lhs, &rhs, &shown, out);
            }
            return true;
        }
    }
    false
}

#[cfg(test)]
pub(super) fn vim_chord_for_tests(lhs: &str) -> Result<String, String> {
    vim_chord(lhs)
}
