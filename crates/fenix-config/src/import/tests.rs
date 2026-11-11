use super::*;

fn catalog() -> Catalog {
    let themes = [
        ("Orbit Dark", false),
        ("Visual Studio Dark", false),
        ("Visual Studio Light", true),
        ("One Dark", false),
        ("One Light", true),
        ("GitHub Dark", false),
        ("GitHub Light", true),
        ("Gruvbox Dark", false),
        ("Gruvbox Light", true),
        ("Tokyo Night Storm", false),
        ("Tokyo Night Day", true),
        ("Catppuccin Mocha", false),
        ("Catppuccin Latte", true),
        ("Rosé Pine", false),
        ("Rosé Pine Dawn", true),
        ("Solarized Dark", false),
        ("Solarized Light", true),
    ];
    Catalog { themes: themes.iter().map(|(n, l)| (n.to_string(), *l)).collect() }
}

fn setting<'a>(p: &'a Proposal, key: &str) -> Option<&'a SettingChange> {
    p.settings.iter().find(|s| s.key == key)
}

fn value(p: &Proposal, key: &str) -> Option<Value> {
    setting(p, key).map(|s| s.value.clone())
}

fn text(s: &str) -> Option<Value> {
    Some(Value::Text(s.into()))
}

const SETTINGS: &str = r#"{
    // My editor
    "editor.fontFamily": "'Cascadia Code', Consolas, monospace",
    "editor.fontSize": 15,
    "workbench.colorTheme": "GitHub Dark Default",
    "editor.tabSize": 2,
    "editor.insertSpaces": false,
    "editor.wordWrap": "bounded",
    "editor.formatOnSave": true,
    "editor.lineNumbers": "relative",
    "files.autoSave": "afterDelay",
    "files.exclude": { "**/node_modules": true, "**/.git": false, "build/out": true },
    "git.autofetch": true,
    "git.autofetchPeriod": 300,
    "editor.minimap.enabled": false,
    "[python]": { "editor.formatOnSave": false },
}"#;

#[test]
fn vs_code_s_settings_become_fenix_s() {
    let p = read_vscode(Some(SETTINGS), None, false, &catalog());
    assert_eq!(value(&p, "editor.font_family"), text("Cascadia Code"));
    assert_eq!(value(&p, "editor.font_size"), Some(Value::Float(15.0)));
    assert_eq!(value(&p, "editor.theme"), text("GitHub Dark"));
    assert!(setting(&p, "editor.theme").unwrap().note.is_some(), "the nearest, said so");
    assert_eq!(value(&p, "editor.tab_width"), Some(Value::Int(2)));
    assert_eq!(value(&p, "editor.indent_width"), Some(Value::Int(2)));
    assert_eq!(value(&p, "editor.wrap"), Some(Value::Bool(true)));
    assert_eq!(value(&p, "editor.format_on_save"), Some(Value::Bool(true)));
    assert_eq!(value(&p, "editor.line_numbers"), text("relative"));
    assert_eq!(value(&p, "editor.auto_save"), text("after_delay"));
    assert_eq!(value(&p, "explorer.hide"), Some(Value::List(vec!["node_modules".into()])));
    assert_eq!(value(&p, "git.auto_fetch"), Some(Value::Int(5)));
    assert_eq!(value(&p, "editor.keys"), text("standard"), "VS Code's keys");
    let why = |from: &str| p.unmapped.iter().find(|u| u.from.contains(from)).map(|u| u.why.clone());
    assert!(why("insertSpaces").unwrap().contains("spaces"));
    assert!(why("minimap").is_some(), "said, not dropped");
    assert!(why("[python]").unwrap().contains("one language"));
    assert!(why("build/out").is_some(), "a path isn't a name");
}

#[test]
fn every_setting_the_importer_writes_is_one_fenix_has_and_takes_that_value() {
    let p = read_vscode(Some(SETTINGS), None, true, &catalog());
    let vim = read_vim(VIMRC, false, &catalog());
    let lua = read_vim(INIT_LUA, true, &catalog());
    for s in p.settings.iter().chain(&vim.settings).chain(&lua.settings) {
        let setting = crate::setting(s.key).unwrap_or_else(|| panic!("{} isn't a setting", s.key));
        let mut config = Config::empty(std::env::temp_dir().join("fenix-import-none.toml"), std::env::temp_dir());
        setting.set(&mut config, Some(s.value.clone())).unwrap_or_else(|e| panic!("{} = {:?}: {e}", s.key, s.value));
    }
}

#[test]
fn vscodevim_means_vim_keys() {
    let p = read_vscode(Some(r#"{ "vim.leader": "<space>", "vim.useSystemClipboard": true }"#), None, false, &catalog());
    assert_eq!(value(&p, "editor.keys"), text("vim"));
    assert_eq!(p.unmapped.len(), 1, "VSCodeVim's own settings, said once: {:?}", p.unmapped);
    assert_eq!(value(&read_vscode(Some("{}"), None, true, &catalog()), "editor.keys"), text("vim"), "the extension installed");
}

#[test]
fn themes_match_by_name_family_and_lightness() {
    let c = catalog();
    assert_eq!(theme_for("Default Dark Modern", None, &c), Some(("Visual Studio Dark".into(), None)));
    assert_eq!(theme_for("Default Light+", None, &c).unwrap().0, "Visual Studio Light");
    assert_eq!(theme_for("Rosé Pine Dawn", None, &c), Some(("Rosé Pine Dawn".into(), None)));
    assert_eq!(theme_for("rose-pine", Some(true), &c).unwrap().0, "Rosé Pine Dawn", "the light one, as background says");
    assert_eq!(theme_for("gruvbox", None, &c).unwrap().0, "Gruvbox Dark");
    assert_eq!(theme_for("gruvbox", Some(true), &c).unwrap().0, "Gruvbox Light");
    assert_eq!(theme_for("tokyonight", None, &c).unwrap().0, "Tokyo Night Storm");
    assert_eq!(theme_for("One Dark Pro", None, &c).unwrap().0, "One Dark");
    assert_eq!(theme_for("Quiet Light", None, &c).unwrap().0, "Visual Studio Light", "unknown, but light");
    assert_eq!(theme_for("Abyss", None, &c), None, "unknown and dark: said, not guessed");
}

const KEYBINDINGS: &str = r#"// Place your key bindings in this file
[
    { "key": "ctrl+shift+d", "command": "editor.action.copyLinesDownAction", "when": "editorTextFocus && !editorReadonly" },
    { "key": "ctrl+e", "command": "workbench.action.quickOpen" },
    { "key": "ctrl+k ctrl+t", "command": "workbench.action.selectTheme" },
    { "key": "ctrl+k ctrl+c", "command": "editor.action.commentLine" },
    { "key": "ctrl+d", "command": "-editor.action.addSelectionToNextFindMatch" },
    { "key": "f5", "command": "workbench.action.debug.start", "when": "debuggersAvailable && !inDebugMode" },
    { "key": "alt+j", "command": "editor.action.moveLinesDownAction", "when": "editorTextFocus" },
    { "key": "ctrl+`", "command": "workbench.action.terminal.focus", "when": "terminalFocus" },
    { "key": "j", "command": "editor.action.moveLinesDownAction" },
]"#;

#[test]
fn vs_code_s_keybindings_become_standard_chords() {
    let p = read_vscode(None, Some(KEYBINDINGS), false, &catalog());
    let keys: Vec<(&str, &str, Option<&str>)> = p.keys.iter().map(|k| (k.keys.as_str(), k.command.as_str(), k.when)).collect();
    assert_eq!(keys, vec![("C-S-d", "edit.copy_lines_down", Some("text")), ("C-e", "project.find_file", None), ("M-j", "edit.move_lines_down", Some("text")), ("C-`", "terminal.focus", Some("terminal"))]);
    assert!(p.keys.iter().all(|k| k.scheme == "standard"));
    let why = |from: &str| p.unmapped.iter().find(|u| u.from.starts_with(from)).map(|u| u.why.clone()).unwrap_or_else(|| panic!("{from}: {:?}", p.unmapped));
    assert!(why("ctrl+k ctrl+t").contains("no command"), "a command Fenix doesn't have");
    assert!(why("ctrl+k ctrl+c").contains("sequence"));
    assert!(why("ctrl+d").contains("takes"));
    assert!(why("f5").contains("when"), "a when Fenix can't tell");
    assert!(why("j ").contains("types"));
}

#[test]
fn vs_code_key_names_are_fenix_s() {
    assert_eq!(vscode::chord("ctrl+shift+alt+pageDown").unwrap(), "C-M-S-PgDn");
    assert_eq!(vscode::chord("shift+f12").unwrap(), "S-F12");
    assert_eq!(vscode::chord("ctrl++").unwrap(), "C-+");
    assert_eq!(vscode::chord("meta+p").unwrap(), "s-p");
    assert!(vscode::chord("tab").is_err(), "a plain key types");
    assert!(vscode::chord("ctrl+numpad1").is_err());
}

const VIMRC: &str = r#"" my vimrc
set nocompatible
set number relativenumber
set tabstop=8 shiftwidth=0 expandtab
set nowrap " long lines go off the side
set background=light
set ignorecase smartcase
set guifont=JetBrains\ Mono:h12
colorscheme gruvbox
let mapleader = " "
Plug 'tpope/vim-fugitive'
nnoremap <silent> <C-s> :w<CR>
inoremap <C-s> <Esc>:w<CR>a
nnoremap <C-p> :Files<CR>
nnoremap <C-h> <C-w>h
nnoremap <leader>w :w<CR>
vnoremap <C-c> "+y
nnoremap ; :
if has('gui_running')
  set guifont=Consolas:h11
endif
function! Strip()
  %s/\s\+$//e
endfunction
autocmd BufWritePre * call Strip()
"#;

#[test]
fn a_vimrc_s_options_and_simple_maps_come_over() {
    let p = read_vim(VIMRC, false, &catalog());
    assert_eq!(value(&p, "editor.line_numbers"), text("relative"));
    assert!(setting(&p, "editor.line_numbers").unwrap().note.is_some(), "hybrid numbers aren't quite");
    assert_eq!(value(&p, "editor.tab_width"), Some(Value::Int(8)));
    assert_eq!(value(&p, "editor.indent_width"), Some(Value::Int(8)), "shiftwidth=0 is the tabstop's");
    assert_eq!(value(&p, "editor.wrap"), Some(Value::Bool(false)));
    assert_eq!(value(&p, "editor.theme"), text("Gruvbox Light"), "background=light");
    assert_eq!(value(&p, "search.smart_case"), Some(Value::Bool(true)));
    assert_eq!(value(&p, "editor.font_family"), text("JetBrains Mono"), "not the one inside the if");
    assert_eq!(value(&p, "editor.font_size"), Some(Value::Float(16.0)), "12 points");
    let keys: Vec<(&str, &str, Option<&str>)> = p.keys.iter().map(|k| (k.keys.as_str(), k.command.as_str(), k.when)).collect();
    assert_eq!(keys, vec![("C-s", "file.save", Some("not_typing")), ("C-s", "file.save", Some("typing")), ("C-p", "project.find_file", Some("not_typing")), ("C-h", "window.navigate_left", Some("not_typing"))]);
    assert!(p.keys.iter().all(|k| k.scheme == "vim"));
    let why = |from: &str| p.unmapped.iter().find(|u| u.from.contains(from)).map(|u| u.why.clone()).unwrap_or_else(|| panic!("{from}: {:#?}", p.unmapped));
    assert!(why("nocompatible").contains("no setting"));
    assert!(why("mapleader").contains("space bar"));
    assert!(why("vim-fugitive").contains("plugin"));
    assert!(why("<leader>w").contains("leader"));
    assert!(why("vnoremap").contains("Visual"));
    assert!(why("nnoremap ;").contains("plain key"));
    assert!(why("lines 19-21: if").contains("doesn't run"), "the if, said once");
    assert!(why("lines 22-24").contains("doesn't run"), "the function");
    assert!(why("autocmd").contains("doesn't run"));
}

#[test]
fn a_vim_map_for_one_key_with_its_modifiers_is_a_chord() {
    assert_eq!(vim::vim_chord_for_tests("<C-S>").unwrap(), "C-s", "Vim's Ctrl keys have no case");
    assert_eq!(vim::vim_chord_for_tests("<M-j>").unwrap(), "M-j");
    assert_eq!(vim::vim_chord_for_tests("<S-F5>").unwrap(), "S-F5");
    assert_eq!(vim::vim_chord_for_tests("<C-Up>").unwrap(), "C-Up");
    assert!(vim::vim_chord_for_tests("<Up>").is_err());
    assert!(vim::vim_chord_for_tests("gd").is_err());
    assert!(vim::vim_chord_for_tests("<C-w>h").is_err());
}

const INIT_LUA: &str = r#"-- init.lua
vim.g.mapleader = " "
local opt = vim.opt
opt.number = true
opt.relativenumber = false
vim.o.tabstop = 4
vim.opt.shiftwidth = 4 -- indent
vim.opt.wrap = false
vim.opt.clipboard = "unnamedplus"
vim.cmd("colorscheme tokyonight")
vim.keymap.set("n", "<C-s>", "<cmd>w<cr>", { silent = true })
vim.keymap.set({ "n", "i" }, "<F2>", "<cmd>lua vim.lsp.buf.rename()<CR>")
vim.keymap.set("n", "<leader>ff", "<cmd>Telescope find_files<cr>")
vim.keymap.set("n", "K", function() vim.lsp.buf.hover() end)
require("lazy").setup({
  "folke/tokyonight.nvim",
  { "nvim-telescope/telescope.nvim", tag = "0.1.5" },
})
if vim.g.neovide then
  vim.o.guifont = "Fira Code:h14"
end
--[[ an old
vim.opt.wrap = true
]]
"#;

#[test]
fn an_init_lua_s_plain_options_and_maps_come_over_and_nothing_is_run() {
    let p = read_vim(INIT_LUA, true, &catalog());
    assert_eq!(p.source, "Neovim");
    assert_eq!(value(&p, "editor.line_numbers"), text("absolute"));
    assert_eq!(value(&p, "editor.tab_width"), Some(Value::Int(4)));
    assert_eq!(value(&p, "editor.indent_width"), Some(Value::Int(4)));
    assert_eq!(value(&p, "editor.wrap"), Some(Value::Bool(false)), "not the commented-out one");
    assert_eq!(value(&p, "editor.theme"), text("Tokyo Night Storm"));
    assert_eq!(value(&p, "editor.font_family"), None, "inside an if");
    let keys: Vec<(&str, &str, Option<&str>)> = p.keys.iter().map(|k| (k.keys.as_str(), k.command.as_str(), k.when)).collect();
    assert_eq!(keys, vec![("C-s", "file.save", Some("not_typing")), ("F2", "code.lsp_rename", Some("not_typing")), ("F2", "code.lsp_rename", Some("typing"))]);
    let why = |from: &str| p.unmapped.iter().find(|u| u.from.contains(from)).map(|u| u.why.clone()).unwrap_or_else(|| panic!("{from}: {:#?}", p.unmapped));
    assert!(why("clipboard").contains("no setting"));
    assert!(why("<leader>ff").contains("leader"));
    assert!(why("function() vim.lsp").contains("Lua function"));
    assert!(why("lines 15-18").contains("doesn't run"), "the plugins' setup, said once");
    assert!(why("lines 19-21").contains("doesn't run"));
}

#[test]
fn sources_are_found_where_each_editor_keeps_them() {
    let dir = std::env::temp_dir().join(format!("fenix-import-sources-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let home = dir.join("home");
    let config = home.join(".config");
    for file in [config.join("Code/User/settings.json"), config.join("VSCodium/User/keybindings.json"), config.join("nvim/init.lua"), home.join(".vimrc"), home.join(".vscode/extensions/vscodevim.vim-1.27.0/package.json")] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "{}").unwrap();
    }
    let places = Places { home: Some(home.clone()), xdg_config: Some(config.clone()), ..Places::default() };
    let found = find_sources(&places);
    let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["VS Code", "VSCodium", "Neovim", "Vim"]);
    assert!(found[0].vim_extension, "VSCodeVim in ~/.vscode/extensions");
    assert!(!found[1].vim_extension);
    assert_eq!(found[2].files, [config.join("nvim/init.lua")]);

    // Windows keeps VS Code's under %APPDATA%, Neovim's under %LOCALAPPDATA%, and ~/_vimrc first.
    let appdata = dir.join("AppData/Roaming");
    let local = dir.join("AppData/Local");
    for file in [appdata.join("Cursor/User/settings.json"), local.join("nvim/init.vim"), home.join("_vimrc")] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "").unwrap();
    }
    let windows = Places { home: Some(home.clone()), appdata: Some(appdata), local_appdata: Some(local.clone()), xdg_config: None, windows: true, mac: false };
    let found = find_sources(&windows);
    let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Cursor", "Neovim", "Vim"]);
    assert_eq!(found[1].files, [local.join("nvim/init.vim")]);
    assert_eq!(found[2].files, [home.join("_vimrc")]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_plan_keeps_yours_and_apply_writes_only_what_s_new_unless_replacing() {
    let dir = std::env::temp_dir().join(format!("fenix-import-apply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let settings = dir.join("settings.toml");
    std::fs::write(&settings, "# mine\n[editor]\ntab_width = 4\nindent_width = 2\n").unwrap();
    let keybindings = dir.join("keybindings.toml");
    std::fs::write(&keybindings, "# my keys\n[[keys]]\nkeys = \"C-e\"\ncommand = \"file.recent\"\nscheme = \"standard\"\n").unwrap();

    let mut config = Config::load_at(settings.clone(), dir.join("state")).unwrap();
    let proposal = read_vscode(Some(r#"{ "editor.tabSize": 2, "editor.formatOnSave": true }"#), Some(KEYBINDINGS), false, &catalog());
    let mut plan = plan(proposal, &config, std::fs::read_to_string(&keybindings).ok().as_deref());
    let fate = |key: &str| plan.settings.iter().find(|(s, _)| s.key == key).map(|(_, f)| f.clone()).unwrap();
    assert_eq!(fate("editor.tab_width"), Fate::Yours("4".into()), "yours, shown");
    assert_eq!(fate("editor.indent_width"), Fate::Same);
    assert_eq!(fate("editor.format_on_save"), Fate::New);
    let key_fate = |plan: &Plan, keys: &str| plan.keys.iter().find(|(k, _)| k.keys == keys).map(|(_, f)| f.clone()).unwrap();
    assert_eq!(key_fate(&plan, "C-e"), Fate::Yours("file.recent".into()));
    plan.mark_same_keys(|k| k.keys == "C-`");
    assert_eq!(key_fate(&plan, "C-`"), Fate::Same, "Fenix binds it so already");
    assert!(plan.conflicts());
    assert_eq!(plan.writes(false), (2, 2), "format on save and the keys; standard keys too");

    let applied = apply(&plan, false, &mut config, &keybindings);
    assert_eq!((applied.settings, applied.keys), (2, 2), "{:?}", applied.failed);
    let written = std::fs::read_to_string(&settings).unwrap();
    assert!(written.starts_with("# mine\n"), "your comments stay: {written}");
    assert!(written.contains("tab_width = 4"), "yours kept: {written}");
    assert!(written.contains("format_on_save = true"), "{written}");
    let keys = std::fs::read_to_string(&keybindings).unwrap();
    assert!(keys.contains("command = \"file.recent\""), "yours kept: {keys}");
    assert!(keys.contains("# from VS Code\n[[keys]]\nkeys = \"C-S-d\""), "{keys}");
    assert!(!keys.contains("C-`"), "nothing written for what Fenix has already: {keys}");

    // Replacing: yours that differ go too, and only those.
    let mut config = Config::load_at(settings.clone(), dir.join("state")).unwrap();
    let plan = super::plan(read_vscode(Some(r#"{ "editor.tabSize": 2 }"#), Some(KEYBINDINGS), false, &catalog()), &config, std::fs::read_to_string(&keybindings).ok().as_deref());
    let applied = apply(&plan, true, &mut config, &keybindings);
    assert!(applied.failed.is_empty(), "{:?}", applied.failed);
    assert!(std::fs::read_to_string(&settings).unwrap().contains("tab_width = 2"));
    let keys = std::fs::read_to_string(&keybindings).unwrap();
    assert!(keys.contains("keys = \"C-e\"\ncommand = \"project.find_file\""), "{keys}");
    assert_eq!(keys.matches("keys = \"C-S-d\"").count(), 1, "the same entry, not another: {keys}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_that_can_t_be_read_is_said_and_the_rest_still_read() {
    let p = read_vscode(Some("{ not json"), Some(r#"[{ "key": "ctrl+e", "command": "workbench.action.quickOpen" }]"#), false, &catalog());
    assert!(p.unmapped.iter().any(|u| u.from == "settings.json" && u.why.contains("line 1")), "{:?}", p.unmapped);
    assert_eq!(p.keys.len(), 1);
}
