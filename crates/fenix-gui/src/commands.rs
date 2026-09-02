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
        registry.register("app.save_all_and_quit", "Save every unsaved buffer, then quit Fenix", cmd_save_all_and_quit);
        registry.register(
            "view.cycle_line_numbers",
            "Cycle the line-number gutter: off, absolute, relative",
            cmd_cycle_line_numbers,
        );
        registry.register("view.pick_theme", "Pick a theme by name", cmd_pick_theme);
        registry.register("view.toggle_fullscreen", "Toggle fullscreen", cmd_toggle_fullscreen);
        registry.register("view.toggle_animations", "Toggle caret/scroll/pulse animations", cmd_toggle_animations);
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
        registry.register(
            "file.explore",
            "Browse the filesystem from your home directory; open a file directly, or fuzzy-search recursively from wherever you navigate to",
            cmd_file_explore,
        );
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
        registry.register("vnc.open", "Open or switch to a configured VNC session", cmd_vnc_open);
        registry.register("vnc.close", "Close the focused VNC session", cmd_vnc_close);
        registry.register("vnc.screenshot", "Save the focused VNC session's current frame as a PNG", cmd_vnc_screenshot);
        registry.register("pdf.next_page", "Turn the focused PDF session to the next page", cmd_pdf_next_page);
        registry.register("pdf.prev_page", "Turn the focused PDF session to the previous page", cmd_pdf_prev_page);
        registry.register("pdf.documents", "Open a document from the config.ini [documents] index", cmd_pdf_documents);
        registry.register("pdf.first_page", "Jump the focused PDF session to the first page", cmd_pdf_first_page);
        registry.register("pdf.last_page", "Jump the focused PDF session to the last page", cmd_pdf_last_page);
        registry.register("pdf.goto_page", "Prompt for a page number and jump to it", cmd_pdf_goto_page);
        registry.register("pdf.zoom_in", "Zoom the focused PDF session in", cmd_pdf_zoom_in);
        registry.register("pdf.zoom_out", "Zoom the focused PDF session out", cmd_pdf_zoom_out);
        registry.register("pdf.fit_page", "Fit the focused PDF session's page to the pane", cmd_pdf_fit_page);
        registry.register("pdf.fit_width", "Fit the focused PDF session's page to the pane's width", cmd_pdf_fit_width);
        registry.register("pdf.toggle_outline", "Toggle the focused PDF session's outline/bookmarks panel", cmd_pdf_toggle_outline);
        registry.register("pdf.search", "Search the focused PDF session's text for a word or phrase", cmd_pdf_search);
        registry.register("git.open", "Show the Git status/files/branches/commits/stash panel", cmd_git_open);
        registry.register("git.close", "Close the Git panel session", cmd_git_close);
        registry.register("jira.open", "Show the Jira projects/users/issues/detail panel", cmd_jira_open);
        registry.register("jira.close", "Close the Jira panel session", cmd_jira_close);
        registry.register("jira.refresh", "Re-fetch the Jira panel's current issues/detail", cmd_jira_refresh);
        registry.register("jira.goto_issue", "Jump straight to any issue by key", cmd_jira_goto_issue);
        registry.register("jira.add_project", "Track a new Jira project by key", cmd_jira_add_project);
        registry.register("jira.delete_project", "Stop tracking a Jira project", cmd_jira_delete_project);
        registry.register("jira.add_user", "Track a new Jira user by id", cmd_jira_add_user);
        registry.register("jira.delete_user", "Stop tracking a Jira user", cmd_jira_delete_user);
        registry.register("jira.create_issue", "Create a new Jira issue in a tracked project", cmd_jira_create_issue);
        registry.register("jira.submit_edit", "Submit the pending Jira comment/description edit", cmd_jira_submit_edit);
        registry.register("jira.cancel_edit", "Cancel the pending Jira comment/description edit", cmd_jira_cancel_edit);
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
        registry.register("buffer.switch", "Fuzzy-switch to another open buffer", cmd_switch_buffer);
        registry.register("buffer.next", "Switch to the next open buffer", cmd_next_buffer);
        registry.register("buffer.prev", "Switch to the previous open buffer", cmd_prev_buffer);
        registry.register("buffer.kill", "Close the focused buffer (refuses if it has unsaved changes)", cmd_kill_buffer);
        registry.register("buffer.kill_force", "Close the focused buffer immediately, discarding any unsaved changes", cmd_kill_buffer_force);
        registry.register("buffer.save_and_kill", "Save the focused buffer (if needed) then close it", cmd_save_and_kill_buffer);
        registry.register("buffer.scratch", "Open a new scratch buffer", cmd_new_scratch_buffer);
        registry.register("workspace.new", "Create a new workspace", cmd_new_workspace);
        registry.register("workspace.next", "Switch to the next workspace", cmd_next_workspace);
        registry.register("workspace.prev", "Switch to the previous workspace", cmd_prev_workspace);
        registry.register("workspace.remove", "Remove the active workspace", cmd_remove_workspace);
        registry.register("workspace.switch", "Switch to an open workspace by name", cmd_switch_workspace);
        registry.register(
            "workspace.find",
            "Open a configured workspace by name, creating it if it isn't already open",
            cmd_find_workspace,
        );
        registry.register("workspace.rename", "Rename the active workspace", cmd_rename_workspace);
        registry.register(
            "completion.refresh_tags",
            "Refresh Tcl completion tags (re-scans the project with ctags)",
            cmd_completion_refresh_tags,
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

fn cmd_save_all_and_quit(ctx: &mut CommandCtx) {
    ctx.app.request_save_all_and_quit(ctx.event_loop);
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

fn cmd_search_replace_buffer(ctx: &mut CommandCtx) {
    ctx.app.start_replace_buffer();
}

fn cmd_search_replace_project(ctx: &mut CommandCtx) {
    ctx.app.start_replace_project();
}

fn cmd_file_find(ctx: &mut CommandCtx) {
    ctx.app.start_find_file_prompt();
}

fn cmd_file_explore(ctx: &mut CommandCtx) {
    ctx.app.start_explore_from_home();
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

fn cmd_vnc_open(ctx: &mut CommandCtx) {
    ctx.app.start_vnc_picker();
}

fn cmd_vnc_close(ctx: &mut CommandCtx) {
    ctx.app.vnc_close_focused_session();
}

fn cmd_vnc_screenshot(ctx: &mut CommandCtx) {
    ctx.app.vnc_screenshot();
}

fn cmd_pdf_next_page(ctx: &mut CommandCtx) {
    ctx.app.pdf_next_page();
}

fn cmd_pdf_prev_page(ctx: &mut CommandCtx) {
    ctx.app.pdf_prev_page();
}

fn cmd_pdf_documents(ctx: &mut CommandCtx) {
    ctx.app.start_document_picker();
}

fn cmd_pdf_first_page(ctx: &mut CommandCtx) {
    ctx.app.pdf_first_page();
}

fn cmd_pdf_last_page(ctx: &mut CommandCtx) {
    ctx.app.pdf_last_page();
}

fn cmd_pdf_goto_page(ctx: &mut CommandCtx) {
    ctx.app.start_pdf_goto_page_prompt();
}

fn cmd_pdf_zoom_in(ctx: &mut CommandCtx) {
    ctx.app.pdf_zoom_in();
}

fn cmd_pdf_zoom_out(ctx: &mut CommandCtx) {
    ctx.app.pdf_zoom_out();
}

fn cmd_pdf_fit_page(ctx: &mut CommandCtx) {
    ctx.app.pdf_zoom_fit_page();
}

fn cmd_pdf_fit_width(ctx: &mut CommandCtx) {
    ctx.app.pdf_zoom_fit_width();
}

fn cmd_pdf_toggle_outline(ctx: &mut CommandCtx) {
    ctx.app.pdf_toggle_outline();
}

fn cmd_pdf_search(ctx: &mut CommandCtx) {
    ctx.app.start_pdf_search_prompt();
}

fn cmd_git_open(ctx: &mut CommandCtx) {
    ctx.app.open_git_panel();
}

fn cmd_git_close(ctx: &mut CommandCtx) {
    ctx.app.git_session_close();
}

fn cmd_jira_open(ctx: &mut CommandCtx) {
    ctx.app.open_jira_panel();
}

fn cmd_jira_close(ctx: &mut CommandCtx) {
    ctx.app.jira_session_close();
}

fn cmd_jira_refresh(ctx: &mut CommandCtx) {
    ctx.app.jira_refresh();
}

fn cmd_jira_goto_issue(ctx: &mut CommandCtx) {
    ctx.app.jira_start_goto_issue_prompt();
}

fn cmd_jira_add_project(ctx: &mut CommandCtx) {
    ctx.app.jira_start_add_project_prompt();
}

fn cmd_jira_delete_project(ctx: &mut CommandCtx) {
    ctx.app.picker_delete_jira_project();
}

fn cmd_jira_add_user(ctx: &mut CommandCtx) {
    ctx.app.jira_start_add_user_prompt();
}

fn cmd_jira_delete_user(ctx: &mut CommandCtx) {
    ctx.app.picker_delete_jira_user();
}

fn cmd_jira_create_issue(ctx: &mut CommandCtx) {
    ctx.app.picker_create_jira_issue();
}

fn cmd_jira_submit_edit(ctx: &mut CommandCtx) {
    ctx.app.jira_submit_edit();
}

fn cmd_jira_cancel_edit(ctx: &mut CommandCtx) {
    ctx.app.jira_cancel_edit();
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

fn cmd_kill_buffer_force(ctx: &mut CommandCtx) {
    ctx.app.force_kill_buffer();
}

fn cmd_save_and_kill_buffer(ctx: &mut CommandCtx) {
    ctx.app.save_and_close_buffer();
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

fn cmd_switch_workspace(ctx: &mut CommandCtx) {
    ctx.app.picker_switch_workspace();
}

fn cmd_find_workspace(ctx: &mut CommandCtx) {
    ctx.app.start_workspace_launcher_picker();
}

fn cmd_rename_workspace(ctx: &mut CommandCtx) {
    ctx.app.start_rename_workspace_prompt();
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
