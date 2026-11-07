//! Notifications (`crate::notify`) in the editor: toasts drawn in a
//! corner of the focused window, over everything but the caret; their
//! clock, run only while Fenix is in front; the commands that act on
//! them (`SPC o n` the history, `SPC o x` dismiss, `SPC o X` all of them,
//! `SPC o .` the newest's action, `SPC t q` do not disturb); a click on
//! one (left: its action, any other button: dismiss); and, when Fenix
//! isn't in front, the system's own notification too (`notify-send`, a
//! Windows toast, macOS's `osascript`), as `notifications.system` says.

use super::*;
use crate::notify::{self, Pushed, Request, Settings, Urgency};
use crate::notifications_page::{Action as PageAction, NotificationsPage};

/// The most toasts at once, whatever the setting says: each has a glyph
/// buffer of its own.
pub(super) const MAX_TOASTS: usize = 8;
/// Space inside a toast, around its text.
const TOAST_PAD_X: f32 = 14.0;
const TOAST_PAD_Y: f32 = 8.0;
/// Space between two toasts.
const TOAST_GAP: f32 = 8.0;
/// The coloured rail down a toast's left edge.
const RAIL: f32 = 4.0;

/// Where the toasts stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Corner {
    TopRight,
    BottomRight,
    TopLeft,
    BottomLeft,
}

/// One toast as this frame draws it.
#[derive(Debug, Clone)]
pub(super) struct Toast {
    pub id: u64,
    pub slot: u8,
    pub rect: fenix_window::Rect,
    pub spans: RowSpans,
    pub rail: [f32; 4],
    /// Its bar, 0 to 1, and where it is: `(y, width)` from the toast's top.
    pub progress: Option<f32>,
}

impl App {
    /// `[notifications]`, as `notify` takes them.
    pub(super) fn notification_settings(&self) -> Settings {
        let c = &self.config;
        let secs = |key: &str, default: u64| Duration::from_secs(c.int_value(key).map_or(default, |n| n.max(0) as u64));
        let critical = c.int_value("notifications.timeout_critical").unwrap_or(0);
        let (rules, _) = notify::parse_rules(c.map_value("notifications.rules"));
        Settings {
            enabled: c.bool_value("notifications.enabled").unwrap_or(true),
            max_visible: (c.int_value("notifications.max_visible").unwrap_or(4).clamp(1, MAX_TOASTS as i64)) as usize,
            timeout_low: secs("notifications.timeout_low", 4),
            timeout_normal: secs("notifications.timeout_normal", 8),
            timeout_critical: (critical > 0).then(|| Duration::from_secs(critical as u64)),
            rules,
        }
    }

    fn notification_corner(&self) -> Corner {
        match self.config.text_value("notifications.position").unwrap_or("top-right") {
            "bottom-right" => Corner::BottomRight,
            "top-left" => Corner::TopLeft,
            "bottom-left" => Corner::BottomLeft,
            _ => Corner::TopRight,
        }
    }

    /// Shows a notification: as a toast, in the history, and -- when
    /// Fenix isn't in front -- the system's too.
    pub(crate) fn notify(&mut self, request: Request) -> Pushed {
        let settings = self.notification_settings();
        let system = request.clone();
        let pushed = self.notifications.push(request, &settings);
        if let Pushed::Shown(_) | Pushed::Updated(_) = pushed {
            let when = self.config.text_value("notifications.system").unwrap_or("unfocused");
            let away = !self.window_is_focused();
            if settings.enabled && (when == "always" || (when == "unfocused" && away)) && !matches!(pushed, Pushed::Updated(_) if system.tag.is_some()) {
                system_notify(&system);
            }
        }
        self.refresh_notifications_page();
        self.wake_caret();
        self.request_redraw_all();
        pushed
    }

    /// Whether one of Fenix's windows is in front.
    pub(super) fn window_is_focused(&self) -> bool {
        (0..self.frames.len()).filter_map(|f| self.frame_window(f)).any(|w| w.has_focus())
    }

    fn request_redraw_all(&self) {
        for frame in 0..self.frames.len() {
            if let Some(window) = self.frame_window(frame) {
                window.request_redraw();
            }
        }
    }

    /// The toasts' clock: when the next one times out, after redrawing
    /// for any that just did.
    pub(super) fn notifications_tick(&mut self, now: Instant) -> Option<Instant> {
        if self.notifications.is_empty() {
            return None;
        }
        let settings = self.notification_settings();
        let focused = self.window_is_focused();
        if self.notifications.tick(now, focused, &settings) {
            self.request_redraw_all();
        }
        if !focused {
            return None;
        }
        self.notifications.next_deadline(now, &settings).map(|due| due.max(now + Duration::from_millis(16)))
    }

    /// The toasts to draw in a window `window_width` wide, between `top`
    /// and the modeline's top.
    pub(super) fn toasts(&self, window_width: f32, top: f32, modeline_top: f32, char_width: f32, line_height: f32) -> Vec<Toast> {
        let settings = self.notification_settings();
        let visible = self.notifications.visible(&settings);
        // The history in front of you shows them already.
        let history_shown = self.pages.get(&self.focused_buffer_id()).is_some_and(|s| matches!(s.model, PageModel::Notifications(_)));
        if visible.is_empty() || history_shown {
            return Vec::new();
        }
        let theme = self.theme;
        let chars = self.config.int_value("notifications.width").unwrap_or(46).clamp(24, 120) as usize;
        let max_w = (window_width - 2.0 * text::WHICH_KEY_MARGIN).max(120.0);
        let width = (chars as f32 * char_width + 2.0 * TOAST_PAD_X + RAIL).min(max_w);
        let text_chars = ((width - 2.0 * TOAST_PAD_X - RAIL) / char_width).floor().max(10.0) as usize;
        let corner = self.notification_corner();
        let waiting = self.notifications.waiting(&settings);
        let newest = self.notifications.newest();
        // The newest nearest the corner.
        let mut y_top = top + text::WHICH_KEY_MARGIN;
        let mut y_bottom = modeline_top - text::WHICH_KEY_MARGIN;
        let x = match corner {
            Corner::TopRight | Corner::BottomRight => window_width - width - text::WHICH_KEY_MARGIN,
            Corner::TopLeft | Corner::BottomLeft => text::WHICH_KEY_MARGIN,
        }
        .max(0.0);
        let mut out = Vec::new();
        for (slot, note) in visible.iter().rev().enumerate() {
            let mut spans: RowSpans = Vec::new();
            let head_color = if note.urgency == Urgency::Critical { theme.git_conflicted } else { theme.fg };
            let source = crate::page::fit(&note.source, 16);
            spans.push((format!("{source}  "), theme.gutter_fg, false));
            let mut summary = note.summary.clone();
            if note.count > 1 {
                summary.push_str(&format!("  ×{}", note.count));
            }
            let room = text_chars.saturating_sub(source.chars().count() + 2);
            spans.push((crate::page::fit(&summary, room.max(8)), head_color, false));
            let mut rows = 1;
            for line in notify::wrap(&note.body, text_chars, 4) {
                spans.push(("\n".into(), theme.fg_modeline, false));
                spans.push((line, theme.fg_modeline, false));
                rows += 1;
            }
            let mut hint = Vec::new();
            if let Some(action) = note.actions.first() {
                hint.push(format!("↵ {}", action.label));
            }
            if note.actions.len() > 1 {
                hint.push(format!("+{} more", note.actions.len() - 1));
            }
            if Some(note.id) == newest && slot == 0 {
                hint.push(if note.actions.is_empty() { "SPC o x dismiss".into() } else { "SPC o . · SPC o x".into() });
            }
            if slot + 1 == visible.len() && waiting > 0 {
                hint.push(format!("{waiting} more waiting"));
            }
            if !hint.is_empty() {
                spans.push(("\n".into(), theme.fg_modeline, false));
                spans.push((crate::page::fit(&hint.join("   "), text_chars), rgba_to_glyphon(theme.caret), false));
                rows += 1;
            }
            let bar = if note.progress.is_some() { 8.0 } else { 0.0 };
            let height = rows as f32 * line_height + 2.0 * TOAST_PAD_Y + bar;
            let y = match corner {
                Corner::TopRight | Corner::TopLeft => {
                    let y = y_top;
                    y_top += height + TOAST_GAP;
                    y
                }
                Corner::BottomRight | Corner::BottomLeft => {
                    y_bottom -= height;
                    let y = y_bottom;
                    y_bottom -= TOAST_GAP;
                    y
                }
            };
            // Past the window's room, the rest wait.
            if y < top || y + height > modeline_top {
                break;
            }
            let rail = match note.urgency {
                Urgency::Low => glyphon_to_rgba(theme.gutter_fg),
                Urgency::Normal => theme.caret,
                Urgency::Critical => glyphon_to_rgba(theme.git_conflicted),
            };
            out.push(Toast { id: note.id, slot: slot as u8, rect: fenix_window::Rect { x, y, w: width, h: height }, spans, rail, progress: note.progress });
        }
        out
    }

    /// Draws toast `toast`'s box: the fill and border, its rail, its bar.
    pub(super) fn paint_toast(rects: &mut crate::rect::RectRenderer, gpu: &GpuState, toast: &Toast, rect: fenix_window::Rect, alpha: f32, theme: &Theme, radius: f32, shadows: bool) {
        let a = |[r, g, b, x]: [f32; 4]| [r, g, b, x * alpha];
        if shadows {
            rects.push_shadow(gpu, rect.x, rect.y + 4.0, rect.w, rect.h, radius, 12.0, [0.0, 0.0, 0.0, 0.40 * alpha]);
        }
        rects.push_box(gpu, rect.x, rect.y, rect.w, rect.h, crate::rect::BoxStyle {
            radius,
            fill: a(theme.bg_modeline),
            border: 1.0,
            border_color: a(glyphon_to_rgba(theme.gutter_fg)),
        });
        let inset = radius.min(RAIL * 2.0);
        rects.push_box(gpu, rect.x + 1.0, rect.y + inset, RAIL, (rect.h - 2.0 * inset).max(1.0), crate::rect::BoxStyle { radius: RAIL / 2.0, fill: a(toast.rail), border: 0.0, border_color: [0.0; 4] });
        if let Some(progress) = toast.progress {
            let (x, w) = (rect.x + TOAST_PAD_X + RAIL, rect.w - 2.0 * TOAST_PAD_X - RAIL);
            let y = rect.y + rect.h - TOAST_PAD_Y - 4.0;
            let track = glyphon_to_rgba(theme.gutter_fg);
            rects.push_box(gpu, x, y, w, 4.0, crate::rect::BoxStyle { radius: 2.0, fill: a([track[0], track[1], track[2], 0.35]), border: 0.0, border_color: [0.0; 4] });
            rects.push_box(gpu, x, y, (w * progress).max(4.0), 4.0, crate::rect::BoxStyle { radius: 2.0, fill: a(toast.rail), border: 0.0, border_color: [0.0; 4] });
        }
    }

    /// Where a toast's text starts inside it.
    pub(super) fn toast_inset() -> (f32, f32) {
        (TOAST_PAD_X + RAIL, TOAST_PAD_Y)
    }

    /// A click at `pos` on a toast: the left button does its action,
    /// any other dismisses it. True when it was on one.
    pub(super) fn toast_click(&mut self, pos: (f32, f32), button: MouseButton) -> bool {
        let (x, y) = pos;
        let Some(&(id, _)) = self.toast_rects.iter().find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) else { return false };
        if button == MouseButton::Left {
            self.notification_act(id, 0);
        } else {
            self.notifications.dismiss(id);
        }
        self.request_redraw_all();
        true
    }

    /// Runs notification `id`'s action `index`.
    pub(super) fn notification_act(&mut self, id: u64, index: usize) {
        if let Some(command) = self.notifications.act(id, index) {
            self.run_notification_command(&command);
        }
        self.refresh_notifications_page();
        self.request_redraw_all();
    }

    /// A notification's command: Fenix's own (`agents.thread:<id>`), or
    /// one the palette runs.
    fn run_notification_command(&mut self, command: &str) {
        if let Some(thread) = command.strip_prefix("agents.thread:") {
            self.agents_show(fenix_acp::hub::ThreadId(thread.to_string()));
            return;
        }
        if let Some(path) = command.strip_prefix("open:") {
            let (path, line) = match path.rsplit_once(':') {
                Some((p, l)) if l.parse::<usize>().is_ok() => (p, l.parse::<usize>().ok()),
                _ => (path, None),
            };
            let path = PathBuf::from(path);
            self.open_file_from_picker(&path);
            if let Some(line) = line {
                self.jump_to_grep_match(&fenix_project::GrepMatch { path, line: line.max(1), col: 1, text: String::new() });
            }
            return;
        }
        // A file or folder, in the system's own app for it (a screenshot,
        // the folder it's in).
        if let Some(path) = command.strip_prefix("reveal:") {
            #[cfg(not(test))]
            if let Err(e) = super::open_in_browser(path) {
                self.set_error(format!("{path}: {e}"));
            }
            #[cfg(test)]
            let _ = path;
            return;
        }
        if let Some(url) = command.strip_prefix("url:") {
            if url.starts_with("https://") || url.starts_with("http://") {
                #[cfg(not(test))]
                if let Err(e) = super::open_in_browser(url) {
                    self.set_error(format!("{url}: {e}"));
                }
            }
            return;
        }
        self.run_command_for_extension(command);
    }

    /// `SPC o .`: the newest toast's action.
    pub(crate) fn notifications_act_newest(&mut self) {
        match self.notifications.newest() {
            Some(id) => self.notification_act(id, 0),
            None => self.set_message("no notification showing"),
        }
    }

    /// `SPC o x`.
    pub(crate) fn notifications_dismiss_newest(&mut self) {
        match self.notifications.newest() {
            Some(id) => {
                self.notifications.dismiss(id);
                self.request_redraw_all();
            }
            None => self.set_message("no notification showing"),
        }
    }

    /// `SPC o X`.
    pub(crate) fn notifications_dismiss_all(&mut self) {
        let n = self.notifications.dismiss_all();
        self.set_message(match n {
            0 => "no notification showing".to_string(),
            1 => "1 notification dismissed".to_string(),
            n => format!("{n} notifications dismissed"),
        });
        self.request_redraw_all();
    }

    /// `SPC t q`: do not disturb, on or off.
    pub(crate) fn notifications_toggle_dnd(&mut self) {
        let on = !self.notifications.dnd;
        let missed = self.notifications.set_dnd(on);
        self.set_message(match (on, missed) {
            (true, _) => "do not disturb: on -- only critical notifications show (SPC o n keeps the rest)".to_string(),
            (false, 0) => "do not disturb: off".to_string(),
            (false, n) => format!("do not disturb: off -- {n} came meanwhile: SPC o n"),
        });
        self.refresh_notifications_page();
        self.request_redraw_all();
    }

    /// The modeline's word on notifications: do not disturb, what it kept.
    pub(super) fn notifications_segment(&self) -> String {
        match (self.notifications.dnd, self.notifications.missed) {
            (false, _) => String::new(),
            (true, 0) => "   ◌ quiet".to_string(),
            (true, n) => format!("   ◌ quiet: {n} kept"),
        }
    }

    /// A task in the task pane ended: a notification, as
    /// `notifications.tasks` says.
    pub(super) fn task_notify(&mut self, ok: bool, took: Option<Duration>) {
        let when = self.config.text_value("notifications.tasks").unwrap_or("long");
        let long = took.is_some_and(|t| t >= Duration::from_secs(10));
        let wanted = match when {
            "all" => true,
            "failed" => !ok,
            "never" => false,
            _ => long,
        };
        if !wanted {
            return;
        }
        let Some(result) = self.wasm.last_task.clone() else { return };
        let name = if result.name.is_empty() { "the task".to_string() } else { result.name.clone() };
        let mut body = Vec::new();
        if let Some((passed, failed)) = result.tests {
            body.push(if failed > 0 { format!("{failed} test{} failed, {passed} passed", if failed == 1 { "" } else { "s" }) } else { format!("{passed} tests passed") });
        }
        if let Some(t) = took {
            let secs = t.as_secs();
            body.push(if secs >= 60 { format!("in {} min {} s", secs / 60, secs % 60) } else { format!("in {secs} s") });
        }
        let errors = self.quickfix.iter().filter(|e| matches!(e, QuickfixEntry::Task(_))).count();
        let mut request = Request::new("tasks", if ok { format!("{name} succeeded") } else { format!("{name} failed") })
            .body(body.join(" · "))
            .urgency(if ok { Urgency::Low } else { Urgency::Normal })
            .tag("task");
        if !ok && errors > 0 {
            request = request.action(format!("Go to its problems ({errors})"), "project.quickfix_next");
        }
        request = request.action("Run it again", "task.rerun_last");
        self.notify(request);
    }

    /// Extension `ext`'s notification (`notify-with`). An action is one
    /// of its own commands or Fenix's, a web address, or a file; past 10
    /// a minute they're only kept in the history.
    pub(super) fn extension_notify(&mut self, ext: &str, n: fenix_wasm::host::Notification) {
        let now = Instant::now();
        let recent = self.notification_rates.entry(ext.to_string()).or_default();
        recent.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
        recent.push(now);
        let quiet = recent.len() > 10;
        let actions = n
            .actions
            .into_iter()
            .filter(|a| a.command.starts_with("url:https://") || a.command.starts_with("url:http://") || a.command.starts_with("open:") || self.commands.get(&a.command).is_some())
            .map(|a| notify::Action { label: a.label, command: a.command })
            .collect();
        let urgency = match n.urgency {
            fenix_wasm::host::Urgency::Low => Urgency::Low,
            fenix_wasm::host::Urgency::Normal => Urgency::Normal,
            fenix_wasm::host::Urgency::Critical => Urgency::Critical,
        };
        let request = Request {
            source: ext.to_string(),
            summary: n.summary,
            body: n.body,
            urgency: Some(urgency),
            actions,
            tag: n.tag.map(|t| format!("{ext}:{t}")),
            timeout: n.timeout_ms.map(|ms| Duration::from_millis(ms.clamp(500, 600_000) as u64)),
            progress: n.progress,
            quiet,
        };
        self.notify(request);
    }

    /// Extension `ext`'s toast tagged `tag`, taken away.
    pub(super) fn extension_notify_dismiss(&mut self, ext: &str, tag: &str) {
        if self.notifications.dismiss_tag(ext, &format!("{ext}:{tag}")) {
            self.request_redraw_all();
        }
    }

    /// `notifications.test`: one of each, to see how they look.
    pub(crate) fn notifications_test(&mut self) {
        self.notify(Request::new("fenix", "Notifications work").body("This is a normal one. It goes after a while, counted only while Fenix is in front.").urgency(Urgency::Normal).action("Open the history", "notifications.history"));
        self.notify(Request::new("tasks", "cargo test: 812 passed").body("in 1 min 12 s").urgency(Urgency::Low));
        self.notify(Request { progress: Some(0.6), ..Request::new("fenix", "Indexing the project").body("3,204 of 5,340 files").tag("notifications.test.progress") });
        self.notify(Request::new("agents", "Claude Code asks to run a command").body("cargo test --workspace").urgency(Urgency::Critical).action("Open the history", "notifications.history"));
    }

    /// `SPC o n`: the history page.
    pub(crate) fn open_notifications_page(&mut self) {
        match self.find_page(|m| matches!(m, PageModel::Notifications(_))) {
            Some(id) => {
                self.refresh_notifications_page();
                self.show_page(id);
            }
            None => {
                let notes = self.notifications.history().iter().rev().cloned().collect();
                let page = NotificationsPage::new(notes, self.notifications.dnd);
                self.open_page(PageModel::Notifications(Box::new(page)));
            }
        }
        // Looked at: what do not disturb kept isn't news any more.
        self.notifications.missed = 0;
    }

    pub(super) fn refresh_notifications_page(&mut self) {
        let Some(id) = self.find_page(|m| matches!(m, PageModel::Notifications(_))) else { return };
        let notes: Vec<notify::Note> = self.notifications.history().iter().rev().cloned().collect();
        let dnd = self.notifications.dnd;
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::Notifications(page) = &mut state.model {
                page.update(notes, dnd);
            }
            state.stale = true;
        }
    }

    pub(super) fn notifications_page_action(&mut self, id: BufferId, action: PageAction) {
        match action {
            PageAction::None => {}
            PageAction::Close => self.close_page(id),
            PageAction::Act(note, index) => self.notification_act(note, index),
            PageAction::Reshow(note) => {
                let settings = self.notification_settings();
                if !self.notifications.reshow(note, &settings) {
                    self.set_message("it's showing");
                }
                self.request_redraw_all();
            }
            PageAction::Forget(note) => {
                self.notifications.forget(note);
                self.refresh_notifications_page();
                self.request_redraw_all();
            }
            PageAction::ClearAll => {
                self.notifications.clear_history();
                self.refresh_notifications_page();
            }
            PageAction::ToggleDnd => self.notifications_toggle_dnd(),
        }
    }
}

/// The system's own notification, in a thread of its own: `notify-send`
/// on Linux, a toast through PowerShell on Windows, `osascript` on macOS.
/// Nothing when there's none to be had.
fn system_notify(request: &Request) {
    let summary = format!("{} · {}", request.source, request.summary);
    let body = request.body.clone();
    let urgency = request.urgency.unwrap_or(Urgency::Normal);
    std::thread::spawn(move || {
        let mut command = system_command(&summary, &body, urgency);
        if let Some(command) = command.as_mut() {
            command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            if let Ok(mut child) = command.spawn() {
                let _ = child.wait();
            }
        }
    });
}

/// The command that shows the system's notification, if there's one here.
fn system_command(summary: &str, body: &str, urgency: Urgency) -> Option<std::process::Command> {
    if cfg!(target_os = "windows") {
        // Windows' own toast, through WinRT from PowerShell: nothing to
        // install. The texts go as arguments, never into the script.
        let script = "param($t, $b) \
            [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
            $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
            $n = $x.GetElementsByTagName('text'); $n.Item(0).AppendChild($x.CreateTextNode($t)) > $null; $n.Item(1).AppendChild($x.CreateTextNode($b)) > $null; \
            [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Fenix').Show([Windows.UI.Notifications.ToastNotification]::new($x))";
        let mut c = std::process::Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &format!("& {{ {script} }}"), summary, body]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        Some(c)
    } else if cfg!(target_os = "macos") {
        let quote = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let mut c = std::process::Command::new("osascript");
        c.args(["-e", &format!("display notification {} with title \"Fenix\" subtitle {}", quote(body), quote(summary))]);
        Some(c)
    } else {
        let program = which("notify-send")?;
        let mut c = std::process::Command::new(program);
        c.args(["--app-name=Fenix", "--icon=fenix", &format!("--urgency={}", urgency.name()), "--", summary, body]);
        Some(c)
    }
}

/// `program` on `PATH`.
fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toasts_stack_in_their_corner_newest_first_and_wait_past_the_room() {
        let mut app = App::with_file(None);
        app.notify(Request::new("tasks", "first"));
        app.notify(Request::new("agents", "second").body("a body\nof two lines").action("Open", "notifications.history"));
        let toasts = app.toasts(1200.0, 0.0, 800.0, 9.0, 20.0);
        assert_eq!(toasts.len(), 2);
        let text = |t: &Toast| t.spans.iter().map(|s| s.0.as_str()).collect::<String>();
        assert!(text(&toasts[0]).contains("second") && text(&toasts[0]).contains("↵ Open"), "the newest nearest the corner: {}", text(&toasts[0]));
        assert!(toasts[0].rect.y < toasts[1].rect.y, "top-right: newest on top");
        assert!(toasts[0].rect.x + toasts[0].rect.w <= 1200.0);
        app.config.set_value("notifications.position", Some(fenix_config::Value::Text("bottom-left".into())));
        let toasts = app.toasts(1200.0, 0.0, 800.0, 9.0, 20.0);
        assert!(toasts[0].rect.y > toasts[1].rect.y && toasts[0].rect.x < 100.0, "bottom-left: newest at the bottom");
        assert!(toasts.iter().all(|t| t.rect.y + t.rect.h <= 800.0));
        // A window too short for both: the one that doesn't fit waits.
        let toasts = app.toasts(1200.0, 0.0, 110.0, 9.0, 20.0);
        assert_eq!(toasts.len(), 1);
    }

    #[test]
    fn the_commands_dismiss_toggle_and_keep_a_history_page() {
        let mut app = App::with_file(None);
        app.notify(Request::new("tasks", "one"));
        app.notify(Request::new("tasks", "two"));
        app.notifications_dismiss_newest();
        assert_eq!(app.notifications.visible(&app.notification_settings()).len(), 1);
        app.notifications_dismiss_all();
        assert!(app.notifications.is_empty());
        app.notifications_toggle_dnd();
        assert!(app.notifications_segment().contains("quiet"));
        assert!(matches!(app.notify(Request::new("tasks", "three")), Pushed::Kept(_)));
        assert!(app.notifications_segment().contains("1 kept"));
        app.open_notifications_page();
        let id = app.find_page(|m| matches!(m, PageModel::Notifications(_))).expect("the page");
        let Some(PageModel::Notifications(page)) = app.pages.get(&id).map(|s| &s.model) else { panic!() };
        assert_eq!(page.notes.len(), 3);
        assert!(page.dnd);
        assert_eq!(app.notifications_segment(), "   ◌ quiet", "looked at");
    }

    #[test]
    fn an_extensions_notifications_keep_to_its_actions_and_its_rate() {
        let mut app = App::with_file(None);
        let n = |summary: &str| fenix_wasm::host::Notification {
            summary: summary.into(),
            body: String::new(),
            urgency: fenix_wasm::host::Urgency::Normal,
            actions: vec![
                fenix_wasm::host::NotificationAction { label: "History".into(), command: "notifications.history".into() },
                fenix_wasm::host::NotificationAction { label: "Nothing".into(), command: "no.such.command".into() },
                fenix_wasm::host::NotificationAction { label: "Web".into(), command: "url:https://example.com".into() },
            ],
            tag: Some("ci".into()),
            progress: None,
            timeout_ms: None,
        };
        app.extension_notify("acme.ci", n("red"));
        let shown = &app.notifications.visible(&app.notification_settings())[0];
        assert_eq!(shown.source, "acme.ci");
        assert_eq!(shown.actions.iter().map(|a| a.label.as_str()).collect::<Vec<_>>(), vec!["History", "Web"], "a command Fenix doesn't have is dropped");
        app.extension_notify("acme.ci", n("green"));
        assert_eq!(app.notifications.visible(&app.notification_settings()).len(), 1, "its tag replaces");
        app.extension_notify_dismiss("acme.ci", "ci");
        assert!(app.notifications.is_empty());
        for i in 0..9 {
            app.extension_notify("acme.ci", fenix_wasm::host::Notification { tag: None, ..n(&format!("n{i}")) });
        }
        assert_eq!(app.notifications.visible(&app.notification_settings()).len() + app.notifications.waiting(&app.notification_settings()), 8, "past 10 a minute, only kept");
        assert_eq!(app.notifications.history().len(), 10, "the tagged one once, then nine");
    }

    #[test]
    fn rules_and_off_are_read_from_the_settings() {
        let mut app = App::with_file(None);
        app.config.set_value("notifications.rules", Some(fenix_config::Value::Map(vec![("fenix.pets".into(), "mute".into())])));
        assert_eq!(app.notify(Request::new("fenix.pets", "nap")), Pushed::Muted);
        app.config.set_value("notifications.enabled", Some(fenix_config::Value::Bool(false)));
        assert!(matches!(app.notify(Request::new("tasks", "x")), Pushed::Kept(_)));
        assert!(app.notifications.is_empty());
    }
}
