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
        registry.register("app.quit", "Quit Fenix", cmd_quit);
        registry.register(
            "view.cycle_line_numbers",
            "Cycle the line-number gutter: off, absolute, relative",
            cmd_cycle_line_numbers,
        );
        registry.register("view.cycle_theme", "Cycle to the next theme", cmd_cycle_theme);
        registry.register(
            "explorer.jump",
            "Open a full-buffer directory listing at the current file's directory",
            cmd_explorer_jump,
        );
        registry.register("explorer.toggle_sidebar", "Toggle the file explorer sidebar", cmd_toggle_sidebar);
        registry.register("project.find_file", "Fuzzy-find a file in the current project", cmd_project_find_file);
        registry.register("project.grep", "Search the current project (ripgrep)", cmd_project_grep);
        registry.register("project.switch_project", "Switch to a different known project", cmd_project_switch);
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
    ctx.event_loop.exit();
}

fn cmd_cycle_line_numbers(ctx: &mut CommandCtx) {
    ctx.app.cycle_line_number_mode();
}

fn cmd_cycle_theme(ctx: &mut CommandCtx) {
    ctx.app.cycle_theme();
}

fn cmd_explorer_jump(ctx: &mut CommandCtx) {
    ctx.app.explorer_jump();
}

fn cmd_toggle_sidebar(ctx: &mut CommandCtx) {
    ctx.app.toggle_sidebar();
}

fn cmd_project_find_file(ctx: &mut CommandCtx) {
    ctx.app.picker_find_file();
}

fn cmd_project_grep(ctx: &mut CommandCtx) {
    ctx.app.picker_grep_prompt();
}

fn cmd_project_switch(ctx: &mut CommandCtx) {
    ctx.app.picker_switch_project();
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

fn cmd_increase_font_size(ctx: &mut CommandCtx) {
    ctx.app.increase_font_size();
}

fn cmd_decrease_font_size(ctx: &mut CommandCtx) {
    ctx.app.decrease_font_size();
}

fn cmd_reset_font_size(ctx: &mut CommandCtx) {
    ctx.app.reset_font_size();
}
