use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fenix_keymap::{KeyPress, Matcher, NamedKey as FenixNamedKey, Step};
use fenix_vim::{Mode, VimEvent, VimState, VisualKind};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowId};

use fenix_core::{Buffer, Cursor};

use crate::commands::CommandRegistry;
use crate::gpu::GpuState;
use crate::keymap;
use crate::rect::RectRenderer;
use crate::text::{self, TextPipeline};
use crate::theme::{self, Theme};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);
/// How long the caret takes to fade in/out at each blink toggle, instead
/// of flipping instantly.
const BLINK_FADE: Duration = Duration::from_millis(120);
/// Redraw cadence while an animation (blink fade, later scroll/pulse) is
/// actively transitioning. Idle time uses the much longer `WaitUntil`s
/// below instead -- see the plan's "animations are short bursts, not
/// continuous idle work" rationale.
const ANIM_TICK: Duration = Duration::from_millis(16);
/// How long a yank/paste pulse stays visible before fully fading out.
/// Modeled on orbit-emacs's own yank/paste pulse feature.
const PULSE_DURATION: Duration = Duration::from_millis(300);
/// Alpha the pulse starts at before fading -- brighter than the steady
/// Visual-selection overlay so it reads as a distinct flash, not a
/// held selection.
const PULSE_PEAK_ALPHA: f32 = 0.45;
/// How long the viewport takes to ease to a new scroll position.
const SCROLL_DURATION: Duration = Duration::from_millis(150);
/// Jumps larger than this many screens snap instantly instead of
/// animating. Panning smoothly through a huge jump (`G` on a large file)
/// would either blur past unreadably fast or need fetching far more
/// content than the viewport actually needs, fighting the whole point of
/// windowed rendering (`Buffer::visible_text` only ever fetches what's
/// on screen).
const SCROLL_SNAP_SCREENS: usize = 3;

/// An active yank/paste highlight, fading out over `PULSE_DURATION`.
struct Pulse {
    range: std::ops::Range<usize>,
    started: Instant,
}

/// An in-flight scroll transition: eases `rendered_scroll` from `from`
/// to the (now-current) `scroll_line` target.
struct ScrollAnim {
    from: f32,
    to: usize,
    started: Instant,
}

/// Per-visible-line highlight segments: (view_row, col_start, col_end).
type Segments = Vec<(usize, usize, usize)>;

/// Smallest adjustment to `scroll_line` that brings `cursor_line` into the
/// `[scroll_line, scroll_line + visible_lines)` window.
fn scroll_to_include(scroll_line: usize, cursor_line: usize, visible_lines: usize) -> usize {
    if cursor_line < scroll_line {
        cursor_line
    } else if cursor_line >= scroll_line + visible_lines {
        cursor_line + 1 - visible_lines
    } else {
        scroll_line
    }
}

/// Ease-out cubic: fast start, gentle settle. Used for every short
/// transition animation (caret fade now; scroll/pulse reuse it too).
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    text: Option<TextPipeline>,
    /// Opaque panel backgrounds (modeline bar, which-key popup) and the
    /// selection highlight -- drawn *before* text, so they sit behind it
    /// instead of covering it. A single alpha-blended draw call can't do
    /// both "opaque background behind this text" and "opaque background
    /// in front of that text" at once, so the caret gets its own renderer
    /// drawn after text instead of sharing this one.
    bg_rect: Option<RectRenderer>,
    caret_rect: Option<RectRenderer>,

    buffer: Buffer,
    cursor: Cursor,
    /// Index of the topmost buffer line currently on screen -- the
    /// *target* `ensure_cursor_visible` maintains; rendering uses
    /// `rendered_scroll` instead, which eases toward this.
    scroll_line: usize,
    /// The actual (possibly fractional, mid-transition) render position.
    /// `rendered_scroll.floor()` is which buffer line rendering starts
    /// from; the fractional part is a sub-line-height pixel shift, giving
    /// the smooth-scroll illusion without changing how much content gets
    /// fetched.
    rendered_scroll: f32,
    scroll_anim: Option<ScrollAnim>,

    vim: VimState,
    /// Persists across keystrokes so a `SPC f s` sequence can span several
    /// `handle_key` calls; `'static` because the leader trie is a global
    /// singleton (see `keymap::leader_trie`), which sidesteps
    /// `Matcher` borrowing from a trie `App` would otherwise also own.
    leader_matcher: Matcher<'static, &'static str>,

    theme: &'static Theme,

    modifiers: ModifiersState,
    /// Whether the caret is fading toward visible or toward hidden --
    /// the *target* of the current transition, not necessarily what's on
    /// screen right now (see `caret_alpha`).
    blink_visible: bool,
    /// When the current fade started; `caret_alpha` eases from this.
    blink_transition_start: Instant,
    next_blink: Instant,
    pulse: Option<Pulse>,
}

impl App {
    pub fn new() -> Self {
        Self::with_file(env::args().nth(1))
    }

    fn with_file(file_arg: Option<String>) -> Self {
        let buffer = match file_arg.as_deref() {
            Some(path) => Buffer::from_path(path).unwrap_or_else(|err| {
                eprintln!("fenix: couldn't open {path} ({err}), starting empty buffer");
                Buffer::empty()
            }),
            None => Buffer::empty(),
        };

        Self {
            window: None,
            gpu: None,
            text: None,
            bg_rect: None,
            caret_rect: None,
            buffer,
            cursor: Cursor::at_start(),
            scroll_line: 0,
            rendered_scroll: 0.0,
            scroll_anim: None,
            vim: VimState::new(),
            leader_matcher: keymap::leader_trie().matcher(),
            theme: &theme::ORBIT_DARK,
            modifiers: ModifiersState::empty(),
            blink_visible: true,
            blink_transition_start: Instant::now() - BLINK_FADE,
            next_blink: Instant::now() + BLINK_INTERVAL,
            pulse: None,
        }
    }

    pub(crate) fn save(&mut self) {
        if self.buffer.path().is_none() {
            eprintln!("fenix: no file path to save to yet; pass a file path as the first argument");
            return;
        }
        match self.buffer.save() {
            Ok(()) => println!("fenix: saved {:?}", self.buffer.path().unwrap()),
            Err(err) => eprintln!("fenix: save failed: {err}"),
        }
        self.wake_caret();
    }

    pub(crate) fn undo(&mut self) {
        self.buffer.undo(&mut self.cursor);
        self.wake_caret();
    }

    pub(crate) fn redo(&mut self) {
        self.buffer.redo(&mut self.cursor);
        self.wake_caret();
    }

    /// Resets the caret blink timer so an edit or navigation always leaves
    /// the caret visible instead of possibly mid-blink.
    fn wake_caret(&mut self) {
        let now = Instant::now();
        self.blink_visible = true;
        // Already-elapsed transition: caret snaps to fully visible right
        // away. A fade-in here would read as input lag, not polish -- the
        // soft fade is for idle blinking, not for the moment right after
        // a keypress.
        self.blink_transition_start = now - BLINK_FADE;
        self.next_blink = now + BLINK_INTERVAL;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 0.0 (hidden) to 1.0 (fully visible), eased across `BLINK_FADE` from
    /// whichever state the caret last toggled into.
    fn caret_alpha(&self) -> f32 {
        let elapsed = Instant::now().duration_since(self.blink_transition_start);
        let t = ease_out_cubic(elapsed.as_secs_f32() / BLINK_FADE.as_secs_f32());
        if self.blink_visible { t } else { 1.0 - t }
    }

    fn handle_key(&mut self, event: &KeyEvent, event_loop: &ActiveEventLoop) {
        if event.state != ElementState::Pressed {
            return;
        }

        if self.modifiers.control_key() {
            if let Key::Character(s) = &event.logical_key {
                let id = if s.eq_ignore_ascii_case("s") {
                    Some("file.save")
                } else if s.eq_ignore_ascii_case("z") && self.modifiers.shift_key() {
                    Some("edit.redo")
                } else if s.eq_ignore_ascii_case("z") {
                    Some("edit.undo")
                } else if s.eq_ignore_ascii_case("y") {
                    Some("edit.redo")
                } else if s.eq_ignore_ascii_case("q") {
                    Some("app.quit")
                } else {
                    None
                };
                if let Some(id) = id {
                    // Built fresh per chord rather than stored on App: it's
                    // just a few fn-pointer entries, and this sidesteps
                    // borrowing `self` both as receiver and argument.
                    CommandRegistry::with_builtins().run(self, event_loop, id);
                    self.wake_caret();
                    return;
                }
            }
            // Not a recognized global chord (e.g. Ctrl-r is Vim's redo):
            // fall through instead of swallowing it.
        }

        let Some(keypress) = keymap::to_keypress(event, self.modifiers) else { return };

        // Window-size-aware paging is a GUI concern, handled the same way
        // regardless of Vim mode -- fenix-vim doesn't know about viewport size.
        if keypress == KeyPress::named(FenixNamedKey::PageUp)
            || keypress == KeyPress::named(FenixNamedKey::PageDown)
        {
            let page_size = self
                .gpu
                .as_ref()
                .map(|gpu| text::visible_line_count(gpu.size.height as f32))
                .unwrap_or(20);
            let down = keypress == KeyPress::named(FenixNamedKey::PageDown);
            self.buffer.move_page(&mut self.cursor, page_size, down);
            self.wake_caret();
            return;
        }

        // Leader sequences span multiple keystrokes: stay routed here while
        // one is already in progress, or start one on SPC from Normal mode
        // (matching orbit-emacs, where SPC is a Normal-mode-only leader --
        // in Insert mode it should just insert a space).
        if self.leader_matcher.is_pending()
            || (self.vim.mode() == Mode::Normal && keypress == KeyPress::char(' '))
        {
            if let Step::Matched(&id) = self.leader_matcher.feed(keypress) {
                CommandRegistry::with_builtins().run(self, event_loop, id);
            }
            self.wake_caret();
            return;
        }

        match self.vim.handle_key(&mut self.buffer, &mut self.cursor, keypress) {
            VimEvent::RequestSave => {
                CommandRegistry::with_builtins().run(self, event_loop, "file.save");
            }
            VimEvent::RequestQuit => {
                CommandRegistry::with_builtins().run(self, event_loop, "app.quit");
            }
            VimEvent::RequestSaveAndQuit => {
                CommandRegistry::with_builtins().run(self, event_loop, "file.save");
                CommandRegistry::with_builtins().run(self, event_loop, "app.quit");
            }
            VimEvent::Pulse(range) => {
                self.pulse = Some(Pulse { range, started: Instant::now() });
            }
            VimEvent::None => {}
        }
        self.wake_caret();
    }

    /// Keeps the cursor's line within the visible window, scrolling as
    /// needed. Must be called with the same `visible_lines` used to render.
    fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        let (line, _) = self.buffer.line_col(&self.cursor);
        let target = scroll_to_include(self.scroll_line, line, visible_lines);
        if target != self.scroll_line {
            let jump = target.abs_diff(self.scroll_line);
            if jump > visible_lines.saturating_mul(SCROLL_SNAP_SCREENS) {
                self.scroll_anim = None;
                self.rendered_scroll = target as f32;
            } else {
                self.scroll_anim = Some(ScrollAnim { from: self.rendered_scroll, to: target, started: Instant::now() });
            }
            self.scroll_line = target;
        }
        self.update_rendered_scroll();
    }

    /// Advances `rendered_scroll` toward `scroll_line` if a transition is
    /// in flight, clearing it once settled.
    fn update_rendered_scroll(&mut self) {
        let Some(anim) = &self.scroll_anim else {
            self.rendered_scroll = self.scroll_line as f32;
            return;
        };
        let t = Instant::now().duration_since(anim.started).as_secs_f32() / SCROLL_DURATION.as_secs_f32();
        if t >= 1.0 {
            self.rendered_scroll = anim.to as f32;
            self.scroll_anim = None;
        } else {
            self.rendered_scroll = anim.from + (anim.to as f32 - anim.from) * ease_out_cubic(t);
        }
    }

    /// The buffer line rendering starts from -- `rendered_scroll` rounded
    /// down. Content, caret, hl-line, selection, and pulse all anchor
    /// their row math to this (not `scroll_line`, which is only the
    /// *target* `rendered_scroll` is easing toward).
    fn render_base_line(&self) -> usize {
        self.rendered_scroll.floor().max(0.0) as usize
    }

    /// Sub-line-height pixel offset (0.0..1.0 of `LINE_HEIGHT`) to shift
    /// every content-area row up by, so a transition between two integer
    /// scroll positions looks like a smooth pan instead of a jump.
    fn render_frac(&self) -> f32 {
        self.rendered_scroll - self.rendered_scroll.floor()
    }

    /// (mode label, rest-of-modeline suffix) -- `None` while typing a `:`
    /// command, since that replaces the whole modeline with raw command
    /// text instead of the usual badge + filename + position layout.
    fn modeline_pieces(&self) -> Option<(&'static str, String)> {
        if self.vim.mode() == Mode::Command {
            return None;
        }
        let filename = self
            .buffer
            .path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "[No Name]".to_string());
        let modified = if self.buffer.is_dirty() { " [+]" } else { "" };
        let (line, col) = self.buffer.line_col(&self.cursor);
        let mode_label =
            if self.vim.mode() == Mode::Visual { self.vim.visual_kind().label() } else { self.vim.mode().label() };
        let suffix = format!("│ {filename}{modified}   Ln {}, Col {} ", line + 1, col + 1);
        Some((mode_label, suffix))
    }

    /// Test-only: the modeline as one plain string, for assertions --
    /// `redraw` renders it as separately-colored spans instead (see
    /// `modeline_pieces`/`mode_colors`), so nothing else needs this.
    #[cfg(test)]
    fn modeline_text(&self) -> String {
        if self.vim.mode() == Mode::Command {
            return format!(":{}", self.vim.command_line());
        }
        let (mode_label, suffix) = self.modeline_pieces().unwrap();
        format!(" {mode_label:^width$}{suffix}", width = text::MODE_BADGE_CHARS)
    }

    /// (badge background, badge text color) for the current mode. Visual's
    /// three kinds all share one accent (matching orbit-emacs's own
    /// evil-state table, which has a single "Visual" entry) -- only the
    /// badge's label text differs between them.
    fn mode_colors(&self) -> ([f32; 4], glyphon::Color) {
        let theme = self.theme;
        match self.vim.mode() {
            Mode::Normal => (theme.mode_normal, theme.mode_text_dark),
            Mode::Insert => (theme.mode_insert, theme.mode_text_dark),
            Mode::Visual => (theme.mode_visual, theme.mode_text_light),
            Mode::Replace => (theme.mode_replace, theme.mode_text_dark),
            Mode::Command => (theme.mode_command, theme.mode_text_dark),
        }
    }

    /// Per-visible-line (view_row, col_start, col_end) segments of the
    /// active Visual-mode selection, for highlighting. Empty outside Visual
    /// mode. Shape depends on `visual_kind()`: Char is a single charwise
    /// span, Line highlights whole lines regardless of column, Block is a
    /// column-range rectangle across lines (clamped per ragged line, like
    /// the Block operators themselves).
    fn visual_selection_segments(&self, visible_lines: usize) -> Segments {
        if self.vim.mode() != Mode::Visual {
            return Vec::new();
        }
        let anchor = self.vim.visual_anchor();
        let last_visible = (self.render_base_line() + visible_lines).min(self.buffer.line_count());
        let mut segments = Vec::new();

        match self.vim.visual_kind() {
            VisualKind::Char => {
                let cursor_idx = self.cursor.char_idx;
                let (lo, hi) =
                    if anchor <= cursor_idx { (anchor, cursor_idx + 1) } else { (cursor_idx, anchor + 1) };
                let hi = hi.min(self.buffer.len_chars());
                segments = self.range_to_segments(lo..hi, visible_lines);
            }
            VisualKind::Line => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                for line in self.render_base_line().max(line_lo)..last_visible.min(line_hi + 1) {
                    // at least 1 col wide so an empty line still shows a sliver
                    let width = self.buffer.line_len(line).max(1);
                    segments.push((line - self.render_base_line(), 0, width));
                }
            }
            VisualKind::Block => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                let anchor_cursor = Cursor { char_idx: anchor, sticky_col: 0 };
                let (_, anchor_col) = self.buffer.line_col(&anchor_cursor);
                let (_, cursor_col) = self.buffer.line_col(&self.cursor);
                let (col_lo, col_hi) = if anchor_col <= cursor_col {
                    (anchor_col, cursor_col + 1)
                } else {
                    (cursor_col, anchor_col + 1)
                };
                for line in self.render_base_line().max(line_lo)..last_visible.min(line_hi + 1) {
                    let len = self.buffer.line_len(line);
                    let seg_start = col_lo.min(len);
                    let seg_end = col_hi.min(len);
                    if seg_start < seg_end {
                        segments.push((line - self.render_base_line(), seg_start, seg_end));
                    }
                }
            }
        }
        segments
    }

    /// (min, max) line index spanned by the visual anchor and the cursor.
    fn anchor_cursor_line_range(&self, anchor: usize) -> (usize, usize) {
        let anchor_cursor = Cursor { char_idx: anchor, sticky_col: 0 };
        let (anchor_line, _) = self.buffer.line_col(&anchor_cursor);
        let (cursor_line, _) = self.buffer.line_col(&self.cursor);
        if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) }
    }

    /// Per-visible-line (view_row, col_start, col_end) segments a plain
    /// char range covers -- shared by Char-kind Visual selection and the
    /// yank/paste pulse, which are both "highlight this contiguous span"
    /// even though they come from different sources.
    fn range_to_segments(&self, range: std::ops::Range<usize>, visible_lines: usize) -> Segments {
        let last_visible = (self.render_base_line() + visible_lines).min(self.buffer.line_count());
        let mut segments = Vec::new();
        for line in self.render_base_line()..last_visible {
            let line_start = self.buffer.line_start_char(line);
            let line_end = line_start + self.buffer.line_len(line);
            let seg_start = range.start.max(line_start);
            let seg_end = range.end.min(line_end);
            if seg_start < seg_end {
                segments.push((line - self.render_base_line(), seg_start - line_start, seg_end - line_start));
            }
        }
        segments
    }

    /// Key/label pairs for whichever pending sequence is currently active
    /// (the leader menu takes priority, since it's the outermost one --
    /// Vim can't be mid-sequence while a leader sequence is in progress).
    /// Empty when nothing is pending.
    fn pending_hints(&self) -> Vec<(KeyPress, &'static str)> {
        if self.leader_matcher.is_pending() {
            self.leader_matcher.pending_children()
        } else {
            self.vim.pending_children()
        }
    }

    /// Segments and current alpha for an active yank/paste pulse, or
    /// `None` when there isn't one. Fades quickly at first then lingers
    /// faintly, via the same ease-out curve as the caret (inverted: pulses
    /// start bright and fade, blink fades in toward its target instead).
    fn pulse_overlay(&self, visible_lines: usize) -> Option<(Segments, f32)> {
        let pulse = self.pulse.as_ref()?;
        let t = Instant::now().duration_since(pulse.started).as_secs_f32() / PULSE_DURATION.as_secs_f32();
        let alpha = PULSE_PEAK_ALPHA * (1.0 - ease_out_cubic(t));
        if alpha <= 0.0 {
            return None;
        }
        Some((self.range_to_segments(pulse.range.clone(), visible_lines), alpha))
    }

    /// Which-key popup text, sorted alphabetically by label for
    /// scannability, one `"key  label"` per line.
    fn which_key_lines(&self) -> Vec<String> {
        let mut hints = self.pending_hints();
        hints.sort_by(|a, b| a.1.cmp(b.1));
        hints.iter().map(|(k, label)| format!("{:<6}{}", keymap::describe_keypress(k), label)).collect()
    }

    fn redraw(&mut self) {
        let Some(window_height) = self.gpu.as_ref().map(|gpu| gpu.size.height as f32) else {
            return;
        };
        let visible_lines = text::visible_line_count(window_height);
        self.ensure_cursor_visible(visible_lines);

        // Fetch one extra line beyond what's strictly visible: mid-scroll,
        // render_frac() shifts everything up by a partial line, so the
        // trailing edge needs one more line of content to reveal.
        let render_base_line = self.render_base_line();
        let render_frac = self.render_frac();
        let content_text = self.buffer.visible_text(render_base_line, visible_lines + 1);
        let modeline_pieces = self.modeline_pieces();
        let modeline_command_text =
            if modeline_pieces.is_none() { Some(format!(":{}", self.vim.command_line())) } else { None };
        let (badge_bg, badge_fg) = self.mode_colors();
        let (line, col) = self.buffer.line_col(&self.cursor);
        // During a large animated pan the cursor's actual line can
        // legitimately be outside the currently-fetched window for part
        // of the transition (it hasn't panned into view yet) -- None
        // means "don't draw the caret/hl-line this frame", not a bug.
        let caret_row_in_view =
            line.checked_sub(render_base_line).filter(|&row| row <= visible_lines);
        let selection_segments = self.visual_selection_segments(visible_lines + 1);
        let pulse_overlay = self.pulse_overlay(visible_lines + 1);
        let which_key_lines = self.which_key_lines();
        let caret_alpha = self.caret_alpha();
        let theme = self.theme;

        let (Some(window), Some(gpu), Some(text), Some(bg_rect), Some(caret_rect)) =
            (&self.window, &mut self.gpu, &mut self.text, &mut self.bg_rect, &mut self.caret_rect)
        else {
            return;
        };

        text.set_text(&content_text);
        match &modeline_pieces {
            Some((mode_label, suffix)) => {
                let badge = format!(" {:^width$}", mode_label, width = text::MODE_BADGE_CHARS);
                text.set_modeline_text(&[(badge.as_str(), badge_fg), (suffix.as_str(), theme.fg_modeline)]);
            }
            None => {
                let cmd = modeline_command_text.as_deref().unwrap_or("");
                text.set_modeline_text(&[(cmd, theme.fg_modeline)]);
            }
        }

        let modeline_top = gpu.size.height as f32 - text::MODELINE_HEIGHT;
        // Top-right corner, clear of both the content the user is actively
        // editing (top-left, where the cursor usually is) and the modeline
        // (bottom) -- least likely to sit under whatever they're looking at.
        let which_key_panel = if which_key_lines.is_empty() {
            None
        } else {
            let panel_height = which_key_lines.len() as f32 * text::LINE_HEIGHT + 8.0;
            text.set_which_key_text(&which_key_lines.join("\n"));
            let left = gpu.size.width as f32 - text::WHICH_KEY_WIDTH - text::WHICH_KEY_MARGIN;
            Some((left, text::WHICH_KEY_MARGIN, panel_height))
        };

        // Every content-row rect shares this: row index (relative to
        // render_base_line) -> pixel y, shifted up by the mid-scroll
        // fractional offset so it pans in step with the text.
        let row_y = |row: usize| text::PAD_TOP + row as f32 * text::LINE_HEIGHT - render_frac * text::LINE_HEIGHT;

        bg_rect.clear();
        if let Some(row) = caret_row_in_view {
            let hl_line_y = row_y(row);
            bg_rect.push_rect(gpu, 0.0, hl_line_y, gpu.size.width as f32, text::LINE_HEIGHT, theme.hl_line);
        }
        bg_rect.push_rect(gpu, 0.0, modeline_top, gpu.size.width as f32, text::MODELINE_HEIGHT, theme.bg_modeline);
        if modeline_pieces.is_some() {
            // Starts at PAD_LEFT, matching where the badge text itself
            // starts rendering (`text.rs`'s modeline TextArea uses the same
            // left inset) -- starting this at the window edge instead left
            // the rendered label overflowing past the badge's right edge,
            // throwing off how centered it looked inside the colored badge.
            let badge_width = (1.0 + text::MODE_BADGE_CHARS as f32) * text::CHAR_WIDTH;
            bg_rect.push_rect(gpu, text::PAD_LEFT, modeline_top, badge_width, text::MODELINE_HEIGHT, badge_bg);
        }
        if let Some((left, top, height)) = which_key_panel {
            bg_rect.push_rect(gpu, left, top, text::WHICH_KEY_WIDTH, height, theme.bg_modeline);
        }
        for (row, col_start, col_end) in selection_segments {
            let x = text::PAD_LEFT + col_start as f32 * text::CHAR_WIDTH;
            let y = row_y(row);
            let w = (col_end - col_start) as f32 * text::CHAR_WIDTH;
            bg_rect.push_rect(gpu, x, y, w, text::LINE_HEIGHT, theme.selection);
        }
        if let Some((segments, alpha)) = pulse_overlay {
            let [r, g, b, _] = theme.caret;
            for (row, col_start, col_end) in segments {
                let x = text::PAD_LEFT + col_start as f32 * text::CHAR_WIDTH;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * text::CHAR_WIDTH;
                bg_rect.push_rect(gpu, x, y, w, text::LINE_HEIGHT, [r, g, b, alpha]);
            }
        }
        bg_rect.flush(gpu);

        caret_rect.clear();
        if let Some(row) = caret_row_in_view {
            if caret_alpha > 0.0 {
                let caret_x = text::PAD_LEFT + col as f32 * text::CHAR_WIDTH;
                let caret_y = row_y(row);
                let [r, g, b, a] = theme.caret;
                caret_rect.push_rect(gpu, caret_x, caret_y, 2.0, text::LINE_HEIGHT, [r, g, b, a * caret_alpha]);
            }
        }
        caret_rect.flush(gpu);

        text.prepare(gpu, theme, render_frac * text::LINE_HEIGHT, which_key_panel);

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            status @ (wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation) => {
                eprintln!("fenix: surface acquire failed: {status:?}");
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let [r, g, b, a] = theme.bg;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: r as f64,
                            g: g as f64,
                            b: b as f64,
                            a: a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            bg_rect.render(&mut pass);
            text.render(&mut pass);
            caret_rect.render(&mut pass);
        }
        gpu.queue.submit(Some(encoder.finish()));
        window.pre_present_notify();
        gpu.queue.present(frame);
        text.trim();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes().with_title("Fenix");
        let window =
            Arc::new(event_loop.create_window(attrs).expect("failed to create window"));

        let gpu = pollster::block_on(GpuState::new(window.clone()));
        let mut text = TextPipeline::new(&gpu);
        text.set_text(&self.buffer.visible_text(0, text::visible_line_count(gpu.size.height as f32)));
        let bg_rect = RectRenderer::new(&gpu);
        let caret_rect = RectRenderer::new(&gpu);

        self.window = Some(window);
        self.gpu = Some(gpu);
        self.text = Some(text);
        self.bg_rect = Some(bg_rect);
        self.caret_rect = Some(caret_rect);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                if let Some(text) = &mut self.text {
                    text.resize(size.width as f32, size.height as f32);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(&event, event_loop);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut needs_redraw = false;

        if now >= self.next_blink {
            self.blink_visible = !self.blink_visible;
            self.blink_transition_start = now;
            self.next_blink = now + BLINK_INTERVAL;
            needs_redraw = true;
        }

        // Keep redrawing at animation cadence only while something is
        // actually transitioning (blink fade or an active pulse); once
        // everything settles, fall back to the long, efficient wait for
        // the next blink toggle -- an idle editor still does no per-frame
        // work between blinks.
        let blink_transitioning = now.duration_since(self.blink_transition_start) < BLINK_FADE;
        let pulse_active = match &self.pulse {
            Some(p) if now.duration_since(p.started) < PULSE_DURATION => true,
            Some(_) => {
                self.pulse = None; // expired -- drop it so redraw stops drawing it
                needs_redraw = true;
                false
            }
            None => false,
        };
        let animating = blink_transitioning || pulse_active || self.scroll_anim.is_some();
        if animating {
            needs_redraw = true;
        }

        if needs_redraw {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        let wait_until = if animating { now + ANIM_TICK } else { self.next_blink };
        event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(wait_until));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_stays_put_when_cursor_already_visible() {
        assert_eq!(scroll_to_include(5, 7, 10), 5);
    }

    #[test]
    fn scroll_jumps_up_when_cursor_is_above_the_window() {
        assert_eq!(scroll_to_include(10, 3, 10), 3);
    }

    #[test]
    fn scroll_advances_when_cursor_is_below_the_window() {
        // window [10, 20); cursor at line 20 is the first line below it
        assert_eq!(scroll_to_include(10, 20, 10), 11);
    }

    #[test]
    fn single_visible_line_tracks_cursor_exactly() {
        assert_eq!(scroll_to_include(0, 42, 1), 42);
    }

    #[test]
    fn app_auto_scrolls_to_keep_cursor_in_view() {
        let mut app = App::with_file(None);
        for _ in 0..30 {
            app.buffer.insert_char(&mut app.cursor, '\n');
        }
        // cursor is now on line 30; a 10-line viewport starting at 0 doesn't include it
        app.ensure_cursor_visible(10);
        assert_eq!(app.scroll_line, 21);
    }

    #[test]
    fn small_scroll_change_starts_an_animation_not_an_instant_jump() {
        let mut app = App::with_file(None);
        for _ in 0..5 {
            app.buffer.insert_char(&mut app.cursor, '\n');
        }
        app.ensure_cursor_visible(3); // 6 lines, 3-line viewport -> scrolls a bit
        assert!(app.scroll_anim.is_some());
        assert_ne!(app.rendered_scroll, app.scroll_line as f32); // still mid-ease, not snapped
    }

    #[test]
    fn huge_scroll_jump_snaps_instantly_without_animating() {
        let mut app = App::with_file(None);
        for _ in 0..500 {
            app.buffer.insert_char(&mut app.cursor, '\n');
        }
        app.ensure_cursor_visible(10); // jump of ~490 lines, way past the snap threshold
        assert!(app.scroll_anim.is_none());
        assert_eq!(app.rendered_scroll, app.scroll_line as f32);
    }

    #[test]
    fn rendered_scroll_eases_toward_target_and_settles() {
        let mut app = App::with_file(None);
        app.scroll_line = 10;
        app.rendered_scroll = 0.0;
        app.scroll_anim = Some(ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION / 2 });
        app.update_rendered_scroll();
        assert!(app.rendered_scroll > 0.0 && app.rendered_scroll < 10.0, "should be partway there");
        assert!(app.scroll_anim.is_some());

        app.scroll_anim = Some(ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION * 2 });
        app.update_rendered_scroll();
        assert_eq!(app.rendered_scroll, 10.0);
        assert!(app.scroll_anim.is_none()); // settled, animation cleared
    }

    #[test]
    fn render_base_line_and_frac_split_a_fractional_scroll_position() {
        let mut app = App::with_file(None);
        app.rendered_scroll = 4.25;
        assert_eq!(app.render_base_line(), 4);
        assert!((app.render_frac() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn modeline_reflects_filename_dirty_state_mode_and_position() {
        let mut app = App::with_file(None);
        assert_eq!(app.modeline_text(), "  NORMAL │ [No Name]   Ln 1, Col 1 ");

        app.buffer.insert_char(&mut app.cursor, 'a');
        app.buffer.insert_char(&mut app.cursor, 'b');
        assert_eq!(app.modeline_text(), "  NORMAL │ [No Name] [+]   Ln 1, Col 3 ");
    }

    #[test]
    fn modeline_shows_command_line_while_typing_an_ex_command() {
        let mut app = App::with_file(None);
        for ch in [':', 'w', 'q'] {
            app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char(ch));
        }
        assert_eq!(app.modeline_text(), ":wq");
    }

    #[test]
    fn visual_selection_segments_cover_the_selected_range() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.cursor = Cursor::at_start();
        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('v'));
        for _ in 0..4 {
            app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('l'));
        }
        // anchor 0, cursor now at 4 ("hello"[0..5))
        assert_eq!(app.visual_selection_segments(10), vec![(0, 0, 5)]);
    }

    #[test]
    fn visual_selection_segments_empty_outside_visual_mode() {
        let app = App::with_file(None);
        assert!(app.visual_selection_segments(10).is_empty());
    }

    #[test]
    fn modeline_shows_visual_kind_not_just_visual() {
        let mut app = App::with_file(None);
        for ch in "one\ntwo\nthree".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.cursor = Cursor::at_start();

        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('v'));
        assert!(app.modeline_text().contains("VISUAL"));

        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('V'));
        assert!(app.modeline_text().contains("V-LINE"));

        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('v').with_ctrl());
        assert!(app.modeline_text().contains("V-BLOCK"));
    }

    #[test]
    fn mode_colors_differ_per_mode_and_visual_kinds_share_one_accent() {
        let mut app = App::with_file(None);
        let (normal_bg, _) = app.mode_colors();

        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('i'));
        let (insert_bg, _) = app.mode_colors();
        assert_ne!(normal_bg, insert_bg);
        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::named(FenixNamedKey::Escape));

        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('v'));
        let (char_visual_bg, _) = app.mode_colors();
        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('V'));
        let (line_visual_bg, _) = app.mode_colors();
        assert_eq!(char_visual_bg, line_visual_bg); // one accent for all Visual kinds
        assert_ne!(char_visual_bg, normal_bg);
    }

    #[test]
    fn visual_line_segments_cover_whole_lines_regardless_of_column() {
        let mut app = App::with_file(None);
        for ch in "one\ntwo\nthree".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.cursor = Cursor { char_idx: 5, sticky_col: 1 }; // column 1 of "two"
        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('V'));
        assert_eq!(app.visual_selection_segments(10), vec![(1, 0, 3)]);
    }

    #[test]
    fn visual_block_segments_form_a_column_rectangle() {
        let mut app = App::with_file(None);
        for ch in "abc\ndef\nghi".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.cursor = Cursor::at_start();
        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('v').with_ctrl());
        for ch in ['j', 'j', 'l'] {
            app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char(ch));
        }
        assert_eq!(app.visual_selection_segments(10), vec![(0, 0, 2), (1, 0, 2), (2, 0, 2)]);
    }

    #[test]
    fn ease_out_cubic_starts_at_zero_ends_at_one_and_is_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert_eq!(ease_out_cubic(2.0), 1.0); // clamps past the end
        assert_eq!(ease_out_cubic(-1.0), 0.0); // clamps before the start

        let mut prev = 0.0;
        for i in 1..=10 {
            let v = ease_out_cubic(i as f32 / 10.0);
            assert!(v >= prev, "not monotonic at step {i}");
            prev = v;
        }
    }

    #[test]
    fn ease_out_cubic_front_loads_motion_more_than_linear() {
        // "ease-out": faster at the start than a linear ramp would be
        assert!(ease_out_cubic(0.25) > 0.25);
    }

    #[test]
    fn fresh_app_caret_is_immediately_fully_visible_no_fade_in_delay() {
        let app = App::with_file(None);
        assert_eq!(app.caret_alpha(), 1.0);
    }

    #[test]
    fn wake_caret_snaps_to_visible_without_fading_in() {
        let mut app = App::with_file(None);
        app.blink_visible = false;
        app.blink_transition_start = Instant::now(); // mid fade-out
        app.wake_caret();
        assert_eq!(app.caret_alpha(), 1.0);
    }

    #[test]
    fn no_pulse_by_default() {
        let app = App::with_file(None);
        assert!(app.pulse_overlay(10).is_none());
    }

    #[test]
    fn fresh_pulse_is_near_peak_alpha_and_covers_its_range() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.pulse = Some(Pulse { range: 0..5, started: Instant::now() });
        let (segments, alpha) = app.pulse_overlay(10).expect("pulse should be active");
        assert_eq!(segments, vec![(0, 0, 5)]);
        assert!(alpha > 0.0 && alpha <= PULSE_PEAK_ALPHA);
    }

    #[test]
    fn expired_pulse_produces_no_overlay() {
        let mut app = App::with_file(None);
        for ch in "hello".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.pulse = Some(Pulse { range: 0..5, started: Instant::now() - PULSE_DURATION * 2 });
        assert!(app.pulse_overlay(10).is_none());
    }

    // App::handle_key itself needs a real winit KeyEvent/ActiveEventLoop,
    // which aren't constructible in a unit test (same constraint as the
    // rest of the winit-facing integration, verified by code review
    // instead -- see the plan). This exercises the layer below it: that a
    // VimEvent::Pulse from VimState turns into a renderable pulse_overlay,
    // which is the actual logic worth covering here; App's own arm that
    // wires the two together is two lines and reviewed by eye.
    #[test]
    fn vim_pulse_event_yields_a_renderable_pulse_overlay() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.buffer.insert_char(&mut app.cursor, ch);
        }
        app.cursor = Cursor::at_start();
        assert!(app.pulse.is_none());

        app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('y'));
        let event = app.vim.handle_key(&mut app.buffer, &mut app.cursor, KeyPress::char('w'));
        let fenix_vim::VimEvent::Pulse(range) = event else { panic!("expected a Pulse event from yw") };
        app.pulse = Some(Pulse { range, started: Instant::now() });
        assert!(app.pulse_overlay(10).is_some());
    }
}
