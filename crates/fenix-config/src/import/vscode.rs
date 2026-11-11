//! VS Code's `settings.json` and `keybindings.json`, as Fenix settings
//! and `[[keys]]`.

use serde_json::Value as Json;

use super::{jsonc, theme_for, Catalog, Proposal};
use crate::Value;

/// `"key": value` as it was written, for what a change came from.
fn shown(key: &str, value: &Json) -> String {
    format!("\"{key}\": {}", value)
}

const NO_SETTING: &str = "Fenix has no setting for this";

pub(super) fn settings(text: &str, vim_extension: bool, catalog: &Catalog, out: &mut Proposal) {
    let json = match jsonc::parse(text) {
        Ok(json) => json,
        Err(e) => {
            out.unmapped("settings.json", format!("couldn't be read: {e}"));
            return;
        }
    };
    let Some(map) = json.as_object() else {
        out.unmapped("settings.json", "isn't an object of settings");
        return;
    };
    let get = |key: &str| map.get(key);
    // VSCodeVim's settings say its keys were Vim's.
    let vim_settings = map.keys().filter(|k| k.starts_with("vim.")).count();
    if vim_extension || vim_settings > 0 {
        let from = if vim_settings > 0 { format!("{vim_settings} VSCodeVim settings (\"vim.*\")") } else { "the VSCodeVim extension".to_string() };
        out.set("editor.keys", Value::Text("vim".into()), from.clone(), None);
        if vim_settings > 0 {
            out.unmapped(from, "VSCodeVim's own settings: Fenix's Vim keys are its own, and SPC is their leader");
        }
    } else {
        out.set("editor.keys", Value::Text("standard".into()), "VS Code's keys", Some("Ctrl+K opens Fenix's menu of everything else".into()));
    }
    // The indent: tabSize, unless indentSize says otherwise.
    let indent = get("editor.indentSize").and_then(Json::as_i64);
    for (key, value) in map {
        let from = shown(key, value);
        match key.as_str() {
            "editor.fontFamily" => match value.as_str().and_then(first_font) {
                Some(font) => out.set("editor.font_family", Value::Text(font.clone()), from, value.as_str().filter(|v| v.contains(',')).map(|_| format!("the first of the list, {font}"))),
                None => out.unmapped(from, "names no font but a generic one"),
            },
            "editor.fontSize" => match value.as_f64() {
                Some(size) if (6.0..=48.0).contains(&size) => out.set("editor.font_size", Value::Float(size), from, None),
                _ => out.unmapped(from, "Fenix's text size is from 6 to 48"),
            },
            "workbench.colorTheme" => match value.as_str() {
                Some(name) => match theme_for(name, None, catalog) {
                    Some((theme, note)) => out.set("editor.theme", Value::Text(theme), from, note),
                    None => out.unmapped(from, "Fenix has no theme like it (SPC t p lists them)"),
                },
                None => out.unmapped(from, "isn't a theme's name"),
            },
            "editor.tabSize" => match value.as_i64() {
                Some(n @ 1..=16) => {
                    out.set("editor.tab_width", Value::Int(n), from.clone(), None);
                    if indent.is_none() {
                        out.set("editor.indent_width", Value::Int(n), from, None);
                    }
                }
                _ => out.unmapped(from, "Fenix's tab width is from 1 to 16"),
            },
            "editor.indentSize" => match value.as_i64() {
                Some(n @ 1..=16) => out.set("editor.indent_width", Value::Int(n), from, None),
                // "tabSize": the same as the tab, set above.
                _ if value.as_str() == Some("tabSize") => {}
                _ => out.unmapped(from, "Fenix's indent width is from 1 to 16"),
            },
            "editor.insertSpaces" if value.as_bool() == Some(false) => out.unmapped(from, "Fenix always indents with spaces"),
            "editor.insertSpaces" => {}
            "editor.wordWrap" => match value.as_str() {
                Some("off") => out.set("editor.wrap", Value::Bool(false), from, None),
                Some("on") => out.set("editor.wrap", Value::Bool(true), from, None),
                Some("wordWrapColumn" | "bounded") => out.set("editor.wrap", Value::Bool(true), from, Some("at the pane's width: Fenix doesn't wrap at a column".into())),
                _ => out.unmapped(from, "isn't one of VS Code's wrap settings"),
            },
            "editor.formatOnSave" => bool_setting(out, "editor.format_on_save", value, from),
            "editor.lineNumbers" => match value.as_str() {
                Some("on") => out.set("editor.line_numbers", Value::Text("absolute".into()), from, None),
                Some("interval") => out.set("editor.line_numbers", Value::Text("absolute".into()), from, Some("every line's number: Fenix has no interval".into())),
                Some("relative") => out.set("editor.line_numbers", Value::Text("relative".into()), from, None),
                Some("off") => out.set("editor.line_numbers", Value::Text("off".into()), from, None),
                _ => out.unmapped(from, "isn't one of VS Code's line number settings"),
            },
            "files.autoSave" => match value.as_str() {
                Some("off") => out.set("editor.auto_save", Value::Text("off".into()), from, None),
                Some("afterDelay") => out.set("editor.auto_save", Value::Text("after_delay".into()), from, None),
                Some("onFocusChange") => out.set("editor.auto_save", Value::Text("on_focus_change".into()), from, None),
                Some("onWindowChange") => out.set("editor.auto_save", Value::Text("on_focus_change".into()), from, Some("Fenix saves when you leave the buffer or the window".into())),
                _ => out.unmapped(from, "isn't one of VS Code's auto-save settings"),
            },
            "files.autoSaveDelay" => match value.as_i64() {
                Some(n @ 100..=600_000) => out.set("editor.auto_save_delay_ms", Value::Int(n), from, None),
                _ => out.unmapped(from, "Fenix's auto-save delay is from 100 to 600000 milliseconds"),
            },
            "editor.stickyScroll.enabled" => bool_setting(out, "appearance.sticky_scroll", value, from),
            "editor.stickyScroll.maxLineCount" => match value.as_i64() {
                Some(n @ 1..=6) => out.set("appearance.sticky_lines", Value::Int(n), from, None),
                _ => out.unmapped(from, "Fenix keeps from 1 to 6 sticky lines"),
            },
            "editor.bracketPairColorization.enabled" => bool_setting(out, "appearance.rainbow_brackets", value, from),
            "editor.guides.indentation" => bool_setting(out, "appearance.indent_guides", value, from),
            "workbench.editor.enablePreview" => bool_setting(out, "editor.preview_tab", value, from),
            "editor.hover.enabled" => match (value.as_bool(), value.as_str()) {
                (Some(on), _) => out.set("editor.mouse_hover", Value::Bool(on), from, None),
                (_, Some("on")) => out.set("editor.mouse_hover", Value::Bool(true), from, None),
                (_, Some("off")) => out.set("editor.mouse_hover", Value::Bool(false), from, None),
                _ => out.unmapped(from, "Fenix's hover is on or off"),
            },
            "editor.hover.delay" => match value.as_i64() {
                Some(n @ 0..=5000) => out.set("editor.hover_delay_ms", Value::Int(n), from, None),
                _ => out.unmapped(from, "Fenix's hover delay is from 0 to 5000 milliseconds"),
            },
            "editor.multiCursorModifier" => match value.as_str() {
                // VS Code's names the key that adds a cursor with a click.
                Some("alt") => out.set("editor.multi_cursor_modifier", Value::Text("alt".into()), from, None),
                Some("ctrlCmd") => out.set("editor.multi_cursor_modifier", Value::Text("ctrl".into()), from, None),
                _ => out.unmapped(from, "isn't alt or ctrlCmd"),
            },
            "files.exclude" => globs(out, "explorer.hide", value, from),
            "search.exclude" => globs(out, "search.exclude", value, from),
            "explorer.compactFolders" => bool_setting(out, "explorer.compact", value, from),
            "search.smartCase" => bool_setting(out, "search.smart_case", value, from),
            "git.autofetch" => match (value.as_bool(), value.as_str()) {
                (Some(true), _) | (_, Some("all")) => {
                    let seconds = get("git.autofetchPeriod").and_then(Json::as_i64).unwrap_or(180);
                    let minutes = ((seconds + 59) / 60).max(1);
                    let note = (seconds % 60 != 0).then(|| format!("every {minutes} minutes: Fenix counts in minutes"));
                    out.set("git.auto_fetch", Value::Int(minutes), from, note);
                }
                (Some(false), _) => {}
                _ => out.unmapped(from, "isn't on or off"),
            },
            "git.autofetchPeriod" => {}
            "editor.semanticHighlighting.enabled" => match value.as_bool() {
                Some(on) => out.set("lsp.semantic_highlighting", Value::Bool(on), from, None),
                None => out.unmapped(from, "Fenix's is on or off, whatever the theme"),
            },
            "editor.codeLens" => bool_setting(out, "lsp.code_lens", value, from),
            "editor.smoothScrolling" => bool_setting(out, "motion.smooth_scroll", value, from),
            "editor.cursorSmoothCaretAnimation" => match value.as_str() {
                Some("on" | "explicit") => out.set("motion.caret_glide", Value::Bool(true), from, None),
                Some("off") => out.set("motion.caret_glide", Value::Bool(false), from, None),
                _ => out.unmapped(from, "isn't on, explicit or off"),
            },
            k if k.starts_with("vim.") => {}
            k if k.starts_with('[') => out.unmapped(from, "for one language's files: Fenix's settings are for every file"),
            k if k.starts_with("terminal.integrated.defaultProfile") || k.starts_with("terminal.integrated.profiles") => {
                out.unmapped(from, "names one of VS Code's terminal profiles: Fenix's Terminal shell (SPC ,) takes a command line")
            }
            _ => out.unmapped(from, NO_SETTING),
        }
    }
}

fn bool_setting(out: &mut Proposal, key: &'static str, value: &Json, from: String) {
    match value.as_bool() {
        Some(on) => out.set(key, Value::Bool(on), from, None),
        None => out.unmapped(from, "isn't true or false"),
    }
}

/// `files.exclude`'s globs that are on, as names: `**/node_modules` is
/// `node_modules`. One with a folder in it can't be a name.
fn globs(out: &mut Proposal, key: &'static str, value: &Json, from: String) {
    let Some(map) = value.as_object() else {
        out.unmapped(from, "isn't a list of globs");
        return;
    };
    let mut names = Vec::new();
    for (glob, on) in map {
        if on.as_bool() != Some(true) {
            continue;
        }
        let name = glob.trim_start_matches("**/").trim_end_matches("/**");
        if name.contains('/') || name.contains('{') {
            out.unmapped(format!("\"{glob}\" in {}", from.split(':').next().unwrap_or("")), "a path: Fenix matches names, not folders within folders");
        } else if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    if !names.is_empty() {
        out.set(key, Value::List(names), from, None);
    }
}

/// The first font of a CSS list that's a font's name, its quotes off;
/// none for `monospace` and the like.
fn first_font(list: &str) -> Option<String> {
    list.split(',')
        .map(|f| f.trim().trim_matches(|c| c == '\'' || c == '"').trim().to_string())
        .find(|f| !f.is_empty() && !matches!(f.to_ascii_lowercase().as_str(), "monospace" | "serif" | "sans-serif" | "ui-monospace" | "system-ui" | "cursive" | "fantasy"))
}

/// VS Code's commands Fenix has, and Fenix's for each. Fenix's own
/// standard keys are VS Code's defaults, so these are the commands of
/// `default_keymap.toml`'s `scheme = "standard"` entries, and a few more.
/// (fenix-gui's tests check every one is a command.)
pub const COMMANDS: &[(&str, &str)] = &[
    ("workbench.action.quickOpen", "project.find_file"),
    ("workbench.action.files.openFile", "file.find"),
    ("workbench.action.files.openFileFolder", "file.find"),
    ("workbench.action.openRecent", "file.recent"),
    ("workbench.action.files.newUntitledFile", "buffer.scratch"),
    ("workbench.action.files.save", "file.save"),
    ("workbench.action.closeActiveEditor", "tab.close"),
    ("workbench.action.reopenClosedEditor", "tab.reopen"),
    ("workbench.action.nextEditor", "tab.next"),
    ("workbench.action.previousEditor", "tab.prev"),
    ("workbench.action.quickOpenPreviousRecentlyUsedEditorInGroup", "buffer.switch_recent"),
    ("workbench.action.showCommands", "commands.palette"),
    ("workbench.action.toggleSidebarVisibility", "explorer.toggle_sidebar"),
    ("workbench.action.focusSideBar", "window.focus_sidebar"),
    ("workbench.view.explorer", "explorer.reveal_current"),
    ("workbench.files.action.showActiveFileInExplorer", "explorer.reveal_current"),
    ("workbench.view.scm", "git.open"),
    ("workbench.action.terminal.toggleTerminal", "terminal.toggle"),
    ("workbench.action.togglePanel", "terminal.toggle"),
    ("workbench.action.terminal.focus", "terminal.focus"),
    ("workbench.action.openSettings", "settings.open"),
    ("workbench.action.openSettings2", "settings.open"),
    ("workbench.action.newWindow", "frame.new"),
    ("workbench.action.splitEditor", "window.split_vertical"),
    ("workbench.action.splitEditorRight", "window.split_vertical"),
    ("workbench.action.splitEditorDown", "window.split_horizontal"),
    ("workbench.action.closeOtherEditors", "tab.close_others"),
    ("workbench.action.moveEditorToNextGroup", "window.move_tab_next"),
    ("workbench.action.moveEditorToPreviousGroup", "window.move_tab_prev"),
    ("workbench.action.focusFirstEditorGroup", "window.focus_1"),
    ("workbench.action.focusSecondEditorGroup", "window.focus_2"),
    ("workbench.action.focusThirdEditorGroup", "window.focus_3"),
    ("workbench.action.focusLeftGroup", "window.navigate_left"),
    ("workbench.action.focusRightGroup", "window.navigate_right"),
    ("workbench.action.focusAboveGroup", "window.navigate_up"),
    ("workbench.action.focusBelowGroup", "window.navigate_down"),
    ("workbench.action.zoomIn", "view.increase_font_size"),
    ("workbench.action.zoomOut", "view.decrease_font_size"),
    ("workbench.action.zoomReset", "view.reset_font_size"),
    ("editor.action.fontZoomIn", "view.increase_font_size"),
    ("editor.action.fontZoomOut", "view.decrease_font_size"),
    ("editor.action.fontZoomReset", "view.reset_font_size"),
    ("workbench.action.toggleFullScreen", "view.toggle_fullscreen"),
    ("workbench.action.quit", "app.quit"),
    ("undo", "edit.undo"),
    ("redo", "edit.redo"),
    ("editor.action.clipboardCopyAction", "edit.copy"),
    ("editor.action.clipboardCutAction", "edit.cut"),
    ("editor.action.clipboardPasteAction", "edit.paste"),
    ("editor.action.selectAll", "edit.select_all"),
    ("editor.action.commentLine", "code.toggle_comment"),
    ("editor.action.moveLinesUpAction", "edit.move_lines_up"),
    ("editor.action.moveLinesDownAction", "edit.move_lines_down"),
    ("editor.action.copyLinesUpAction", "edit.copy_lines_up"),
    ("editor.action.copyLinesDownAction", "edit.copy_lines_down"),
    ("editor.action.duplicateSelection", "edit.copy_lines_down"),
    ("editor.action.deleteLines", "edit.delete_line"),
    ("editor.action.insertLineAfter", "edit.new_line_below"),
    ("editor.action.insertLineBefore", "edit.new_line_above"),
    ("expandLineSelection", "edit.select_line"),
    ("editor.action.indentLines", "edit.indent_lines"),
    ("editor.action.outdentLines", "edit.outdent_lines"),
    ("editor.action.jumpToBracket", "nav.matching_bracket"),
    ("editor.fold", "code.fold"),
    ("editor.unfold", "code.unfold"),
    ("editor.toggleFold", "code.toggle_fold"),
    ("cursorUndo", "edit.cursor_undo"),
    ("editor.action.addSelectionToNextFindMatch", "edit.add_next_occurrence"),
    ("editor.action.selectHighlights", "edit.select_all_occurrences"),
    ("editor.action.insertCursorAbove", "edit.add_cursor_above"),
    ("editor.action.insertCursorBelow", "edit.add_cursor_below"),
    ("editor.action.toggleWordWrap", "view.toggle_wrap"),
    ("workbench.action.gotoLine", "nav.goto_line"),
    ("actions.find", "search.file"),
    ("editor.action.startFindReplaceAction", "search.replace_buffer"),
    ("workbench.action.findInFiles", "search.project"),
    ("workbench.view.search", "search.project"),
    ("workbench.action.replaceInFiles", "search.replace_project"),
    ("editor.action.nextMatchFindAction", "project.quickfix_next"),
    ("editor.action.previousMatchFindAction", "project.quickfix_prev"),
    ("editor.action.revealDefinition", "code.goto_definition"),
    ("editor.action.goToDefinition", "code.goto_definition"),
    ("editor.action.openLink", "nav.follow_link"),
    ("editor.action.goToReferences", "code.references"),
    ("editor.action.referenceSearch.trigger", "code.references"),
    ("references-view.findReferences", "code.references"),
    ("editor.action.goToImplementation", "code.implementations"),
    ("references-view.showCallHierarchy", "code.callers"),
    ("editor.action.showHover", "code.hover"),
    ("editor.action.rename", "code.lsp_rename"),
    ("editor.action.quickFix", "code.lsp_code_action"),
    ("editor.action.formatDocument", "code.format_buffer"),
    ("workbench.action.gotoSymbol", "code.outline"),
    ("workbench.action.showAllSymbols", "code.project_symbols"),
    ("workbench.actions.view.problems", "code.problems"),
    ("editor.action.marker.nextInFiles", "nav.next_problem"),
    ("editor.action.marker.prevInFiles", "nav.prev_problem"),
    ("editor.action.marker.next", "nav.next_problem"),
    ("editor.action.marker.prev", "nav.prev_problem"),
    ("workbench.action.editor.nextChange", "nav.next_hunk"),
    ("workbench.action.editor.previousChange", "nav.prev_hunk"),
    ("workbench.action.navigateBack", "nav.jump_back"),
    ("workbench.action.navigateForward", "nav.jump_forward"),
    ("workbench.action.debug.start", "debug.start_or_continue"),
    ("workbench.action.debug.continue", "debug.start_or_continue"),
    ("workbench.action.debug.stop", "debug.stop"),
    ("workbench.action.debug.restart", "debug.restart"),
    ("workbench.action.debug.pause", "debug.pause"),
    ("workbench.action.debug.stepOver", "debug.step_over"),
    ("workbench.action.debug.stepInto", "debug.step_into"),
    ("workbench.action.debug.stepOut", "debug.step_out"),
    ("editor.debug.action.toggleBreakpoint", "debug.toggle_breakpoint"),
    ("editor.debug.action.runToCursor", "debug.run_to_cursor"),
];

pub(super) fn keybindings(text: &str, out: &mut Proposal) {
    let json = match jsonc::parse(text) {
        Ok(json) => json,
        Err(e) => {
            out.unmapped("keybindings.json", format!("couldn't be read: {e}"));
            return;
        }
    };
    let Some(list) = json.as_array() else {
        out.unmapped("keybindings.json", "isn't a list of key bindings");
        return;
    };
    for entry in list {
        let key = entry.get("key").and_then(Json::as_str).unwrap_or("");
        let command = entry.get("command").and_then(Json::as_str).unwrap_or("");
        let when = entry.get("when").and_then(Json::as_str).map(str::trim).filter(|w| !w.is_empty());
        let from = match when {
            Some(w) => format!("{key} → {command} (when {w})"),
            None => format!("{key} → {command}"),
        };
        if let Some(removed) = command.strip_prefix('-') {
            out.unmapped(from, format!("takes {removed} off VS Code's key: Fenix's own keys stay (unbind one in keybindings.toml)"));
            continue;
        }
        if entry.get("args").is_some() {
            out.unmapped(from, "passes the command arguments, which Fenix's commands don't take");
            continue;
        }
        let Some(fenix) = COMMANDS.iter().find(|(vs, _)| *vs == command).map(|(_, f)| *f) else {
            out.unmapped(from, "Fenix has no command like it");
            continue;
        };
        let keys = match chord(key) {
            Ok(keys) => keys,
            Err(why) => {
                out.unmapped(from, why);
                continue;
            }
        };
        let when = match when.map(when_of) {
            None => None,
            Some(Some(w)) => Some(w),
            Some(None) => {
                out.unmapped(from, "only where VS Code's when says, which Fenix can't tell");
                continue;
            }
        };
        out.key(keys, fenix, "standard", when, from);
    }
}

/// A `when` Fenix's chords can say: a text editor, a terminal. `None`
/// for any other.
fn when_of(when: &str) -> Option<&'static str> {
    let parts: Vec<&str> = when.split("&&").map(str::trim).filter(|p| !matches!(*p, "!editorReadonly" | "editorHasSelection" | "!isInDiffEditor")).collect();
    match parts.as_slice() {
        ["editorTextFocus" | "editorFocus" | "textInputFocus"] => Some("text"),
        ["terminalFocus"] => Some("terminal"),
        [] => Some("text"),
        _ => None,
    }
}

/// VS Code's `ctrl+shift+p` as a Fenix chord, `C-S-p`. A sequence
/// (`ctrl+k ctrl+c`) isn't one.
pub(crate) fn chord(key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.split_whitespace().count() != 1 {
        return Err("a sequence of keys: Fenix's chords are one key each (a sequence goes under the leader, [[leader]] in keybindings.toml)".into());
    }
    let lower = key.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('+').collect();
    // `ctrl++`: the last key is the plus itself.
    if lower.ends_with("++") {
        parts.pop();
        if let Some(last) = parts.last_mut() {
            *last = "+";
        }
    }
    let Some((name, mods)) = parts.split_last() else { return Err("no key".into()) };
    let (mut ctrl, mut alt, mut shift, mut sup) = (false, false, false, false);
    for m in mods {
        match *m {
            "ctrl" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "meta" | "cmd" | "win" | "super" => sup = true,
            other => return Err(format!("{other} isn't a key Fenix knows")),
        }
    }
    let name = match *name {
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "enter" => "Enter".to_string(),
        "tab" => "Tab".to_string(),
        "escape" => "Esc".to_string(),
        "backspace" => "Backspace".to_string(),
        "delete" => "Delete".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" => "PgUp".to_string(),
        "pagedown" => "PgDn".to_string(),
        "space" => "SPC".to_string(),
        f if f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => f.to_ascii_uppercase(),
        c if c.chars().count() == 1 => c.to_string(),
        other => return Err(format!("{other} isn't a key Fenix knows")),
    };
    if !(ctrl || alt || sup) && !(name.starts_with('F') && name.len() > 1) {
        return Err("a key that types: only keys with Ctrl or Alt, and F-keys, are chords".into());
    }
    let mut out = String::new();
    for (on, m) in [(ctrl, "C-"), (alt, "M-"), (sup, "s-"), (shift, "S-")] {
        if on {
            out.push_str(m);
        }
    }
    out.push_str(&name);
    Ok(out)
}
