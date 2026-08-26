use winit::event_loop::ActiveEventLoop;

use crate::app::App;

/// Context a command runs with: the editor state plus the event loop handle
/// needed for app-level actions like quitting.
pub struct CommandCtx<'a> {
    pub app: &'a mut App,
    pub event_loop: &'a ActiveEventLoop,
}

type CommandFn = fn(&mut CommandCtx);

/// A named, invokable editor action. Every user-facing action should
/// eventually be one of these rather than a function wired directly to a
/// key, so the same action can be bound in multiple keymaps (Phase 2) and,
/// later, exposed to Lua scripting (Phase 6) without a rewrite.
pub struct Command {
    pub id: &'static str,
    #[allow(dead_code)] // will back a command palette in a later phase
    pub description: &'static str,
    run: CommandFn,
}

pub struct CommandRegistry {
    commands: Vec<Command>,
}

impl CommandRegistry {
    pub fn with_builtins() -> Self {
        let mut registry = Self { commands: Vec::new() };
        registry.register("file.save", "Save the current file", cmd_save);
        registry.register("edit.undo", "Undo the last edit", cmd_undo);
        registry.register("edit.redo", "Redo the last undone edit", cmd_redo);
        registry.register("app.quit", "Quit Fenix (confirms first if there are unsaved buffers)", cmd_quit);
        registry.register("app.quit_force", "Quit Fenix immediately, discarding any unsaved changes", cmd_quit_force);
        registry.register(
            "view.cycle_line_numbers",
            "Cycle the line-number gutter: off, absolute, relative",
            cmd_cycle_line_numbers,
        );
        registry.register("view.pick_theme", "Pick a theme by name", cmd_pick_theme);
        registry.register("view.toggle_fullscreen", "Toggle fullscreen", cmd_toggle_fullscreen);
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
        registry.register("search.replace_buffer", "Search and replace in the current buffer", cmd_search_replace_buffer);
        registry.register(
            "search.replace_project",
            "Search and replace across the project",
            cmd_search_replace_project,
        );
        registry.register("file.find", "Open a file by typing its path (bypasses .gitignore)", cmd_file_find);
        registry.register("file.find_all", "Fuzzy-find a file in the project, including gitignored ones", cmd_file_find_all);
        registry.register("file.recent", "Fuzzy-find a recently-opened file", cmd_file_recent);
        registry.register("file.rename", "Rename the current file on disk", cmd_file_rename);
        registry.register("file.delete", "Delete the current file (with confirmation)", cmd_file_delete);
        registry.register("file.yank_path", "Copy the current file's path to the clipboard", cmd_file_yank_path);
        registry.register("project.find_file", "Fuzzy-find a file in the current project", cmd_project_find_file);
        registry.register("project.grep", "Search the current project (ripgrep)", cmd_project_grep);
        registry.register("project.quickfix_next", "Jump to the next match in the last project search", cmd_quickfix_next);
        registry.register(
            "project.quickfix_prev",
            "Jump to the previous match in the last project search",
            cmd_quickfix_prev,
        );
        registry.register("project.switch_project", "Switch to a different known project", cmd_project_switch);
        registry.register("project.add", "Register a project in the switch-project list", cmd_project_add);
        registry.register("project.delete", "Remove a project from the switch-project list", cmd_project_delete);
        registry.register("dashboard.open", "Show the startup dashboard", cmd_dashboard_open);
        registry.register("terminal.toggle", "Toggle the terminal panel", cmd_toggle_terminal);
        registry.register("docker.open", "Show the Docker container/image panel", cmd_docker_open);
        registry.register("docker.build", "Build an image from the current project's Dockerfile", cmd_docker_build);
        registry.register("docker.close", "Close the Docker panel session", cmd_docker_close);
        registry.register("git.open", "Show the Git status/files/branches/commits/stash panel", cmd_git_open);
        registry.register("git.close", "Close the Git panel session", cmd_git_close);
        registry.register("window.split_vertical", "Split the focused window side by side", cmd_split_vertical);
        registry.register("window.split_horizontal", "Split the focused window stacked", cmd_split_horizontal);
        registry.register("window.navigate_left", "Move focus to the window on the left", cmd_navigate_left);
        registry.register("window.navigate_right", "Move focus to the window on the right", cmd_navigate_right);
        registry.register("window.navigate_up", "Move focus to the window above", cmd_navigate_up);
        registry.register("window.navigate_down", "Move focus to the window below", cmd_navigate_down);
        registry.register("window.cycle", "Cycle focus to the next window", cmd_cycle_window);
        registry.register("window.close", "Close the focused window", cmd_close_window);
        registry.register("window.only", "Close every window except the focused one", cmd_only_window);
        registry.register("window.balance", "Reset every split ratio to 0.5", cmd_balance_windows);
        registry.register("buffer.switch", "Fuzzy-switch to another open buffer", cmd_switch_buffer);
        registry.register("buffer.next", "Switch to the next open buffer", cmd_next_buffer);
        registry.register("buffer.prev", "Switch to the previous open buffer", cmd_prev_buffer);
        registry.register("buffer.kill", "Close the focused buffer", cmd_kill_buffer);
        registry.register("buffer.scratch", "Open a new scratch buffer", cmd_new_scratch_buffer);
        registry.register("workspace.new", "Create a new workspace", cmd_new_workspace);
        registry.register("workspace.next", "Switch to the next workspace", cmd_next_workspace);
        registry.register("workspace.prev", "Switch to the previous workspace", cmd_prev_workspace);
        registry.register("workspace.remove", "Remove the active workspace", cmd_remove_workspace);
        registry.register(
            "completion.refresh_tags",
            "Refresh Tcl completion tags (re-scans the project with ctags)",
            cmd_completion_refresh_tags,
        );
        registry.register("code.format_selection", "Format the active Visual selection", cmd_format_selection);
        registry.register("code.format_buffer", "Format the whole focused buffer", cmd_format_buffer);
        registry.register(
            "code.symbols",
            "Fuzzy-find a Tcl definition by its fully-qualified name and jump to it",
            cmd_symbols,
        );
        registry.register("nav.jump_back", "Jump to the previous position in the jumplist (Ctrl-O)", cmd_jump_back);
        registry.register("nav.jump_forward", "Jump to the next position in the jumplist (Ctrl-I)", cmd_jump_forward);
        registry.register("mib.lookup_telecommand", "Fuzzy-find a MIB telecommand and view its details", cmd_mib_lookup_telecommand);
        registry.register("mib.insert_telecommand", "Build and insert a telecommand from the MIB", cmd_mib_insert_telecommand);
        registry.register("mib.lookup_tm_packet", "Fuzzy-find a MIB TM packet and view its details", cmd_mib_lookup_tm_packet);
        registry.register("mib.lookup_tm_parameter", "Fuzzy-find a MIB TM parameter and view its details", cmd_mib_lookup_tm_parameter);
        registry.register("mib.lookup_calibration", "Fuzzy-find a MIB calibration definition and view its details", cmd_mib_lookup_calibration);
        registry.register("mib.refresh_index", "Reparse the configured MIB roots from disk", cmd_mib_refresh_index);
        registry.register("mib.add_root", "Browse to and register a new MIB root directory", cmd_mib_add_root);
        registry.register("mib.delete_root", "Fuzzy-find and remove a configured MIB root", cmd_mib_delete_root);
        registry.register("view.increase_font_size", "Increase the body text size", cmd_increase_font_size);
        registry.register("view.decrease_font_size", "Decrease the body text size", cmd_decrease_font_size);
        registry.register("view.reset_font_size", "Reset the body text size to the default", cmd_reset_font_size);
        registry
    }

    fn register(&mut self, id: &'static str, description: &'static str, run: CommandFn) {
        self.commands.push(Command { id, description, run });
    }

    #[allow(dead_code)] // will back a command palette in a later phase
    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.iter()
    }

    /// Runs the command named `id` against `app`. Returns `false` if no
    /// command with that id is registered.
    pub fn run(&self, app: &mut App, event_loop: &ActiveEventLoop, id: &str) -> bool {
        let Some(cmd) = self.commands.iter().find(|c| c.id == id) else { return false };
        let mut ctx = CommandCtx { app, event_loop };
        (cmd.run)(&mut ctx);
        true
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
    ctx.event_loop.exit();
}

fn cmd_cycle_line_numbers(ctx: &mut CommandCtx) {
    ctx.app.cycle_line_number_mode();
}

fn cmd_pick_theme(ctx: &mut CommandCtx) {
    ctx.app.picker_pick_theme();
}

fn cmd_toggle_fullscreen(ctx: &mut CommandCtx) {
    ctx.app.toggle_fullscreen();
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

fn cmd_search_replace_buffer(ctx: &mut CommandCtx) {
    ctx.app.start_replace_buffer();
}

fn cmd_search_replace_project(ctx: &mut CommandCtx) {
    ctx.app.start_replace_project();
}

fn cmd_file_find(ctx: &mut CommandCtx) {
    ctx.app.start_find_file_prompt();
}

fn cmd_file_find_all(ctx: &mut CommandCtx) {
    ctx.app.picker_find_file_all();
}

fn cmd_file_recent(ctx: &mut CommandCtx) {
    ctx.app.picker_recent_files();
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

fn cmd_project_find_file(ctx: &mut CommandCtx) {
    ctx.app.picker_find_file();
}

fn cmd_project_grep(ctx: &mut CommandCtx) {
    ctx.app.picker_grep_prompt();
}

fn cmd_quickfix_next(ctx: &mut CommandCtx) {
    ctx.app.quickfix_next();
}

fn cmd_quickfix_prev(ctx: &mut CommandCtx) {
    ctx.app.quickfix_prev();
}

fn cmd_project_switch(ctx: &mut CommandCtx) {
    ctx.app.picker_switch_project();
}

fn cmd_project_add(ctx: &mut CommandCtx) {
    ctx.app.picker_add_project_prompt();
}

fn cmd_project_delete(ctx: &mut CommandCtx) {
    ctx.app.picker_delete_project();
}

fn cmd_dashboard_open(ctx: &mut CommandCtx) {
    ctx.app.open_dashboard();
}

fn cmd_toggle_terminal(ctx: &mut CommandCtx) {
    ctx.app.toggle_terminal();
}

fn cmd_docker_open(ctx: &mut CommandCtx) {
    ctx.app.open_docker_panel();
}

fn cmd_docker_build(ctx: &mut CommandCtx) {
    ctx.app.docker_build();
}

fn cmd_docker_close(ctx: &mut CommandCtx) {
    ctx.app.docker_session_close();
}

fn cmd_git_open(ctx: &mut CommandCtx) {
    ctx.app.open_git_panel();
}

fn cmd_git_close(ctx: &mut CommandCtx) {
    ctx.app.git_session_close();
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

fn cmd_balance_windows(ctx: &mut CommandCtx) {
    ctx.app.balance_windows();
}

fn cmd_switch_buffer(ctx: &mut CommandCtx) {
    ctx.app.picker_switch_buffer();
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

fn cmd_new_scratch_buffer(ctx: &mut CommandCtx) {
    ctx.app.new_scratch_buffer();
}

fn cmd_new_workspace(ctx: &mut CommandCtx) {
    ctx.app.new_workspace();
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

fn cmd_completion_refresh_tags(ctx: &mut CommandCtx) {
    ctx.app.refresh_completion_tags();
}

fn cmd_format_selection(ctx: &mut CommandCtx) {
    ctx.app.format_selection();
}

fn cmd_format_buffer(ctx: &mut CommandCtx) {
    ctx.app.format_buffer();
}

fn cmd_symbols(ctx: &mut CommandCtx) {
    ctx.app.picker_symbols();
}

fn cmd_jump_back(ctx: &mut CommandCtx) {
    ctx.app.jump_back();
}

fn cmd_jump_forward(ctx: &mut CommandCtx) {
    ctx.app.jump_forward();
}

fn cmd_mib_lookup_telecommand(ctx: &mut CommandCtx) {
    ctx.app.mib_lookup_telecommand();
}

fn cmd_mib_insert_telecommand(ctx: &mut CommandCtx) {
    ctx.app.mib_insert_telecommand();
}

fn cmd_mib_lookup_tm_packet(ctx: &mut CommandCtx) {
    ctx.app.mib_lookup_tm_packet();
}

fn cmd_mib_lookup_tm_parameter(ctx: &mut CommandCtx) {
    ctx.app.mib_lookup_tm_parameter();
}

fn cmd_mib_lookup_calibration(ctx: &mut CommandCtx) {
    ctx.app.mib_lookup_calibration();
}

fn cmd_mib_refresh_index(ctx: &mut CommandCtx) {
    ctx.app.mib_refresh_index();
}

fn cmd_mib_add_root(ctx: &mut CommandCtx) {
    ctx.app.picker_add_mib_root_prompt();
}

fn cmd_mib_delete_root(ctx: &mut CommandCtx) {
    ctx.app.picker_delete_mib_root();
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
