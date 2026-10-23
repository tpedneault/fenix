use std::borrow::Cow;

use winit::event_loop::ActiveEventLoop;

use crate::app::App;

/// Context a command runs with: the editor state plus the event loop handle
/// needed for app-level actions like quitting.
pub struct CommandCtx<'a> {
    pub app: &'a mut App,
    pub event_loop: &'a ActiveEventLoop,
}

/// What a command runs. Boxed, so a command can be registered at run
/// time with whatever it needs (an extension's, later).
type CommandFn = Box<dyn Fn(&mut CommandCtx)>;

/// What a command marked external runs: the editor and nothing else.
/// No event loop, so it can be run from the control channel (which has
/// none to hand). Nor should it read the focused buffer: a caller
/// outside Fenix can't know what that is. `control_host`'s
/// `external_commands_dont_assume_a_focused_text_buffer` holds every
/// external command to that.
type ExternalFn = Box<dyn Fn(&mut App)>;

enum Handler {
    Ctx(CommandFn),
    External(ExternalFn),
    /// A feature's command that isn't external: it needs no event loop
    /// either, but only runs from inside Fenix.
    App(ExternalFn),
}

fn cmd_snippets_from_selection(ctx: &mut CommandCtx) {
    ctx.app.snippet_from_selection();
}

fn cmd_insert_snippet(ctx: &mut CommandCtx) {
    ctx.app.start_snippet_picker();
}

/// A named, invokable editor action. Every user-facing action should
/// eventually be one of these rather than a function wired directly to a
/// key, so the same action can be bound in multiple keymaps (Phase 2) and,
/// later, exposed to Lua scripting (Phase 6) without a rewrite.
pub struct Command {
    /// `agenda.open`. Owned or static, so ids can come from data or an
    /// extension at run time, not only from this file.
    pub id: Cow<'static, str>,
    pub description: Cow<'static, str>,
    /// The extension it comes from (`fenix.jira`); `None` for core's.
    pub source: Option<&'static str>,
    run: Handler,
}

impl Command {
    /// Whether it may be run from outside Fenix (`fenix/runCommand`):
    /// it makes sense with nobody looking at a file in Fenix, like
    /// opening the agenda or switching project, rather than acting on
    /// what's in front of you, like saving or staging a hunk.
    pub fn external(&self) -> bool {
        matches!(self.run, Handler::External(_))
    }
}

/// Why `run_external` didn't run a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalError {
    Unknown,
    /// It acts on what's in front of you in Fenix.
    NotExternal,
}

pub struct CommandRegistry {
    commands: Vec<Command>,
    /// The features switched off, whose commands aren't registered.
    off: Vec<&'static str>,
}

impl CommandRegistry {
    pub fn with_builtins() -> Self {
        Self::with_builtins_where(&crate::app::feature::enabled)
    }

    /// Core's commands and those of the features `on` says are on.
    pub fn with_builtins_where(on: &dyn Fn(&str) -> bool) -> Self {
        let off = crate::app::feature::BUILTIN_SPECS.iter().filter(|spec| !on(spec.id)).map(|spec| spec.id).collect();
        let mut registry = Self { commands: Vec::new(), off };
        registry.external("snippets.open", "Every snippet: make, edit, copy, delete and try them", |app| app.open_snippets_page(None));
        registry.register("snippets.from_selection", "Make a snippet from the last Visual selection", cmd_snippets_from_selection);
        registry.register("insert.snippet", "Choose a snippet for the current document", cmd_insert_snippet);
        registry.register("file.save", "Save the current file", cmd_save);
        registry.register("file.save_force", "Save, overwriting a file that changed on disk (:w!)", cmd_save_force);
        registry.register("file.reload", "Re-read the current file from disk, discarding unsaved edits (:e!)", cmd_file_reload);
        registry.register("edit.undo", "Undo the last edit", cmd_undo);
        registry.register("edit.redo", "Redo the last undone edit", cmd_redo);
        registry.register("app.quit", "Quit Fenix (confirms first if there are unsaved buffers)", cmd_quit);
        registry.register("app.quit_force", "Quit Fenix immediately, discarding any unsaved changes", cmd_quit_force);
        registry.register("app.save_all_and_quit", "Save every unsaved buffer, then quit Fenix", cmd_save_all_and_quit);
        registry.register(
            "view.cycle_line_numbers",
            "Cycle the line-number gutter: off, absolute, relative",
            cmd_cycle_line_numbers,
        );
        registry.external("view.pick_theme", "Pick a theme by name", |app| app.picker_pick_theme());
        registry.register("view.toggle_fullscreen", "Toggle fullscreen", cmd_toggle_fullscreen);
        registry.register("view.toggle_animations", "Cycle motion: off, subtle, full", cmd_toggle_animations);
        registry.register("view.cycle_diagnostics", "Cycle inline problems: all, errors, off", |ctx| ctx.app.cycle_inline_diagnostics());
        registry.register(
            "explorer.jump",
            "Open a full-buffer directory listing at the current file's directory",
            cmd_explorer_jump,
        );
        registry.register("explorer.toggle_sidebar", "Toggle the file explorer sidebar", cmd_toggle_sidebar);
        registry.register(
            "table.toggle",
            "Toggle the focused buffer between plain text and elastic-column table view",
            cmd_table_toggle,
        );
        registry.register("search.buffer", "Fuzzy-find a line in the current buffer", cmd_search_buffer);
        registry.register("search.todos", "List the TODO comments in the current buffer", cmd_search_todos);
        registry.external("search.todos_project", "List the TODO comments across the project", |app| app.picker_project_todos());
        registry.register("code.xml_validate", "Check the current XML buffer is well-formed", cmd_code_xml_validate);
        registry.register("code.xml_path", "Copy the XPath of the XML element under the cursor", cmd_code_xml_path);
        registry.register("search.replace_buffer", "Search and replace in the current buffer", cmd_search_replace_buffer);
        registry.register(
            "search.replace_project",
            "Search and replace across the project",
            cmd_search_replace_project,
        );
        registry.external("file.find", "Open a file by typing its path (bypasses .gitignore)", |app| app.start_find_file_prompt());
        registry.external("file.explore", "Browse the filesystem from your home directory; open a file directly, or fuzzy-search recursively from wherever you navigate to", |app| app.start_explore_from_home());
        registry.external("file.find_all", "Fuzzy-find a file in the project, including gitignored ones", |app| app.picker_find_file_all());
        registry.external("file.recent", "Fuzzy-find a recently-opened file", |app| app.picker_recent_files());
        registry.external("file.recover", "Recover unsaved work a previous session left behind", |app| app.picker_recovery());
        registry.register("file.rename", "Rename the current file on disk", cmd_file_rename);
        registry.register("file.delete", "Delete the current file (with confirmation)", cmd_file_delete);
        registry.register("file.yank_path", "Copy the current file's path to the clipboard", cmd_file_yank_path);
        registry.external("project.find_file", "Fuzzy-find a file in the current project", |app| app.picker_find_file());
        registry.external("project.grep", "Search the current project (ripgrep)", |app| app.picker_grep_prompt());
        registry.register("project.quickfix_next", "Jump to the next match in the last project search", cmd_quickfix_next);
        registry.register(
            "project.quickfix_prev",
            "Jump to the previous match in the last project search",
            cmd_quickfix_prev,
        );
        registry.external("project.switch_project", "Switch to a different known project", |app| app.picker_switch_project());
        registry.external("project.new", "Create a new project from a template", |app| app.cmd_project_new());
        registry.external("project.hub", "Every project: what it is, how it's doing, open one", |app| app.cmd_project_hub());
        registry.external("project.doctor", "Check the project's toolchain and offer fixes", |app| app.cmd_project_doctor());
        registry.external("project.settings", "The project's tasks, servers, launch and kind", |app| app.cmd_project_settings());
        registry.external("project.add", "Register a project in the switch-project list", |app| app.picker_add_project_prompt());
        registry.register("project.delete", "Remove a project from the switch-project list", cmd_project_delete);
        registry.external("task.run", "Fuzzy-pick and run a discovered project task", |app| app.picker_tasks());
        registry.external("task.rerun_last", "Rerun the most recently run task", |app| app.rerun_last_task());
        registry.register("task.kill", "End the currently running task", cmd_task_kill);
        registry.external("dashboard.open", "Show Home, the start-up dashboard", |app| app.open_dashboard());
        registry.external("terminal.toggle", "Toggle the terminal panel", |app| app.toggle_terminal());
        registry.external("terminal.open_buffer", "Open a shell in the focused pane", |app| app.open_terminal_buffer());
        registry.register("explorer.dual_pane", "Two listings side by side", cmd_explorer_dual_pane);
        registry.register("explorer.cancel", "Stop the running file operation", cmd_explorer_cancel);
        registry.register("explorer.open_external", "Open with the system's default application", cmd_explorer_open_external);
        registry.register("explorer.reveal", "Show this in Explorer", cmd_explorer_reveal);
        registry.register("explorer.yank_path", "Copy the full path", cmd_explorer_yank_path);
        registry.register("explorer.terminal_here", "Open a shell in this directory", cmd_explorer_terminal_here);
        registry.register("explorer.grep_here", "Search this directory", cmd_explorer_grep_here);
        registry.register("explorer.git_here", "Make this the project and open the Git panel", cmd_explorer_git_here);
        registry.register("explorer.rename_mode", "Edit the listing's names as text", cmd_explorer_rename_mode);
        registry.register("explorer.rename_apply", "Apply the edited names", cmd_explorer_rename_apply);
        registry.register("explorer.go_to_path", "Go to a path you type", cmd_explorer_go_to_path);
        registry.external("explorer.places", "Bookmarks, drives, recent directories", |app| app.picker_places());
        registry.external("explorer.recent_dirs", "Directories you have been to", |app| app.picker_recent_dirs());
        registry.register("explorer.bookmark", "Bookmark the directory you are in", cmd_explorer_bookmark);
        #[cfg(feature = "docker")]
        registry.feature(&crate::app::DOCKER_SPEC);
        registry.register("debug.start_or_continue", "Start a debug session, or continue one that's stopped", cmd_debug_start_or_continue);
        registry.register("debug.toggle_breakpoint", "Toggle a breakpoint on the focused buffer's current line", cmd_debug_toggle_breakpoint);
        registry.register("debug.step_over", "Step over the current line", cmd_debug_step_over);
        registry.register("debug.step_into", "Step into the current call", cmd_debug_step_into);
        registry.register("debug.step_out", "Step out of the current function", cmd_debug_step_out);
        registry.register("debug.add_watch", "Watch the identifier before the cursor", cmd_debug_add_watch);
        registry.register("debug.stop", "End the running debug session", cmd_debug_stop);
        registry.external("tools.status", "Show which LSP servers/DAP adapters are found on PATH and running", |app| app.open_tool_status_panel());
        registry.feature(&crate::app::VNC_SPEC);
        registry.feature(&crate::app::PDF_SPEC);
        registry.register("git.open", "Show the Git status/files/branches/commits/stash panel", |ctx| ctx.app.open_git());
        registry.register("git.close", "Close the Git panel session", cmd_git_close);
        registry.register("git.history", "Show the commit graph, refs and commit detail", |ctx| ctx.app.open_history_view());
        registry.register("git.history_close", "Close the History view", cmd_git_history_close);
        registry.register("git.fetch", "Fetch all remotes and prune deleted branches", |ctx| ctx.app.git_fetch());
        registry.register("git.versus", "Set this file beside its version on a branch, lined up", cmd_git_versus);
        registry.register("git.compare", "Compare HEAD (or the working tree) with a branch: files, commits, diffs", |ctx| ctx.app.start_compare_picker());
        registry.register("git.operations", "Show the Git operation log, to undo what Fenix ran", |ctx| ctx.app.open_git_operations());
        registry.register("keys.describe_key", "Say what a key runs: type it after this", |ctx| ctx.app.describe_key_start());
        registry.register("keys.describe_command", "Say what a command does, which keys run it and where it's from", |ctx| ctx.app.describe_command_picker());
        registry.register("keys.bindings", "The keys bound here, yours marked; Enter runs one", |ctx| ctx.app.open_bindings_picker());
        registry.register("keys.conflicts", "Where keys disagree: Fenix's own, and yours over Fenix's", |ctx| ctx.app.open_key_conflicts());
        registry.external("keys.edit", "Your keybindings (keybindings.toml): saving puts them in use", |app| app.edit_keybindings());
        registry.register("commands.palette", "Every command, by name: run one (SPC :, Alt-x)", |ctx| ctx.app.open_command_palette());
        registry.external("extensions.open", "The built-in extensions: switch them on or off", |app| app.open_extensions_page());
        registry.external("extensions.welcome", "The welcome: packs of extensions to switch on together", |app| app.open_welcome_page());
        registry.external("settings.open", "Every setting, on one page: search, change, reset", |app| app.open_settings_page(crate::settings_page::Scope::You, None));
        registry.register("git.log", "Show the history, with a menu on every commit", |ctx| ctx.app.open_git_log(crate::git_log::Scope::Branch));
        registry.register("git.close_view", "Close the Git view in front: the panel, graph, comparison, conflicts or merge requests", cmd_git_close_view);
        registry.register("git.file_history", "Show the history of the focused file", cmd_git_file_history);
        registry.register("git.line_history", "Show the history of the selected lines", cmd_git_line_history);
        registry.register("git.hunk_stage", "Stage the changed hunk under the cursor", cmd_git_hunk_stage);
        registry.register("git.hunk_discard", "Discard the changed hunk under the cursor", cmd_git_hunk_discard);
        registry.register("git.hunk_preview", "Show the changed hunk under the cursor", cmd_git_hunk_preview);
        registry.register("git.switch", "Switch branch, most recently used first", |ctx| ctx.app.start_switch_picker());
        registry.register("git.blame", "Show or hide who last changed each line", cmd_git_blame);
        registry.register("git.blame_explain", "Show the commit that last changed this line", cmd_git_blame_explain);
        registry.register("git.rebase", "Rebase the current branch onto another ref", cmd_git_rebase);
        registry.register("git.continue", "Continue the suspended rebase/merge", cmd_git_continue);
        registry.register("git.abort", "Abort the suspended rebase/merge", cmd_git_abort);
        registry.register("git.merge", "Merge another ref into the current branch", cmd_git_merge);
        registry.register("git.pull_rebase", "Pull, rebasing local commits on top", cmd_git_pull_rebase);
        registry.register("git.force_push", "Force-push with lease (confirms first)", cmd_git_force_push);
        registry.register("git.next_conflict", "Jump to the next conflict in this file", cmd_git_next_conflict);
        registry.register("git.prev_conflict", "Jump to the previous conflict in this file", cmd_git_prev_conflict);
        registry.register("git.keep_ours", "Resolve the conflict under the cursor, keeping ours", cmd_git_keep_ours);
        registry.register("git.keep_theirs", "Resolve the conflict under the cursor, keeping theirs", cmd_git_keep_theirs);
        registry.register("git.keep_both", "Resolve the conflict under the cursor, keeping both sides", cmd_git_keep_both);
        #[cfg(feature = "review")]
        registry.feature(&crate::app::REVIEW_SPEC);
        registry.register("git.merge_view", "Open the Merge view (resolve conflicts side by side)", cmd_git_merge_view);
        registry.register("git.merge_close", "Close the Merge view", cmd_git_merge_close);
        registry.register("git.stage_resolved", "Stage the selected conflicted file as resolved", cmd_git_stage_resolved);
        #[cfg(feature = "jira")]
        registry.feature(&crate::app::JIRA_SPEC);
        registry.register("leader.local", "Open SPC m, the menu for the focused buffer's project and language", cmd_leader_local);
        #[cfg(feature = "embedded")]
        registry.feature(&crate::app::EMBEDDED_SPEC);
        #[cfg(feature = "notebook")]
        registry.feature(&crate::app::NOTEBOOK_SPEC);
        #[cfg(feature = "diagrams")]
        registry.feature(&crate::app::DIAGRAMS_SPEC);
        registry.register("notebook.preview", "The reading view, beside the Markdown (again closes it)", |ctx| ctx.app.open_reading_view(false));
        registry.register("notebook.read", "Read this Markdown rendered, in its place (again goes back)", |ctx| ctx.app.open_reading_view(true));
        #[cfg(feature = "agenda")]
        registry.feature(&crate::app::AGENDA_SPEC);
        registry.register("window.split_vertical", "Split the focused window side by side", cmd_split_vertical);
        registry.register("window.split_horizontal", "Split the focused window stacked", cmd_split_horizontal);
        registry.register("window.navigate_left", "Move focus to the window on the left", cmd_navigate_left);
        registry.register("window.navigate_right", "Move focus to the window on the right", cmd_navigate_right);
        registry.register("window.navigate_up", "Move focus to the window above", cmd_navigate_up);
        registry.register("window.navigate_down", "Move focus to the window below", cmd_navigate_down);
        registry.register("window.cycle", "Cycle focus to the next window", cmd_cycle_window);
        registry.register("window.close", "Close the focused window", cmd_close_window);
        registry.register("window.only", "Close every window except the focused one", cmd_only_window);
        // "Frame" is an OS window; "window" above is a split inside one
        // (see `app::FrameState`'s own doc comment on the naming).
        registry.register("frame.new", "Open another OS window, on the next free monitor", cmd_new_frame);
        registry.register("frame.close", "Close the focused OS window", cmd_close_frame);
        registry.register("frame.cycle", "Cycle focus to the next OS window", cmd_cycle_frame);
        registry.register("frame.only", "Close every OS window except the focused one", cmd_only_frame);
        registry.register("window.balance", "Reset every split ratio to 0.5", cmd_balance_windows);
        registry.external("buffer.switch", "Fuzzy-switch to another open buffer", |app| app.picker_switch_buffer());
        registry.register("buffer.next", "Switch to the next open buffer", cmd_next_buffer);
        registry.register("buffer.prev", "Switch to the previous open buffer", cmd_prev_buffer);
        registry.register("buffer.kill", "Close the focused buffer (refuses if it has unsaved changes)", cmd_kill_buffer);
        registry.register("tab.close", "Close the focused pane's tab (the buffer stays open)", |ctx| ctx.app.close_focused_tab());
        registry.register("tab.close_others", "Close every other tab in the focused pane", |ctx| ctx.app.close_other_tabs());
        registry.register("tab.reopen", "Reopen the tab this pane closed last", |ctx| ctx.app.reopen_closed_tab());
        registry.register("tab.keep", "Keep the preview tab as an ordinary tab", |ctx| ctx.app.keep_focused_preview());
        registry.register("tab.move_left", "Move the tab one place left", |ctx| ctx.app.shift_focused_tab(-1));
        registry.register("tab.move_right", "Move the tab one place right", |ctx| ctx.app.shift_focused_tab(1));
        registry.register("buffer.kill_force", "Close the focused buffer immediately, discarding any unsaved changes", cmd_kill_buffer_force);
        registry.register("buffer.save_and_kill", "Save the focused buffer (if needed) then close it", cmd_save_and_kill_buffer);
        registry.external("buffer.scratch", "Open a new scratch buffer", |app| app.new_scratch_buffer());
        registry.external("workspace.new", "Create a new workspace", |app| app.new_workspace());
        registry.register("workspace.next", "Switch to the next workspace", cmd_next_workspace);
        registry.register("workspace.prev", "Switch to the previous workspace", cmd_prev_workspace);
        registry.register("workspace.remove", "Remove the active workspace", cmd_remove_workspace);
        registry.external("workspace.switch", "Switch to an open workspace by name", |app| app.picker_switch_workspace());
        registry.external("workspace.find", "Open a configured workspace by name, creating it if it isn't already open", |app| app.start_workspace_launcher_picker());
        registry.register("workspace.rename", "Rename the active workspace", cmd_rename_workspace);
        #[cfg(feature = "tcl")]
        registry.feature(&crate::app::TCL_SPEC);
        registry.register(
            "code.lsp_rename",
            "Rename the symbol under the cursor across the project (LSP)",
            cmd_lsp_rename,
        );
        registry.register(
            "code.lsp_code_action",
            "Request and apply the first available code action from the attached language server (LSP)",
            cmd_lsp_code_action,
        );
        registry.register(
            "code.format_selection",
            "Reindent the active Visual selection structurally, language-independent (Emacs' indent-region)",
            cmd_format_selection,
        );
        registry.register(
            "code.format_buffer",
            "Reindent the whole focused buffer structurally, language-independent (Emacs' indent-region)",
            cmd_format_buffer,
        );
        registry.register("code.toggle_checkbox", "Toggle the GFM task checkbox on the current line", cmd_toggle_checkbox);
        registry.register("code.outline", "Fuzzy-find a Markdown heading and jump to it", cmd_outline);
        registry.register("code.scope_parent", "Jump to the enclosing scope header", |ctx| ctx.app.jump_scope_parent());
        registry.register("code.breadcrumbs", "Navigate this document's scope headers", |ctx| ctx.app.picker_document_scopes());
        registry.register("code.toggle_fold", "Collapse or expand the current code scope", |ctx| ctx.app.toggle_code_fold());
        registry.register("nav.jump_back", "Jump to the previous position in the jumplist (Ctrl-O)", cmd_jump_back);
        registry.register("nav.jump_forward", "Jump to the next position in the jumplist (Ctrl-I)", cmd_jump_forward);
        #[cfg(feature = "mission")]
        registry.feature(&crate::app::MIB_SPEC);
        #[cfg(feature = "mission")]
        registry.feature(&crate::app::CCSDS_SPEC);
        registry.register("view.increase_font_size", "Increase the body text size", cmd_increase_font_size);
        registry.register("view.decrease_font_size", "Decrease the body text size", cmd_decrease_font_size);
        registry.register("view.reset_font_size", "Reset the body text size to the default", cmd_reset_font_size);
        registry
    }

    fn register(&mut self, id: &'static str, description: &'static str, run: impl Fn(&mut CommandCtx) + 'static) {
        self.commands.push(Command { id: id.into(), description: description.into(), source: None, run: Handler::Ctx(Box::new(run)) });
    }

    /// Registers a command that also makes sense run from outside Fenix
    /// (`Command::external`). Deliberately a choice per command: the
    /// default, `register`, is not external.
    fn external(&mut self, id: &'static str, description: &'static str, run: impl Fn(&mut App) + 'static) {
        self.commands.push(Command { id: id.into(), description: description.into(), source: None, run: Handler::External(Box::new(run)) });
    }

    /// Registers a feature's commands (`FeatureSpec::commands`), in order.
    fn feature(&mut self, spec: &crate::app::feature::FeatureSpec) {
        if self.off.contains(&spec.id) {
            return;
        }
        for command in spec.commands {
            let run: ExternalFn = Box::new(command.run);
            let run = if command.external { Handler::External(run) } else { Handler::App(run) };
            self.commands.push(Command { id: command.id.into(), description: command.description.into(), source: Some(spec.id), run });
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.iter()
    }

    /// The command with this id.
    pub fn get(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.id == id)
    }

    /// Runs the command named `id` against `app`. Returns `false` if no
    /// command with that id is registered.
    pub fn run(&self, app: &mut App, event_loop: &ActiveEventLoop, id: &str) -> bool {
        let Some(cmd) = self.commands.iter().find(|c| c.id == id) else { return false };
        match &cmd.run {
            Handler::Ctx(run) => run(&mut CommandCtx { app, event_loop }),
            Handler::External(run) | Handler::App(run) => run(app),
        }
        true
    }

    /// Runs `id` for a caller outside Fenix, which only external
    /// commands allow.
    pub fn run_external(&self, app: &mut App, id: &str) -> Result<(), ExternalError> {
        let cmd = self.commands.iter().find(|c| c.id == id).ok_or(ExternalError::Unknown)?;
        match &cmd.run {
            Handler::External(run) => {
                run(app);
                Ok(())
            }
            Handler::Ctx(_) | Handler::App(_) => Err(ExternalError::NotExternal),
        }
    }
}

fn cmd_save(ctx: &mut CommandCtx) {
    ctx.app.save();
}

fn cmd_undo(ctx: &mut CommandCtx) {
    ctx.app.undo();
}

fn cmd_redo(ctx: &mut CommandCtx) {
    ctx.app.redo();
}

fn cmd_quit(ctx: &mut CommandCtx) {
    ctx.app.request_quit(ctx.event_loop);
}

fn cmd_quit_force(ctx: &mut CommandCtx) {
    if ctx.app.discard_session_edits() { ctx.event_loop.exit(); }
}

fn cmd_save_all_and_quit(ctx: &mut CommandCtx) {
    ctx.app.request_save_all_and_quit(ctx.event_loop);
}

fn cmd_cycle_line_numbers(ctx: &mut CommandCtx) {
    ctx.app.cycle_line_number_mode();
}

fn cmd_toggle_fullscreen(ctx: &mut CommandCtx) {
    ctx.app.toggle_fullscreen();
}

fn cmd_toggle_animations(ctx: &mut CommandCtx) {
    ctx.app.toggle_animations();
}

fn cmd_explorer_jump(ctx: &mut CommandCtx) {
    ctx.app.explorer_jump();
}

fn cmd_toggle_sidebar(ctx: &mut CommandCtx) {
    ctx.app.toggle_sidebar();
}

fn cmd_table_toggle(ctx: &mut CommandCtx) {
    ctx.app.toggle_table_view();
}

fn cmd_search_buffer(ctx: &mut CommandCtx) {
    ctx.app.picker_search_buffer();
}

fn cmd_search_todos(ctx: &mut CommandCtx) {
    ctx.app.picker_buffer_todos();
}

fn cmd_code_xml_validate(ctx: &mut CommandCtx) {
    ctx.app.xml_validate();
}

fn cmd_code_xml_path(ctx: &mut CommandCtx) {
    ctx.app.yank_xml_path();
}

fn cmd_search_replace_buffer(ctx: &mut CommandCtx) {
    ctx.app.start_replace_buffer();
}

fn cmd_search_replace_project(ctx: &mut CommandCtx) {
    ctx.app.start_replace_project();
}

fn cmd_save_force(ctx: &mut CommandCtx) {
    ctx.app.save_force();
}

fn cmd_file_reload(ctx: &mut CommandCtx) {
    ctx.app.reload_focused_file();
}

fn cmd_file_rename(ctx: &mut CommandCtx) {
    ctx.app.start_rename_file_prompt();
}

fn cmd_file_delete(ctx: &mut CommandCtx) {
    ctx.app.start_delete_file_confirm();
}

fn cmd_file_yank_path(ctx: &mut CommandCtx) {
    ctx.app.yank_file_path();
}

fn cmd_quickfix_next(ctx: &mut CommandCtx) {
    ctx.app.quickfix_next();
}

fn cmd_quickfix_prev(ctx: &mut CommandCtx) {
    ctx.app.quickfix_prev();
}

fn cmd_project_delete(ctx: &mut CommandCtx) {
    ctx.app.picker_delete_project();
}

fn cmd_task_kill(ctx: &mut CommandCtx) {
    ctx.app.kill_running_task();
}

fn cmd_explorer_dual_pane(ctx: &mut CommandCtx) {
    ctx.app.explorer_dual_pane();
}

fn cmd_explorer_cancel(ctx: &mut CommandCtx) {
    ctx.app.cancel_explorer_job();
}

fn cmd_explorer_open_external(ctx: &mut CommandCtx) {
    ctx.app.explorer_open_externally();
}

fn cmd_explorer_reveal(ctx: &mut CommandCtx) {
    ctx.app.explorer_reveal();
}

fn cmd_explorer_yank_path(ctx: &mut CommandCtx) {
    ctx.app.explorer_yank_path();
}

fn cmd_explorer_terminal_here(ctx: &mut CommandCtx) {
    ctx.app.explorer_terminal_here();
}

fn cmd_explorer_grep_here(ctx: &mut CommandCtx) {
    ctx.app.explorer_grep_here();
}

fn cmd_explorer_git_here(ctx: &mut CommandCtx) {
    ctx.app.explorer_git_here();
}

fn cmd_explorer_rename_mode(ctx: &mut CommandCtx) {
    ctx.app.start_rename_mode();
}

fn cmd_explorer_rename_apply(ctx: &mut CommandCtx) {
    ctx.app.apply_rename_mode();
}

fn cmd_explorer_go_to_path(ctx: &mut CommandCtx) {
    ctx.app.start_path_prompt();
}

fn cmd_explorer_bookmark(ctx: &mut CommandCtx) {
    ctx.app.bookmark_current_directory();
}

fn cmd_debug_start_or_continue(ctx: &mut CommandCtx) {
    ctx.app.debug_start_or_continue();
}

fn cmd_debug_toggle_breakpoint(ctx: &mut CommandCtx) {
    ctx.app.debug_toggle_breakpoint();
}

fn cmd_debug_step_over(ctx: &mut CommandCtx) {
    ctx.app.debug_step_over();
}

fn cmd_debug_step_into(ctx: &mut CommandCtx) {
    ctx.app.debug_step_into();
}

fn cmd_debug_step_out(ctx: &mut CommandCtx) {
    ctx.app.debug_step_out();
}

fn cmd_debug_add_watch(ctx: &mut CommandCtx) {
    ctx.app.debug_add_watch();
}

fn cmd_debug_stop(ctx: &mut CommandCtx) {
    ctx.app.debug_stop();
}

fn cmd_git_close(ctx: &mut CommandCtx) {
    ctx.app.git_session_close();
}

fn cmd_leader_local(ctx: &mut CommandCtx) {
    ctx.app.start_local_leader();
}

fn cmd_split_vertical(ctx: &mut CommandCtx) {
    ctx.app.split_vertical();
}

fn cmd_split_horizontal(ctx: &mut CommandCtx) {
    ctx.app.split_horizontal();
}

fn cmd_navigate_left(ctx: &mut CommandCtx) {
    ctx.app.navigate_window(fenix_window::NavDirection::Left);
}

fn cmd_navigate_right(ctx: &mut CommandCtx) {
    ctx.app.navigate_window(fenix_window::NavDirection::Right);
}

fn cmd_navigate_up(ctx: &mut CommandCtx) {
    ctx.app.navigate_window(fenix_window::NavDirection::Up);
}

fn cmd_navigate_down(ctx: &mut CommandCtx) {
    ctx.app.navigate_window(fenix_window::NavDirection::Down);
}

fn cmd_cycle_window(ctx: &mut CommandCtx) {
    ctx.app.cycle_window();
}

fn cmd_close_window(ctx: &mut CommandCtx) {
    ctx.app.close_window();
}

fn cmd_only_window(ctx: &mut CommandCtx) {
    ctx.app.only_window();
}

fn cmd_new_frame(ctx: &mut CommandCtx) {
    ctx.app.new_frame(ctx.event_loop);
}

fn cmd_close_frame(ctx: &mut CommandCtx) {
    ctx.app.close_frame(ctx.event_loop);
}

fn cmd_cycle_frame(ctx: &mut CommandCtx) {
    ctx.app.cycle_frame();
}

fn cmd_only_frame(ctx: &mut CommandCtx) {
    ctx.app.only_frame();
}

fn cmd_balance_windows(ctx: &mut CommandCtx) {
    ctx.app.balance_windows();
}

fn cmd_next_buffer(ctx: &mut CommandCtx) {
    ctx.app.next_buffer();
}

fn cmd_prev_buffer(ctx: &mut CommandCtx) {
    ctx.app.prev_buffer();
}

fn cmd_kill_buffer(ctx: &mut CommandCtx) {
    ctx.app.kill_buffer();
}

fn cmd_kill_buffer_force(ctx: &mut CommandCtx) {
    ctx.app.force_kill_buffer();
}

fn cmd_save_and_kill_buffer(ctx: &mut CommandCtx) {
    ctx.app.save_and_close_buffer();
}

fn cmd_next_workspace(ctx: &mut CommandCtx) {
    ctx.app.next_workspace();
}

fn cmd_prev_workspace(ctx: &mut CommandCtx) {
    ctx.app.prev_workspace();
}

fn cmd_remove_workspace(ctx: &mut CommandCtx) {
    ctx.app.remove_workspace();
}

fn cmd_rename_workspace(ctx: &mut CommandCtx) {
    ctx.app.start_rename_workspace_prompt();
}

fn cmd_lsp_rename(ctx: &mut CommandCtx) {
    ctx.app.open_lsp_rename_prompt();
}

fn cmd_lsp_code_action(ctx: &mut CommandCtx) {
    ctx.app.request_code_action();
}

fn cmd_format_selection(ctx: &mut CommandCtx) {
    ctx.app.format_selection();
}

fn cmd_format_buffer(ctx: &mut CommandCtx) {
    if ctx.app.request_lsp_format() {
        return;
    }
    ctx.app.format_buffer();
}

fn cmd_toggle_checkbox(ctx: &mut CommandCtx) {
    ctx.app.toggle_checkbox();
}

fn cmd_outline(ctx: &mut CommandCtx) {
    ctx.app.picker_outline();
}

fn cmd_jump_back(ctx: &mut CommandCtx) {
    ctx.app.jump_back();
}

fn cmd_jump_forward(ctx: &mut CommandCtx) {
    ctx.app.jump_forward();
}

fn cmd_increase_font_size(ctx: &mut CommandCtx) {
    ctx.app.increase_font_size();
}

fn cmd_decrease_font_size(ctx: &mut CommandCtx) {
    ctx.app.decrease_font_size();
}

fn cmd_reset_font_size(ctx: &mut CommandCtx) {
    ctx.app.reset_font_size();
}

fn cmd_git_history_close(ctx: &mut CommandCtx) {
    ctx.app.history_close();
}

fn cmd_git_hunk_stage(ctx: &mut CommandCtx) {
    ctx.app.git_hunk_stage();
}

fn cmd_git_hunk_discard(ctx: &mut CommandCtx) {
    ctx.app.git_hunk_discard_prompt();
}

fn cmd_git_hunk_preview(ctx: &mut CommandCtx) {
    ctx.app.git_hunk_preview();
}

fn cmd_git_blame(ctx: &mut CommandCtx) {
    ctx.app.git_blame_toggle();
}

fn cmd_git_blame_explain(ctx: &mut CommandCtx) {
    ctx.app.git_blame_explain();
}

fn cmd_git_close_view(ctx: &mut CommandCtx) {
    ctx.app.close_git_view();
}

fn cmd_git_file_history(ctx: &mut CommandCtx) {
    ctx.app.open_file_history();
}

fn cmd_git_line_history(ctx: &mut CommandCtx) {
    ctx.app.open_line_history();
}

fn cmd_git_versus(ctx: &mut CommandCtx) {
    ctx.app.start_side_by_side_picker();
}

fn cmd_git_rebase(ctx: &mut CommandCtx) {
    ctx.app.start_rebase_picker();
}

fn cmd_git_continue(ctx: &mut CommandCtx) {
    ctx.app.git_operation_continue();
}

fn cmd_git_abort(ctx: &mut CommandCtx) {
    ctx.app.git_operation_abort();
}

fn cmd_git_merge(ctx: &mut CommandCtx) {
    ctx.app.start_merge_picker();
}

fn cmd_git_pull_rebase(ctx: &mut CommandCtx) {
    ctx.app.git_pull_rebase();
}

fn cmd_git_force_push(ctx: &mut CommandCtx) {
    ctx.app.git_force_push();
}

fn cmd_git_next_conflict(ctx: &mut CommandCtx) {
    ctx.app.goto_conflict(true);
}

fn cmd_git_prev_conflict(ctx: &mut CommandCtx) {
    ctx.app.goto_conflict(false);
}

fn cmd_git_keep_ours(ctx: &mut CommandCtx) {
    ctx.app.resolve_conflict_at_cursor(fenix_git::Resolution::Ours);
}

fn cmd_git_keep_theirs(ctx: &mut CommandCtx) {
    ctx.app.resolve_conflict_at_cursor(fenix_git::Resolution::Theirs);
}

fn cmd_git_keep_both(ctx: &mut CommandCtx) {
    ctx.app.resolve_conflict_at_cursor(fenix_git::Resolution::Both);
}

fn cmd_git_merge_view(ctx: &mut CommandCtx) {
    ctx.app.open_merge_view();
}

fn cmd_git_merge_close(ctx: &mut CommandCtx) {
    ctx.app.merge_close();
}

fn cmd_git_stage_resolved(ctx: &mut CommandCtx) {
    ctx.app.merge_stage_resolved();
}
