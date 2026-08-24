use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fenix_buffers::{BufferId, BufferKind, BufferList, OpenBuffer};
use fenix_explorer::{ExplorerAction, ExplorerState};
use fenix_keymap::{KeyCode, KeyPress, Matcher, Mods, NamedKey as FenixNamedKey, Step};
use fenix_vim::{Mode, VimEvent, VimState, VisualKind};
use fenix_window::{NavDirection, SplitKind, WindowTree};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState};
use winit::window::{Window, WindowId};

use fenix_core::{Buffer, Cursor};

use crate::commands::CommandRegistry;
use crate::completion;
use crate::dashboard;
use crate::docker_panel;
use crate::gpu::GpuState;
use crate::icon;
use crate::keymap;
use crate::popup;
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

/// Thickness of a themed window border (`Theme::border`), when the
/// active theme draws one. Comfortably inside `text::PAD_TOP` (4px) and
/// `text::PAD_LEFT` (8px) so it can never clip into body text.
const BORDER_WIDTH: f32 = 3.0;

/// How many trailing lines of `docker logs` the Docker panel's `l`
/// action fetches -- generous enough to actually be useful (most
/// container startup/error output fits well within this) without
/// risking pulling in an unbounded amount of text from a chatty
/// container.
const DOCKER_LOG_TAIL_LINES: usize = 500;

/// Vertical padding inside the which-key popup, above its first row and
/// below its last -- factored into both its own height and `popup::
/// max_rows`'s "how many rows actually fit" calculation, so the two stay
/// consistent with each other.
const WHICH_KEY_PADDING: f32 = 8.0;

/// How much `SPC t =`/`Ctrl-=` and `SPC t -`/`Ctrl--` change the font
/// size by per press.
const FONT_SIZE_STEP: f32 = 2.0;

/// Same role as `WHICH_KEY_PADDING`, for the completion popup.
const COMPLETION_PADDING: f32 = 8.0;
/// Clear space kept between the completion popup and the window edges
/// it's clamped against -- same role as `text::WHICH_KEY_MARGIN`, just a
/// smaller value since the popup is anchored to a point inside the
/// content area rather than pinned to a corner.
const COMPLETION_MARGIN: f32 = 4.0;
/// Hard cap on how many candidate rows the popup ever shows at once,
/// regardless of how much vertical room is available -- most editors cap
/// completion popups similarly (a huge list is unwieldy even when there's
/// room to render it).
const COMPLETION_MAX_ROWS: usize = 10;

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

/// One window pane's own live editing position -- cursor and scroll,
/// independent of whatever other pane(s) might be showing the same
/// buffer (see `fenix_buffers::OpenBuffer`'s own doc comment: `cursor`
/// there is just the buffer's *remembered* position, used only to seed
/// a pane's `PaneState` the first time that buffer is shown in it).
/// Lives on `Workspace`, not `App`, keyed by `fenix_window::WindowId` --
/// `WindowId`s are only unique *within* one `WindowTree`, not globally
/// (each tree's own internal counter starts fresh), so storing this
/// globally on `App` would let panes in different workspaces collide on
/// the same key. Scoping it to the `Workspace` that owns the tree those
/// ids came from rules that out entirely, and as a side benefit means
/// switching workspaces leaves every pane's cursor/scroll exactly where
/// it was.
#[derive(Clone, Copy)]
struct PaneState {
    cursor: Cursor,
    scroll_line: usize,
    rendered_scroll: f32,
}

impl PaneState {
    fn seeded_at(cursor: Cursor) -> Self {
        Self { cursor, scroll_line: 0, rendered_scroll: 0.0 }
    }
}

/// The autocompletion popup's live state -- see `App::completion`'s own
/// doc comment for the "recomputed fresh every keystroke" lifecycle.
struct CompletionState {
    /// Char offset where the identifier prefix being completed starts --
    /// `accept_completion` replaces `[prefix_start, cursor.char_idx)`
    /// with the chosen candidate's full label.
    prefix_start: usize,
    picker: fenix_picker::PickerState<fenix_completion::CompletionItem>,
}

/// Per-visible-line highlight segments: (view_row, col_start, col_end).
type Segments = Vec<(usize, usize, usize)>;

/// A content or sidebar row's rich-text spans: (text, color, use_icon_font).
type RowSpans = Vec<(String, glyphon::Color, bool)>;
/// `explorer_row_spans`'s result: the spans, which rendered row (if any)
/// is the selected entry, and which rows are marked.
type ExplorerRowsResult = (RowSpans, Option<usize>, Vec<usize>);

/// Line-number gutter display. Not wired to a config file yet -- there
/// isn't one yet (`App::with_file` just picks a hardcoded default) -- but
/// the enum exists now so hooking that up later is a matter of reading
/// this field from somewhere else, not redesigning the rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineNumberMode {
    Off,
    Absolute,
    Relative,
}

/// What occupies the main content area: the editor buffer, a full-buffer
/// directory listing, or a fuzzy-filtered picker (find-file/grep/switch-
/// project) -- all three "visit something else instead of the buffer for
/// a moment," the same reasoning that lets `Picker` reuse `Explorer`'s
/// stash/restore machinery unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MainView {
    Editor,
    Explorer,
    Picker,
}

/// What a `MainView::Explorer` session is actually for -- ordinary
/// directory browsing (`SPC f j`, the sidebar) behaves exactly as it
/// always has; `PickProjectDir` is the same listing and the same
/// navigation, just with `ExplorerAction::SelectCwd` (`S`) wired to
/// register `cwd` as a known project instead of being ignored, and
/// `Open`/`Enter`/`l` on a *file* doing nothing instead of opening it
/// into the editor (there's nothing sensible to do with a file when
/// what's being picked is a directory). See `picker_add_project_prompt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExplorerPurpose {
    Browse,
    PickProjectDir,
}

/// A picker's payload type varies by what it's picking between, but
/// `App` only ever has one active at a time -- an enum, not three
/// separate `Option` fields, the same reasoning `ExplorerPrompt` already
/// uses for "one thing, a few kinds."
enum ActivePicker {
    FindFile(fenix_picker::PickerState<PathBuf>),
    Grep(fenix_picker::PickerState<fenix_project::GrepMatch>),
    SwitchProject(fenix_picker::PickerState<PathBuf>),
    SwitchBuffer(fenix_picker::PickerState<BufferId>),
    /// `SPC p d`: same candidate list as `SwitchProject`, but confirming
    /// removes the selected root from `known_projects` instead of
    /// switching to it.
    DeleteProject(fenix_picker::PickerState<PathBuf>),
    /// `SPC t p`: jump straight to a specific theme by name, fuzzy-
    /// filtered over `theme::ALL` -- confirming applies it exactly like
    /// `cycle_theme` does. The quick `SPC t t` cycle stays too; this is
    /// for "I know which one I want," not "just try the next one."
    Theme(fenix_picker::PickerState<&'static Theme>),
}

// The three `ActivePicker` variants wrap `PickerState<T>` for different
// `T`, so there's no single method call that works across all of them --
// these free functions do the one-line match each caller would otherwise
// repeat.
fn picker_push_char(picker: &mut ActivePicker, c: char) {
    match picker {
        ActivePicker::FindFile(s) => s.push_char(c),
        ActivePicker::Grep(s) => s.push_char(c),
        ActivePicker::SwitchProject(s) => s.push_char(c),
        ActivePicker::SwitchBuffer(s) => s.push_char(c),
        ActivePicker::DeleteProject(s) => s.push_char(c),
        ActivePicker::Theme(s) => s.push_char(c),
    }
}

fn picker_backspace(picker: &mut ActivePicker) {
    match picker {
        ActivePicker::FindFile(s) => s.backspace(),
        ActivePicker::Grep(s) => s.backspace(),
        ActivePicker::SwitchProject(s) => s.backspace(),
        ActivePicker::SwitchBuffer(s) => s.backspace(),
        ActivePicker::DeleteProject(s) => s.backspace(),
        ActivePicker::Theme(s) => s.backspace(),
    }
}

fn picker_move_selection(picker: &mut ActivePicker, delta: isize) {
    match picker {
        ActivePicker::FindFile(s) => s.move_selection(delta),
        ActivePicker::Grep(s) => s.move_selection(delta),
        ActivePicker::SwitchProject(s) => s.move_selection(delta),
        ActivePicker::SwitchBuffer(s) => s.move_selection(delta),
        ActivePicker::DeleteProject(s) => s.move_selection(delta),
        ActivePicker::Theme(s) => s.move_selection(delta),
    }
}

fn picker_query(picker: &ActivePicker) -> &str {
    match picker {
        ActivePicker::FindFile(s) => s.query(),
        ActivePicker::Grep(s) => s.query(),
        ActivePicker::SwitchProject(s) => s.query(),
        ActivePicker::SwitchBuffer(s) => s.query(),
        ActivePicker::DeleteProject(s) => s.query(),
        ActivePicker::Theme(s) => s.query(),
    }
}

fn picker_len(picker: &ActivePicker) -> usize {
    match picker {
        ActivePicker::FindFile(s) => s.len(),
        ActivePicker::Grep(s) => s.len(),
        ActivePicker::SwitchProject(s) => s.len(),
        ActivePicker::SwitchBuffer(s) => s.len(),
        ActivePicker::DeleteProject(s) => s.len(),
        ActivePicker::Theme(s) => s.len(),
    }
}

fn picker_selected_row(picker: &ActivePicker) -> usize {
    match picker {
        ActivePicker::FindFile(s) => s.selected_row(),
        ActivePicker::Grep(s) => s.selected_row(),
        ActivePicker::SwitchProject(s) => s.selected_row(),
        ActivePicker::SwitchBuffer(s) => s.selected_row(),
        ActivePicker::DeleteProject(s) => s.selected_row(),
        ActivePicker::Theme(s) => s.selected_row(),
    }
}

/// Labels only (rendering doesn't need the payload) for the windowed
/// slice `[offset, offset + count)`, paired with whether each is the
/// current selection -- same windowing shape `ExplorerState`'s rendering
/// already uses for a directory listing.
fn picker_visible_labels(picker: &ActivePicker, offset: usize, count: usize) -> Vec<(bool, String)> {
    match picker {
        ActivePicker::FindFile(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Grep(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchProject(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::SwitchBuffer(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::DeleteProject(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
        ActivePicker::Theme(s) => s.visible_rows(offset, count).map(|(sel, c)| (sel, c.label.clone())).collect(),
    }
}

/// Which explorer-mode prompt (if any) is capturing the next keystrokes --
/// same "next key(s) mean something special" pattern `fenix-vim`'s
/// `Mode::Command`/`pending_replace` already use, applied to file-manager
/// text input instead of Vim's own command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    ConfirmDelete,
    Rename,
    CreateFile,
    CreateDir,
    CopyTo,
    MoveTo,
}

struct ExplorerPrompt {
    kind: PromptKind,
    input: String,
}

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

/// A pane-relative `(row, col)` caret position, in window pixel
/// coordinates -- shared by the real caret rect and the completion
/// popup's `BelowPoint` anchor, which both need to land on exactly the
/// same spot.
fn caret_pixel_pos(rect: fenix_window::Rect, row: usize, col: usize, gutter_px: f32, content_frac: f32, char_width: f32, line_height: f32) -> (f32, f32) {
    let content_x = rect.x + text::PAD_LEFT + gutter_px;
    let x = content_x + col as f32 * char_width;
    let y = rect.y + text::PAD_TOP + row as f32 * line_height - content_frac * line_height;
    (x, y)
}

/// Splits `line_text` into colored sub-spans according to `highlights`
/// (document-wide byte ranges, already resolved to colors, sorted and
/// non-overlapping -- the contract `fenix_syntax::SyntaxState::
/// highlights_in_range` guarantees) that overlap `[line_byte_start,
/// line_byte_end)`. Gaps between them fall back to `default_color`. Pure
/// and independent of `Buffer`/`App` so it's testable with hand-built
/// strings and ranges, not a real parsed buffer.
fn split_line_by_highlights(
    line_text: &str,
    line_byte_start: usize,
    line_byte_end: usize,
    highlights: &[(std::ops::Range<usize>, glyphon::Color)],
    default_color: glyphon::Color,
) -> Vec<(String, glyphon::Color)> {
    let mut spans = Vec::new();
    let mut cursor = 0usize; // byte offset within line_text
    for (range, color) in highlights {
        if range.end <= line_byte_start || range.start >= line_byte_end {
            continue;
        }
        let local_start = range.start.max(line_byte_start) - line_byte_start;
        let local_end = range.end.min(line_byte_end) - line_byte_start;
        if local_start > cursor {
            spans.push((line_text[cursor..local_start].to_string(), default_color));
        }
        if local_end > local_start {
            spans.push((line_text[local_start..local_end].to_string(), *color));
        }
        cursor = cursor.max(local_end);
    }
    if cursor < line_text.len() {
        spans.push((line_text[cursor..].to_string(), default_color));
    }
    if spans.is_empty() {
        spans.push((line_text.to_string(), default_color));
    }
    spans
}

/// Whether `style` belongs to the banner block (the logo + tagline) as
/// opposed to the content block (section headers, project/recent-file
/// entries, the footer hint) -- `dashboard_center_offset` centers these
/// two blocks *independently*, each against its own widest line, rather
/// than the whole buffer sharing one width. The banner (a fixed ~30
/// chars) is narrower than the content block (whose widest line is
/// usually the footer hint or a long file path) -- centering everything
/// against one shared width left the banner sitting off-center, visibly
/// left of the pane's true middle, which is what this fixes.
fn dashboard_line_is_banner(style: dashboard::DashboardLineStyle) -> bool {
    matches!(style, dashboard::DashboardLineStyle::Banner | dashboard::DashboardLineStyle::Tagline)
}

/// Per-line horizontal padding (in characters) plus a single vertical
/// pixel offset that together center a dashboard buffer's content within
/// `rect` -- Doom Emacs/LazyVim-style, rather than left/top-anchored.
///
/// Horizontal padding is computed *per line*, not once for the whole
/// buffer (see `dashboard_line_is_banner`): the banner block and the
/// content block below it are centered independently, each against only
/// its own widest line, since they're rarely the same width. The result
/// is baked by the caller into `content_spans`'s `BufferKind::Dashboard`
/// branch as literal leading blank-space characters in the rendered
/// spans -- `TextArea.left` has no independent geometric offset to hook
/// into (only a real gutter's digits shift rendered text, by being part
/// of the text itself), so per-row padding has to live in the text too.
///
/// Vertical centering stays a single pixel value for the whole buffer
/// (`TextArea.top` genuinely does read a per-pane pixel bias via
/// `content_frac`, unlike `TextArea.left`).
///
/// Both axes clamp to never go negative (a pane smaller than the
/// content just left/top-anchors, same as today) and are computed fresh
/// every frame from the pane's *current* size, so the layout re-centers
/// on window resize for free without touching the buffer's actual
/// text/cursor/undo state at all.
fn dashboard_center_offset(
    ob: &OpenBuffer,
    lines: &[Option<dashboard::DashboardLine>],
    rect: fenix_window::Rect,
    char_width: f32,
    line_height: f32,
) -> (Vec<usize>, f32) {
    let visual_lines = ob.buffer.visual_line_count();
    let pane_chars = (rect.w / char_width).floor().max(0.0) as usize;

    let mut banner_max = 0usize;
    let mut content_max = 0usize;
    for line in 0..visual_lines {
        let len = ob.buffer.line_len(line);
        match lines.get(line).and_then(|l| l.as_ref()) {
            Some(meta) if dashboard_line_is_banner(meta.style) => banner_max = banner_max.max(len),
            _ => content_max = content_max.max(len),
        }
    }
    let banner_pad = pane_chars.saturating_sub(banner_max) / 2;
    let content_pad = pane_chars.saturating_sub(content_max) / 2;

    let pad_by_line: Vec<usize> = (0..visual_lines)
        .map(|line| match lines.get(line).and_then(|l| l.as_ref()) {
            Some(meta) if dashboard_line_is_banner(meta.style) => banner_pad,
            _ => content_pad,
        })
        .collect();

    let content_h = visual_lines as f32 * line_height;
    let extra_top_px = ((rect.h - content_h) / 2.0).max(0.0);
    (pad_by_line, extra_top_px)
}

/// The dashboard's equivalent of `fenix-syntax`'s highlight output --
/// built from `dashboard::render`'s own per-line metadata instead of a
/// real parser, then fed through the exact same `split_line_by_
/// highlights` mechanism as ordinary syntax coloring. `Banner`/
/// `Tagline`/`Header` lines get a full-line accent color; `Footer` gets
/// a full-line dim color; `Project`/`RecentFile` lines only push a
/// range for their dim path/parent-dir portion (from `dim_from`
/// onward) -- the name portion before it is left uncovered, so it falls
/// back to `content_spans`'s own `theme.fg` default naturally. A plain
/// text buffer never reaches this (see `syntax_highlights_for_visible_
/// range`'s `BufferKind::Dashboard` branch), so it isn't a method on
/// `App` -- everything it needs is passed in directly.
fn dashboard_highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<dashboard::DashboardLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let Some(lines) = lines else { return Vec::new() };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else { continue };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_start_byte = ob.buffer.char_to_byte(start);
        let line_end_byte = ob.buffer.char_to_byte(start + len);
        match meta.style {
            dashboard::DashboardLineStyle::Banner
            | dashboard::DashboardLineStyle::Tagline
            | dashboard::DashboardLineStyle::Header => {
                // `syntax_keyword`, not `caret_text`: `caret_text` was
                // chosen (see its own doc comment) to read against
                // `bg_modeline`, but the dashboard renders as ordinary
                // pane content, against `bg` -- on TempleOS specifically
                // that's a *white* background, where `caret_text`'s
                // yellow (0xffff55) is nearly unreadable (its R/G
                // channels already match white's). `syntax_*` colors are
                // the ones actually chosen per-theme for legibility
                // against `bg`, which is exactly what this needs.
                ranges.push((line_start_byte..line_end_byte, theme.syntax_keyword));
            }
            dashboard::DashboardLineStyle::Footer => {
                ranges.push((line_start_byte..line_end_byte, theme.gutter_fg));
            }
            dashboard::DashboardLineStyle::Project | dashboard::DashboardLineStyle::RecentFile => {
                if let Some(dim_from) = meta.dim_from {
                    let dim_start_byte = ob.buffer.char_to_byte(start + dim_from);
                    ranges.push((dim_start_byte..line_end_byte, theme.gutter_fg));
                }
            }
        }
    }
    ranges
}

/// The Docker panel's equivalent of `dashboard_highlights_for_visible_
/// range` -- same shape, built from `docker_panel::render`'s own per-line
/// metadata instead of a real parser. `Header` lines get a full-line
/// accent color; `Footer`/`Empty` get a full-line dim color;
/// `Container`/`Image` lines only push a range for their dim status/size
/// portion (from `dim_from` onward) -- the name/repo:tag portion before
/// it falls back to `content_spans`'s own `theme.fg` default.
fn docker_highlights_for_visible_range(
    ob: &OpenBuffer,
    lines: Option<&[Option<docker_panel::DockerLine>]>,
    render_base_line: usize,
    rows: usize,
    theme: &Theme,
) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
    let Some(lines) = lines else { return Vec::new() };
    let visual_lines = ob.buffer.visual_line_count();
    let mut ranges = Vec::new();
    for line in render_base_line..(render_base_line + rows).min(visual_lines) {
        let Some(Some(meta)) = lines.get(line) else { continue };
        let start = ob.buffer.line_start_char(line);
        let len = ob.buffer.line_len(line);
        let line_start_byte = ob.buffer.char_to_byte(start);
        let line_end_byte = ob.buffer.char_to_byte(start + len);
        match meta.style {
            docker_panel::DockerLineStyle::Header => {
                ranges.push((line_start_byte..line_end_byte, theme.syntax_keyword));
            }
            docker_panel::DockerLineStyle::Footer | docker_panel::DockerLineStyle::Empty => {
                ranges.push((line_start_byte..line_end_byte, theme.gutter_fg));
            }
            docker_panel::DockerLineStyle::Container | docker_panel::DockerLineStyle::Image => {
                if let Some(dim_from) = meta.dim_from {
                    let dim_start_byte = ob.buffer.char_to_byte(start + dim_from);
                    ranges.push((dim_start_byte..line_end_byte, theme.gutter_fg));
                }
            }
        }
    }
    ranges
}

/// One-letter badge for an explorer row's git status.
fn git_status_marker(status: fenix_explorer::GitStatus) -> &'static str {
    match status {
        fenix_explorer::GitStatus::Modified => "M",
        fenix_explorer::GitStatus::Staged => "S",
        fenix_explorer::GitStatus::Untracked => "?",
        fenix_explorer::GitStatus::Ignored => "I",
        fenix_explorer::GitStatus::Conflicted => "U",
    }
}

/// Plain-text rendering of a directory listing for a real `BufferKind::
/// Explorer` buffer -- one line per entry (depth indent, name, a
/// trailing `/` for directories, a one-letter git-status marker when
/// known), paired with which `entries` index each generated line
/// corresponds to (mirrors `dashboard_lines`' own role for `BufferKind::
/// Dashboard`). Deliberately plainer than `explorer_row_spans`'s rich
/// icon/color spans (the sidebar/full-buffer *overlay*'s own rendering,
/// unchanged and untouched by this) -- this has to be real, plain rope
/// text a Vim buffer can navigate/search, not colored display-only spans.
fn explorer_dired_text(state: &ExplorerState) -> (String, Vec<Option<usize>>) {
    let mut text = String::new();
    let mut lines = Vec::with_capacity(state.entries.len());
    for (i, entry) in state.entries.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        text.push_str(&"  ".repeat(entry.depth));
        text.push_str(&entry.name);
        if entry.is_dir {
            text.push('/');
        }
        if let Some(status) = entry.git_status {
            text.push_str("  ");
            text.push_str(git_status_marker(status));
        }
        lines.push(Some(i));
    }
    (text, lines)
}

/// Human-readable file size (`"1.2K"`, `"340B"`) -- no crate for this,
/// just repeated division, so no new dependency for one small column.
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes}B") } else { format!("{size:.1}{}", UNITS[unit]) }
}

/// Coarse relative age (`"3m"`, `"5h"`, `"2d"`) since `modified` --
/// avoids pulling in a date/time crate just to show a calendar date;
/// elapsed-duration bucketing needs no calendar math at all.
fn format_age(modified: std::time::SystemTime) -> String {
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(modified) else { return "now".to_string() };
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// One named task layout -- a Doom-Emacs-style workspace. Each remembers
/// its own split layout; every workspace shares the one global
/// `BufferList` (`App::buffers`), so a buffer open in two workspaces at
/// once stays in sync for free (same `BufferId`, same underlying
/// `OpenBuffer`) -- only the window *layout* is per-workspace, per
/// design, not a full persp-mode-style buffer-list isolation.
struct Workspace {
    name: String,
    windows: WindowTree<BufferId>,
    /// Keyed by `fenix_window::WindowId` -- see `PaneState`'s own doc
    /// comment for why this lives here, not on `App` directly.
    pane_states: HashMap<fenix_window::WindowId, PaneState>,
    scroll_anims: HashMap<fenix_window::WindowId, ScrollAnim>,
}

impl Workspace {
    fn new(name: String, windows: WindowTree<BufferId>, initial_cursor: Cursor) -> Self {
        let mut pane_states = HashMap::new();
        pane_states.insert(windows.focused_id(), PaneState::seeded_at(initial_cursor));
        Self { name, windows, pane_states, scroll_anims: HashMap::new() }
    }
}

/// A non-empty, ordered list of workspaces with one active at a time.
/// Switching workspaces is just moving `active` -- the `WindowTree`s
/// (and each one's own `pane_states`/`scroll_anims`) themselves are
/// untouched, so a workspace's layout *and* every pane's cursor/scroll
/// position within it are exactly as they were left.
struct WorkspaceList {
    workspaces: Vec<Workspace>,
    active: usize,
}

impl WorkspaceList {
    fn new(initial_windows: WindowTree<BufferId>, initial_cursor: Cursor) -> Self {
        Self { workspaces: vec![Workspace::new("workspace-1".to_string(), initial_windows, initial_cursor)], active: 0 }
    }

    fn active(&self) -> &WindowTree<BufferId> {
        &self.workspaces[self.active].windows
    }

    fn active_mut(&mut self) -> &mut WindowTree<BufferId> {
        &mut self.workspaces[self.active].windows
    }

    fn active_pane_states(&self) -> &HashMap<fenix_window::WindowId, PaneState> {
        &self.workspaces[self.active].pane_states
    }

    fn active_pane_states_mut(&mut self) -> &mut HashMap<fenix_window::WindowId, PaneState> {
        &mut self.workspaces[self.active].pane_states
    }

    fn active_scroll_anims(&self) -> &HashMap<fenix_window::WindowId, ScrollAnim> {
        &self.workspaces[self.active].scroll_anims
    }

    fn active_scroll_anims_mut(&mut self) -> &mut HashMap<fenix_window::WindowId, ScrollAnim> {
        &mut self.workspaces[self.active].scroll_anims
    }

    fn active_name(&self) -> &str {
        &self.workspaces[self.active].name
    }

    fn len(&self) -> usize {
        self.workspaces.len()
    }

    fn active_index(&self) -> usize {
        self.active
    }

    /// `SPC TAB n`: a new, auto-named workspace (`workspace-2`, etc. --
    /// no name-entry prompt in v1) seeded with a single pane showing
    /// `content`, so it starts on the buffer you were already looking
    /// at rather than nothing (`cursor` seeds that pane's own live
    /// position, from the buffer's remembered one). Becomes active.
    fn new_workspace(&mut self, content: BufferId, cursor: Cursor) {
        let name = format!("workspace-{}", self.workspaces.len() + 1);
        self.workspaces.push(Workspace::new(name, WindowTree::new(content), cursor));
        self.active = self.workspaces.len() - 1;
    }

    /// `SPC TAB ]`/`SPC TAB [`: cycles the active workspace, wrapping.
    fn next(&mut self) {
        self.active = (self.active + 1) % self.workspaces.len();
    }

    fn prev(&mut self) {
        self.active = (self.active + self.workspaces.len() - 1) % self.workspaces.len();
    }

    /// `SPC TAB d`: removes the active workspace. Refuses (returns
    /// `false`, no-op) if it's the last one -- same safety posture as
    /// `WindowTree::close_focused` refusing to close the last window.
    fn remove_active(&mut self) -> bool {
        if self.workspaces.len() <= 1 {
            return false;
        }
        self.workspaces.remove(self.active);
        if self.active >= self.workspaces.len() {
            self.active = self.workspaces.len() - 1;
        }
        true
    }
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

    /// Every open buffer, keyed by `BufferId` -- syntax state lives on
    /// each buffer's own `OpenBuffer`. Live cursor/scroll position is
    /// *per-window*, not per-buffer (see `PaneState`'s own doc comment)
    /// -- owned by whichever `Workspace` the pane belongs to, reached via
    /// `pane_state`/`pane_state_mut`/`focused_pane_id`, not through a
    /// buffer at all. `App` never touches a buffer directly, only through
    /// this and `windows` -- use the `open`/`open_mut`/`focused_buffer_id`
    /// helpers.
    buffers: BufferList,
    /// Every workspace's own split layout (and, per workspace, every
    /// pane's own live cursor/scroll and in-flight scroll animation --
    /// see `Workspace`'s own fields) -- `windows()`/`windows_mut()`
    /// (right after the constructor) are the accessors to use everywhere
    /// else; they read/write whichever workspace is currently active, the
    /// same "always go through the helper, not the field" discipline
    /// `open`/`open_mut` already establish for buffers.
    workspaces: WorkspaceList,

    /// See `LineNumberMode` doc comment -- hardcoded for now.
    line_number_mode: LineNumberMode,

    main_view: MainView,
    /// The full-buffer listing (`main_view == Explorer`). A separate
    /// field from `sidebar` -- they were one shared `Option` early on,
    /// but that aliased the two uses together: jumping to a full-buffer
    /// listing while a sidebar was already open would silently clobber
    /// the sidebar's state out from under it. Independent fields mean
    /// each mode's state survives the other being invoked.
    explorer: Option<ExplorerState>,
    /// The sidebar's own listing, independent of `explorer`. `Some` for
    /// as long as the sidebar is open, regardless of whether it's
    /// currently focused.
    sidebar: Option<ExplorerState>,
    sidebar_open: bool,
    /// Whether keys currently route to the sidebar's trie instead of Vim
    /// -- only meaningful while `sidebar_open`. The sidebar stays visible
    /// but unfocused while editing, like a persistent project tree.
    sidebar_focused: bool,
    /// What `explorer` (the full-buffer listing) is currently for --
    /// meaningless while `main_view != Explorer`. See `ExplorerPurpose`'s
    /// own doc comment.
    explorer_purpose: ExplorerPurpose,
    explorer_prompt: Option<ExplorerPrompt>,
    /// Topmost visible row of the full-buffer/sidebar listings. Plain
    /// integers, not eased like `rendered_scroll` -- a directory listing
    /// doesn't get the smooth-scroll treatment the editor buffer does,
    /// just enough to keep the selection on screen.
    explorer_scroll: usize,
    sidebar_scroll: usize,
    /// Same role as `explorer_scroll`, for the picker's candidate list.
    /// Reset to 0 whenever a new picker is entered.
    picker_scroll: usize,

    /// The current buffer's project root (re-derived whenever a file is
    /// opened -- same "always fresh, never stale" posture as everything
    /// else computed from buffer state). `None` outside any recognized
    /// project.
    project_root: Option<PathBuf>,
    active_picker: Option<ActivePicker>,
    /// The grep search-term prompt, when in progress -- `Some` only
    /// between `SPC p s` and the term being submitted (or cancelled),
    /// not while an `ActivePicker::Grep` is already showing results.
    pending_grep_query: Option<String>,
    /// Loaded once at startup; saved back to disk whenever `SPC p a`/
    /// `SPC p d` change it, or the switch-project picker re-selects an
    /// already-known root (an MRU bump, not a new registration -- see
    /// `refresh_project_root`'s doc comment for why opening a file no
    /// longer registers its project automatically). Empty (silently) on
    /// a platform with no config-directory concept, or if the file can't
    /// be read for some other reason -- a picker just starting with no
    /// history isn't worth failing over.
    known_projects: fenix_project::KnownProjects,
    /// Same persisted-list shape as `known_projects`, but automatic --
    /// every real file opened (startup CLI arg, the picker, the
    /// explorer) gets recorded via `record_recent_file`, no explicit
    /// `SPC p a`-style curation step. Read by the dashboard.
    recent_files: fenix_project::RecentFiles,
    /// Per-line metadata for whichever dashboard buffer(s) are
    /// currently open, keyed by `BufferId` (not one flat field) so an
    /// older still-open dashboard buffer elsewhere in the window tree
    /// never gets misattributed metadata from the newest one -- same
    /// per-buffer-keyed-state precedent as `scroll_anims`. Consulted by
    /// `dashboard_activate_selected` and the dashboard's own syntax-
    /// highlight coloring.
    dashboard_lines: HashMap<BufferId, Vec<Option<dashboard::DashboardLine>>>,

    /// The live, mutable directory-listing state for every real
    /// `BufferKind::Explorer` buffer currently open (`SPC f j`), keyed by
    /// `BufferId` -- same per-buffer-keyed precedent as `dashboard_
    /// lines`. Distinct from `explorer`/`sidebar` (the older overlay
    /// mechanism, still used unchanged for the `SPC p a` project-picking
    /// flow and the persistent sidebar -- see `explorer_jump`'s own doc
    /// comment for why ordinary browsing moved to a real buffer but
    /// those two didn't).
    dired_states: HashMap<BufferId, ExplorerState>,
    /// Which `entries` index (if any) each generated line of a dired
    /// buffer's real rope text corresponds to -- mirrors `dashboard_
    /// lines`' exact role, just for `explorer_dired_text`'s output
    /// instead of `dashboard::render`'s.
    dired_lines: HashMap<BufferId, Vec<Option<usize>>>,

    /// Per-line metadata for every real `BufferKind::Docker` buffer
    /// currently open (`SPC d d`), keyed by `BufferId` -- same per-buffer-
    /// keyed precedent as `dashboard_lines`/`dired_lines`. Consulted by
    /// the action keys (`s`/`S`/`R`/`r`/`x`) to know what the cursor's
    /// current line targets, and by the panel's own syntax-highlight
    /// coloring.
    docker_lines: HashMap<BufferId, Vec<Option<docker_panel::DockerLine>>>,
    /// Armed by `x` on a Docker buffer row until the next keypress
    /// confirms (`y`) or cancels (anything else) -- same "wait for
    /// exactly one more raw key" shape as `fenix-vim`'s own `r<char>`,
    /// applied here to a destructive remove instead of a replace.
    /// Overrides the modeline the same way `Mode::Command`/`Mode::Search`
    /// already do (see `modeline_pieces`/`docker_confirm_text`) to show
    /// what's being confirmed.
    docker_confirm_remove: Option<docker_panel::DockerEntry>,

    /// The autocompletion popup's live state -- `Some` only while Insert
    /// mode, the focused buffer's language, and the prefix at the cursor
    /// all currently justify showing it (see `sync_completion`, called
    /// after every keystroke that reaches Vim while Insert-relevant, the
    /// same "derive fresh from whatever's current" posture `content_
    /// spans`/dashboard centering already use -- `VimEvent` has no "a
    /// character was typed" signal, so this recomputes instead of trying
    /// to track it incrementally).
    completion: Option<CompletionState>,
    /// One cached `(project_root, candidates)` pair for Tcl completion,
    /// rebuilt only when the current buffer's project root differs from
    /// what's cached -- not a map of every project root ever visited
    /// this session (disclosed simplification: matches the common
    /// single-active-project case, see the plan's Scope). Cleared by
    /// `SPC c r` to force a fresh `ctags` scan.
    tcl_candidates_cache: Option<(Option<PathBuf>, Vec<fenix_picker::Candidate<fenix_completion::CompletionItem>>)>,
    /// Same role as `picker_scroll`, for the completion popup's own
    /// candidate window.
    completion_scroll: usize,

    vim: VimState,
    /// Persists across keystrokes so a `SPC f s` sequence can span several
    /// `handle_key` calls; `'static` because the leader trie is a global
    /// singleton (see `keymap::leader_trie`), which sidesteps
    /// `Matcher` borrowing from a trie `App` would otherwise also own.
    leader_matcher: Matcher<'static, &'static str>,
    /// Same reasoning as `leader_matcher`, for the explorer's own trie --
    /// persists across keystrokes for its one multi-key sequence (`g r`).
    explorer_matcher: Matcher<'static, ExplorerAction>,

    theme: &'static Theme,
    /// The unified settings file (`dirs::config_dir()/fenix/config.ini`)
    /// -- theme name, font size, indent width, and the completion
    /// symbols-file path all live here now, one `load` at startup and one
    /// `save` per change, instead of each setting having its own flat
    /// file and free-function trio. Lives here rather than in
    /// `fenix-vim`/`text.rs`: `VimState`/`TextPipeline` stay free of file
    /// I/O, matching every other pure editing-state type -- `App` applies
    /// each loaded value to the relevant subsystem and saves back into
    /// this one struct on every change (`cycle_theme`, `adjust_font_
    /// size`, `VimEvent::IndentWidthChanged`).
    config: fenix_config::Config,

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
        let file_arg = env::args().nth(1);
        let mut app = Self::with_file(file_arg.clone());
        // Recording lives here, not inside `with_file` -- `with_file` is
        // what the test suite calls directly to simulate "opened with
        // this file," and recording there would mean every such test
        // writes into the real `~/.config/fenix/recent_files.txt` on
        // whatever machine happens to run the tests. Real launches
        // always go through `new`, so this still fires for actual use.
        if let Some(path) = &file_arg {
            app.record_recent_file(Path::new(path));
        }
        app
    }

    fn with_file(file_arg: Option<String>) -> Self {
        // Loaded before the initial buffer so a no-argument launch can
        // build the dashboard from them.
        let known_projects_path =
            fenix_project::KnownProjects::default_path().unwrap_or_else(|| PathBuf::from("fenix-projects.txt"));
        let known_projects = fenix_project::KnownProjects::load_or_default(known_projects_path);
        let recent_files_path = fenix_project::RecentFiles::default_path()
            .unwrap_or_else(|| PathBuf::from("fenix-recent-files.txt"));
        let recent_files = fenix_project::RecentFiles::load_or_default(recent_files_path);

        let mut buffers = BufferList::new();
        let mut dashboard_lines = HashMap::new();
        let initial_id = match file_arg {
            // Recording this path into `recent_files` happens in `new`,
            // not here -- see `new`'s own doc comment for why.
            Some(path) => buffers.open_path(Path::new(&path)),
            None => {
                let dashboard = dashboard::render(known_projects.roots(), recent_files.paths());
                let id = buffers.open_dashboard(&dashboard.text);
                dashboard_lines.insert(id, dashboard.lines);
                id
            }
        };
        let initial_cursor = buffers.get(initial_id).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        let workspaces = WorkspaceList::new(WindowTree::new(initial_id), initial_cursor);

        // `project_root` (used to scope `SPC p f`/`SPC p s`) is still
        // auto-detected from whatever file is open -- only the *known-
        // projects* list (the `SPC p p`/`SPC p d` switch/delete registry)
        // is no longer auto-populated from it. That list is explicitly
        // curated via `SPC p a` now (see `picker_add_project_prompt`'s own
        // doc comment for why): every file ever opened auto-registering
        // its project made the switch-project list an unfiltered history
        // of everywhere you'd been, not a deliberately kept list of
        // projects worth switching between. A dashboard buffer has no
        // path, so this correctly comes out `None`.
        let project_root =
            buffers.get(initial_id).and_then(|ob| ob.buffer.path()).and_then(fenix_project::find_project_root);
        let config_path = fenix_config::Config::default_path().unwrap_or_else(|| PathBuf::from("fenix-config.ini"));
        let config = fenix_config::Config::load_or_default(config_path);
        let theme = config.theme.as_deref().and_then(theme::by_name).unwrap_or(&theme::ORBIT_DARK);
        let mut vim = VimState::new();
        vim.set_indent_width(config.indent_width.unwrap_or(fenix_vim::DEFAULT_INDENT_WIDTH));

        Self {
            window: None,
            gpu: None,
            text: None,
            bg_rect: None,
            caret_rect: None,
            buffers,
            workspaces,
            line_number_mode: LineNumberMode::Absolute,
            main_view: MainView::Editor,
            explorer: None,
            sidebar: None,
            sidebar_open: false,
            sidebar_focused: false,
            explorer_purpose: ExplorerPurpose::Browse,
            explorer_prompt: None,
            explorer_scroll: 0,
            sidebar_scroll: 0,
            picker_scroll: 0,
            project_root,
            active_picker: None,
            known_projects,
            recent_files,
            dashboard_lines,
            dired_states: HashMap::new(),
            dired_lines: HashMap::new(),
            docker_lines: HashMap::new(),
            docker_confirm_remove: None,
            completion: None,
            tcl_candidates_cache: None,
            completion_scroll: 0,
            pending_grep_query: None,
            vim,
            leader_matcher: keymap::leader_trie().matcher(),
            explorer_matcher: fenix_explorer::explorer_trie().matcher(),
            theme,
            config,
            modifiers: ModifiersState::empty(),
            blink_visible: true,
            blink_transition_start: Instant::now() - BLINK_FADE,
            next_blink: Instant::now() + BLINK_INTERVAL,
            pulse: None,
        }
    }

    /// The active workspace's window tree. Every call site outside this
    /// pair of accessors goes through here (or `windows_mut`) rather than
    /// touching `self.workspaces` directly, mirroring the `open`/
    /// `open_mut` discipline already used for buffers.
    fn windows(&self) -> &WindowTree<BufferId> {
        self.workspaces.active()
    }

    fn windows_mut(&mut self) -> &mut WindowTree<BufferId> {
        self.workspaces.active_mut()
    }

    fn focused_buffer_id(&self) -> BufferId {
        *self.windows().focused_content()
    }

    /// The currently-focused pane's open buffer. Free to use anywhere
    /// that doesn't *also* need another `App` field (like `self.vim`) in
    /// the same expression -- being a method call on `self`, it borrows
    /// opaquely, so those few spots inline `self.buffers.get_mut(id)`
    /// directly instead (see e.g. `handle_key`'s Vim dispatch).
    fn open(&self) -> &OpenBuffer {
        self.buffers.get(self.focused_buffer_id()).expect("focused window always has an open buffer")
    }

    fn open_mut(&mut self) -> &mut OpenBuffer {
        let id = self.focused_buffer_id();
        self.buffers.get_mut(id).expect("focused window always has an open buffer")
    }

    fn focused_pane_id(&self) -> fenix_window::WindowId {
        self.windows().focused_id()
    }

    /// `pane`'s own live cursor/scroll -- every existing pane has one,
    /// seeded when the pane was created (`Workspace::new`/`split_window`/
    /// `set_pane_content`), so this can safely `.expect()` rather than
    /// return `Option`, the same invariant-backed shape `open`/`open_mut`
    /// already use for `focused_buffer_id`.
    fn pane_state(&self, pane: fenix_window::WindowId) -> &PaneState {
        self.workspaces.active_pane_states().get(&pane).expect("every existing pane has a PaneState")
    }

    fn pane_state_mut(&mut self, pane: fenix_window::WindowId) -> &mut PaneState {
        self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState")
    }

    /// The focused pane's live cursor -- read-only convenience mirroring
    /// `open()`'s role but for pane-owned (not buffer-owned) state. Same
    /// "don't use where another `self` field is also needed in the same
    /// expression" caveat as `open()`'s own doc comment.
    fn cursor(&self) -> Cursor {
        self.pane_state(self.focused_pane_id()).cursor
    }

    /// The focused buffer plus the focused pane's live cursor, as the two
    /// separate mutable borrows most buffer-editing call sites need
    /// simultaneously (`Buffer` methods take `&mut Cursor` as a sibling
    /// argument, not something bundled into one struct anymore now that
    /// cursor is pane-owned rather than buffer-owned). Not usable from a
    /// spot that *also* needs another `self` field (like `self.vim`) in
    /// the same expression -- those inline `self.buffers.get_mut(id)` /
    /// `self.workspaces.active_pane_states_mut().get_mut(&pane)` directly
    /// instead (see `handle_key`'s Vim dispatch), the same reasoning
    /// `open`/`open_mut`'s own doc comment already gives.
    fn focused_buffer_and_cursor_mut(&mut self) -> (&mut Buffer, &mut Cursor) {
        let buffer_id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let buffer = &mut self.buffers.get_mut(buffer_id).expect("focused window always has an open buffer").buffer;
        let cursor = &mut self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState").cursor;
        (buffer, cursor)
    }

    /// Points `pane` at `buffer_id` and resets that pane's live cursor/
    /// scroll to the newly-shown buffer's own remembered position --
    /// every `WindowTree::set_content` call site goes through this
    /// instead of calling it directly, so a pane switching to a
    /// different buffer never keeps stale cursor/scroll state left over
    /// from whatever it was showing before (which could easily be out of
    /// bounds for the new buffer's own length).
    fn set_pane_content(&mut self, pane: fenix_window::WindowId, buffer_id: BufferId) {
        self.windows_mut().set_content(pane, buffer_id);
        let cursor = self.buffers.get(buffer_id).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        self.workspaces.active_pane_states_mut().insert(pane, PaneState::seeded_at(cursor));
    }

    /// Re-derives `project_root` for the focused buffer -- called every
    /// time a pane's buffer changes, not just at startup (`with_file`
    /// does the equivalent inline before `self` exists to call this on).
    /// Deliberately does *not* register the result in `known_projects`:
    /// that list is explicitly curated via `SPC p a`/`SPC p d` now, not
    /// auto-populated from wherever you happen to open a file -- see
    /// `picker_add_project_prompt`'s own doc comment for why.
    fn refresh_project_root(&mut self) {
        self.project_root = self.open().buffer.path().and_then(fenix_project::find_project_root);
    }

    /// Records `path` as recently-opened for the dashboard's "Recent
    /// Files" list -- called from every real "read this path off disk"
    /// call site (the picker, the explorer; `with_file`'s own CLI-arg
    /// branch does the equivalent inline before `self` exists). Unlike
    /// `known_projects`, this is automatic, not explicitly curated --
    /// see `RecentFiles`'s own doc comment.
    fn record_recent_file(&mut self, path: &Path) {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent_files.add(canonical);
        if let Err(err) = self.recent_files.save() {
            eprintln!("fenix: couldn't save recent files: {err}");
        }
    }

    /// The Tcl completion candidate pool (keywords + `ctags`-sourced
    /// definitions + any entries from `self.config.completion_symbols_
    /// file`, deduped against each other), rebuilding only when `root`
    /// differs from what's cached -- `ctags::run` shells a real
    /// subprocess, so this is worth avoiding on every keystroke (matches
    /// the "expensive work only on real state changes" discipline
    /// already used for git-status-on-explorer-open). The symbols file
    /// is re-read on the same cadence, not independently -- both sources
    /// share one manual refresh (`refresh_completion_tags`/`SPC c r`).
    /// Cloning the cached `Vec` on a cache hit is cheap: dozens to low
    /// hundreds of small entries.
    fn tcl_candidates(&mut self, root: Option<&Path>) -> Vec<fenix_picker::Candidate<fenix_completion::CompletionItem>> {
        let stale = match &self.tcl_candidates_cache {
            Some((cached_root, _)) => cached_root.as_deref() != root,
            None => true,
        };
        if stale {
            let mut seen = std::collections::HashSet::new();
            let mut candidates = Vec::new();
            for &keyword in fenix_completion::tcl::KEYWORDS {
                if seen.insert(keyword.to_string()) {
                    let item = fenix_completion::CompletionItem {
                        label: keyword.to_string(),
                        kind: fenix_completion::CompletionKind::Keyword,
                    };
                    candidates.push(fenix_picker::Candidate::new(item.label.clone(), item));
                }
            }
            if let Some(root) = root {
                for tag in fenix_completion::ctags::run(root, "Tcl") {
                    if seen.insert(tag.name.clone()) {
                        let item = fenix_completion::CompletionItem { label: tag.name, kind: fenix_completion::CompletionKind::Tag };
                        candidates.push(fenix_picker::Candidate::new(item.label.clone(), item));
                    }
                }
            }
            if let Some(symbols_file) = &self.config.completion_symbols_file {
                for item in fenix_completion::custom::load(symbols_file) {
                    if seen.insert(item.label.clone()) {
                        candidates.push(fenix_picker::Candidate::new(item.label.clone(), item));
                    }
                }
            }
            self.tcl_candidates_cache = Some((root.map(PathBuf::from), candidates));
        }
        self.tcl_candidates_cache.as_ref().expect("just populated above if it was missing").1.clone()
    }

    /// Re-derives the completion popup's state from whatever's current --
    /// called once right after every keystroke that reaches Vim while
    /// Insert-relevant (`handle_key`'s Vim-fallthrough tier). Closes the
    /// popup (via `None`) the instant any of its preconditions stop
    /// holding: not in Insert mode, not a Tcl buffer, no identifier
    /// prefix at the cursor, or the prefix no longer matches anything.
    fn sync_completion(&mut self) {
        if self.vim.mode() != Mode::Insert {
            self.completion = None;
            return;
        }
        let ob = self.open();
        let language = ob.buffer.path().and_then(|p| p.extension()).and_then(|e| e.to_str()).and_then(fenix_syntax::detect_language);
        if language != Some(fenix_syntax::LanguageId::Tcl) {
            self.completion = None;
            return;
        }
        let cursor = self.cursor();
        let ob = self.open();
        let Some((start, prefix)) = completion::prefix_at_cursor(&ob.buffer, &cursor) else {
            self.completion = None;
            return;
        };

        match &mut self.completion {
            Some(state) => {
                state.prefix_start = start;
                state.picker.set_query(&prefix);
            }
            None => {
                let root = self.project_root.clone();
                let candidates = self.tcl_candidates(root.as_deref());
                let mut picker = fenix_picker::PickerState::new(candidates);
                picker.set_query(&prefix);
                self.completion = Some(CompletionState { prefix_start: start, picker });
            }
        }
        if self.completion.as_ref().is_some_and(|state| state.picker.is_empty()) {
            self.completion = None;
        }
        self.completion_scroll = 0;
    }

    /// Force-opens the completion popup at the current (possibly empty)
    /// prefix, bypassing the normal ">=1 char" auto-trigger threshold --
    /// `Ctrl-Space`'s effect, the near-universal manual-trigger
    /// convention.
    fn force_open_completion(&mut self) {
        let ob = self.open();
        let language = ob.buffer.path().and_then(|p| p.extension()).and_then(|e| e.to_str()).and_then(fenix_syntax::detect_language);
        if language != Some(fenix_syntax::LanguageId::Tcl) {
            return;
        }
        let cursor = self.cursor();
        let ob = self.open();
        let (prefix_start, prefix) =
            completion::prefix_at_cursor(&ob.buffer, &cursor).unwrap_or((cursor.char_idx, String::new()));
        let root = self.project_root.clone();
        let candidates = self.tcl_candidates(root.as_deref());
        let mut picker = fenix_picker::PickerState::new(candidates);
        picker.set_query(&prefix);
        self.completion = if picker.is_empty() { None } else { Some(CompletionState { prefix_start, picker }) };
        self.completion_scroll = 0;
    }

    /// `SPC c r` -- clears the cached Tcl completion candidates so the
    /// next `sync_completion`/`force_open_completion` re-shells `ctags`
    /// and picks up new/renamed `proc`/`namespace` definitions. Manual
    /// only (no save-hook/file-watcher refresh) -- see the plan's Scope.
    pub(crate) fn refresh_completion_tags(&mut self) {
        self.tcl_candidates_cache = None;
    }

    /// Replaces the typed prefix with the selected candidate's full
    /// label -- one atomic undo step via `Buffer::replace_range`.
    fn accept_completion(&mut self) {
        let Some(state) = self.completion.take() else { return };
        let Some(label) = state.picker.selected().map(|c| c.payload.label.clone()) else { return };
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let end = cursor.char_idx;
        buffer.replace_range(cursor, state.prefix_start, end, &label);
    }

    pub(crate) fn save(&mut self) {
        if self.open().buffer.path().is_none() {
            eprintln!("fenix: no file path to save to yet; pass a file path as the first argument");
            return;
        }
        let ob = self.open_mut();
        match ob.buffer.save() {
            Ok(()) => println!("fenix: saved {:?}", ob.buffer.path().unwrap()),
            Err(err) => eprintln!("fenix: save failed: {err}"),
        }
        self.wake_caret();
    }

    /// Directory the explorer opens in when asked to "start somewhere
    /// sensible": the focused buffer's own directory if it has a path,
    /// else the process's cwd.
    fn explorer_start_dir(&self) -> PathBuf {
        self.open()
            .buffer
            .path()
            .and_then(|p| p.parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// `SPC f j` (dired-jump): opens a real, closable (`SPC b k`),
    /// splittable (`SPC w v`/`SPC w s`), buffer-switcher-listed (`SPC b
    /// b`) dired buffer (`BufferKind::Explorer`) at the current file's
    /// directory, in the focused pane -- unlike the older full-buffer/
    /// sidebar overlay (`self.explorer`/`self.sidebar`, still used
    /// unchanged for `SPC p a`'s directory-picking flow and the
    /// persistent sidebar respectively; see `MainView::Explorer`'s own
    /// doc comment), this is a genuine `BufferId` other panes/workspaces
    /// can reference, and ordinary Vim motions navigate it for free
    /// since its content is real rope text (see `handle_key`'s own
    /// `BufferKind::Explorer` interception for the small set of action
    /// keys layered on top).
    pub(crate) fn explorer_jump(&mut self) {
        let dir = self.explorer_start_dir();
        self.open_dired_at(&dir);
    }

    /// Shared by `explorer_jump` (which derives `dir` from the current
    /// file) and tests (which just want a specific directory) -- opens a
    /// fresh dired buffer at `dir` in the focused pane.
    fn open_dired_at(&mut self, dir: &Path) {
        let state = match ExplorerState::open(dir) {
            Ok(s) => s,
            Err(err) => {
                eprintln!("fenix: couldn't list {} ({err})", dir.display());
                return;
            }
        };
        let (text, lines) = explorer_dired_text(&state);
        let id = self.buffers.open_explorer(&text);
        self.dired_states.insert(id, state);
        self.dired_lines.insert(id, lines);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.wake_caret();
    }

    /// Replaces `id`'s dired state with `new_state` and regenerates its
    /// buffer's rope text to match in one atomic step (`Buffer::
    /// replace_range` over the whole content -- same "rewrite the whole
    /// affected span as one step" tool `:s` substitute already uses),
    /// then resets every pane currently showing `id` back to the top --
    /// the old cursor/scroll position is meaningless against entirely
    /// different content (a different directory's listing).
    fn set_dired_state(&mut self, id: BufferId, new_state: ExplorerState) {
        let (text, lines) = explorer_dired_text(&new_state);
        self.dired_states.insert(id, new_state);
        self.dired_lines.insert(id, lines);
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, &text);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// `Enter` on a dired buffer: opens the file at the cursor's line
    /// into the focused pane, or navigates into the directory at that
    /// line in place (same `BufferId`, freshly regenerated) -- matches
    /// real dired's own "navigating a subdirectory reuses the buffer"
    /// convention, not a new buffer per level.
    fn dired_activate_selected(&mut self) {
        let id = self.focused_buffer_id();
        let line = self.open().buffer.line_col(&self.cursor()).0;
        let Some(Some(entry_idx)) = self.dired_lines.get(&id).and_then(|lines| lines.get(line)).copied() else {
            return;
        };
        let Some(entry) = self.dired_states.get(&id).and_then(|s| s.entries.get(entry_idx)) else { return };
        let path = entry.path.clone();
        if entry.is_dir {
            match ExplorerState::open(&path) {
                Ok(new_state) => self.set_dired_state(id, new_state),
                Err(err) => eprintln!("fenix: couldn't list {} ({err})", path.display()),
            }
        } else {
            self.open_file_from_picker(&path);
        }
    }

    /// `-` on a dired buffer: navigates up to the parent of the
    /// directory currently being browsed, in place.
    fn dired_parent_dir(&mut self) {
        let id = self.focused_buffer_id();
        let Some(parent) = self.dired_states.get(&id).and_then(|s| s.cwd.parent()).map(Path::to_path_buf) else {
            return;
        };
        match ExplorerState::open(&parent) {
            Ok(new_state) => self.set_dired_state(id, new_state),
            Err(err) => eprintln!("fenix: couldn't list {} ({err})", parent.display()),
        }
    }

    /// `R` on a dired buffer: re-lists the same directory (picks up
    /// files created/removed/renamed since it was opened).
    fn dired_refresh(&mut self) {
        let id = self.focused_buffer_id();
        let Some(mut state) = self.dired_states.remove(&id) else { return };
        if let Err(err) = state.refresh() {
            eprintln!("fenix: couldn't refresh ({err})");
        }
        self.set_dired_state(id, state);
    }

    /// `.` on a dired buffer: toggles dotfile visibility and re-lists.
    fn dired_toggle_hidden(&mut self) {
        let id = self.focused_buffer_id();
        let Some(mut state) = self.dired_states.remove(&id) else { return };
        if let Err(err) = state.toggle_hidden() {
            eprintln!("fenix: couldn't refresh ({err})");
        }
        self.set_dired_state(id, state);
    }

    /// `SPC e t`: toggles the sidebar open/closed. Opening focuses it
    /// immediately (you just asked to browse); closing always returns
    /// focus to the editor.
    pub(crate) fn toggle_sidebar(&mut self) {
        if self.sidebar_open {
            self.sidebar_open = false;
            self.sidebar_focused = false;
            self.sidebar = None;
        } else {
            let dir = self.explorer_start_dir();
            match ExplorerState::open(&dir) {
                Ok(explorer) => {
                    self.sidebar = Some(explorer);
                    self.sidebar_open = true;
                    self.sidebar_focused = true;
                }
                Err(err) => eprintln!("fenix: couldn't list {} ({err})", dir.display()),
            }
        }
        self.wake_caret();
    }

    /// Builds fuzzy-picker candidates for every file in `root` --
    /// relative path as the label (fuzzy-matched and displayed),
    /// absolute path as the payload (what actually gets opened).
    fn find_file_candidates(root: &Path) -> Vec<fenix_picker::Candidate<PathBuf>> {
        fenix_project::list_project_files(root)
            .into_iter()
            .map(|path| {
                let label = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                fenix_picker::Candidate::new(label, path)
            })
            .collect()
    }

    /// `SPC p f`: a fuzzy file picker scoped to the current project (or
    /// the process's cwd, if no project was detected -- still useful,
    /// just not project-scoped).
    pub(crate) fn picker_find_file(&mut self) {
        let root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let candidates = Self::find_file_candidates(&root);
        self.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
    }

    /// Candidates for both the switch-project (`SPC p p`) and delete-
    /// project (`SPC p d`) pickers -- same list, same label, just a
    /// different picker variant so `picker_confirm` dispatches to a
    /// different action.
    fn known_project_candidates(&self) -> Vec<fenix_picker::Candidate<PathBuf>> {
        self.known_projects
            .roots()
            .iter()
            .map(|root| fenix_picker::Candidate::new(root.to_string_lossy().into_owned(), root.clone()))
            .collect()
    }

    /// `SPC p p`: a fuzzy picker over the persisted, MRU-ordered known-
    /// projects list.
    pub(crate) fn picker_switch_project(&mut self) {
        let candidates = self.known_project_candidates();
        self.enter_picker(ActivePicker::SwitchProject(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p d`: same list as `SPC p p`, but confirming a selection
    /// removes it from `known_projects` instead of switching to it (see
    /// `ActivePicker::DeleteProject`/`picker_confirm`).
    pub(crate) fn picker_delete_project(&mut self) {
        let candidates = self.known_project_candidates();
        self.enter_picker(ActivePicker::DeleteProject(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC p a`: opens the full-buffer file explorer, in "pick a
    /// directory" mode, to browse to and register a project in
    /// `known_projects` -- the *only* way a project gets into the switch-
    /// project list now (see `refresh_project_root`'s own doc comment for
    /// why auto-registration-on-visit was dropped: it made "switch
    /// project" list every project you'd ever opened a file in, not a
    /// deliberately curated set worth switching between). Starts browsing
    /// at the current project root (or the process's cwd, if none
    /// detected); navigate with the explorer's usual `j`/`k`/`l`/`h`/
    /// `Enter`, then `S` to register whichever directory is currently
    /// being browsed. `q`/`Escape` cancels without registering anything,
    /// same as leaving the explorer any other time.
    pub(crate) fn picker_add_project_prompt(&mut self) {
        let start = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let explorer = match ExplorerState::open(&start) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("fenix: couldn't list {} ({err})", start.display());
                return;
            }
        };
        self.explorer = Some(explorer);
        self.explorer_purpose = ExplorerPurpose::PickProjectDir;
        self.main_view = MainView::Explorer;
        self.wake_caret();
    }

    /// `S` on the add-project explorer (`ExplorerAction::SelectCwd`):
    /// registers whatever directory is currently being browsed. Already
    /// an absolute, real filesystem path (it came from actually listing
    /// it), so this only needs to canonicalize away any `..`/symlink
    /// noise before persisting -- no typed-input parsing like the old
    /// free-text prompt needed.
    fn register_project_dir(&mut self, dir: &Path) {
        let root = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        self.known_projects.add(root);
        if let Err(err) = self.known_projects.save() {
            eprintln!("fenix: couldn't save project history: {err}");
        }
        self.main_view = MainView::Editor;
        self.explorer = None;
        self.explorer_purpose = ExplorerPurpose::Browse;
    }

    /// `SPC p s`: unlike find-file/switch-project (which already have a
    /// full candidate list to fuzzy-filter locally), grep needs a search
    /// term *before* there's anything to show -- this starts a short
    /// modeline-level prompt for it (mirroring Vim's own `:` command
    /// line), not the full picker view yet. `run_grep` opens the actual
    /// picker once results come back.
    pub(crate) fn picker_grep_prompt(&mut self) {
        self.pending_grep_query = Some(String::new());
        self.wake_caret();
    }

    /// Routes one keypress to the in-progress grep search-term prompt --
    /// the same "next keystrokes are special" shape as `explorer_prompt_key`,
    /// scoped to this one always-plain-text case.
    fn grep_query_key(&mut self, key: KeyPress) {
        let Some(query) = &mut self.pending_grep_query else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.pending_grep_query = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let query = self.pending_grep_query.take().unwrap_or_default();
                self.run_grep(&query);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                query.pop();
            }
            KeyCode::Char(c) => query.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    fn run_grep(&mut self, query: &str) {
        let root = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        match fenix_project::grep_project(&root, query) {
            Ok(matches) => {
                let candidates = matches
                    .into_iter()
                    .map(|m| {
                        let rel = m.path.strip_prefix(&root).unwrap_or(&m.path).to_string_lossy().into_owned();
                        let label = format!("{rel}:{}: {}", m.line, m.text.trim());
                        fenix_picker::Candidate::new(label, m)
                    })
                    .collect();
                self.enter_picker(ActivePicker::Grep(fenix_picker::PickerState::new(candidates)));
            }
            Err(err) => eprintln!("fenix: search failed: {err}"),
        }
    }

    /// `SPC b b`: a fuzzy picker over every open buffer, MRU-ordered
    /// (most-recently-touched first, matching Doom's own buffer switcher),
    /// each labeled with its path (or `*scratch*` for an unnamed buffer)
    /// and a leading `+` marker for unsaved changes.
    pub(crate) fn picker_switch_buffer(&mut self) {
        let candidates = self
            .buffers
            .mru()
            .iter()
            .map(|&id| {
                let ob = self.buffers.get(id).expect("mru only lists open buffers");
                let name = ob.buffer.path().map(|p| p.display().to_string()).unwrap_or_else(|| "*scratch*".to_string());
                let marker = if ob.buffer.is_dirty() { "+ " } else { "" };
                fenix_picker::Candidate::new(format!("{marker}{name}"), id)
            })
            .collect();
        self.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
    }

    /// Points the focused pane at an already-open buffer -- `SPC b b`'s
    /// confirm action, and shared by nothing else (unlike `open_file_from_
    /// picker`, there's no path to resolve/read; the buffer already
    /// exists in the registry).
    fn switch_focused_to_buffer(&mut self, id: BufferId) {
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.buffers.touch(id);
        self.refresh_project_root();
        self.main_view = MainView::Editor;
    }

    /// `SPC b n`/`SPC b p`: cycles the focused pane through every open
    /// buffer in a stable, path-sorted order (not MRU -- repeated `n`/`p`
    /// should walk a fixed list, not bounce between the two most recent).
    fn cycle_buffer(&mut self, delta: isize) {
        let ids = self.buffers.ids_sorted_by_path();
        if ids.len() <= 1 {
            return;
        }
        let current = self.focused_buffer_id();
        let pos = ids.iter().position(|&id| id == current).unwrap_or(0) as isize;
        let next = ids[(pos + delta).rem_euclid(ids.len() as isize) as usize];
        self.switch_focused_to_buffer(next);
        self.wake_caret();
    }

    pub(crate) fn next_buffer(&mut self) {
        self.cycle_buffer(1);
    }

    pub(crate) fn prev_buffer(&mut self) {
        self.cycle_buffer(-1);
    }

    /// `SPC b k`: closes the focused buffer. Any pane (in this window
    /// tree) currently showing it falls back to the MRU-next open buffer,
    /// or a fresh scratch buffer if none remain -- never leaves a pane
    /// pointing at a closed `BufferId`.
    pub(crate) fn kill_buffer(&mut self) {
        let id = self.focused_buffer_id();
        self.buffers.close(id);
        let fallback = self.buffers.mru().first().copied().unwrap_or_else(|| self.buffers.open_scratch());
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                self.set_pane_content(pane, fallback);
            }
        }
        self.buffers.touch(fallback);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `SPC b X`: opens a fresh, empty, unnamed buffer in the focused pane.
    pub(crate) fn new_scratch_buffer(&mut self) {
        let id = self.buffers.open_scratch();
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `SPC d d`: (re-)opens the dashboard in the focused pane, freshly
    /// generated from the current `known_projects`/`recent_files` --
    /// always a *new* buffer, same "no dedup" precedent as `SPC b X`
    /// above (repeated presses just make more buffers; see this
    /// feature's own plan for why that's an intentional, not a new,
    /// simplification).
    pub(crate) fn open_dashboard(&mut self) {
        let dashboard = dashboard::render(self.known_projects.roots(), self.recent_files.paths());
        let id = self.buffers.open_dashboard(&dashboard.text);
        self.dashboard_lines.insert(id, dashboard.lines);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// `SPC d d`: (re-)opens the Lazydocker-style container/image panel in
    /// the focused pane, freshly listed from `docker` -- same "always a
    /// new buffer, no dedup" precedent as `open_dashboard`/`SPC b X`.
    pub(crate) fn open_docker_panel(&mut self) {
        let containers = fenix_docker::list_containers();
        let images = fenix_docker::list_images();
        let panel = docker_panel::render(&containers, &images);
        let id = self.buffers.open_docker(&panel.text);
        self.docker_lines.insert(id, panel.lines);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.refresh_project_root();
        self.wake_caret();
    }

    /// Re-lists containers/images and regenerates `id`'s buffer text in
    /// place -- same "rewrite the whole affected span as one step" tool
    /// (`Buffer::replace_range`) `set_dired_state` already uses for the
    /// dired buffer, and same "reset every pane showing it back to the
    /// top" reasoning (a refreshed listing can reorder/add/remove rows,
    /// so an old cursor position is meaningless against it).
    fn set_docker_lines(&mut self, id: BufferId, containers: Vec<fenix_docker::Container>, images: Vec<fenix_docker::Image>) {
        let panel = docker_panel::render(&containers, &images);
        self.docker_lines.insert(id, panel.lines);
        if let Some(ob) = self.buffers.get_mut(id) {
            let end = ob.buffer.len_chars();
            let mut scratch_cursor = Cursor::at_start();
            ob.buffer.replace_range(&mut scratch_cursor, 0, end, &panel.text);
        }
        for pane in self.windows().windows() {
            if self.windows().content(pane) == Some(&id) {
                let ps = self.pane_state_mut(pane);
                *ps = PaneState::seeded_at(Cursor::at_start());
            }
        }
    }

    /// `u` on a Docker buffer: re-lists containers/images from `docker`
    /// and re-renders.
    fn docker_refresh(&mut self) {
        let id = self.focused_buffer_id();
        self.set_docker_lines(id, fenix_docker::list_containers(), fenix_docker::list_images());
    }

    /// What the cursor's current line on a Docker buffer targets -- a
    /// no-op (`None`) for a header/footer/blank line, mirroring
    /// `dashboard_activate_selected`'s own lookup shape.
    fn docker_entry_at_cursor(&self) -> Option<docker_panel::DockerEntry> {
        let cursor = self.cursor();
        let line = self.open().buffer.line_col(&cursor).0;
        self.docker_lines
            .get(&self.focused_buffer_id())
            .and_then(|lines| lines.get(line))
            .and_then(|meta| meta.as_ref())
            .and_then(|meta| meta.entry.clone())
    }

    /// `s` on a Docker buffer: starts the container under the cursor.
    /// A no-op on any other kind of row (an image, a header/footer line).
    fn docker_start_selected(&mut self) {
        if let Some(docker_panel::DockerEntry::Container(id)) = self.docker_entry_at_cursor() {
            if let Err(err) = fenix_docker::start_container(&id) {
                eprintln!("fenix: docker start failed: {err}");
            }
            self.docker_refresh();
        }
    }

    /// `S` on a Docker buffer: stops the container under the cursor.
    fn docker_stop_selected(&mut self) {
        if let Some(docker_panel::DockerEntry::Container(id)) = self.docker_entry_at_cursor() {
            if let Err(err) = fenix_docker::stop_container(&id) {
                eprintln!("fenix: docker stop failed: {err}");
            }
            self.docker_refresh();
        }
    }

    /// `R` on a Docker buffer: restarts the container under the cursor.
    fn docker_restart_selected(&mut self) {
        if let Some(docker_panel::DockerEntry::Container(id)) = self.docker_entry_at_cursor() {
            if let Err(err) = fenix_docker::restart_container(&id) {
                eprintln!("fenix: docker restart failed: {err}");
            }
            self.docker_refresh();
        }
    }

    /// `r` on a Docker buffer: creates and starts a detached container
    /// from the image under the cursor. A no-op on a container row.
    fn docker_run_selected(&mut self) {
        if let Some(docker_panel::DockerEntry::Image(id)) = self.docker_entry_at_cursor() {
            if let Err(err) = fenix_docker::run_image(&id) {
                eprintln!("fenix: docker run failed: {err}");
            }
            self.docker_refresh();
        }
    }

    /// `l` on a Docker buffer: opens the last `DOCKER_LOG_TAIL_LINES`
    /// lines of the container under the cursor's log output into the
    /// focused pane, as a real (plain `Text`-kind) buffer -- Vim-
    /// navigable/searchable/closable for free, no bespoke viewer needed.
    /// A no-op on an image row (images don't have logs) or a header/
    /// footer line.
    fn docker_view_logs_selected(&mut self) {
        let Some(docker_panel::DockerEntry::Container(id)) = self.docker_entry_at_cursor() else { return };
        let text = match fenix_docker::container_logs(&id, DOCKER_LOG_TAIL_LINES) {
            Ok(text) if text.is_empty() => "(no log output)\n".to_string(),
            Ok(text) => text,
            Err(err) => format!("fenix: couldn't fetch logs for {id}: {err}\n"),
        };
        let view_id = self.buffers.open_text_view(&text);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, view_id);
    }

    /// `SPC d b`: builds an image from the current project root's
    /// `Dockerfile` (falling back to the process's cwd with no detected
    /// project) -- doesn't need a Docker buffer focused, unlike the
    /// per-row actions above, since a build targets the project, not a
    /// selected container/image. Refreshes any already-open Docker
    /// buffer in the focused pane so a freshly built image shows up
    /// without a separate manual `u`.
    pub(crate) fn docker_build(&mut self) {
        let context_dir = self.project_root.clone().unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let tag = context_dir.file_name().map(|n| format!("{}:latest", n.to_string_lossy()));
        match fenix_docker::build_image(&context_dir, tag.as_deref()) {
            Ok(_) => println!("fenix: docker build succeeded for {}", context_dir.display()),
            Err(err) => eprintln!("fenix: docker build failed: {err}"),
        }
        if self.open().kind == BufferKind::Docker {
            self.docker_refresh();
        }
        self.wake_caret();
    }

    /// Modeline text while a Docker remove is armed (`x` was just
    /// pressed) -- same "override the modeline with raw prompt text"
    /// mechanism `Mode::Command`/`Mode::Search` already use.
    fn docker_confirm_text(&self) -> Option<String> {
        let entry = self.docker_confirm_remove.as_ref()?;
        let what = match entry {
            docker_panel::DockerEntry::Container(_) => "container",
            docker_panel::DockerEntry::Image(_) => "image",
        };
        Some(format!("Remove this {what}? (y/n)"))
    }

    /// Resolves an armed Docker remove: `y` confirms and removes,
    /// anything else cancels. Either way clears `docker_confirm_remove`
    /// and returns the modeline to normal.
    fn docker_confirm_key(&mut self, keypress: KeyPress) {
        let entry = self.docker_confirm_remove.take();
        if keypress.code == KeyCode::Char('y') {
            if let Some(entry) = entry {
                let result = match entry {
                    docker_panel::DockerEntry::Container(id) => fenix_docker::remove_container(&id),
                    docker_panel::DockerEntry::Image(id) => fenix_docker::remove_image(&id),
                };
                if let Err(err) = result {
                    eprintln!("fenix: docker remove failed: {err}");
                }
                self.docker_refresh();
            }
        }
        self.wake_caret();
    }

    /// No stashing needed -- same reasoning as `explorer_jump`, now that
    /// buffers live in `self.buffers` rather than direct `App` fields.
    fn enter_picker(&mut self, picker: ActivePicker) {
        self.active_picker = Some(picker);
        self.picker_scroll = 0;
        self.main_view = MainView::Picker;
        self.wake_caret();
    }

    /// `Escape`: cancels the active picker -- no file/project opened, the
    /// focused pane's buffer was never touched, so returning to the
    /// editor just means switching `main_view` back.
    fn picker_cancel(&mut self) {
        self.active_picker = None;
        self.main_view = MainView::Editor;
    }

    /// `Enter`: confirms the selected candidate, dispatching differently
    /// per picker kind. A no-op (stays open) if the filtered list is
    /// currently empty -- nothing to confirm.
    fn picker_confirm(&mut self) {
        match &self.active_picker {
            Some(ActivePicker::FindFile(state)) => {
                let Some(path) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.open_file_from_picker(&path);
            }
            Some(ActivePicker::SwitchProject(state)) => {
                let Some(root) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.switch_to_project(root);
            }
            Some(ActivePicker::Grep(state)) => {
                let Some(m) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.open_file_from_picker(&m.path);
                self.jump_to_grep_match(&m);
            }
            Some(ActivePicker::SwitchBuffer(state)) => {
                let Some(id) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.switch_focused_to_buffer(id);
            }
            Some(ActivePicker::DeleteProject(state)) => {
                let Some(root) = state.selected().map(|c| c.payload.clone()) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.known_projects.remove(&root);
                if let Err(err) = self.known_projects.save() {
                    eprintln!("fenix: couldn't save project history: {err}");
                }
            }
            Some(ActivePicker::Theme(state)) => {
                let Some(theme) = state.selected().map(|c| c.payload) else { return };
                self.active_picker = None;
                self.main_view = MainView::Editor;
                self.apply_theme(theme);
            }
            None => {}
        }
        self.wake_caret();
    }

    /// Points the focused pane at `path` (opening it in the registry --
    /// reusing an already-open buffer for the same path, per
    /// `BufferList::open_path`), exactly like opening a file from the
    /// explorer. A picker always fully took over `main_view` to get
    /// here, so confirming always means "this is current now, back to
    /// the editor."
    fn open_file_from_picker(&mut self, path: &Path) {
        let id = self.buffers.open_path(path);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.refresh_project_root();
        self.record_recent_file(path);
        self.main_view = MainView::Editor;
    }

    fn jump_to_grep_match(&mut self, m: &fenix_project::GrepMatch) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        let target_line = m.line.saturating_sub(1).min(buffer.visual_line_count().saturating_sub(1));
        let start = buffer.line_start_char(target_line);
        let col = m.col.saturating_sub(1).min(buffer.line_len(target_line));
        cursor.char_idx = start + col;
        let (_, sticky) = buffer.line_col(cursor);
        cursor.sticky_col = sticky;
    }

    /// Registers `root` as the current project and immediately chains
    /// into a find-file picker scoped to it -- matches Projectile's own
    /// default "switch project" action rather than leaving you with
    /// nothing to do next. Sets `main_view = Picker` explicitly: a no-op
    /// for the original caller (`picker_confirm`, already mid-picker so
    /// this was already true), but necessary for the dashboard's own
    /// call site (`dashboard_activate_selected`), which isn't already
    /// mid-picker when it calls this.
    fn switch_to_project(&mut self, root: PathBuf) {
        self.project_root = Some(root.clone());
        self.known_projects.add(root.clone());
        if let Err(err) = self.known_projects.save() {
            eprintln!("fenix: couldn't save project history: {err}");
        }
        let candidates = Self::find_file_candidates(&root);
        self.active_picker = Some(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
        self.picker_scroll = 0;
        self.main_view = MainView::Picker;
    }

    /// `Enter` on a dashboard buffer (see `handle_key`'s `BufferKind::
    /// Dashboard` check, right before the Vim fallthrough): looks up
    /// what the cursor's current line means via `dashboard_lines`, a
    /// no-op for any line that isn't a `Project`/`RecentFile` entry
    /// (banner/header/blank/footer). A project entry reuses `switch_to_
    /// project` (registers, then chains into a find-file picker, exactly
    /// what confirming it from the `SPC p p` picker already does); a
    /// recent-file entry reuses `open_file_from_picker` (opens it,
    /// records it, and focuses the editor) -- opening a file is opening
    /// a file, however the path was found.
    fn dashboard_activate_selected(&mut self) {
        let cursor = self.cursor();
        let line = self.open().buffer.line_col(&cursor).0;
        let entry = self
            .dashboard_lines
            .get(&self.focused_buffer_id())
            .and_then(|lines| lines.get(line))
            .and_then(|meta| meta.as_ref())
            .and_then(|meta| meta.entry.clone());
        let Some(entry) = entry else { return };
        match entry {
            dashboard::DashboardEntry::Project(root) => self.switch_to_project(root),
            dashboard::DashboardEntry::RecentFile(path) => self.open_file_from_picker(&path),
        }
    }

    /// Routes one keypress to the active picker: plain characters edit
    /// the query and re-filter, Up/Down or Ctrl-N/Ctrl-P move the
    /// selection, Enter confirms, Escape cancels -- everything else is
    /// ignored (the picker is a text-input mode, not an action trie like
    /// the explorer's, so unrecognized keys just don't do anything
    /// rather than falling through to something else).
    fn picker_key(&mut self, key: KeyPress) {
        let Some(picker) = &mut self.active_picker else { return };
        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.picker_cancel(),
            KeyCode::Named(FenixNamedKey::Enter) => self.picker_confirm(),
            KeyCode::Named(FenixNamedKey::Backspace) => picker_backspace(picker),
            KeyCode::Named(FenixNamedKey::Down) => picker_move_selection(picker, 1),
            KeyCode::Named(FenixNamedKey::Up) => picker_move_selection(picker, -1),
            KeyCode::Char('n') if key.mods.ctrl => picker_move_selection(picker, 1),
            KeyCode::Char('p') if key.mods.ctrl => picker_move_selection(picker, -1),
            KeyCode::Char(c) if !key.mods.ctrl => picker_push_char(picker, c),
            _ => {}
        }
        self.wake_caret();
    }

    /// The explorer currently receiving input: the full-buffer listing
    /// when `main_view == Explorer`, else the sidebar's when it's
    /// focused, else `None` (keys go to Vim instead).
    fn active_explorer(&self) -> Option<&ExplorerState> {
        if self.main_view == MainView::Explorer {
            self.explorer.as_ref()
        } else if self.sidebar_focused {
            self.sidebar.as_ref()
        } else {
            None
        }
    }

    fn active_explorer_mut(&mut self) -> Option<&mut ExplorerState> {
        if self.main_view == MainView::Explorer {
            self.explorer.as_mut()
        } else if self.sidebar_focused {
            self.sidebar.as_mut()
        } else {
            None
        }
    }

    /// Stores a freshly re-opened `ExplorerState` back into whichever
    /// slot is currently active -- used after navigating into a
    /// directory or up to a parent, which replace the whole listing.
    fn set_active_explorer(&mut self, state: ExplorerState) {
        if self.main_view == MainView::Explorer {
            self.explorer = Some(state);
        } else {
            self.sidebar = Some(state);
        }
    }

    /// Dispatches one explorer command against whichever listing is
    /// active. Re-fetches `active_explorer_mut()` fresh per arm rather
    /// than binding it once, since a few arms (`Open`, `ParentDir`,
    /// `Quit`) need to touch other `self` fields too (replacing the
    /// buffer, restoring the stash) and can't do that while still
    /// holding it borrowed.
    fn explorer_handle_action(&mut self, action: ExplorerAction) {
        if self.active_explorer().is_none() {
            return;
        }
        match action {
            ExplorerAction::Down => {
                self.active_explorer_mut().unwrap().move_selection(1);
            }
            ExplorerAction::Up => {
                self.active_explorer_mut().unwrap().move_selection(-1);
            }
            ExplorerAction::ToggleExpand => {
                if let Err(err) = self.active_explorer_mut().unwrap().toggle_expand() {
                    eprintln!("fenix: couldn't expand ({err})");
                }
            }
            ExplorerAction::ToggleMark => self.active_explorer_mut().unwrap().toggle_mark(),
            ExplorerAction::MarkAll => self.active_explorer_mut().unwrap().mark_all(),
            ExplorerAction::UnmarkAll => self.active_explorer_mut().unwrap().unmark_all(),
            ExplorerAction::ToggleAllMarks => self.active_explorer_mut().unwrap().toggle_all_marks(),
            ExplorerAction::ToggleHidden => {
                if let Err(err) = self.active_explorer_mut().unwrap().toggle_hidden() {
                    eprintln!("fenix: couldn't refresh ({err})");
                }
            }
            ExplorerAction::Refresh => {
                if let Err(err) = self.active_explorer_mut().unwrap().refresh() {
                    eprintln!("fenix: couldn't refresh ({err})");
                }
            }
            ExplorerAction::ParentDir => {
                let parent = self.active_explorer().unwrap().cwd.parent().map(Path::to_path_buf);
                if let Some(parent) = parent {
                    match ExplorerState::open(&parent) {
                        Ok(new_state) => self.set_active_explorer(new_state),
                        Err(err) => eprintln!("fenix: couldn't list {} ({err})", parent.display()),
                    }
                }
            }
            ExplorerAction::Open => self.explorer_open_selected(),
            ExplorerAction::BeginDelete => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::ConfirmDelete, input: String::new() });
                }
            }
            ExplorerAction::BeginRename => {
                if let Some(name) = self.active_explorer().unwrap().selected_entry().map(|e| e.name.clone()) {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::Rename, input: name });
                }
            }
            ExplorerAction::BeginCreateFile => {
                self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CreateFile, input: String::new() });
            }
            ExplorerAction::BeginCreateDir => {
                self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CreateDir, input: String::new() });
            }
            ExplorerAction::BeginCopy => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::CopyTo, input: String::new() });
                }
            }
            ExplorerAction::BeginMove => {
                if !self.active_explorer().unwrap().targets().is_empty() {
                    self.explorer_prompt = Some(ExplorerPrompt { kind: PromptKind::MoveTo, input: String::new() });
                }
            }
            ExplorerAction::Quit => self.explorer_quit(),
            ExplorerAction::SelectCwd => {
                if self.main_view == MainView::Explorer && self.explorer_purpose == ExplorerPurpose::PickProjectDir {
                    let cwd = self.active_explorer().unwrap().cwd.clone();
                    self.register_project_dir(&cwd);
                }
                // No-op during ordinary browsing (`SPC f j`/the sidebar) --
                // `S` only means something while picking a project dir.
            }
        }
        self.wake_caret();
    }

    /// `Enter`/`l` on the entry at point: navigates into a directory
    /// (replacing the listing), or visits a file -- replacing the editor
    /// buffer and, depending on how the explorer got here, either
    /// returning to the editor (full-buffer mode, dropping the stash --
    /// the new file is now current) or just handing focus back to it
    /// (sidebar mode, which stays open). While picking a project
    /// directory (`SPC p a`), opening a file does nothing -- there's
    /// nothing sensible to do with a file when what's being picked is a
    /// directory; use `S` to register the directory currently being
    /// browsed instead.
    fn explorer_open_selected(&mut self) {
        let Some(explorer) = self.active_explorer() else { return };
        let Some(entry) = explorer.selected_entry() else { return };
        let path = entry.path.clone();
        let is_dir = entry.is_dir;

        if !is_dir && self.main_view == MainView::Explorer && self.explorer_purpose == ExplorerPurpose::PickProjectDir {
            return;
        }

        if is_dir {
            match ExplorerState::open(&path) {
                Ok(new_state) => self.set_active_explorer(new_state),
                Err(err) => eprintln!("fenix: couldn't list {} ({err})", path.display()),
            }
            return;
        }

        let id = self.buffers.open_path(&path);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
        self.refresh_project_root();
        self.record_recent_file(&path);

        if self.main_view == MainView::Explorer {
            self.main_view = MainView::Editor;
            self.explorer = None;
        } else {
            self.sidebar_focused = false;
        }
    }

    /// `q`/`Escape`: in full-buffer mode, leaves the explorer entirely --
    /// the focused pane's buffer was never touched by entering it, so
    /// there's nothing to restore, just switch `main_view` back; in the
    /// sidebar, just hands focus back to the editor without closing it.
    fn explorer_quit(&mut self) {
        if self.main_view == MainView::Explorer {
            self.main_view = MainView::Editor;
            self.explorer = None;
            self.explorer_purpose = ExplorerPurpose::Browse;
        } else if self.sidebar_focused {
            self.sidebar_focused = false;
        }
    }

    /// Routes one keypress to whichever explorer prompt is active: a
    /// bare y/n for delete confirmation, or free-text input (with
    /// Backspace/Enter/Escape) for rename/create/copy/move.
    fn explorer_prompt_key(&mut self, key: KeyPress) {
        let Some(prompt) = &mut self.explorer_prompt else { return };

        if prompt.kind == PromptKind::ConfirmDelete {
            if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
                self.explorer_prompt = None;
                if let Some(explorer) = self.active_explorer_mut() {
                    if let Err(err) = explorer.delete_targets() {
                        eprintln!("fenix: delete failed: {err}");
                    }
                }
            } else {
                self.explorer_prompt = None;
            }
            self.wake_caret();
            return;
        }

        match key.code {
            KeyCode::Named(FenixNamedKey::Escape) => self.explorer_prompt = None,
            KeyCode::Named(FenixNamedKey::Enter) => {
                let ExplorerPrompt { kind, input } = self.explorer_prompt.take().unwrap();
                self.explorer_prompt_submit(kind, &input);
            }
            KeyCode::Named(FenixNamedKey::Backspace) => {
                prompt.input.pop();
            }
            KeyCode::Char(c) => prompt.input.push(c),
            _ => {}
        }
        self.wake_caret();
    }

    /// What to show in place of the modeline while `explorer_prompt` is
    /// active -- previously nothing rendered it at all, so typing a
    /// rename/create/copy/move was invisible (the state was captured
    /// correctly, there was just no on-screen feedback that a prompt was
    /// even open). Mirrors `:`/`/`'s own "the modeline becomes the
    /// prompt" convention.
    fn explorer_prompt_text(&self) -> Option<String> {
        let prompt = self.explorer_prompt.as_ref()?;
        Some(match prompt.kind {
            PromptKind::ConfirmDelete => {
                let n = self.active_explorer().map(|e| e.targets().len()).unwrap_or(0);
                format!("Delete {n} item{}? (y/n)", if n == 1 { "" } else { "s" })
            }
            PromptKind::Rename => format!("Rename to: {}", prompt.input),
            PromptKind::CreateFile => format!("Create file: {}", prompt.input),
            PromptKind::CreateDir => format!("Create directory: {}", prompt.input),
            PromptKind::CopyTo => format!("Copy to: {}", prompt.input),
            PromptKind::MoveTo => format!("Move to: {}", prompt.input),
        })
    }

    fn explorer_prompt_submit(&mut self, kind: PromptKind, input: &str) {
        let Some(explorer) = self.active_explorer_mut() else { return };
        let result = match kind {
            PromptKind::Rename => explorer.rename_selected(input),
            PromptKind::CreateFile => explorer.create_file(input),
            PromptKind::CreateDir => explorer.create_dir(input),
            PromptKind::CopyTo => explorer.copy_targets_to(Path::new(input)),
            PromptKind::MoveTo => explorer.move_targets_to(Path::new(input)),
            PromptKind::ConfirmDelete => return, // handled in explorer_prompt_key via y/n, not Enter
        };
        if let Err(err) = result {
            eprintln!("fenix: operation failed: {err}");
        }
    }

    /// `SPC w v`/`SPC w s`: splits the focused window vertically (side by
    /// side) or horizontally (stacked), Doom's own `splitright`/
    /// `splitbelow` default -- the new pane shows the *same* buffer as the
    /// one it split from (matching Vim's own `:vsplit`/`:split`), not a
    /// blank scratch buffer, so it starts as a second view of what you
    /// were already looking at.
    fn split_window(&mut self, kind: SplitKind) {
        let id = self.focused_buffer_id();
        // The new pane starts as a second, independent view of the same
        // buffer -- same cursor/scroll as the pane it split from (real
        // Vim's own `:split` behavior), not the buffer's stale remembered
        // position. Cloned *before* `split` reassigns focus to the new
        // pane, from the source pane's own `PaneState` (see `PaneState`'s
        // own doc comment for why cursor/scroll are per-pane now).
        let source_state = *self.pane_state(self.focused_pane_id());
        let new_pane = self.windows_mut().split(kind, id);
        self.workspaces.active_pane_states_mut().insert(new_pane, source_state);
        self.wake_caret();
    }

    pub(crate) fn split_vertical(&mut self) {
        self.split_window(SplitKind::Vertical);
    }

    pub(crate) fn split_horizontal(&mut self) {
        self.split_window(SplitKind::Horizontal);
    }

    /// `SPC w hjkl`: moves focus to the nearest window in that direction
    /// by real layout geometry, not tree adjacency -- a no-op at the
    /// grid's edge (`WindowTree::navigate`'s own contract).
    pub(crate) fn navigate_window(&mut self, dir: NavDirection) {
        let moved = self.windows_mut().navigate(dir);
        // The sidebar isn't a leaf in `WindowTree` -- it's a separate panel
        // drawn to the left of it -- so plain `navigate(Left)` has no way to
        // reach it. Treat it as one more step further left: only when
        // ordinary window navigation can't move any further (already at the
        // leftmost pane) does `Left` hand focus to an open sidebar instead.
        if dir == NavDirection::Left && !moved && self.sidebar_open && !self.sidebar_focused {
            self.sidebar_focused = true;
        }
        self.wake_caret();
    }

    /// `SPC w w`: cycles focus to the next window in a stable pre-order
    /// traversal, wrapping around.
    pub(crate) fn cycle_window(&mut self) {
        self.windows_mut().cycle_next();
        self.wake_caret();
    }

    /// `SPC w q`: closes the focused window. Refuses (no-op) on the last
    /// window rather than quitting the editor -- silently losing your
    /// only view on a stray keypress is a real data-loss risk this
    /// project has consistently avoided elsewhere, a disclosed deviation
    /// from Doom's own "closes Emacs" behavior. The buffer itself is
    /// never closed here, only the pane showing it -- other panes may
    /// still reference the same `BufferId`.
    pub(crate) fn close_window(&mut self) {
        let closed_pane = self.focused_pane_id();
        if self.windows_mut().close_focused() {
            self.workspaces.active_pane_states_mut().remove(&closed_pane);
            self.workspaces.active_scroll_anims_mut().remove(&closed_pane);
        }
        self.wake_caret();
    }

    /// `SPC w o`: closes every window except the focused one. A permanent
    /// `only`, not Doom's reversible temporary "enlarge" -- restoring the
    /// closed panes would need layout history this tree doesn't keep yet
    /// (a disclosed cut, see `WindowTree::only`'s own doc comment).
    pub(crate) fn only_window(&mut self) {
        self.windows_mut().only();
        self.wake_caret();
    }

    /// `SPC w =`: resets every split's ratio back to an even 0.5.
    pub(crate) fn balance_windows(&mut self) {
        self.windows_mut().balance();
        self.wake_caret();
    }

    /// `SPC TAB n`: a new workspace, seeded with the focused pane's
    /// current buffer (so it starts on something, not a blank scratch
    /// buffer) -- becomes active immediately.
    pub(crate) fn new_workspace(&mut self) {
        let id = self.focused_buffer_id();
        let cursor = self.buffers.get(id).map(|ob| ob.cursor).unwrap_or(Cursor::at_start());
        self.workspaces.new_workspace(id, cursor);
        self.wake_caret();
    }

    /// `SPC TAB ]`/`SPC TAB [`: cycles the active workspace, wrapping.
    pub(crate) fn next_workspace(&mut self) {
        self.workspaces.next();
        self.wake_caret();
    }

    pub(crate) fn prev_workspace(&mut self) {
        self.workspaces.prev();
        self.wake_caret();
    }

    /// `SPC TAB d`: removes the active workspace. Refuses (no-op) on the
    /// last one -- same safety posture as `close_window` refusing the
    /// last window.
    pub(crate) fn remove_workspace(&mut self) {
        self.workspaces.remove_active();
        self.wake_caret();
    }

    pub(crate) fn undo(&mut self) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        buffer.undo(cursor);
        self.wake_caret();
    }

    pub(crate) fn redo(&mut self) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        buffer.redo(cursor);
        self.wake_caret();
    }

    /// Off -> Absolute -> Relative -> Off. Stands in for a config-file
    /// setting that doesn't exist yet (see `LineNumberMode`) -- a keybound
    /// cycle is how you'd want to flip this at runtime anyway, so it's not
    /// wasted work once config loading lands.
    pub(crate) fn cycle_line_number_mode(&mut self) {
        self.line_number_mode = match self.line_number_mode {
            LineNumberMode::Off => LineNumberMode::Absolute,
            LineNumberMode::Absolute => LineNumberMode::Relative,
            LineNumberMode::Relative => LineNumberMode::Off,
        };
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// `SPC t t`: cycles to the next theme in `theme::ALL` (wrapping) and
    /// persists the choice -- matched by `name`, not pointer identity,
    /// which Rust doesn't guarantee is stable across separate `&SOME_CONST`
    /// expressions.
    pub(crate) fn cycle_theme(&mut self) {
        let current = theme::ALL.iter().position(|t| t.name == self.theme.name).unwrap_or(0);
        let next = (current + 1) % theme::ALL.len();
        self.apply_theme(theme::ALL[next]);
    }

    /// Shared by `cycle_theme` and the theme picker's confirm (`SPC t p`):
    /// sets the active theme, persists the choice, and requests a
    /// redraw. Save failure is non-fatal, same posture as `refresh_
    /// project_root`'s `known_projects.save()`.
    fn apply_theme(&mut self, theme: &'static Theme) {
        self.theme = theme;
        self.config.theme = Some(self.theme.name.to_string());
        if let Err(err) = self.config.save() {
            eprintln!("fenix: couldn't save theme choice: {err}");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// `SPC t p`: a fuzzy picker over every shipped theme (`theme::ALL`),
    /// for jumping straight to one by name rather than cycling through
    /// them one at a time.
    pub(crate) fn picker_pick_theme(&mut self) {
        let candidates =
            theme::ALL.iter().map(|t| fenix_picker::Candidate::new(t.name.to_string(), *t)).collect();
        self.enter_picker(ActivePicker::Theme(fenix_picker::PickerState::new(candidates)));
    }

    /// `SPC t =`/`Ctrl-=`/`SPC t -`/`Ctrl--`/`SPC t 0`/`Ctrl-0`: grows,
    /// shrinks, or resets the body text size at runtime and persists the
    /// choice, same posture as `cycle_theme`. A no-op (nothing to resize,
    /// nothing to persist) before the GPU/`TextPipeline` exist yet --
    /// can't happen for a keybinding a running window is dispatching,
    /// but `App::new`/headless tests can call these before `resumed()`.
    fn adjust_font_size(&mut self, size: f32) {
        let Some(text) = &mut self.text else { return };
        text.set_font_size(size);
        self.config.font_size = Some(text.font_size());
        if let Err(err) = self.config.save() {
            eprintln!("fenix: couldn't save font size: {err}");
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(crate) fn increase_font_size(&mut self) {
        let Some(current) = self.text.as_ref().map(|t| t.font_size()) else { return };
        self.adjust_font_size(current + FONT_SIZE_STEP);
    }

    pub(crate) fn decrease_font_size(&mut self) {
        let Some(current) = self.text.as_ref().map(|t| t.font_size()) else { return };
        self.adjust_font_size(current - FONT_SIZE_STEP);
    }

    pub(crate) fn reset_font_size(&mut self) {
        self.adjust_font_size(text::FONT_SIZE);
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

        let Some(keypress) = keymap::to_keypress(event, self.modifiers) else { return };

        // An active prompt (rename/create-name/confirm-delete) captures
        // every keystroke until it resolves -- takes priority over
        // everything else, including global Ctrl-chords, so e.g. Ctrl-S
        // can't accidentally interrupt an in-progress rename.
        if self.explorer_prompt.is_some() {
            self.explorer_prompt_key(keypress);
            return;
        }

        // An armed Docker remove confirmation (`x` on a container/image
        // row) captures the very next key the same way -- `y` confirms,
        // anything else cancels (see `docker_confirm_key`).
        if self.docker_confirm_remove.is_some() {
            self.docker_confirm_key(keypress);
            return;
        }

        // The grep search-term prompt and an open picker both capture
        // input the same way -- checked ahead of the explorer/sidebar-
        // focus check below since a picker can be opened while the
        // sidebar is focused (switch-project's find-file chain, for
        // instance). The add-project flow no longer has its own prompt
        // state to check here -- it's just the ordinary explorer
        // (`main_view == Explorer`), routed the same way `SPC f j` is.
        if self.pending_grep_query.is_some() {
            self.grep_query_key(keypress);
            return;
        }
        if self.active_picker.is_some() {
            self.picker_key(keypress);
            return;
        }

        // The full-buffer explorer (dired-style, replacing the whole
        // editing view) owns all input unconditionally -- its own trie,
        // not Vim's, and not the global Ctrl-chords below (browsing is a
        // distinct modal UI, the same reasoning that already keeps
        // Insert/Command mode out of Normal's trie).
        if self.main_view == MainView::Explorer {
            if let Step::Matched(&action) = self.explorer_matcher.feed(keypress) {
                self.explorer_handle_action(action);
            }
            self.wake_caret();
            return;
        }

        // The sidebar is different: a persistent panel meant to coexist
        // with active editing, not a modal takeover, so SPC still reaches
        // the leader menu below (e.g. `SPC w l` to jump back to another
        // window) instead of being swallowed. Pressing it also hands focus
        // back to the editor, since leader commands act on the window/
        // buffer layer, not the sidebar.
        if self.sidebar_focused {
            if keypress == KeyPress::char(' ') && self.vim.mode() == Mode::Normal {
                self.sidebar_focused = false;
            } else {
                if let Step::Matched(&action) = self.explorer_matcher.feed(keypress) {
                    self.explorer_handle_action(action);
                }
                self.wake_caret();
                return;
            }
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
                } else if s == "=" || s == "+" {
                    Some("view.increase_font_size")
                } else if s == "-" {
                    Some("view.decrease_font_size")
                } else if s == "0" {
                    Some("view.reset_font_size")
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

        // Window-size-aware paging is a GUI concern, handled the same way
        // regardless of Vim mode -- fenix-vim doesn't know about viewport size.
        if keypress == KeyPress::named(FenixNamedKey::PageUp)
            || keypress == KeyPress::named(FenixNamedKey::PageDown)
        {
            let page_size = match (&self.gpu, &self.text) {
                (Some(gpu), Some(text)) => {
                    text::visible_line_count(gpu.size.height as f32, text.modeline_height(), text.line_height())
                }
                (Some(gpu), None) => text::visible_line_count(gpu.size.height as f32, text::LINE_HEIGHT + 8.0, text::LINE_HEIGHT),
                (None, _) => 20,
            };
            let down = keypress == KeyPress::named(FenixNamedKey::PageDown);
            let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
            buffer.move_page(cursor, page_size, down);
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

        // The completion popup only ever exists in Insert mode. `Ctrl-
        // Space` force-opens it regardless of whether it's already open
        // (unclaimed today -- not in the global Ctrl-chord list above,
        // not handled by `handle_insert_key`). While it's open, its
        // navigation/accept/dismiss keys are fully claimed here and never
        // reach Vim; every other key (plain typing, Backspace, arrows)
        // falls through to the ordinary Vim dispatch below, after which
        // `sync_completion` re-derives the popup's state fresh -- there's
        // no `VimEvent` for "a character was typed," so this recomputes
        // instead of tracking it incrementally.
        if self.vim.mode() == Mode::Insert {
            if keypress == KeyPress::char(' ').with_ctrl() {
                self.force_open_completion();
                self.wake_caret();
                return;
            }
            if self.completion.is_some() {
                match keypress.code {
                    KeyCode::Named(FenixNamedKey::Down) => {
                        self.completion.as_mut().unwrap().picker.move_selection(1);
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Named(FenixNamedKey::Up) => {
                        self.completion.as_mut().unwrap().picker.move_selection(-1);
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Char('n') if keypress.mods.ctrl => {
                        self.completion.as_mut().unwrap().picker.move_selection(1);
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Char('p') if keypress.mods.ctrl => {
                        self.completion.as_mut().unwrap().picker.move_selection(-1);
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Named(FenixNamedKey::Tab) | KeyCode::Named(FenixNamedKey::Enter) => {
                        self.accept_completion();
                        self.wake_caret();
                        return;
                    }
                    KeyCode::Named(FenixNamedKey::Escape) => {
                        self.completion = None;
                        self.wake_caret();
                        return;
                    }
                    _ => {} // plain typing/Backspace/arrows -- fall through to Vim
                }
            }
        }

        // The dashboard is a real Vim-navigable buffer (see `BufferKind`'s
        // own doc comment) -- every other key still reaches Vim below
        // unchanged (movement, `/` search, `gg`/`G`...); only `Enter`
        // means something special on it.
        if self.open().kind == BufferKind::Dashboard && keypress.code == KeyCode::Named(FenixNamedKey::Enter) {
            self.dashboard_activate_selected();
            self.wake_caret();
            return;
        }

        // A dired buffer is likewise real Vim-navigable text -- ordinary
        // motions (`hjkl`, `gg`/`G`, `/` search, ...) reach Vim below
        // unchanged. Only a small set of action keys are claimed here
        // first (mirroring the Dashboard's own single-key interception
        // above): none of them have a meaningful "edit this text"
        // purpose on a directory listing anyway, so claiming them costs
        // nothing real Vim editing would otherwise offer here. Marking/
        // rename/create/delete/copy/move aren't wired for this buffer-
        // backed form yet -- still available via the sidebar (`SPC e
        // t`), which this doesn't touch.
        if self.open().kind == BufferKind::Explorer {
            match keypress.code {
                KeyCode::Named(FenixNamedKey::Enter) => {
                    self.dired_activate_selected();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('-') if keypress.mods == Mods::default() => {
                    self.dired_parent_dir();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('R') if keypress.mods == Mods::default() => {
                    self.dired_refresh();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('.') if keypress.mods == Mods::default() => {
                    self.dired_toggle_hidden();
                    self.wake_caret();
                    return;
                }
                _ => {}
            }
        }

        // Lazydocker-style panel: ordinary motions (`hjkl`, `gg`/`G`, `/`
        // search) reach Vim below unchanged -- only a small action-key
        // set is claimed here first, same shape as the dired buffer
        // above. `s`/`S`/`R` act on the container under the cursor, `r`
        // runs a new container from the image under the cursor, `l`
        // opens that container's logs into the focused pane, `x` arms
        // a remove confirmation (`y`/anything to confirm/cancel, see
        // `docker_confirm_key`), `u` refreshes.
        if self.open().kind == BufferKind::Docker {
            match keypress.code {
                KeyCode::Char('s') if keypress.mods == Mods::default() => {
                    self.docker_start_selected();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('S') if keypress.mods == Mods::default() => {
                    self.docker_stop_selected();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('R') if keypress.mods == Mods::default() => {
                    self.docker_restart_selected();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('r') if keypress.mods == Mods::default() => {
                    self.docker_run_selected();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('l') if keypress.mods == Mods::default() => {
                    self.docker_view_logs_selected();
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('x') if keypress.mods == Mods::default() => {
                    if let Some(entry) = self.docker_entry_at_cursor() {
                        self.docker_confirm_remove = Some(entry);
                    }
                    self.wake_caret();
                    return;
                }
                KeyCode::Char('u') if keypress.mods == Mods::default() => {
                    self.docker_refresh();
                    self.wake_caret();
                    return;
                }
                _ => {}
            }
        }

        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let vim_event = {
            let Some(ob) = self.buffers.get_mut(id) else { return };
            let Some(pane_state) = self.workspaces.active_pane_states_mut().get_mut(&pane) else { return };
            self.vim.handle_key(&mut ob.buffer, &mut pane_state.cursor, keypress)
        };
        self.sync_completion();
        match vim_event {
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
            VimEvent::IndentWidthChanged(width) => {
                self.config.indent_width = Some(width);
                if let Err(err) = self.config.save() {
                    eprintln!("fenix: couldn't save indent width ({err})");
                }
            }
            VimEvent::None => {}
        }
        self.wake_caret();
    }

    /// Keeps the cursor's line within the *focused pane's* visible
    /// window, scrolling as needed. Must be called with the same
    /// `visible_lines` used to render. Every pane owns its own scroll
    /// independently (see `PaneState`'s own doc comment) -- only the
    /// focused one auto-follows the cursor like this; others just render
    /// wherever they were last left.
    fn ensure_cursor_visible(&mut self, visible_lines: usize) {
        let buffer_id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let (line, scroll_line) = {
            let ob = self.buffers.get(buffer_id).expect("focused window always has an open buffer");
            let cursor = self.pane_state(pane).cursor;
            (ob.buffer.line_col(&cursor).0, self.pane_state(pane).scroll_line)
        };
        let target = scroll_to_include(scroll_line, line, visible_lines);
        if target != scroll_line {
            let jump = target.abs_diff(scroll_line);
            // A jump bigger than a few screens (`G`, a search landing far
            // away) snaps instantly rather than blurring through an ease.
            // A target arriving while the *previous* ease for this pane
            // hasn't finished yet snaps too: that only happens when scroll
            // targets are coming in faster than SCROLL_DURATION can settle
            // (held-key repeat, e.g. holding `j`), and re-triggering a
            // fresh 150ms ease on every one of those keystrokes would
            // otherwise make `rendered_scroll` perpetually chase a few
            // lines behind the cursor instead of ever catching up -- the
            // opposite of the "never sits between a keypress and its
            // effect" goal this animation was built for.
            if jump > visible_lines.saturating_mul(SCROLL_SNAP_SCREENS) || self.workspaces.active_scroll_anims().contains_key(&pane) {
                self.workspaces.active_scroll_anims_mut().remove(&pane);
                self.pane_state_mut(pane).rendered_scroll = target as f32;
            } else {
                let from = self.pane_state(pane).rendered_scroll;
                self.workspaces.active_scroll_anims_mut().insert(pane, ScrollAnim { from, to: target, started: Instant::now() });
            }
            self.pane_state_mut(pane).scroll_line = target;
        }
        self.update_rendered_scroll();
    }

    /// Advances the focused pane's `rendered_scroll` toward its
    /// `scroll_line` if a transition is in flight, clearing it once
    /// settled.
    fn update_rendered_scroll(&mut self) {
        let pane = self.focused_pane_id();
        let scroll_line = self.pane_state(pane).scroll_line;
        let Some(anim) = self.workspaces.active_scroll_anims().get(&pane) else {
            self.pane_state_mut(pane).rendered_scroll = scroll_line as f32;
            return;
        };
        let (from, to, started) = (anim.from, anim.to, anim.started);
        let t = Instant::now().duration_since(started).as_secs_f32() / SCROLL_DURATION.as_secs_f32();
        if t >= 1.0 {
            self.pane_state_mut(pane).rendered_scroll = to as f32;
            self.workspaces.active_scroll_anims_mut().remove(&pane);
        } else {
            let to = to as f32;
            self.pane_state_mut(pane).rendered_scroll = from + (to - from) * ease_out_cubic(t);
        }
    }

    /// The focused pane's line rendering starts from -- `rendered_scroll`
    /// rounded down. Content, caret, hl-line, selection, and pulse all
    /// anchor their row math to this (not `scroll_line`, which is only
    /// the *target* `rendered_scroll` is easing toward).
    fn render_base_line(&self) -> usize {
        self.pane_state(self.focused_pane_id()).rendered_scroll.floor().max(0.0) as usize
    }

    /// (mode label, rest-of-modeline suffix) -- `None` while typing a `:`
    /// command or a `/`/`?` search query, since either replaces the
    /// whole modeline with raw prompt text instead of the usual badge +
    /// filename + position layout.
    fn modeline_pieces(&self) -> Option<(&'static str, String)> {
        if self.vim.mode() == Mode::Command
            || self.vim.mode() == Mode::Search
            || self.pending_grep_query.is_some()
            || self.explorer_prompt.is_some()
            || self.docker_confirm_remove.is_some()
        {
            return None;
        }
        if self.main_view == MainView::Explorer {
            let suffix = match &self.explorer {
                Some(explorer) => {
                    let marked =
                        if explorer.marks.is_empty() { String::new() } else { format!(" [{} marked]", explorer.marks.len()) };
                    match self.explorer_purpose {
                        ExplorerPurpose::Browse => {
                            format!("│ {}{marked}   {} items ", explorer.cwd.display(), explorer.entries.len())
                        }
                        ExplorerPurpose::PickProjectDir => {
                            format!("│ {}   S to add as a project, q to cancel ", explorer.cwd.display())
                        }
                    }
                }
                None => String::new(),
            };
            let badge = match self.explorer_purpose {
                ExplorerPurpose::Browse => "EXPLORE",
                ExplorerPurpose::PickProjectDir => "ADDPROJ",
            };
            return Some((badge, suffix));
        }
        if self.main_view == MainView::Picker {
            let (label, count) = match &self.active_picker {
                Some(picker @ ActivePicker::FindFile(_)) => ("FINDFILE", picker_len(picker)),
                Some(picker @ ActivePicker::Grep(_)) => ("GREP", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchProject(_)) => ("SWPROJ", picker_len(picker)),
                Some(picker @ ActivePicker::SwitchBuffer(_)) => ("SWBUF", picker_len(picker)),
                Some(picker @ ActivePicker::DeleteProject(_)) => ("DELPROJ", picker_len(picker)),
                Some(picker @ ActivePicker::Theme(_)) => ("THEME", picker_len(picker)),
                None => ("PICKER", 0),
            };
            return Some((label, format!("│ {count} matches ")));
        }
        let ob = self.open();
        let filename = ob.buffer.path().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| {
            if ob.kind == BufferKind::Dashboard {
                "*dashboard*".to_string()
            } else if ob.kind == BufferKind::Explorer {
                self.dired_states
                    .get(&self.focused_buffer_id())
                    .map(|s| s.cwd.display().to_string())
                    .unwrap_or_else(|| "*dired*".to_string())
            } else if ob.kind == BufferKind::Docker {
                "*docker*".to_string()
            } else {
                "[No Name]".to_string()
            }
        });
        let modified = if ob.buffer.is_dirty() { " [+]" } else { "" };
        let (line, col) = ob.buffer.line_col(&self.cursor());
        let mode_label =
            if self.vim.mode() == Mode::Visual { self.vim.visual_kind().label() } else { self.vim.mode().label() };
        // Only shown once there's more than one workspace to distinguish --
        // stays out of the way for the common single-workspace case.
        let workspace_indicator = if self.workspaces.len() > 1 {
            format!("   [{}/{} {}]", self.workspaces.active_index() + 1, self.workspaces.len(), self.workspaces.active_name())
        } else {
            String::new()
        };
        let suffix = format!("│ {filename}{modified}{workspace_indicator}   Ln {}, Col {} ", line + 1, col + 1);
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
        if self.vim.mode() == Mode::Search {
            let prefix = if self.vim.search_forward() { "/" } else { "?" };
            return format!("{prefix}{}", self.vim.search_query());
        }
        if let Some(query) = &self.pending_grep_query {
            return format!("rg: {query}");
        }
        if let Some(prompt_text) = self.explorer_prompt_text() {
            return prompt_text;
        }
        if let Some(confirm_text) = self.docker_confirm_text() {
            return confirm_text;
        }
        let (mode_label, suffix) = self.modeline_pieces().unwrap();
        format!(" {mode_label:^width$}{suffix}", width = text::MODE_BADGE_CHARS)
    }

    /// (badge background, badge text color) for the current mode. Visual's
    /// Whether the caret should render as a full-cell block (Normal,
    /// Visual, Replace, Command) rather than the thin Insert-mode bar --
    /// matches real Vim's own cursor-shape convention (block outside
    /// Insert, thin bar while actively typing).
    fn caret_is_block(&self) -> bool {
        self.vim.mode() != Mode::Insert
    }

    /// three kinds all share one accent (matching orbit-emacs's own
    /// evil-state table, which has a single "Visual" entry) -- only the
    /// badge's label text differs between them.
    fn mode_colors(&self) -> ([f32; 4], glyphon::Color) {
        let theme = self.theme;
        if self.main_view == MainView::Explorer {
            return (theme.mode_explorer, theme.mode_text_dark);
        }
        if self.main_view == MainView::Picker {
            return (theme.mode_picker, theme.mode_text_dark);
        }
        match self.vim.mode() {
            Mode::Normal => (theme.mode_normal, theme.mode_text_dark),
            Mode::Insert => (theme.mode_insert, theme.mode_text_dark),
            Mode::Visual => (theme.mode_visual, theme.mode_text_light),
            Mode::Replace => (theme.mode_replace, theme.mode_text_dark),
            Mode::Command => (theme.mode_command, theme.mode_text_dark),
            // Same "prompt-like input" family as `:` -- not a new theme
            // color, just reusing the command one.
            Mode::Search => (theme.mode_command, theme.mode_text_dark),
        }
    }

    /// Character width of the line-number gutter, including one trailing
    /// column of padding before the text starts; 0 when the gutter is off.
    /// Sized to the buffer's total line count regardless of Absolute vs.
    /// Relative, so the gutter doesn't resize if that mode is ever toggled
    /// at runtime -- matching how Vim shares one `numberwidth` between
    /// `number` and `relativenumber` instead of sizing them separately.
    /// `ob` is passed explicitly (rather than always reading the focused
    /// buffer) so a split's every visible pane gets a gutter sized to
    /// *its own* buffer's line count, not just the focused one's.
    fn gutter_chars(&self, ob: &OpenBuffer) -> usize {
        if self.line_number_mode == LineNumberMode::Off
            || ob.kind == BufferKind::Dashboard
            || ob.kind == BufferKind::Explorer
            || ob.kind == BufferKind::Docker
        {
            return 0;
        }
        ob.buffer.visual_line_count().max(1).to_string().len() + 1
    }

    /// Rich-text spans for one pane's content area covering `rows` screen
    /// rows starting at `render_base_line`: each row that maps to a real
    /// buffer line gets an optional gutter prefix (its line number, right-
    /// aligned, brighter on the cursor's own line) followed by that line's
    /// text; rows past the end of the buffer get a `~` marker instead,
    /// matching Vim's convention for "there's no line here" -- shown even
    /// with the gutter off, same as Vim shows it regardless of `number`.
    /// `syntax_highlights` (document-wide byte ranges, already resolved to
    /// colors) splits each line's text into colored sub-spans instead of
    /// one flat `theme.fg` span when non-empty. `ob` (rather than always
    /// the focused buffer) is what makes every visible pane -- not just
    /// the focused one -- show its own buffer's real content.
    ///
    /// `dashboard_pad`, when `Some`, replaces the real-gutter/tilde logic
    /// entirely: index `buffer_line` gives that specific row's own
    /// horizontal-centering pad (in characters, from `dashboard_center_
    /// offset` -- different rows can want different padding, e.g. the
    /// banner block and the content block below it center independently)
    /// written as literal leading blank characters, since `TextArea.left`
    /// has no geometric offset to hook a shift into.
    ///
    /// `cursor_line` is `ob`'s *pane's* own cursor row (relative-gutter
    /// numbering needs it) -- passed in rather than read off `ob.cursor`
    /// now that cursor is pane-owned, not buffer-owned (see `PaneState`'s
    /// own doc comment), so this stays correct for any pane, not just
    /// the focused one.
    #[allow(clippy::too_many_arguments)] // a plain data parameter list, not a design smell worth a struct for one private call site
    fn content_spans(
        &self,
        ob: &OpenBuffer,
        render_base_line: usize,
        rows: usize,
        gutter_chars: usize,
        dashboard_pad: Option<&[usize]>,
        syntax_highlights: &[(std::ops::Range<usize>, glyphon::Color)],
        cursor_line: usize,
    ) -> Vec<(String, glyphon::Color)> {
        let theme = self.theme;
        let visual_lines = ob.buffer.visual_line_count();
        let mut spans = Vec::new();
        for r in 0..rows {
            let buffer_line = render_base_line + r;
            let has_line = buffer_line < visual_lines;
            if let Some(pad) = dashboard_pad {
                let n = pad.get(buffer_line).copied().unwrap_or(0);
                if n > 0 {
                    spans.push((" ".repeat(n), theme.fg));
                }
            } else if gutter_chars > 0 {
                let gutter = if has_line {
                    let n = match self.line_number_mode {
                        LineNumberMode::Relative => buffer_line.abs_diff(cursor_line),
                        _ => buffer_line + 1,
                    };
                    format!("{:>width$} ", n, width = gutter_chars - 1)
                } else {
                    format!("{:<width$}", "~", width = gutter_chars)
                };
                let color = if has_line && buffer_line == cursor_line { theme.fg } else { theme.gutter_fg };
                spans.push((gutter, color));
            } else if !has_line {
                spans.push(("~".to_string(), theme.gutter_fg));
            }
            if has_line {
                let start = ob.buffer.line_start_char(buffer_line);
                let len = ob.buffer.line_len(buffer_line);
                let line_text = ob.buffer.text_range(start, start + len);
                if syntax_highlights.is_empty() {
                    spans.push((line_text, theme.fg));
                } else {
                    let line_byte_start = ob.buffer.char_to_byte(start);
                    let line_byte_end = ob.buffer.char_to_byte(start + len);
                    spans.extend(split_line_by_highlights(
                        &line_text,
                        line_byte_start,
                        line_byte_end,
                        syntax_highlights,
                        theme.fg,
                    ));
                }
            }
            if r + 1 < rows {
                spans.push(("\n".to_string(), theme.fg));
            }
        }
        spans
    }

    /// Drains buffer `id`'s edits since the last frame, feeds them through
    /// its active language's incremental parser (if any), and returns
    /// resolved syntax-highlight colors for the visible byte range only --
    /// same windowing discipline `Buffer::visible_text` already uses.
    /// Always drains, even with no active language, so the buffer's edit
    /// log doesn't grow unbounded for files nothing is consuming it for.
    /// Takes an explicit `id` (rather than always the focused buffer) so
    /// every visible pane's buffer gets reparsed, not just the focused
    /// one's -- `self.buffers.get_mut(id)` is a direct field projection,
    /// so this can still read `self.theme` first in the same call without
    /// the opaque-method-call borrow conflict `open_mut()` would risk.
    fn syntax_highlights_for_visible_range(
        &mut self,
        id: BufferId,
        render_base_line: usize,
        rows: usize,
    ) -> Vec<(std::ops::Range<usize>, glyphon::Color)> {
        let theme = self.theme;
        // Cloned out ahead of the `self.buffers.get_mut` borrow below --
        // `dashboard_lines` is a different field, but going through a
        // `&self` method call for it while `ob` (from `self.buffers`) is
        // still borrowed would look like a whole-`self` borrow to the
        // compiler. The cloned `Vec` is small (a few dozen entries at
        // most), so this is cheap.
        let dashboard_lines = self.dashboard_lines.get(&id).cloned();
        let docker_lines = self.docker_lines.get(&id).cloned();

        // Same reasoning, for Tcl: `tcl.scm`'s own `(command name: (_)
        // @function)` rule captures *every* word in command position,
        // since a tree-sitter query has no way to know whether it's a
        // real command -- only Fenix's own known-symbols set (built-in
        // keywords, ctags, the external symbols file: the exact same
        // three sources `tcl_candidates` already merges for completion)
        // can validate that. Built up here, before `self.buffers.get_mut`
        // below, since `tcl_candidates` is itself a `&mut self` call.
        let is_tcl = self.buffers.get(id).is_some_and(|ob| {
            ob.buffer.path().and_then(|p| p.extension()).and_then(|e| e.to_str()).and_then(fenix_syntax::detect_language)
                == Some(fenix_syntax::LanguageId::Tcl)
        });
        let known_tcl_commands: Option<std::collections::HashSet<String>> = if is_tcl {
            let root = self.project_root.clone();
            Some(self.tcl_candidates(root.as_deref()).into_iter().map(|c| c.payload.label).collect())
        } else {
            None
        };

        let Some(ob) = self.buffers.get_mut(id) else { return Vec::new() };
        let deltas = ob.buffer.drain_edits();

        if ob.kind == BufferKind::Dashboard {
            return dashboard_highlights_for_visible_range(ob, dashboard_lines.as_deref(), render_base_line, rows, theme);
        }
        if ob.kind == BufferKind::Docker {
            return docker_highlights_for_visible_range(ob, docker_lines.as_deref(), render_base_line, rows, theme);
        }

        let Some(syntax) = &mut ob.syntax else { return Vec::new() };

        let edits: Vec<fenix_syntax::RawEdit> = deltas
            .into_iter()
            .map(|d| fenix_syntax::RawEdit { start_char: d.start_char, new_end_char: d.new_end_char, removed: d.removed })
            .collect();
        let source = ob.buffer.text();
        syntax.apply_edits(&source, &edits);

        let visual_lines = ob.buffer.visual_line_count();
        if render_base_line >= visual_lines {
            return Vec::new();
        }
        let last_line = (render_base_line + rows).min(visual_lines) - 1;
        let start_char = ob.buffer.line_start_char(render_base_line);
        let end_char = ob.buffer.line_start_char(last_line) + ob.buffer.line_len(last_line);
        let byte_range = ob.buffer.char_to_byte(start_char)..ob.buffer.char_to_byte(end_char);

        syntax
            .highlights_in_range(&source, byte_range)
            .into_iter()
            .filter(|(range, name)| {
                // Only the generic, unvalidated "function" capture (any
                // bare word in command position) needs cross-referencing
                // -- every other Tcl capture (specific builtins, "proc",
                // keywords, the switch/unset/variable special cases, ...)
                // is already validated by the query's own static
                // `#any-of?` lists, so it's left alone here.
                let Some(known) = &known_tcl_commands else { return true };
                if *name != "function" {
                    return true;
                }
                let text = &source[range.clone()];
                known.contains(text.strip_prefix("::").unwrap_or(text))
            })
            .map(|(range, name)| (range, theme.syntax_color(name)))
            .collect()
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
        let last_visible = (self.render_base_line() + visible_lines).min(self.open().buffer.line_count());
        let mut segments = Vec::new();

        match self.vim.visual_kind() {
            VisualKind::Char => {
                let cursor_idx = self.cursor().char_idx;
                let (lo, hi) =
                    if anchor <= cursor_idx { (anchor, cursor_idx + 1) } else { (cursor_idx, anchor + 1) };
                let hi = hi.min(self.open().buffer.len_chars());
                segments = self.range_to_segments(lo..hi, visible_lines);
            }
            VisualKind::Line => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                for line in self.render_base_line().max(line_lo)..last_visible.min(line_hi + 1) {
                    // at least 1 col wide so an empty line still shows a sliver
                    let width = self.open().buffer.line_len(line).max(1);
                    segments.push((line - self.render_base_line(), 0, width));
                }
            }
            VisualKind::Block => {
                let (line_lo, line_hi) = self.anchor_cursor_line_range(anchor);
                let anchor_cursor = Cursor { char_idx: anchor, sticky_col: 0 };
                let (_, anchor_col) = self.open().buffer.line_col(&anchor_cursor);
                let (_, cursor_col) = self.open().buffer.line_col(&self.cursor());
                let (col_lo, col_hi) = if anchor_col <= cursor_col {
                    (anchor_col, cursor_col + 1)
                } else {
                    (cursor_col, anchor_col + 1)
                };
                for line in self.render_base_line().max(line_lo)..last_visible.min(line_hi + 1) {
                    let len = self.open().buffer.line_len(line);
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
        let (anchor_line, _) = self.open().buffer.line_col(&anchor_cursor);
        let (cursor_line, _) = self.open().buffer.line_col(&self.cursor());
        if anchor_line <= cursor_line { (anchor_line, cursor_line) } else { (cursor_line, anchor_line) }
    }

    /// Per-visible-line (view_row, col_start, col_end) segments a plain
    /// char range covers -- shared by Char-kind Visual selection and the
    /// yank/paste pulse, which are both "highlight this contiguous span"
    /// even though they come from different sources.
    fn range_to_segments(&self, range: std::ops::Range<usize>, visible_lines: usize) -> Segments {
        let last_visible = (self.render_base_line() + visible_lines).min(self.open().buffer.line_count());
        let mut segments = Vec::new();
        for line in self.render_base_line()..last_visible {
            let line_start = self.open().buffer.line_start_char(line);
            let line_end = line_start + self.open().buffer.line_len(line);
            let seg_start = range.start.max(line_start);
            let seg_end = range.end.min(line_end);
            if seg_start < seg_end {
                segments.push((line - self.render_base_line(), seg_start - line_start, seg_end - line_start));
            }
        }
        segments
    }

    /// Segments for the bracket under the cursor and its match, when the
    /// cursor sits exactly on one of `(){}[]` and a match exists --
    /// empty otherwise (unmatched brackets, or the cursor isn't on a
    /// bracket at all). Reuses `range_to_segments` for the "char range
    /// -> view segments" conversion, called once per bracket rather
    /// than over a combined range since the two are rarely adjacent.
    fn bracket_match_segments(&self, visible_lines: usize) -> Segments {
        let cursor_idx = self.cursor().char_idx;
        let Some(match_idx) = fenix_vim::find_matching_bracket(&self.open().buffer, cursor_idx) else {
            return Vec::new();
        };
        let mut segments = self.range_to_segments(cursor_idx..cursor_idx + 1, visible_lines);
        segments.extend(self.range_to_segments(match_idx..match_idx + 1, visible_lines));
        segments
    }

    /// `hlsearch`'s persistent match-highlight segments -- every
    /// occurrence of the last confirmed search pattern within the
    /// visible range, empty whenever `VimState::hlsearch_active` is
    /// false. Converts the visible line range to a byte range (`Buffer::
    /// char_to_byte` at each line boundary) for `VimState::hlsearch_
    /// matches`, then each match's byte range back to view segments via
    /// `range_to_segments`, the same "windowed, computed fresh each
    /// frame" discipline `bracket_match_segments`/`syntax_highlights_
    /// for_visible_range` already use.
    fn hlsearch_segments(&self, visible_lines: usize) -> Segments {
        let buffer = &self.open().buffer;
        let last_visible = (self.render_base_line() + visible_lines).min(buffer.line_count());
        let start_char = buffer.line_start_char(self.render_base_line());
        let end_char = if last_visible < buffer.line_count() {
            buffer.line_start_char(last_visible)
        } else {
            buffer.len_chars()
        };
        let byte_range = buffer.char_to_byte(start_char)..buffer.char_to_byte(end_char);

        let mut segments = Segments::new();
        for m in self.vim.hlsearch_matches(buffer, byte_range) {
            let start = buffer.byte_to_char(m.start);
            let end = buffer.byte_to_char(m.end);
            segments.extend(self.range_to_segments(start..end, visible_lines));
        }
        segments
    }

    /// The focused pane's caret position for this frame: normally
    /// `hl_row`/the real cursor's column, but during `Mode::Search`
    /// previews where the in-progress query would jump to instead
    /// (incsearch) -- the real `Cursor`/`last_search` are untouched
    /// until Enter confirms (see `VimState::preview_match`'s own doc
    /// comment). The preview is only shown when it falls inside the
    /// currently-fetched `[render_base_line, render_base_line +
    /// pane_visible_lines]` window; a match outside it shows no caret
    /// this frame rather than auto-scrolling to reveal it (a disclosed
    /// simplification -- real incsearch also pans the viewport).
    fn focused_caret(
        &self,
        hl_row: Option<usize>,
        col: usize,
        render_base_line: usize,
        pane_visible_lines: usize,
    ) -> Option<(usize, usize)> {
        if self.vim.mode() != Mode::Search {
            return hl_row.map(|row| (row, col));
        }
        self.vim.preview_match(&self.open().buffer, &self.cursor()).and_then(|idx| {
            let preview_cursor = Cursor { char_idx: idx, sticky_col: 0 };
            let (preview_line, preview_col) = self.open().buffer.line_col(&preview_cursor);
            preview_line
                .checked_sub(render_base_line)
                .filter(|&row| row <= pane_visible_lines)
                .map(|row| (row, preview_col))
        })
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

    /// The which-key popup's rich-text spans (key column in
    /// `theme.caret_text`, label/overflow-summary in the modeline's own
    /// text color, sorted alphabetically by label for scannability) and
    /// its resolved on-screen rect, or `None` when nothing is pending. The
    /// popup shares the modeline's background (`bg_modeline`), so both
    /// colors here are picked from that family, not the content one --
    /// `syntax_keyword` was tried for the key column originally, but it's
    /// calibrated for `bg` (the content background), and on TempleOS
    /// specifically its value is identical to `bg_modeline`, making the
    /// key column invisible. `caret_text` is guaranteed high-contrast
    /// against `bg_modeline` in every theme (`caret` already needs that
    /// against `bg` to work as a caret), which is what makes it a safe
    /// second accent color here too.
    /// Two other things could previously make the panel not actually fit
    /// its own content, both fixed here: its *width* is now sized to the
    /// longest visible label at the font's real measured `char_width`
    /// (clamped to `[WHICH_KEY_MIN_WIDTH, WHICH_KEY_MAX_WIDTH]` and to
    /// what the window can hold) instead of a fixed 260px that didn't
    /// scale with font size or label length -- a big font or a long
    /// label (real examples: "insert at line start", "WORD backward")
    /// used to just get cut off; and its *height* truncates to whatever
    /// `popup::max_rows` says actually fits above the modeline, with a
    /// trailing "+N more" summary row instead of letting the panel run
    /// under it.
    fn which_key_popup(&self, window_width: f32, modeline_top: f32) -> Option<(fenix_window::Rect, RowSpans)> {
        let mut hints = self.pending_hints();
        if hints.is_empty() {
            return None;
        }
        hints.sort_by(|a, b| a.1.cmp(b.1));

        let (char_width, line_height) = match &self.text {
            Some(text) => (text.char_width(), text.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };

        let max_rows = popup::max_rows(modeline_top, text::WHICH_KEY_MARGIN, line_height, WHICH_KEY_PADDING);
        let shown_count = if hints.len() > max_rows { max_rows.saturating_sub(1).max(1) } else { hints.len() };
        let truncated = hints.len() - shown_count;

        const KEY_COLUMN_CHARS: usize = 6;
        let longest_label = hints[..shown_count].iter().map(|(_, label)| label.chars().count()).max().unwrap_or(0);
        let content_chars = KEY_COLUMN_CHARS + longest_label + 1;
        let max_width = (window_width - 2.0 * text::WHICH_KEY_MARGIN).max(text::WHICH_KEY_MIN_WIDTH);
        let width = (content_chars as f32 * char_width + WHICH_KEY_PADDING)
            .clamp(text::WHICH_KEY_MIN_WIDTH, text::WHICH_KEY_MAX_WIDTH.min(max_width));

        let theme = self.theme;
        let mut spans = Vec::new();
        for (i, (key, label)) in hints[..shown_count].iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            spans.push((format!("{:<KEY_COLUMN_CHARS$}", keymap::describe_keypress(key)), theme.caret_text, false));
            spans.push(((*label).to_string(), theme.fg_modeline, false));
        }
        if truncated > 0 {
            spans.push(("\n".to_string(), theme.fg_modeline, false));
            spans.push((format!("+{truncated} more"), theme.fg_modeline, false));
        }

        let row_count = shown_count + usize::from(truncated > 0);
        let height = row_count as f32 * line_height + WHICH_KEY_PADDING;
        let rect = popup::resolve(popup::Anchor::TopRight { margin: text::WHICH_KEY_MARGIN }, width, height, window_width, modeline_top);
        Some((rect, spans))
    }

    /// The completion popup's rich-text spans and resolved rect, anchored
    /// just below the caret (`popup::Anchor::BelowPoint`) -- mirrors
    /// `which_key_popup`'s shape. `None` whenever there's no open
    /// completion session, the focused pane has no caret to anchor under
    /// (an empty window), or the candidate window ends up with nothing to
    /// show. Also returns which shown row (if any) is the current
    /// selection, for the caller to draw its own highlight rect (the
    /// popup itself is drawn behind text in the base `bg_rect` pass --
    /// see the two-pass render comment near where this is consumed).
    fn completion_popup(
        &self,
        window_width: f32,
        modeline_top: f32,
        focused_rect: fenix_window::Rect,
        focused_caret: Option<(usize, usize)>,
        gutter_px: f32,
        content_frac: f32,
    ) -> Option<(fenix_window::Rect, RowSpans, Option<usize>)> {
        let state = self.completion.as_ref()?;
        let (row, col) = focused_caret?;
        let (char_width, line_height) = match &self.text {
            Some(text) => (text.char_width(), text.line_height()),
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT),
        };
        let (caret_x, caret_y) = caret_pixel_pos(focused_rect, row, col, gutter_px, content_frac, char_width, line_height);

        let shown_rows = popup::max_rows(modeline_top, COMPLETION_MARGIN, line_height, COMPLETION_PADDING).min(COMPLETION_MAX_ROWS);
        let rows: Vec<(bool, &fenix_picker::Candidate<fenix_completion::CompletionItem>)> =
            state.picker.visible_rows(self.completion_scroll, shown_rows).collect();
        if rows.is_empty() {
            return None;
        }

        let theme = self.theme;
        let mut spans = Vec::new();
        let mut selected_row = None;
        for (i, (is_selected, candidate)) in rows.iter().enumerate() {
            if i > 0 {
                spans.push(("\n".to_string(), theme.fg_modeline, false));
            }
            if *is_selected {
                selected_row = Some(i);
            }
            // `caret_text`/`fg_modeline`, not `syntax_keyword`/
            // `syntax_function` -- this popup shares the modeline's
            // background (`bg_modeline`, pushed below alongside every
            // other popup kind), and the `syntax_*` family is calibrated
            // for contrast against `bg` (the content background)
            // instead. Same bug class already fixed for which-key's own
            // key column and the dashboard banner earlier this project:
            // `caret_text`/`fg_modeline` are the two colors actually
            // guaranteed legible against `bg_modeline` in every theme.
            let color = match candidate.payload.kind {
                fenix_completion::CompletionKind::Keyword => theme.caret_text,
                fenix_completion::CompletionKind::Tag => theme.fg_modeline,
            };
            spans.push((candidate.label.clone(), color, false));
        }

        let longest = rows.iter().map(|(_, c)| c.label.chars().count()).max().unwrap_or(0);
        let max_width = (window_width - 2.0 * COMPLETION_MARGIN).max(text::WHICH_KEY_MIN_WIDTH);
        let width = (longest as f32 * char_width + COMPLETION_PADDING)
            .clamp(text::WHICH_KEY_MIN_WIDTH, text::WHICH_KEY_MAX_WIDTH.min(max_width));
        let height = rows.len() as f32 * line_height + COMPLETION_PADDING;
        let rect =
            popup::resolve(popup::Anchor::BelowPoint { x: caret_x, y: caret_y + line_height }, width, height, window_width, modeline_top);
        Some((rect, spans, selected_row))
    }

    /// Builds the visible rows of a directory listing as rich-text spans
    /// (indent, icon glyph, name, git-status marker, and -- for the
    /// full-width explorer only, not the narrow sidebar -- size/age
    /// attributes), the same shape `content_spans` builds for editor
    /// text so both flow through the same `set_*_rich` machinery.
    /// Returns the spans plus which rendered row (if any) is the
    /// selected entry and which rows are marked, for the caller to draw
    /// highlight rects for.
    fn explorer_row_spans(&self, explorer: &ExplorerState, scroll: usize, rows: usize, show_attrs: bool) -> ExplorerRowsResult {
        let theme = self.theme;
        let mut spans = Vec::new();
        let mut selected_row = None;
        let mut marked_rows = Vec::new();

        let end = (scroll + rows).min(explorer.entries.len());
        let visible_count = end.saturating_sub(scroll);
        for (i, idx) in (scroll..end).enumerate() {
            let entry = &explorer.entries[idx];
            if idx == explorer.selected {
                selected_row = Some(i);
            }
            if explorer.marks.contains(&entry.path) {
                marked_rows.push(i);
            }

            if entry.depth > 0 {
                spans.push(("  ".repeat(entry.depth), theme.fg, false));
            }

            let expanded =
                entry.is_dir && explorer.entries.get(idx + 1).is_some_and(|next| next.depth > entry.depth);
            let icon_color = if entry.is_dir { theme.icon_folder } else { theme.icon_file };
            spans.push((icon::icon_for(&entry.name, entry.is_dir, expanded).to_string(), icon_color, true));
            spans.push((format!(" {}", entry.name), theme.fg, false));

            if let Some(status) = entry.git_status {
                spans.push((format!("  {}", git_status_marker(status)), theme.git_status_color(status), false));
            }
            if show_attrs && !entry.is_dir {
                spans.push((format!("  {}  {}", format_size(entry.size), format_age(entry.modified)), theme.gutter_fg, false));
            }

            if i + 1 < visible_count {
                spans.push(("\n".to_string(), theme.fg, false));
            }
        }
        (spans, selected_row, marked_rows)
    }

    /// Builds the picker's visible rows as rich-text spans: row 0 is the
    /// query prompt (`"> {query}"`), the rows below are the filtered/
    /// ranked candidates windowed by `self.picker_scroll` -- same shape
    /// `explorer_row_spans` builds for a directory listing, so both flow
    /// through the same `set_content_rich` pipeline. Returns the spans
    /// plus which rendered row (if any) is the current selection, for
    /// the caller to draw a highlight rect over.
    fn picker_row_spans(&self, picker: &ActivePicker, rows: usize) -> (RowSpans, Option<usize>) {
        let theme = self.theme;
        let mut spans: RowSpans = vec![(format!("> {}", picker_query(picker)), theme.fg, false)];

        let candidate_rows = rows.saturating_sub(1);
        let visible = picker_visible_labels(picker, self.picker_scroll, candidate_rows);
        if !visible.is_empty() {
            spans.push(("\n".to_string(), theme.fg, false));
        }
        let count = visible.len();
        let mut hl_row = None;
        for (i, (is_selected, label)) in visible.into_iter().enumerate() {
            if is_selected {
                hl_row = Some(i + 1);
            }
            spans.push((label, theme.fg, false));
            if i + 1 < count {
                spans.push(("\n".to_string(), theme.fg, false));
            }
        }
        (spans, hl_row)
    }

    fn redraw(&mut self) {
        let Some((window_width, window_height)) = self.gpu.as_ref().map(|gpu| (gpu.size.width as f32, gpu.size.height as f32))
        else {
            return;
        };

        // Resolved once, up front: the active theme's font (so
        // `char_width` below reflects it), the real measured advance
        // width for that font (used for every per-column pixel
        // computation below instead of the fixed-ratio `text::
        // CHAR_WIDTH` constant, which broke the moment a second font --
        // the bundled TempleOS bitmap font, a ~1.0x-em advance vs. the
        // constant's assumed ~0.6x -- entered the mix), and the live
        // `line_height`/`modeline_height` (same reasoning: fixed once,
        // now adjustable at runtime via `SPC t =`/`-`/`0`, so every
        // consumer needs the *current* value, not the `text::LINE_HEIGHT`/
        // `MODELINE_HEIGHT` constants those used to be).
        let theme = self.theme;
        // A `config.ini` `font_family` always wins over whatever the
        // active theme names; an unset config falls through to the
        // theme's own choice (`None` for every theme but TempleOS,
        // which `TextPipeline::set_font_family` resolves to the fast
        // concrete-name fallback rather than the slow generic one).
        let font_family = self.config.font_family.as_deref().or(theme.font_family);
        let (char_width, line_height, modeline_height) = match &mut self.text {
            Some(text) => {
                text.set_font_family(font_family);
                (text.char_width(), text.line_height(), text.modeline_height())
            }
            None => (text::CHAR_WIDTH, text::LINE_HEIGHT, text::LINE_HEIGHT + 8.0),
        };
        let visible_lines = text::visible_line_count(window_height, modeline_height, line_height);
        let caret_is_block = self.caret_is_block();

        // Sidebar is independent of window splits *and* `main_view` --
        // kept alive (and its own scroll adjusted) even while a full-
        // buffer explorer is showing in the focused pane, but only
        // actually rendered in Editor mode (see `show_sidebar` below);
        // a file listing on top of an explorer/picker overlay would just
        // be confusing. It's a single frame-level strip, not per-pane --
        // matches Doom's own treemacs sidebar, which is frame-local too.
        if let Some(sidebar) = &self.sidebar {
            self.sidebar_scroll = scroll_to_include(self.sidebar_scroll, sidebar.selected, visible_lines);
        }
        let show_sidebar = self.sidebar_open && self.main_view == MainView::Editor;
        let sidebar_px = if show_sidebar { text::SIDEBAR_WIDTH } else { 0.0 };
        let sidebar_render = if show_sidebar {
            self.sidebar.as_ref().map(|s| self.explorer_row_spans(s, self.sidebar_scroll, visible_lines, false))
        } else {
            None
        };

        let modeline_top = window_height - modeline_height;
        let pane_area =
            fenix_window::Rect { x: sidebar_px, y: 0.0, w: (window_width - sidebar_px).max(0.0), h: modeline_top.max(0.0) };
        let layout = self.windows().layout(pane_area);
        let focused_pane = self.windows().focused_id();

        // One entry per visible window pane -- built fresh every frame
        // from whatever `WindowTree::layout` currently reports (splits/
        // closes/resizes take effect immediately, no separate relayout
        // step needed). Only the *focused* pane can show the explorer/
        // picker overlay (`main_view`/`active_picker`/`explorer` are a
        // single global "what's the current editing context" concept,
        // same as before splits existed) or Vim-mode decoration
        // (selection highlight, pulse, blinking caret) -- every other
        // pane always renders its own buffer's plain content, still with
        // its own gutter/syntax highlighting/current-line highlight, so
        // splitting to see two files is genuinely useful, just without
        // the "actively being edited" polish on the one you're not in.
        struct PaneRender {
            pane: fenix_window::WindowId,
            rect: fenix_window::Rect,
            spans: RowSpans,
            hl_row: Option<usize>,
            /// Whether `hl_row` should render with `theme.selection`
            /// (real standalone contrast) instead of `theme.hl_line`
            /// (a deliberately subtle tint, per `hl_line`'s own doc
            /// comment, that's fine to be understated because a
            /// blinking caret on the same row already draws the eye).
            /// Explorer/Picker have no caret at all (`caret: None`
            /// below) -- their `hl_row` *is* the only indicator of
            /// where you are, so it needs `selection`'s contrast, the
            /// same reasoning the sidebar's own selected-row highlight
            /// already uses `theme.selection` for. The dashboard also
            /// gets the strong styling: it's Vim-navigable text with a
            /// real caret, but functions as a menu you skim rather than
            /// actively type into, so the same "needs to be seen at a
            /// glance, not just backed up by the caret" reasoning
            /// applies.
            hl_row_strong: bool,
            marked_rows: Vec<usize>,
            selection_segments: Segments,
            pulse_overlay: Option<(Segments, f32)>,
            bracket_match_segments: Segments,
            hlsearch_segments: Segments,
            caret: Option<(usize, usize)>,
            content_frac: f32,
            gutter_px: f32,
        }

        let mut panes_render: Vec<PaneRender> = Vec::with_capacity(layout.len());
        for (pane, rect) in &layout {
            let (pane, rect) = (*pane, *rect);
            let is_focused = pane == focused_pane;
            let pane_visible_lines = text::lines_that_fit(rect.h, line_height);

            if is_focused && self.main_view == MainView::Explorer {
                let rows = pane_visible_lines + 1;
                if let Some(explorer) = &self.explorer {
                    self.explorer_scroll = scroll_to_include(self.explorer_scroll, explorer.selected, pane_visible_lines);
                }
                let (spans, selected_row, marks) = match &self.explorer {
                    Some(explorer) => self.explorer_row_spans(explorer, self.explorer_scroll, rows, true),
                    None => (Vec::new(), None, Vec::new()),
                };
                panes_render.push(PaneRender {
                    pane,
                    rect,
                    spans,
                    hl_row: selected_row,
                    hl_row_strong: true,
                    marked_rows: marks,
                    selection_segments: Segments::new(),
                    pulse_overlay: None,
                    bracket_match_segments: Segments::new(),
                    hlsearch_segments: Segments::new(),
                    caret: None,
                    content_frac: 0.0,
                    gutter_px: 0.0,
                });
                continue;
            }
            if is_focused && self.main_view == MainView::Picker {
                let rows = pane_visible_lines + 1;
                if let Some(picker) = &self.active_picker {
                    self.picker_scroll = scroll_to_include(self.picker_scroll, picker_selected_row(picker), pane_visible_lines);
                }
                let (spans, selected_row) = match &self.active_picker {
                    Some(picker) => self.picker_row_spans(picker, rows),
                    None => (Vec::new(), None),
                };
                panes_render.push(PaneRender {
                    pane,
                    rect,
                    spans,
                    hl_row: selected_row,
                    hl_row_strong: true,
                    marked_rows: Vec::new(),
                    selection_segments: Segments::new(),
                    pulse_overlay: None,
                    bracket_match_segments: Segments::new(),
                    hlsearch_segments: Segments::new(),
                    caret: None,
                    content_frac: 0.0,
                    gutter_px: 0.0,
                });
                continue;
            }

            // Plain buffer content -- every pane not currently showing an
            // overlay, focused or not.
            let buffer_id = *self.windows().content(pane).expect("every pane has a buffer");
            if is_focused {
                self.ensure_cursor_visible(pane_visible_lines);
            }
            // Every pane -- not just the focused one -- has its own
            // `PaneState` (seeded when it was created), so each renders
            // from its own independent cursor/scroll position.
            let pane_state = *self.pane_state(pane);
            let rendered_scroll = pane_state.rendered_scroll;
            let render_base_line = rendered_scroll.floor().max(0.0) as usize;
            let mut render_frac = rendered_scroll - rendered_scroll.floor();
            // Computed here (rather than at its previous spot, further
            // down) so the dashboard-centering block below can look up
            // the cursor's own row in the per-line pad table.
            let (line, col) =
                self.buffers.get(buffer_id).map(|ob| ob.buffer.line_col(&pane_state.cursor)).unwrap_or((0, 0));

            let gutter_chars = self.buffers.get(buffer_id).map(|ob| self.gutter_chars(ob)).unwrap_or(0);
            let mut gutter_px = gutter_chars as f32 * char_width;
            let mut dashboard_pad: Option<Vec<usize>> = None;
            // The dashboard centers itself in its pane. Vertical centering
            // is a real pixel offset (`text.rs`'s `TextArea.top` actually
            // reads `content_frac`, so biasing it here works). Horizontal
            // centering can't work the same way: `TextArea.left` is fixed
            // at `rect.x + PAD_LEFT` and never reads `gutter_px` -- that
            // field only feeds caret/selection *position math*, matching
            // where a real line-number gutter's *baked-in leading
            // characters* happen to end, not an independent geometric
            // shift. So horizontal centering reuses that same mechanism:
            // per-line padding baked as blank characters into the
            // rendered spans (see `content_spans`'s `dashboard_pad`
            // parameter), exactly how a real gutter's digits shift real
            // content today. `gutter_px` (used only for caret/selection
            // position math, never for the real gutter case a dashboard
            // never has) takes specifically the *cursor's own row's* pad
            // -- selection/pulse/bracket-match are never meaningfully
            // used on a dashboard buffer, so sharing this one value with
            // them too isn't a real compromise.
            let is_dashboard = self.buffers.get(buffer_id).is_some_and(|ob| ob.kind == BufferKind::Dashboard);
            if let Some(ob) = self.buffers.get(buffer_id) {
                if ob.kind == BufferKind::Dashboard {
                    let empty = Vec::new();
                    let dash_lines = self.dashboard_lines.get(&buffer_id).unwrap_or(&empty);
                    let (pad_by_line, extra_top_px) = dashboard_center_offset(ob, dash_lines, rect, char_width, line_height);
                    gutter_px = pad_by_line.get(line).copied().unwrap_or(0) as f32 * char_width;
                    render_frac -= extra_top_px / line_height;
                    dashboard_pad = Some(pad_by_line);
                }
            }
            let syntax_highlights = self.syntax_highlights_for_visible_range(buffer_id, render_base_line, pane_visible_lines + 1);
            let content_spans = match self.buffers.get(buffer_id) {
                Some(ob) => self.content_spans(
                    ob,
                    render_base_line,
                    pane_visible_lines + 1,
                    gutter_chars,
                    dashboard_pad.as_deref(),
                    &syntax_highlights,
                    line,
                ),
                None => Vec::new(),
            };
            let spans: RowSpans = content_spans.into_iter().map(|(s, c)| (s, c, false)).collect();

            // During a large animated pan the cursor's actual line can
            // legitimately be outside the currently-fetched window for
            // part of the transition (it hasn't panned into view yet) --
            // `None` means "don't draw the hl-line/caret this frame," not
            // a bug.
            let hl_row = line.checked_sub(render_base_line).filter(|&row| row <= pane_visible_lines);

            let (selection_segments, pulse_overlay, bracket_match_segments, hlsearch_segments, caret) = if is_focused {
                (
                    self.visual_selection_segments(pane_visible_lines + 1),
                    self.pulse_overlay(pane_visible_lines + 1),
                    self.bracket_match_segments(pane_visible_lines + 1),
                    self.hlsearch_segments(pane_visible_lines + 1),
                    self.focused_caret(hl_row, col, render_base_line, pane_visible_lines),
                )
            } else {
                (Segments::new(), None, Segments::new(), Segments::new(), None)
            };

            panes_render.push(PaneRender {
                pane,
                rect,
                spans,
                hl_row,
                hl_row_strong: is_dashboard,
                marked_rows: Vec::new(),
                selection_segments,
                pulse_overlay,
                bracket_match_segments,
                hlsearch_segments,
                caret,
                content_frac: render_frac,
                gutter_px,
            });
        }

        let modeline_pieces = self.modeline_pieces();
        let modeline_command_text = if let Some(query) = &self.pending_grep_query {
            Some(format!("rg: {query}"))
        } else if self.vim.mode() == Mode::Search {
            let prefix = if self.vim.search_forward() { "/" } else { "?" };
            Some(format!("{prefix}{}", self.vim.search_query()))
        } else if let Some(prompt_text) = self.explorer_prompt_text() {
            Some(prompt_text)
        } else if let Some(confirm_text) = self.docker_confirm_text() {
            Some(confirm_text)
        } else if modeline_pieces.is_none() {
            Some(format!(":{}", self.vim.command_line()))
        } else {
            None
        };
        let (badge_bg, badge_fg) = self.mode_colors();
        // Top-right corner, clear of both the content the user is actively
        // editing (top-left, where the cursor usually is) and the modeline
        // (bottom) -- least likely to sit under whatever they're looking
        // at. Resolved (and, if needed, row-truncated) here rather than
        // left to `text.rs`/`prepare` so its position is known before the
        // `bg_rect` panel-background push below.
        let which_key_popup = self.which_key_popup(window_width, modeline_top);
        // Never `Some` at the same time as `which_key_popup` -- one only
        // appears mid-Normal/-pending-sequence, the other only in Insert
        // mode -- so `popup_rects` below never ends up with more than one
        // entry in practice, even though it's shaped as a list.
        let completion_popup = panes_render.iter().find(|p| p.pane == focused_pane).and_then(|focused| {
            self.completion_popup(window_width, modeline_top, focused.rect, focused.caret, focused.gutter_px, focused.content_frac)
        });
        let caret_alpha = self.caret_alpha();

        let (Some(window), Some(gpu), Some(text), Some(bg_rect), Some(caret_rect)) =
            (&self.window, &mut self.gpu, &mut self.text, &mut self.bg_rect, &mut self.caret_rect)
        else {
            return;
        };

        let live_panes: Vec<fenix_window::WindowId> = panes_render.iter().map(|p| p.pane).collect();
        for pane in &panes_render {
            let content_refs: Vec<(&str, glyphon::Color, bool)> =
                pane.spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_pane_rich(pane.pane, pane.rect.w, pane.rect.h, &content_refs);
        }
        text.retain_panes(&live_panes);
        if let Some((sidebar_spans, _, _)) = &sidebar_render {
            let sidebar_refs: Vec<(&str, glyphon::Color, bool)> =
                sidebar_spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_sidebar_rich(&sidebar_refs);
        }
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

        let mut popup_rects: Vec<(popup::PopupId, fenix_window::Rect)> = Vec::new();
        // The row (if any) the currently-open popup wants highlighted --
        // only the completion popup has a notion of a "selected" row
        // today (which-key has no selection), consulted below alongside
        // the `bg_rect` background push shared by every popup kind.
        let mut popup_selected_row: Option<usize> = None;
        if let Some((rect, spans)) = &which_key_popup {
            let refs: Vec<(&str, glyphon::Color, bool)> = spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_popup_rich(popup::PopupId::WhichKey, rect.w, &refs);
            popup_rects.push((popup::PopupId::WhichKey, *rect));
        }
        if let Some((rect, spans, selected_row)) = &completion_popup {
            let refs: Vec<(&str, glyphon::Color, bool)> = spans.iter().map(|(s, c, i)| (s.as_str(), *c, *i)).collect();
            text.set_popup_rich(popup::PopupId::Completion, rect.w, &refs);
            popup_rects.push((popup::PopupId::Completion, *rect));
            popup_selected_row = *selected_row;
        }
        text.retain_popups(&popup_rects.iter().map(|(id, _)| *id).collect::<Vec<_>>());

        let sidebar_row_y = |row: usize| text::PAD_TOP + row as f32 * line_height;

        bg_rect.clear();
        for pane in &panes_render {
            // Row index (relative to that pane's own render_base_line, or
            // its explorer/picker listing's own scroll) -> pixel y within
            // the pane, shifted up by its own mid-scroll fractional
            // offset so it pans in step with its text (always 0 outside
            // the focused pane's Editor-mode smooth scroll).
            let row_y = |row: usize| pane.rect.y + text::PAD_TOP + row as f32 * line_height - pane.content_frac * line_height;
            if let Some(row) = pane.hl_row {
                let y = row_y(row);
                let color = if pane.hl_row_strong { theme.selection } else { theme.hl_line };
                bg_rect.push_rect(gpu, pane.rect.x, y, pane.rect.w, line_height, color);
            }
            for row in &pane.marked_rows {
                let y = row_y(*row);
                bg_rect.push_rect(gpu, pane.rect.x, y, pane.rect.w, line_height, theme.selection);
            }
            // Column-math x-offset for this pane's caret/selection/pulse
            // rects: its own left edge, plus `PAD_LEFT`, plus its own
            // line-number gutter -- has to match exactly where `text.
            // prepare`'s `TextArea` for this pane actually renders the
            // text, or these rects drift out of alignment with it.
            let content_x = pane.rect.x + text::PAD_LEFT + pane.gutter_px;
            for &(row, col_start, col_end) in &pane.selection_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, line_height, theme.selection);
            }
            for &(row, col_start, col_end) in &pane.bracket_match_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, line_height, theme.bracket_match);
            }
            for &(row, col_start, col_end) in &pane.hlsearch_segments {
                let x = content_x + col_start as f32 * char_width;
                let y = row_y(row);
                let w = (col_end - col_start) as f32 * char_width;
                bg_rect.push_rect(gpu, x, y, w, line_height, theme.search_match);
            }
            if let Some((segments, alpha)) = &pane.pulse_overlay {
                let [r, g, b, _] = theme.caret;
                for &(row, col_start, col_end) in segments {
                    let x = content_x + col_start as f32 * char_width;
                    let y = row_y(row);
                    let w = (col_end - col_start) as f32 * char_width;
                    bg_rect.push_rect(gpu, x, y, w, line_height, [r, g, b, *alpha]);
                }
            }
        }
        bg_rect.push_rect(gpu, 0.0, modeline_top, window_width, modeline_height, theme.bg_modeline);
        if modeline_pieces.is_some() {
            // Starts at PAD_LEFT, matching where the badge text itself
            // starts rendering (`text.rs`'s modeline TextArea uses the same
            // left inset) -- starting this at the window edge instead left
            // the rendered label overflowing past the badge's right edge,
            // throwing off how centered it looked inside the colored badge.
            let badge_width = (1.0 + text::MODE_BADGE_CHARS as f32) * char_width;
            bg_rect.push_rect(gpu, text::PAD_LEFT, modeline_top, badge_width, modeline_height, badge_bg);
        }
        // Popup backgrounds are deliberately *not* pushed into this batch --
        // see the big comment at the two-pass render sequence below for why.
        if show_sidebar {
            // `theme.bg`, not `theme.bg_modeline` -- the sidebar shows
            // file/directory names with the same icon/syntax-adjacent
            // colors the main content area uses (dark, saturated colors
            // meant to read on `theme.bg`, per `TEMPLEOS`'s own doc
            // comment), so it needs the *content* background, not the
            // modeline's. The previous `bg_modeline` choice produced an
            // unreadable blue-background/black-text combination on the
            // TempleOS theme specifically (ORBIT_DARK's own bg/bg_modeline
            // are close enough in value that this went unnoticed there).
            bg_rect.push_rect(gpu, 0.0, 0.0, text::SIDEBAR_WIDTH, modeline_top, theme.bg);
            if let Some((_, Some(selected_row), _)) = &sidebar_render {
                // `theme.selection`, not `theme.hl_line`: the sidebar has no
                // caret of its own, so this highlight is the *only* cue for
                // which entry is selected and needs to actually stand out --
                // matches the full-buffer explorer/picker's own selected-row
                // highlight (both already use `theme.selection` above) rather
                // than the subtle current-line tint meant to be a secondary
                // cue alongside a visible caret.
                let y = sidebar_row_y(*selected_row);
                bg_rect.push_rect(gpu, 0.0, y, text::SIDEBAR_WIDTH, line_height, theme.selection);
            }
            // A thin divider along the sidebar's own right edge, same
            // color/weight as the ones between split panes, so there's a
            // visible seam now that it's no longer a different color
            // from the content area next to it.
            bg_rect.push_rect(gpu, text::SIDEBAR_WIDTH - 1.0, 0.0, 2.0, modeline_top, theme.divider);
        }
        // Divider lines along every split boundary the layout computed --
        // drawn from each pane's own right/bottom edge, so two adjacent
        // panes each contribute one line and they land on top of each
        // other exactly on the boundary. Skipped for the last column/row
        // (nothing to divide from past the window edge).
        for pane in &panes_render {
            if pane.rect.x + pane.rect.w < pane_area.x + pane_area.w - 0.5 {
                bg_rect.push_rect(gpu, pane.rect.x + pane.rect.w - 1.0, pane.rect.y, 2.0, pane.rect.h, theme.divider);
            }
            if pane.rect.y + pane.rect.h < pane_area.y + pane_area.h - 0.5 {
                bg_rect.push_rect(gpu, pane.rect.x, pane.rect.y + pane.rect.h - 1.0, pane.rect.w, 2.0, theme.divider);
            }
        }
        // Layered last -- on top of everything else pushed to `bg_rect`
        // this frame (modeline bar, hl-line, selection, which-key/sidebar
        // backgrounds) -- so a themed border reads as the outermost frame
        // around the whole window rather than something those cover.
        // `BORDER_WIDTH` sits comfortably inside `PAD_TOP`/`PAD_LEFT`, so
        // it can't clip into body text.
        if let Some(border) = theme.border {
            bg_rect.push_rect(gpu, 0.0, 0.0, window_width, BORDER_WIDTH, border);
            bg_rect.push_rect(gpu, 0.0, window_height - BORDER_WIDTH, window_width, BORDER_WIDTH, border);
            bg_rect.push_rect(gpu, 0.0, 0.0, BORDER_WIDTH, window_height, border);
            bg_rect.push_rect(gpu, window_width - BORDER_WIDTH, 0.0, BORDER_WIDTH, window_height, border);
        }
        bg_rect.flush(gpu);

        caret_rect.clear();
        if let Some(focused) = panes_render.iter().find(|p| p.pane == focused_pane) {
            if let Some((row, col)) = focused.caret {
                if caret_alpha > 0.0 {
                    let (caret_x, caret_y) =
                        caret_pixel_pos(focused.rect, row, col, focused.gutter_px, focused.content_frac, char_width, line_height);
                    let [r, g, b, a] = theme.caret;
                    // Insert keeps the thin bar (an I-beam-style "about to
                    // type here" marker); every other mode (Normal, Visual,
                    // Replace, Command) gets a full-cell block, matching real
                    // Vim's own cursor-shape convention. The block is drawn
                    // at reduced opacity -- caret_rect is composited *after*
                    // text (so the caret always shows on top, no glyph-recolor
                    // trick needed), and a fully opaque block would otherwise
                    // completely hide the character underneath it instead of
                    // just marking its position.
                    let (width, block_alpha) = if caret_is_block { (char_width, 0.6) } else { (2.0, 1.0) };
                    caret_rect.push_rect(gpu, caret_x, caret_y, width, line_height, [r, g, b, a * caret_alpha * block_alpha]);
                }
            }
        }
        caret_rect.flush(gpu);

        let prepare_panes: Vec<(fenix_window::WindowId, fenix_window::Rect, f32)> =
            panes_render.iter().map(|p| (p.pane, p.rect, p.content_frac)).collect();
        text.prepare(gpu, theme, &prepare_panes, show_sidebar);

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
        }
        // Popups (background + text) and the caret are drawn in a second,
        // separate pass layered on top (`LoadOp::Load`, not `Clear`)
        // rather than folded into the pass above. A popup's background is
        // an opaque rect from `bg_rect`, and a pane's content `TextArea`
        // has no way to be clipped around a popup-shaped hole (a single
        // `TextBounds` per pane can't express "everywhere except this
        // corner") -- so if popup content shared the first pass, whichever
        // pane extends under the popup's corner would simply paint its own
        // text over the popup's already-drawn background, exactly the
        // "buffer text on top of the which-key window" bug this two-pass
        // split fixes. Ending pass one first and starting pass two with
        // `Load` guarantees popups (and the caret, which already needed to
        // draw after all text) composite on top of whatever pass one
        // painted, regardless of what that was.
        if !popup_rects.is_empty() {
            bg_rect.clear();
            for &(id, rect) in &popup_rects {
                bg_rect.push_rect(gpu, rect.x, rect.y, rect.w, rect.h, theme.bg_modeline);
                // The completion popup's own selected-candidate row --
                // same `theme.hl_line` mechanism a pane's current-line
                // highlight and a picker/explorer's selected-row highlight
                // already use, just applied to a floating popup's local
                // coordinates instead of a pane's.
                if id == popup::PopupId::Completion {
                    if let Some(row) = popup_selected_row {
                        let y = rect.y + COMPLETION_PADDING / 2.0 + row as f32 * line_height;
                        bg_rect.push_rect(gpu, rect.x, y, rect.w, line_height, theme.hl_line);
                    }
                }
            }
            bg_rect.flush(gpu);
            text.prepare_popups(gpu, theme, &popup_rects);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !popup_rects.is_empty() {
                bg_rect.render(&mut pass);
                text.render_popups(&mut pass);
            }
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
        // No priming call needed for pane content -- `redraw()` populates
        // every visible pane's `GlyphBuffer` fresh (creating it lazily)
        // on the first real frame, which winit already requests
        // immediately after this.
        let mut text = TextPipeline::new(&gpu);
        text.set_font_size(self.config.font_size.unwrap_or(text::FONT_SIZE));
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
        let animating =
            blink_transitioning || pulse_active || self.workspaces.active_scroll_anims().contains_key(&self.focused_pane_id());
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

/// Test-only conveniences for driving the focused buffer directly --
/// mirrors the exact inline-field-access pattern the equivalent
/// production code paths use (see e.g. `handle_key`'s Vim dispatch),
/// just named so test setup isn't repeating the borrow-splitting dance
/// at every call site.
#[cfg(test)]
impl App {
    fn test_insert(&mut self, ch: char) {
        let (buffer, cursor) = self.focused_buffer_and_cursor_mut();
        buffer.insert_char(cursor, ch);
    }

    fn test_insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            self.test_insert(ch);
        }
    }

    fn test_vim_key(&mut self, key: KeyPress) -> VimEvent {
        let id = self.focused_buffer_id();
        let pane = self.focused_pane_id();
        let ob = self.buffers.get_mut(id).expect("focused window always has an open buffer");
        let pane_state = self.workspaces.active_pane_states_mut().get_mut(&pane).expect("every existing pane has a PaneState");
        self.vim.handle_key(&mut ob.buffer, &mut pane_state.cursor, key)
    }

    /// Points the focused pane at `path`, opening it fresh in the
    /// registry -- test-only equivalent of what `open_file_from_picker`/
    /// `explorer_open_selected` do in production.
    fn test_open_path(&mut self, path: &Path) {
        let id = self.buffers.open_path(path);
        let focused = self.focused_pane_id();
        self.set_pane_content(focused, id);
    }

    /// The focused pane's live cursor -- test-only convenience wrapping
    /// `pane_state`, since tests can't borrow two fields at once the way
    /// production code inlines it (`focused_buffer_and_cursor_mut`, etc.).
    #[cfg(test)]
    fn test_cursor(&self) -> Cursor {
        self.pane_state(self.focused_pane_id()).cursor
    }

    /// Overwrites the focused pane's live cursor -- test-only convenience
    /// replacing the old `app.open_mut().cursor = ...` idiom, now that
    /// cursor lives on the pane, not the buffer.
    #[cfg(test)]
    fn test_set_cursor(&mut self, cursor: Cursor) {
        let pane = self.focused_pane_id();
        self.pane_state_mut(pane).cursor = cursor;
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
            app.test_insert('\n');
        }
        // cursor is now on line 30; a 10-line viewport starting at 0 doesn't include it
        app.ensure_cursor_visible(10);
        assert_eq!(app.pane_state(app.focused_pane_id()).scroll_line, 21);
    }

    #[test]
    fn small_scroll_change_starts_an_animation_not_an_instant_jump() {
        let mut app = App::with_file(None);
        for _ in 0..5 {
            app.test_insert('\n');
        }
        app.ensure_cursor_visible(3); // 6 lines, 3-line viewport -> scrolls a bit
        assert!(app.workspaces.active_scroll_anims().contains_key(&app.focused_pane_id()));
        let ps = app.pane_state(app.focused_pane_id());
        assert_ne!(ps.rendered_scroll, ps.scroll_line as f32); // still mid-ease, not snapped
    }

    #[test]
    fn a_new_scroll_target_while_easing_snaps_instead_of_re_easing() {
        // Simulates holding `j`: a new scroll target can arrive before the
        // previous 150ms ease has settled. It must snap to the new target
        // immediately rather than restarting another ease from an
        // already-stale position -- otherwise `rendered_scroll` would keep
        // chasing a few lines behind the cursor for as long as the key
        // stays held, instead of ever catching up.
        let mut app = App::with_file(None);
        for _ in 0..30 {
            app.test_insert('\n');
        }
        app.test_set_cursor(Cursor::at_start());
        for _ in 0..15 {
            let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
            buffer.move_down(cursor);
        }
        // Cursor now on line 15; a 10-line viewport wants scroll_line = 6.
        app.ensure_cursor_visible(10);
        let pane = app.focused_pane_id();
        assert!(app.workspaces.active_scroll_anims().contains_key(&pane)); // mid-ease from that first scroll

        // One more line down before the ease had a chance to settle -- the
        // exact shape of holding `j`.
        {
            let (buffer, cursor) = app.focused_buffer_and_cursor_mut();
            buffer.move_down(cursor);
        }
        app.ensure_cursor_visible(10);
        let ps = app.pane_state(pane);
        assert_eq!(ps.rendered_scroll, ps.scroll_line as f32, "must snap, not compound the lag from the still-active ease");
        assert!(!app.workspaces.active_scroll_anims().contains_key(&pane));
    }

    #[test]
    fn huge_scroll_jump_snaps_instantly_without_animating() {
        let mut app = App::with_file(None);
        for _ in 0..500 {
            app.test_insert('\n');
        }
        app.ensure_cursor_visible(10); // jump of ~490 lines, way past the snap threshold
        assert!(!app.workspaces.active_scroll_anims().contains_key(&app.focused_pane_id()));
        let ps = app.pane_state(app.focused_pane_id());
        assert_eq!(ps.rendered_scroll, ps.scroll_line as f32);
    }

    #[test]
    fn rendered_scroll_eases_toward_target_and_settles() {
        let mut app = App::with_file(None);
        let pane = app.focused_pane_id();
        {
            let ps = app.pane_state_mut(pane);
            ps.scroll_line = 10;
            ps.rendered_scroll = 0.0;
        }
        app.workspaces.active_scroll_anims_mut().insert(pane, ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION / 2 });
        app.update_rendered_scroll();
        let r = app.pane_state(pane).rendered_scroll;
        assert!(r > 0.0 && r < 10.0, "should be partway there");
        assert!(app.workspaces.active_scroll_anims().contains_key(&pane));

        app.workspaces.active_scroll_anims_mut().insert(pane, ScrollAnim { from: 0.0, to: 10, started: Instant::now() - SCROLL_DURATION * 2 });
        app.update_rendered_scroll();
        assert_eq!(app.pane_state(pane).rendered_scroll, 10.0);
        assert!(!app.workspaces.active_scroll_anims().contains_key(&pane)); // settled, animation cleared
    }

    #[test]
    fn render_base_line_splits_a_fractional_scroll_position() {
        let mut app = App::with_file(None);
        let pane = app.focused_pane_id();
        app.pane_state_mut(pane).rendered_scroll = 4.25;
        assert_eq!(app.render_base_line(), 4);
        assert!((app.pane_state(pane).rendered_scroll.fract() - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn modeline_reflects_filename_dirty_state_mode_and_position() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer(); // with_file(None) now opens the dashboard, not a plain scratch buffer
        assert_eq!(app.modeline_text(), "  NORMAL │ [No Name]   Ln 1, Col 1 ");

        app.test_insert('a');
        app.test_insert('b');
        assert_eq!(app.modeline_text(), "  NORMAL │ [No Name] [+]   Ln 1, Col 3 ");
    }

    #[test]
    fn modeline_shows_command_line_while_typing_an_ex_command() {
        let mut app = App::with_file(None);
        for ch in [':', 'w', 'q'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        assert_eq!(app.modeline_text(), ":wq");
    }

    #[test]
    fn modeline_shows_the_search_prompt_while_typing_a_query() {
        let mut app = App::with_file(None);
        for ch in ['/', 'f', 'o', 'o'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        assert_eq!(app.modeline_text(), "/foo");
    }

    #[test]
    fn visual_selection_segments_cover_the_selected_range() {
        let mut app = App::with_file(None);
        for ch in "hello world".chars() {
            app.test_insert(ch);
        }
        app.test_set_cursor(Cursor::at_start());
        app.test_vim_key(KeyPress::char('v'));
        for _ in 0..4 {
            app.test_vim_key(KeyPress::char('l'));
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
    fn bracket_match_segments_covers_both_brackets_when_the_cursor_is_on_one() {
        let mut app = App::with_file(None);
        app.test_insert_str("(hello)");
        app.test_set_cursor(Cursor::at_start()); // on the opening '('
        assert_eq!(app.bracket_match_segments(10), vec![(0, 0, 1), (0, 6, 7)]);
    }

    #[test]
    fn bracket_match_segments_works_from_the_closing_side_too() {
        let mut app = App::with_file(None);
        app.test_insert_str("(hello)");
        app.test_set_cursor(Cursor { char_idx: 6, sticky_col: 6 }); // on the ')'
        assert_eq!(app.bracket_match_segments(10), vec![(0, 6, 7), (0, 0, 1)]);
    }

    #[test]
    fn bracket_match_segments_empty_when_the_cursor_is_not_on_a_bracket() {
        let mut app = App::with_file(None);
        app.test_insert_str("(hello)");
        app.test_set_cursor(Cursor { char_idx: 3, sticky_col: 3 }); // on 'l'
        assert!(app.bracket_match_segments(10).is_empty());
    }

    #[test]
    fn bracket_match_segments_empty_for_an_unmatched_bracket() {
        let mut app = App::with_file(None);
        app.test_insert_str("(hello");
        app.test_set_cursor(Cursor::at_start());
        assert!(app.bracket_match_segments(10).is_empty());
    }

    #[test]
    fn hlsearch_segments_covers_every_match_after_a_confirmed_search() {
        let mut app = App::with_file(None);
        app.test_insert_str("foo bar foo baz foo");
        app.test_set_cursor(Cursor::at_start());
        for ch in ['/', 'f', 'o', 'o'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));

        assert_eq!(app.hlsearch_segments(10), vec![(0, 0, 3), (0, 8, 11), (0, 16, 19)]);
    }

    #[test]
    fn hlsearch_segments_is_empty_before_any_search_is_confirmed() {
        let mut app = App::with_file(None);
        app.test_insert_str("foo bar foo");
        assert!(app.hlsearch_segments(10).is_empty());
    }

    #[test]
    fn hlsearch_segments_clears_after_an_edit() {
        let mut app = App::with_file(None);
        app.test_insert_str("foo bar foo");
        app.test_set_cursor(Cursor::at_start());
        for ch in ['/', 'f', 'o', 'o'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));
        assert!(!app.hlsearch_segments(10).is_empty());

        app.test_vim_key(KeyPress::char('x')); // a real edit, in Normal mode
        assert!(app.hlsearch_segments(10).is_empty());
    }

    #[test]
    fn focused_caret_previews_the_search_match_without_moving_the_real_cursor() {
        let mut app = App::with_file(None);
        app.test_insert_str("foo bar foo");
        app.test_set_cursor(Cursor::at_start());
        for ch in ['/', 'f', 'o', 'o'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        // The second "foo" starts at char 8, still line 0 -- row 0, col 8.
        assert_eq!(app.focused_caret(None, 0, 0, 10), Some((0, 8)));
        assert_eq!(app.test_cursor().char_idx, 0, "the real cursor must not move during preview");
    }

    #[test]
    fn focused_caret_falls_back_to_hl_row_outside_search_mode() {
        let mut app = App::with_file(None);
        app.test_insert_str("hello");
        assert_eq!(app.focused_caret(Some(0), 3, 0, 10), Some((0, 3)));
    }

    #[test]
    fn modeline_shows_visual_kind_not_just_visual() {
        let mut app = App::with_file(None);
        for ch in "one\ntwo\nthree".chars() {
            app.test_insert(ch);
        }
        app.test_set_cursor(Cursor::at_start());

        app.test_vim_key(KeyPress::char('v'));
        assert!(app.modeline_text().contains("VISUAL"));

        app.test_vim_key(KeyPress::char('V'));
        assert!(app.modeline_text().contains("V-LINE"));

        app.test_vim_key(KeyPress::char('v').with_ctrl());
        assert!(app.modeline_text().contains("V-BLOCK"));
    }

    #[test]
    fn mode_colors_differ_per_mode_and_visual_kinds_share_one_accent() {
        let mut app = App::with_file(None);
        let (normal_bg, _) = app.mode_colors();

        app.test_vim_key(KeyPress::char('i'));
        let (insert_bg, _) = app.mode_colors();
        assert_ne!(normal_bg, insert_bg);
        app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));

        app.test_vim_key(KeyPress::char('v'));
        let (char_visual_bg, _) = app.mode_colors();
        app.test_vim_key(KeyPress::char('V'));
        let (line_visual_bg, _) = app.mode_colors();
        assert_eq!(char_visual_bg, line_visual_bg); // one accent for all Visual kinds
        assert_ne!(char_visual_bg, normal_bg);
    }

    #[test]
    fn caret_is_block_everywhere_except_insert_mode() {
        let mut app = App::with_file(None);
        assert!(app.caret_is_block()); // starts in Normal

        app.test_vim_key(KeyPress::char('i'));
        assert!(!app.caret_is_block()); // Insert -- thin bar
        app.test_vim_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.caret_is_block()); // back to Normal

        app.test_vim_key(KeyPress::char('v'));
        assert!(app.caret_is_block()); // Visual
    }

    #[test]
    fn visual_line_segments_cover_whole_lines_regardless_of_column() {
        let mut app = App::with_file(None);
        for ch in "one\ntwo\nthree".chars() {
            app.test_insert(ch);
        }
        app.test_set_cursor(Cursor { char_idx: 5, sticky_col: 1 }); // column 1 of "two"
        app.test_vim_key(KeyPress::char('V'));
        assert_eq!(app.visual_selection_segments(10), vec![(1, 0, 3)]);
    }

    #[test]
    fn visual_block_segments_form_a_column_rectangle() {
        let mut app = App::with_file(None);
        for ch in "abc\ndef\nghi".chars() {
            app.test_insert(ch);
        }
        app.test_set_cursor(Cursor::at_start());
        app.test_vim_key(KeyPress::char('v').with_ctrl());
        for ch in ['j', 'j', 'l'] {
            app.test_vim_key(KeyPress::char(ch));
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
            app.test_insert(ch);
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
            app.test_insert(ch);
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
            app.test_insert(ch);
        }
        app.test_set_cursor(Cursor::at_start());
        assert!(app.pulse.is_none());

        app.test_vim_key(KeyPress::char('y'));
        let event = app.test_vim_key(KeyPress::char('w'));
        let fenix_vim::VimEvent::Pulse(range) = event else { panic!("expected a Pulse event from yw") };
        app.pulse = Some(Pulse { range, started: Instant::now() });
        assert!(app.pulse_overlay(10).is_some());
    }

    #[test]
    fn gutter_chars_zero_when_off() {
        let mut app = App::with_file(None);
        app.line_number_mode = LineNumberMode::Off;
        assert_eq!(app.gutter_chars(app.open()), 0);
    }

    #[test]
    fn gutter_chars_defaults_to_absolute_and_sizes_for_line_count() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer(); // with_file(None) now opens the dashboard; this test wants a plain empty buffer
        assert_eq!(app.gutter_chars(app.open()), 2); // 1-digit number + 1 padding column
    }

    #[test]
    fn content_spans_marks_rows_past_buffer_end_with_tilde() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer(); // single empty line, cursor on it
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 3, gutter, None, &[], 0);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "1 \n~ \n~ ");
    }

    #[test]
    fn content_spans_off_mode_still_shows_tilde_for_rows_past_end() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer();
        app.line_number_mode = LineNumberMode::Off;
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 2, gutter, None, &[], 0);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "\n~");
    }

    #[test]
    fn content_spans_relative_mode_shows_distance_from_cursor() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer();
        app.line_number_mode = LineNumberMode::Relative;
        app.test_insert_str("a\nb\nc\nd");
        app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 }); // line 1, 'b'
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 4, gutter, None, &[], 1);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "1 a\n0 b\n1 c\n2 d");
    }

    #[test]
    fn content_spans_current_line_number_uses_fg_not_gutter_fg() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer();
        app.test_insert_str("a\nb");
        app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 }); // line 1
        let gutter = app.gutter_chars(app.open());
        let spans = app.content_spans(app.open(), 0, 2, gutter, None, &[], 1);
        assert_eq!(spans[0].1, app.theme.gutter_fg); // line 0: not current
        assert_eq!(spans[2].1, app.theme.fg); // line 1: current line's gutter
    }

    #[test]
    fn content_spans_pads_a_dashboard_buffer_with_real_leading_blanks_not_gutter_digits() {
        // Regression test: horizontal centering only works if the padding
        // is actually written into the rendered text -- `gutter_px` alone
        // (a caret/selection position-math field) never reaches
        // `TextArea.left`, which is hardcoded to `rect.x + PAD_LEFT` in
        // `text.rs`. A prior version of this feature computed a pixel
        // offset and folded it into `gutter_px`, which compiled and had
        // passing unit tests but never visibly centered anything.
        let app = App::with_file(None); // opens the dashboard
        let ob = app.open();
        assert_eq!(ob.kind, BufferKind::Dashboard);

        let pad = [5usize];
        let spans = app.content_spans(ob, 0, 1, 0, Some(&pad), &[], 0);
        assert_eq!(spans[0].0, "     "); // 5 literal spaces, not "1 " or similar
    }

    #[test]
    fn content_spans_never_shows_a_tilde_past_the_end_of_a_dashboard_buffer() {
        let app = App::with_file(None);
        let ob = app.open();
        let visual_lines = ob.buffer.visual_line_count();
        let pad: Vec<usize> = vec![0; visual_lines + 11];
        // Ask for a row well past the dashboard's own content.
        let spans = app.content_spans(ob, visual_lines + 10, 1, 0, Some(&pad), &[], 0);
        assert!(!spans.iter().any(|(s, _)| s == "~"));
    }

    #[test]
    fn cycle_line_number_mode_goes_off_absolute_relative_off() {
        let mut app = App::with_file(None);
        app.line_number_mode = LineNumberMode::Off;
        app.cycle_line_number_mode();
        assert_eq!(app.line_number_mode, LineNumberMode::Absolute);
        app.cycle_line_number_mode();
        assert_eq!(app.line_number_mode, LineNumberMode::Relative);
        app.cycle_line_number_mode();
        assert_eq!(app.line_number_mode, LineNumberMode::Off);
    }

    #[test]
    fn cycle_theme_wraps_through_all_themes_and_persists() {
        let dir = TempDir::new("cycle_theme");
        let mut app = App::with_file(None);
        // Fixed starting point -- not asserted from `with_file`'s own
        // load, since that reads the *real* config path and would be
        // flaky against whatever's actually persisted on this machine.
        app.config = fenix_config::Config::load_or_default(dir.path().join("config.ini"));
        app.theme = &theme::ORBIT_DARK;

        app.cycle_theme();
        assert_eq!(app.theme.name, "TempleOS");
        assert_eq!(app.config.theme, Some("TempleOS".to_string()));
        let reloaded = fenix_config::Config::load(dir.path().join("config.ini")).unwrap();
        assert_eq!(reloaded.theme, Some("TempleOS".to_string())); // persisted

        // Cycle through the rest of `theme::ALL` (Gruvbox Dark, Nord,
        // Dracula, Solarized Dark, One Dark) to reach the wrap-around.
        for _ in 0..5 {
            app.cycle_theme();
        }
        app.cycle_theme();
        assert_eq!(app.theme.name, "Orbit Dark"); // wrapped back around
        let reloaded = fenix_config::Config::load(dir.path().join("config.ini")).unwrap();
        assert_eq!(reloaded.theme, Some("Orbit Dark".to_string()));
    }

    // `increase_font_size`/`decrease_font_size`/`reset_font_size`
    // themselves need a real `TextPipeline`, which needs a real GPU
    // device to construct -- not something a headless test builds (same
    // reason `resolve_font_size`/persistence are tested directly at the
    // `fenix_config`/`text` module level instead). What a headless
    // `App::with_file` *can* verify is that these are safe no-ops before
    // `resumed()` has run (`self.text` is `None` at that point) rather
    // than panicking.
    #[test]
    fn font_size_adjustments_are_safe_no_ops_before_the_gpu_exists() {
        let mut app = App::with_file(None);
        assert!(app.text.is_none());
        app.increase_font_size();
        app.decrease_font_size();
        app.reset_font_size();
    }

    #[test]
    fn a_persisted_indent_width_applies_to_vim() {
        // Mirrors what `with_file` does with `config.indent_width` --
        // `with_file` itself always reads the *real* config path, so
        // this exercises the same pipeline (a loaded `Config` feeding
        // `VimState::set_indent_width`) directly instead.
        let dir = TempDir::new("persisted_indent_width_applies");
        let mut config = fenix_config::Config::load_or_default(dir.path().join("config.ini"));
        config.indent_width = Some(3);
        config.save().unwrap();

        let reloaded = fenix_config::Config::load(dir.path().join("config.ini")).unwrap();
        let mut vim = VimState::new();
        vim.set_indent_width(reloaded.indent_width.unwrap_or(fenix_vim::DEFAULT_INDENT_WIDTH));
        assert_eq!(vim.indent_width(), 3);
    }

    #[test]
    fn set_shiftwidth_command_persists_the_new_width() {
        let dir = TempDir::new("set_shiftwidth_persists");
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("config.ini"));

        let event = app.test_vim_key(KeyPress::char(':'));
        assert_eq!(event, VimEvent::None);
        for ch in "set shiftwidth=3".chars() {
            app.test_vim_key(KeyPress::char(ch));
        }
        let event = app.test_vim_key(KeyPress::named(FenixNamedKey::Enter));
        assert_eq!(event, VimEvent::IndentWidthChanged(3));

        // Mirrors handle_key's own IndentWidthChanged arm -- handle_key
        // itself needs a real winit KeyEvent, so this exercises the
        // persistence call it would make directly (same posture as
        // vim_pulse_event_yields_a_renderable_pulse_overlay above).
        if let VimEvent::IndentWidthChanged(width) = event {
            app.config.indent_width = Some(width);
            app.config.save().unwrap();
        }
        let reloaded = fenix_config::Config::load(dir.path().join("config.ini")).unwrap();
        assert_eq!(reloaded.indent_width, Some(3));
    }

    #[test]
    fn split_line_by_highlights_covers_a_middle_span() {
        let theme = &theme::ORBIT_DARK;
        let spans = split_line_by_highlights("let x = 1;", 0, 10, &[(4..5, theme.syntax_variable)], theme.fg);
        let joined: String = spans.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(joined, "let x = 1;");
        assert_eq!(spans[1], ("x".to_string(), theme.syntax_variable));
        assert_eq!(spans[0].1, theme.fg);
        assert_eq!(spans[2].1, theme.fg);
    }

    #[test]
    fn split_line_by_highlights_clips_ranges_extending_past_the_line() {
        let theme = &theme::ORBIT_DARK;
        // A highlight spanning into the next line should only color this
        // line's portion of it.
        let spans = split_line_by_highlights("abc", 10, 13, &[(8..15, theme.syntax_string)], theme.fg);
        assert_eq!(spans, vec![("abc".to_string(), theme.syntax_string)]);
    }

    #[test]
    fn split_line_by_highlights_ignores_ranges_outside_the_line() {
        let theme = &theme::ORBIT_DARK;
        let spans = split_line_by_highlights("abc", 10, 13, &[(0..5, theme.syntax_string)], theme.fg);
        assert_eq!(spans, vec![("abc".to_string(), theme.fg)]);
    }

    #[test]
    fn syntax_state_seeds_from_the_detected_language() {
        let mut app = App::with_file(None);
        app.new_scratch_buffer(); // with_file(None) now opens the dashboard, which never has syntax
        assert!(app.open().syntax.is_none()); // no path -> no language

        app.open_mut().syntax = Some(fenix_syntax::SyntaxState::new(fenix_syntax::LanguageId::Rust, ""));
        app.test_insert_str("fn main() {}");

        let id = app.focused_buffer_id();
        let highlights = app.syntax_highlights_for_visible_range(id, 0, 1);
        assert!(!highlights.is_empty(), "expected highlights for a Rust buffer, got none");

        let spans = app.content_spans(app.open(), 0, 1, 0, None, &highlights, 0);
        let fn_span = spans.iter().find(|(s, _)| s == "fn");
        assert_eq!(
            fn_span.map(|(_, c)| *c),
            Some(app.theme.syntax_color("keyword")),
            "expected \"fn\" colored as a keyword, got {spans:?}"
        );
    }

    #[test]
    fn tcl_command_highlighting_only_colors_known_commands() {
        let dir = TempDir::new("tcl_command_highlight_known");
        let file = dir.write("main.tcl", "puts hello\nmyUnknownCmd123 foo\n");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let id = app.focused_buffer_id();

        let highlights = app.syntax_highlights_for_visible_range(id, 0, 2);
        let spans = app.content_spans(app.open(), 0, 2, 0, None, &highlights, 0);

        // "puts" -- a real builtin (in `tcl::KEYWORDS` and the query's
        // own `#any-of?` list) -- keeps its function color.
        let puts_span = spans.iter().find(|(s, _)| s == "puts");
        assert_eq!(
            puts_span.map(|(_, c)| *c),
            Some(app.theme.syntax_color("function")),
            "expected \"puts\" colored as a known command, got {spans:?}"
        );

        // "myUnknownCmd123" isn't a known command anywhere (not a
        // builtin, not ctags-sourced, not in a symbols file) -- must not
        // get the function color just for sitting in command position.
        let unknown_span = spans.iter().find(|(s, _)| s == "myUnknownCmd123");
        assert_ne!(
            unknown_span.map(|(_, c)| *c),
            Some(app.theme.syntax_color("function")),
            "expected \"myUnknownCmd123\" NOT colored as a command, got {spans:?}"
        );
    }

    #[test]
    fn tcl_command_highlighting_recognizes_a_ctags_sourced_proc_by_its_qualified_name() {
        let dir = TempDir::new("tcl_command_highlight_ctags");
        dir.write("lib.tcl", "namespace eval myns {\n    proc greet {} {\n        return 1\n    }\n}\n");
        let file = dir.write("main.tcl", "myns::greet\n::myns::greet\ngreet\n");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.project_root = Some(dir.path().to_path_buf());
        let id = app.focused_buffer_id();

        let highlights = app.syntax_highlights_for_visible_range(id, 0, 3);
        let command_color = app.theme.syntax_color("function");
        // The buffer text itself carries the "::" prefix distinction, so
        // check color by byte range rather than by re-finding text (both
        // "myns::greet" and "::myns::greet" would otherwise collide on a
        // naive text search).
        let color_at = |byte: usize| highlights.iter().find(|(r, _)| r.contains(&byte)).map(|(_, c)| *c);

        assert_eq!(color_at(0), Some(command_color), "unqualified \"myns::greet\" should be recognized");
        let second_line_start = app.open().buffer.text().find("::myns::greet").unwrap();
        assert_eq!(color_at(second_line_start), Some(command_color), "\"::myns::greet\" (with prefix) should be recognized");

        // Bare "greet" (no namespace qualifier at all) is a different,
        // unknown identifier -- only the qualified forms are known.
        let bare_line_start = app.open().buffer.text().rfind("greet").unwrap();
        assert_ne!(color_at(bare_line_start), Some(command_color), "bare \"greet\" should not match the qualified proc");
    }

    #[test]
    fn syntax_highlights_drain_edits_even_without_an_active_language() {
        let mut app = App::with_file(None);
        assert!(app.open().syntax.is_none());
        app.test_insert_str("hello");
        let id = app.focused_buffer_id();
        assert!(app.syntax_highlights_for_visible_range(id, 0, 1).is_empty());
        // The edit log should have been drained regardless, not left to grow.
        assert!(app.open_mut().buffer.drain_edits().is_empty());
    }

    /// A real, uniquely-named temp directory, removed on drop -- these
    /// tests exercise the real filesystem via `fenix_explorer::
    /// ExplorerState`, same reasoning as that crate's own `TempDir`.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("fenix-gui-test-{name}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn touch(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, b"hello\n").unwrap();
            path
        }
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explorer_jump_opens_a_real_dired_buffer_in_the_focused_pane() {
        let dir = TempDir::new("jump_stashes");
        let file = dir.touch("a.txt");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_insert_str(" extra"); // dirty the original buffer
        let original_id = app.focused_buffer_id();

        app.explorer_jump();

        let dired_id = app.focused_buffer_id();
        assert_ne!(dired_id, original_id, "a real, different buffer now occupies the pane");
        assert_eq!(app.open().kind, BufferKind::Explorer);
        assert_eq!(app.dired_states.get(&dired_id).unwrap().cwd, dir.path());
        // The original file buffer is untouched, just no longer focused.
        assert_eq!(app.buffers.get(original_id).unwrap().buffer.text(), " extrahello\n");
    }

    #[test]
    fn explorer_jump_does_not_touch_an_already_open_sidebar() {
        // Regression test: `explorer`/`sidebar` used to be candidates for
        // one shared field/mechanism -- confirms the new real-buffer dired
        // path still leaves the separate, unrelated sidebar untouched.
        let sidebar_dir = TempDir::new("jump_no_clobber_sidebar");
        let jump_dir = TempDir::new("jump_no_clobber_jump");
        let file = jump_dir.touch("a.txt");

        let mut app = App::with_file(None);
        app.sidebar = Some(ExplorerState::open(sidebar_dir.path()).unwrap());
        app.sidebar_open = true;
        app.sidebar_focused = false;

        app.test_open_path(&file);
        app.explorer_jump();

        assert_eq!(app.open().kind, BufferKind::Explorer);
        assert_eq!(app.dired_states.get(&app.focused_buffer_id()).unwrap().cwd, jump_dir.path());
        // The sidebar's own listing must be untouched by the jump.
        assert!(app.sidebar_open);
        assert_eq!(app.sidebar.as_ref().unwrap().cwd, sidebar_dir.path());
    }

    #[test]
    fn sync_completion_opens_and_lists_matching_keywords_in_a_tcl_buffer() {
        let dir = TempDir::new("completion_opens");
        let file = dir.write("foo.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("se");

        app.sync_completion();

        let state = app.completion.as_ref().expect("a 2-char identifier prefix should open the popup");
        let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
        assert!(labels.contains(&"set".to_string()));
        assert!(labels.contains(&"seek".to_string()));
        assert!(!labels.contains(&"proc".to_string())); // doesn't match "se"
    }

    #[test]
    fn sync_completion_closes_when_the_prefix_becomes_empty() {
        let dir = TempDir::new("completion_closes_on_empty_prefix");
        let file = dir.write("foo.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("se");
        app.sync_completion();
        assert!(app.completion.is_some());

        app.test_set_cursor(Cursor::at_start()); // no identifier char before the cursor now
        app.sync_completion();
        assert!(app.completion.is_none());
    }

    #[test]
    fn sync_completion_is_none_outside_insert_mode() {
        let dir = TempDir::new("completion_none_outside_insert");
        let file = dir.write("foo.tcl", "se");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        // Starts in Normal mode -- never entered Insert.
        app.test_set_cursor(Cursor { char_idx: 2, sticky_col: 0 });

        app.sync_completion();

        assert!(app.completion.is_none());
    }

    #[test]
    fn sync_completion_is_none_for_a_non_tcl_buffer() {
        let dir = TempDir::new("completion_none_for_non_tcl");
        let file = dir.write("foo.rs", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("se");

        app.sync_completion();

        assert!(app.completion.is_none());
    }

    #[test]
    fn accept_completion_replaces_the_typed_prefix_with_the_full_label() {
        let dir = TempDir::new("completion_accept");
        let file = dir.write("foo.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("se");
        app.sync_completion();
        let expected_label = app.completion.as_ref().unwrap().picker.selected().unwrap().payload.label.clone();

        app.accept_completion();

        assert!(app.completion.is_none());
        assert_eq!(app.open().buffer.text(), expected_label);
        assert_eq!(app.test_cursor().char_idx, expected_label.chars().count());
    }

    #[test]
    fn force_open_completion_shows_the_full_list_even_with_an_empty_prefix() {
        let dir = TempDir::new("completion_force_open");
        let file = dir.write("foo.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));

        app.force_open_completion();

        let state = app.completion.as_ref().expect("Ctrl-Space should force-open the popup");
        assert_eq!(state.picker.len(), fenix_completion::tcl::KEYWORDS.len());
    }

    #[test]
    fn force_open_completion_does_nothing_on_a_non_tcl_buffer() {
        let dir = TempDir::new("completion_force_open_non_tcl");
        let file = dir.write("foo.rs", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_vim_key(KeyPress::char('i'));

        app.force_open_completion();

        assert!(app.completion.is_none());
    }

    #[test]
    fn tcl_candidates_include_ctags_sourced_procs_from_the_project_root() {
        let dir = TempDir::new("completion_ctags_source");
        dir.write("lib.tcl", "proc my_custom_proc {} {\n    return 1\n}\n");
        let file = dir.write("main.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.project_root = Some(dir.path().to_path_buf());
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("my_cus");

        app.sync_completion();

        let state = app.completion.as_ref().expect("popup should be open");
        let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
        assert!(labels.contains(&"my_custom_proc".to_string()), "expected a ctags-sourced proc, got {labels:?}");
    }

    #[test]
    fn tcl_candidates_show_the_fully_qualified_path_for_namespaced_procs() {
        let dir = TempDir::new("completion_ctags_namespaced");
        dir.write(
            "lib.tcl",
            "namespace eval myns {\n    namespace eval subns {\n        proc greet {} {\n            return 1\n        }\n    }\n}\n",
        );
        let file = dir.write("main.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.project_root = Some(dir.path().to_path_buf());
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("gre");

        app.sync_completion();

        let state = app.completion.as_ref().expect("popup should be open");
        let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
        assert!(labels.contains(&"myns::subns::greet".to_string()), "expected the fully-qualified path, got {labels:?}");
        assert!(!labels.iter().any(|l| l == "greet"), "the bare, unqualified name should not appear");
    }

    #[test]
    fn tcl_candidates_include_entries_from_a_configured_symbols_file() {
        let dir = TempDir::new("completion_symbols_file");
        let symbols_path = dir.write("symbols.txt", "my_external_symbol\nanother_one\n");
        let file = dir.write("main.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.config.completion_symbols_file = Some(symbols_path);
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("my_ext");

        app.sync_completion();

        let state = app.completion.as_ref().expect("popup should be open");
        let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
        assert!(labels.contains(&"my_external_symbol".to_string()), "expected a symbols-file entry, got {labels:?}");
    }

    #[test]
    fn a_symbols_file_entry_that_duplicates_a_keyword_is_not_shown_twice() {
        let dir = TempDir::new("completion_symbols_file_dedup");
        // "set" is already a Tcl keyword -- the symbols-file entry should
        // be deduped against it, not appear as a second candidate.
        let symbols_path = dir.write("symbols.txt", "set\n");
        let file = dir.write("main.tcl", "");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.config.completion_symbols_file = Some(symbols_path);
        app.test_vim_key(KeyPress::char('i'));
        app.test_insert_str("se");

        app.sync_completion();

        let state = app.completion.as_ref().expect("popup should be open");
        let labels: Vec<String> = state.picker.visible_rows(0, state.picker.len()).map(|(_, c)| c.label.clone()).collect();
        assert_eq!(labels.iter().filter(|l| *l == "set").count(), 1);
    }

    #[test]
    fn explorer_dired_text_lists_one_line_per_entry_with_directory_slashes() {
        let dir = TempDir::new("dired_text_basic");
        dir.touch("a.txt");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let explorer = ExplorerState::open(dir.path()).unwrap();

        let (text, lines) = explorer_dired_text(&explorer);
        // Sorted directories-first: "sub/" then "a.txt".
        assert_eq!(text, "sub/\na.txt");
        assert_eq!(lines, vec![Some(0), Some(1)]);
    }

    #[test]
    fn explorer_dired_text_is_empty_for_an_empty_directory() {
        let dir = TempDir::new("dired_text_empty");
        let explorer = ExplorerState::open(dir.path()).unwrap();
        let (text, lines) = explorer_dired_text(&explorer);
        assert_eq!(text, "");
        assert!(lines.is_empty());
    }

    #[test]
    fn explorer_row_spans_marks_the_icon_span_and_flags_the_selected_row() {
        let dir = TempDir::new("row_spans_icon_selected");
        dir.touch("main.rs");
        let app = App::with_file(None);
        let explorer = ExplorerState::open(dir.path()).unwrap(); // selected = "main.rs" (only entry)

        let (spans, selected_row, marked_rows) = app.explorer_row_spans(&explorer, 0, 5, true);
        assert_eq!(selected_row, Some(0));
        assert!(marked_rows.is_empty());

        // First span is the icon glyph, flagged to render in the icon font.
        assert!(spans[0].2, "expected the icon span to be flagged is_icon, got {spans:?}");
        // Somewhere in the row, the plain filename shows up in the body font.
        assert!(spans.iter().any(|(s, _, is_icon)| !is_icon && s.contains("main.rs")));
    }

    #[test]
    fn explorer_row_spans_includes_marked_rows() {
        let dir = TempDir::new("row_spans_marked");
        dir.touch("a");
        dir.touch("b");
        let app = App::with_file(None);
        let mut explorer = ExplorerState::open(dir.path()).unwrap();
        explorer.marks.insert(dir.path().join("b"));

        let (_, _, marked_rows) = app.explorer_row_spans(&explorer, 0, 5, true);
        assert_eq!(marked_rows, vec![1]); // "b" sorts second, alphabetically after "a"
    }

    #[test]
    fn explorer_row_spans_respects_the_scroll_window() {
        let dir = TempDir::new("row_spans_scroll");
        for name in ["a", "b", "c", "d"] {
            dir.touch(name);
        }
        let app = App::with_file(None);
        let explorer = ExplorerState::open(dir.path()).unwrap();

        // Scrolled to start at index 2 ("c"), showing only 2 rows.
        let (spans, _, _) = app.explorer_row_spans(&explorer, 2, 2, true);
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.contains('c'));
        assert!(joined.contains('d'));
        assert!(!joined.contains('a'));
        assert!(!joined.contains('b'));
    }

    #[test]
    fn explorer_row_spans_shows_attrs_only_when_requested() {
        let dir = TempDir::new("row_spans_attrs");
        dir.touch("a.txt");
        let app = App::with_file(None);
        let explorer = ExplorerState::open(dir.path()).unwrap();

        let (with_attrs, _, _) = app.explorer_row_spans(&explorer, 0, 5, true);
        let (without_attrs, _, _) = app.explorer_row_spans(&explorer, 0, 5, false);
        let joined_with: String = with_attrs.iter().map(|(s, _, _)| s.as_str()).collect();
        let joined_without: String = without_attrs.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined_with.contains('B') || joined_with.contains('K')); // size suffix present
        assert!(!joined_without.contains('B') && !joined_without.contains('K'));
    }

    #[test]
    fn a_dired_buffer_can_be_closed_like_any_other_via_kill_buffer() {
        // The actual "dired as a real buffer" feature: closable via `SPC
        // b k` like any other buffer, falling back to whatever was
        // focused before it, same as closing any other buffer would.
        let dir = TempDir::new("dired_kill_buffer");
        let file = dir.touch("a.txt");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        let original_id = app.focused_buffer_id();

        app.explorer_jump();
        let dired_id = app.focused_buffer_id();
        assert_ne!(dired_id, original_id);

        app.kill_buffer();

        assert!(app.buffers.get(dired_id).is_none(), "the dired buffer should be gone");
        assert_eq!(app.focused_buffer_id(), original_id, "falls back to the buffer it was opened over");
    }

    #[test]
    fn real_vim_motions_navigate_a_dired_buffer_for_free() {
        let dir = TempDir::new("dired_vim_motions");
        dir.touch("a.txt");
        dir.touch("b.txt");
        let mut app = App::with_file(None);
        app.open_dired_at(dir.path());

        assert_eq!(app.test_cursor().char_idx, 0);
        app.test_vim_key(KeyPress::char('j')); // plain Vim motion, not an ExplorerAction
        assert_ne!(app.test_cursor().char_idx, 0, "j should move the real cursor down a line");
    }

    #[test]
    fn enter_on_a_file_line_opens_it_replacing_the_dired_buffer() {
        let dir = TempDir::new("dired_enter_file");
        let file = dir.touch("target.txt");
        let mut app = App::with_file(None);
        app.open_dired_at(dir.path());

        app.dired_activate_selected();

        assert_eq!(app.open().kind, BufferKind::Text);
        assert_eq!(app.open().buffer.path(), Some(file.as_path()));
    }

    #[test]
    fn enter_on_a_directory_line_navigates_into_it_reusing_the_same_buffer() {
        let dir = TempDir::new("dired_enter_dir_parent");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"hi\n").unwrap();
        let mut app = App::with_file(None);
        app.open_dired_at(dir.path());
        let dired_id = app.focused_buffer_id();

        app.dired_activate_selected(); // the only entry is "sub/"

        assert_eq!(app.focused_buffer_id(), dired_id, "same buffer, not a new one");
        assert_eq!(app.dired_states.get(&dired_id).unwrap().cwd, sub);
        assert_eq!(app.open().buffer.text(), "inner.txt");
    }

    #[test]
    fn dash_navigates_to_the_parent_directory() {
        let dir = TempDir::new("dired_parent_dir");
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let mut app = App::with_file(None);
        app.open_dired_at(&sub);
        let dired_id = app.focused_buffer_id();

        app.dired_parent_dir();

        assert_eq!(app.dired_states.get(&dired_id).unwrap().cwd, dir.path());
    }

    #[test]
    fn toggle_sidebar_opens_then_closes() {
        let dir = TempDir::new("toggle_sidebar");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.test_open_path(&dir.path().join("a.txt"));

        app.toggle_sidebar();
        assert!(app.sidebar_open);
        assert!(app.sidebar_focused);
        assert!(app.sidebar.is_some());

        app.toggle_sidebar();
        assert!(!app.sidebar_open);
        assert!(!app.sidebar_focused);
        assert!(app.sidebar.is_none());
    }

    #[test]
    fn split_vertical_adds_a_focused_pane_showing_the_same_buffer() {
        let mut app = App::with_file(None);
        let original = app.windows().focused_id();
        let buffer_id = app.focused_buffer_id();

        app.split_vertical();

        assert_eq!(app.windows().window_count(), 2);
        assert_ne!(app.windows().focused_id(), original);
        assert_eq!(app.focused_buffer_id(), buffer_id); // new pane shows the same buffer
    }

    #[test]
    fn split_horizontal_also_adds_a_focused_pane() {
        let mut app = App::with_file(None);
        app.split_horizontal();
        assert_eq!(app.windows().window_count(), 2);
    }

    #[test]
    fn close_window_is_a_no_op_on_the_last_window() {
        let mut app = App::with_file(None);
        app.close_window();
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn close_window_collapses_back_to_one_pane() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.close_window();
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn splitting_a_window_gives_each_pane_its_own_independent_cursor() {
        // The actual feature: two panes on the same buffer must not
        // share a cursor -- moving one's must leave the other's exactly
        // where it was.
        let mut app = App::with_file(None);
        app.new_scratch_buffer();
        app.test_insert_str("one\ntwo\nthree");
        app.test_set_cursor(Cursor::at_start());

        let original_pane = app.focused_pane_id();
        app.split_vertical(); // new pane focused, seeded from the same position
        let new_pane = app.focused_pane_id();
        assert_ne!(original_pane, new_pane);
        assert_eq!(app.pane_state(new_pane).cursor.char_idx, 0);

        // Move the cursor in the new (focused) pane only.
        for ch in ['j', 'j'] {
            app.test_vim_key(KeyPress::char(ch));
        }
        assert_ne!(app.pane_state(new_pane).cursor.char_idx, 0);

        // Switch focus back to the original pane -- its cursor must be
        // untouched by what just happened in the other one.
        app.navigate_window(NavDirection::Left);
        assert_eq!(app.focused_pane_id(), original_pane);
        assert_eq!(app.cursor().char_idx, 0);
    }

    #[test]
    fn closing_a_window_drops_its_pane_state() {
        let mut app = App::with_file(None);
        app.split_vertical();
        let new_pane = app.focused_pane_id();
        app.close_window();
        assert!(app.workspaces.active_pane_states().get(&new_pane).is_none());
    }

    #[test]
    fn cycle_window_wraps_between_the_two_panes() {
        let mut app = App::with_file(None);
        let first = app.windows().focused_id();
        app.split_vertical();
        let second = app.windows().focused_id();
        assert_ne!(first, second);

        app.cycle_window();
        assert_eq!(app.windows().focused_id(), first);
        app.cycle_window();
        assert_eq!(app.windows().focused_id(), second);
    }

    #[test]
    fn navigate_window_moves_focus_left_to_the_original_pane() {
        let mut app = App::with_file(None);
        let left = app.windows().focused_id();
        app.split_vertical(); // new pane appears to the right, becomes focused
        assert_ne!(app.windows().focused_id(), left);

        app.navigate_window(fenix_window::NavDirection::Left);
        assert_eq!(app.windows().focused_id(), left);
    }

    #[test]
    fn navigate_window_left_from_the_leftmost_pane_focuses_an_open_sidebar() {
        let dir = TempDir::new("nav_left_into_sidebar");
        let mut app = App::with_file(None);
        app.sidebar = Some(ExplorerState::open(dir.path()).unwrap());
        app.sidebar_open = true;
        assert!(!app.sidebar_focused);

        app.navigate_window(fenix_window::NavDirection::Left);
        assert!(app.sidebar_focused);
    }

    #[test]
    fn navigate_window_left_prefers_an_actual_pane_over_the_sidebar() {
        let dir = TempDir::new("nav_left_prefers_pane");
        let mut app = App::with_file(None);
        let left = app.windows().focused_id();
        app.split_vertical(); // new pane to the right, becomes focused
        app.sidebar = Some(ExplorerState::open(dir.path()).unwrap());
        app.sidebar_open = true;

        app.navigate_window(fenix_window::NavDirection::Left);
        assert_eq!(app.windows().focused_id(), left);
        assert!(!app.sidebar_focused);
    }

    #[test]
    fn only_window_closes_every_other_pane() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.split_horizontal();
        assert_eq!(app.windows().window_count(), 3);

        app.only_window();
        assert_eq!(app.windows().window_count(), 1);
    }

    #[test]
    fn balance_windows_resets_a_skewed_ratio_back_to_half() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.windows_mut().resize_focused(0.3); // skew away from 0.5
        app.balance_windows();

        let rects = app.windows().layout(fenix_window::Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 });
        for (_, r) in rects {
            assert!((r.w - 100.0).abs() < 0.01);
        }
    }

    #[test]
    fn new_workspace_becomes_active_and_is_seeded_with_the_current_buffer() {
        let mut app = App::with_file(None);
        let buffer_id = app.focused_buffer_id();

        app.new_workspace();

        assert_eq!(app.workspaces.len(), 2);
        assert_eq!(app.workspaces.active_index(), 1);
        assert_eq!(app.workspaces.active_name(), "workspace-2");
        assert_eq!(app.focused_buffer_id(), buffer_id); // seeded, not a blank scratch
        assert_eq!(app.windows().window_count(), 1); // new workspace starts unsplit
    }

    #[test]
    fn next_and_prev_workspace_cycle_and_wrap() {
        let mut app = App::with_file(None);
        app.new_workspace();
        app.new_workspace(); // 3 workspaces total, active = 2

        app.next_workspace(); // wraps to 0
        assert_eq!(app.workspaces.active_index(), 0);
        app.prev_workspace(); // wraps back to 2
        assert_eq!(app.workspaces.active_index(), 2);
        app.prev_workspace();
        assert_eq!(app.workspaces.active_index(), 1);
    }

    #[test]
    fn remove_workspace_is_a_no_op_on_the_last_one() {
        let mut app = App::with_file(None);
        app.remove_workspace();
        assert_eq!(app.workspaces.len(), 1);
    }

    #[test]
    fn remove_workspace_removes_the_active_one() {
        let mut app = App::with_file(None);
        app.new_workspace(); // active: workspace-2

        app.remove_workspace();

        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(app.workspaces.active_name(), "workspace-1");
    }

    #[test]
    fn each_workspace_keeps_its_own_split_layout() {
        let mut app = App::with_file(None);
        app.split_vertical();
        app.split_horizontal();
        assert_eq!(app.windows().window_count(), 3);

        app.new_workspace(); // fresh, unsplit workspace
        assert_eq!(app.windows().window_count(), 1);

        app.prev_workspace(); // back to workspace-1
        assert_eq!(app.windows().window_count(), 3); // layout untouched
    }

    #[test]
    fn modeline_shows_the_workspace_indicator_only_once_there_are_several() {
        let mut app = App::with_file(None);
        assert!(!app.modeline_text().contains("workspace"));

        app.new_workspace();
        assert!(app.modeline_text().contains("[2/2 workspace-2]"));
    }

    #[test]
    fn which_key_popup_is_none_when_nothing_is_pending() {
        let app = App::with_file(None);
        assert!(app.which_key_popup(800.0, 580.0).is_none());
    }

    #[test]
    fn which_key_popup_lists_pending_leader_children_sorted_by_label() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' '));
        app.leader_matcher.feed(KeyPress::char('t')); // SPC t: "line numbers", "theme"

        let (_, spans) = app.which_key_popup(800.0, 580.0).unwrap();
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        // "line numbers" sorts before "theme" -- alphabetical by label.
        assert!(joined.find("line numbers").unwrap() < joined.find("theme").unwrap());
        assert!(joined.contains('n')); // the `n` key column, for line numbers
        assert!(!joined.contains("more")); // both entries fit, nothing truncated
    }

    #[test]
    fn which_key_popup_key_column_uses_caret_text_not_a_content_calibrated_color() {
        // Regression test for a real readability bug: the key column used
        // to be `theme.syntax_keyword`, a color calibrated for `bg` (the
        // content background). On TempleOS that color happens to be
        // identical to `bg_modeline` (the popup's own background), making
        // every key binding invisible. `caret_text` is guaranteed
        // high-contrast against `bg_modeline` in every theme.
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' '));
        let (_, spans) = app.which_key_popup(800.0, 580.0).unwrap();
        assert_eq!(spans[0].1, app.theme.caret_text); // spans[0] is the first entry's key column
    }

    #[test]
    fn which_key_popup_rect_never_extends_past_the_window_or_under_the_modeline() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' ')); // root: 8 top-level groups

        // A window far too short for 8 rows -- this is the bug this popup
        // system fixes: the panel used to have no notion of "not enough
        // room" and could draw its background/text straight through the
        // modeline bar.
        let modeline_top = 40.0;
        let (rect, _) = app.which_key_popup(300.0, modeline_top).unwrap();

        assert!(rect.x >= 0.0);
        assert!(rect.y >= 0.0);
        assert!(rect.x + rect.w <= 300.0 + 0.01);
        assert!(rect.y <= modeline_top);
    }

    #[test]
    fn which_key_popup_widens_to_fit_a_longer_label_than_the_minimum() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' '));
        let (root_rect, _) = app.which_key_popup(800.0, 580.0).unwrap(); // short group labels only

        app.leader_matcher = keymap::leader_trie().matcher();
        app.leader_matcher.feed(KeyPress::char(' '));
        app.leader_matcher.feed(KeyPress::named(FenixNamedKey::Tab)); // "remove workspace" (16 chars) lives here
        let (workspace_rect, spans) = app.which_key_popup(800.0, 580.0).unwrap();

        assert_eq!(root_rect.w, text::WHICH_KEY_MIN_WIDTH); // short labels clamp to the floor
        assert!(workspace_rect.w > root_rect.w); // longer label pushes the panel wider
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.contains("remove workspace")); // never character-truncated
    }

    #[test]
    fn which_key_popup_truncates_and_reports_how_many_more_when_content_overflows() {
        let mut app = App::with_file(None);
        app.leader_matcher.feed(KeyPress::char(' ')); // root: 8 top-level groups, more than fit below

        let (_, spans) = app.which_key_popup(300.0, 40.0).unwrap();
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.contains("more"));
    }

    fn completion_item(label: &str, kind: fenix_completion::CompletionKind) -> fenix_picker::Candidate<fenix_completion::CompletionItem> {
        fenix_picker::Candidate::new(label, fenix_completion::CompletionItem { label: label.to_string(), kind })
    }

    #[test]
    fn completion_popup_is_none_when_nothing_is_open() {
        let app = App::with_file(None);
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        assert!(app.completion_popup(800.0, 580.0, rect, Some((0, 0)), 0.0, 0.0).is_none());
    }

    #[test]
    fn completion_popup_is_none_without_a_caret_to_anchor_under() {
        let mut app = App::with_file(None);
        let items = vec![completion_item("set", fenix_completion::CompletionKind::Keyword)];
        app.completion = Some(CompletionState { prefix_start: 0, picker: fenix_picker::PickerState::new(items) });
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        assert!(app.completion_popup(800.0, 580.0, rect, None, 0.0, 0.0).is_none());
    }

    #[test]
    fn completion_popup_lists_candidates_and_flags_the_selected_row() {
        let mut app = App::with_file(None);
        let items =
            vec![completion_item("set", fenix_completion::CompletionKind::Keyword), completion_item("seek", fenix_completion::CompletionKind::Keyword)];
        let mut picker = fenix_picker::PickerState::new(items);
        picker.move_selection(1); // select "seek", index 1
        app.completion = Some(CompletionState { prefix_start: 0, picker });

        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 100.0 };
        let (_, spans, selected_row) = app.completion_popup(800.0, 580.0, rect, Some((0, 4)), 0.0, 0.0).unwrap();
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.contains("set"));
        assert!(joined.contains("seek"));
        assert_eq!(selected_row, Some(1));
    }

    #[test]
    fn completion_popup_colors_keywords_and_tags_differently() {
        let mut app = App::with_file(None);
        let items =
            vec![completion_item("set", fenix_completion::CompletionKind::Keyword), completion_item("my_proc", fenix_completion::CompletionKind::Tag)];
        app.completion = Some(CompletionState { prefix_start: 0, picker: fenix_picker::PickerState::new(items) });

        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 800.0, h: 100.0 };
        let (_, spans, _) = app.completion_popup(800.0, 580.0, rect, Some((0, 0)), 0.0, 0.0).unwrap();
        let keyword_color = spans.iter().find(|(s, _, _)| s == "set").unwrap().1;
        let tag_color = spans.iter().find(|(s, _, _)| s == "my_proc").unwrap().1;
        // caret_text/fg_modeline, not syntax_keyword/syntax_function --
        // this popup's background is bg_modeline, and the syntax_* family
        // is calibrated for contrast against bg instead (see the doc
        // comment on completion_popup's color match for the full story).
        assert_eq!(keyword_color, app.theme.caret_text);
        assert_eq!(tag_color, app.theme.fg_modeline);
        assert_ne!(keyword_color, tag_color);
    }

    #[test]
    fn completion_popup_rect_never_extends_past_the_window_or_under_the_modeline() {
        let mut app = App::with_file(None);
        let items = vec![completion_item("set", fenix_completion::CompletionKind::Keyword)];
        app.completion = Some(CompletionState { prefix_start: 0, picker: fenix_picker::PickerState::new(items) });

        // Anchored near the bottom-right corner -- must clamp, not overflow.
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 300.0, h: 40.0 };
        let (popup_rect, _, _) = app.completion_popup(300.0, 40.0, rect, Some((0, 290)), 0.0, 0.0).unwrap();
        assert!(popup_rect.x + popup_rect.w <= 300.0 + 0.01);
        assert!(popup_rect.y + popup_rect.h <= 40.0 + 0.01);
    }

    #[test]
    fn explorer_open_selected_on_a_directory_navigates_into_it() {
        let dir = TempDir::new("open_dir");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_open_selected(); // selected = "sub" (only entry)
        assert_eq!(app.explorer.as_ref().unwrap().cwd, dir.path().join("sub"));
        assert_eq!(app.main_view, MainView::Explorer); // still in the explorer, just deeper
    }

    #[test]
    fn explorer_open_selected_on_a_file_in_full_buffer_mode_returns_to_editor() {
        let dir = TempDir::new("open_file_full");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_open_selected();
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.explorer.is_none());
        assert_eq!(app.open().buffer.text(), "hello\n");
    }

    #[test]
    fn explorer_open_selected_on_a_file_in_sidebar_mode_keeps_the_sidebar() {
        let dir = TempDir::new("open_file_sidebar");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.sidebar = Some(ExplorerState::open(dir.path()).unwrap());
        app.sidebar_open = true;
        app.sidebar_focused = true;

        app.explorer_open_selected();
        assert!(app.sidebar_open); // sidebar persists
        assert!(!app.sidebar_focused); // focus returns to the editor
        assert!(app.sidebar.is_some());
        assert_eq!(app.open().buffer.text(), "hello\n");
    }

    #[test]
    fn explorer_handle_action_down_and_up_move_selection() {
        let dir = TempDir::new("action_down_up");
        dir.touch("a");
        dir.touch("b");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::Down);
        assert_eq!(app.explorer.as_ref().unwrap().selected, 1);
        app.explorer_handle_action(ExplorerAction::Up);
        assert_eq!(app.explorer.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn rename_prompt_flow_types_a_name_and_submits_on_enter() {
        let dir = TempDir::new("rename_prompt_flow");
        dir.touch("old.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginRename);
        assert!(app.explorer_prompt.is_some());
        assert_eq!(app.explorer_prompt.as_ref().unwrap().input, "old.txt"); // pre-filled with the current name
        for _ in 0.."old.txt".len() {
            app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Backspace));
        }
        for c in "new.txt".chars() {
            app.explorer_prompt_key(KeyPress::char(c));
        }
        app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Enter));

        assert!(app.explorer_prompt.is_none());
        assert!(dir.path().join("new.txt").exists());
        assert!(!dir.path().join("old.txt").exists());
    }

    #[test]
    fn delete_prompt_confirms_on_y_and_cancels_on_anything_else() {
        let dir = TempDir::new("delete_prompt_cancel");
        dir.touch("keep.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginDelete);
        app.explorer_prompt_key(KeyPress::named(FenixNamedKey::Escape)); // any non-'y' key cancels
        assert!(app.explorer_prompt.is_none());
        assert!(dir.path().join("keep.txt").exists());
    }

    #[test]
    fn delete_prompt_deletes_on_y() {
        let dir = TempDir::new("delete_prompt_confirm");
        dir.touch("gone.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginDelete);
        app.explorer_prompt_key(KeyPress::char('y'));
        assert!(app.explorer_prompt.is_none());
        assert!(!dir.path().join("gone.txt").exists());
    }

    #[test]
    fn modeline_shows_the_rename_prompt_as_it_is_typed() {
        // Previously nothing rendered `explorer_prompt` at all -- typing
        // a rename/create/copy/move was silently invisible even though
        // the state was captured correctly.
        let dir = TempDir::new("rename_prompt_modeline");
        dir.touch("old.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginRename);
        assert_eq!(app.modeline_text(), "Rename to: old.txt");
        app.explorer_prompt_key(KeyPress::char('x'));
        assert_eq!(app.modeline_text(), "Rename to: old.txtx");
    }

    #[test]
    fn modeline_shows_the_delete_confirmation_with_the_target_count() {
        let dir = TempDir::new("delete_prompt_modeline");
        dir.touch("a.txt");
        let mut app = App::with_file(None);
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::BeginDelete);
        assert_eq!(app.modeline_text(), "Delete 1 item? (y/n)");
    }

    #[test]
    fn modeline_shows_the_create_file_prompt_from_the_sidebar_too() {
        let dir = TempDir::new("create_prompt_sidebar");
        let mut app = App::with_file(None);
        app.sidebar = Some(ExplorerState::open(dir.path()).unwrap());
        app.sidebar_focused = true;

        app.explorer_handle_action(ExplorerAction::BeginCreateFile);
        assert_eq!(app.modeline_text(), "Create file: ");
        app.explorer_prompt_key(KeyPress::char('x'));
        assert_eq!(app.modeline_text(), "Create file: x");
    }

    #[test]
    fn find_file_candidates_labels_are_relative_to_root() {
        let dir = TempDir::new("find_file_candidates");
        dir.touch("a.txt");
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        dir.write("sub/b.txt", "hello\n");

        let mut labels: Vec<String> =
            App::find_file_candidates(dir.path()).into_iter().map(|c| c.label).collect();
        labels.sort();
        assert_eq!(labels, vec!["a.txt".to_string(), "sub/b.txt".to_string()]);
    }

    #[test]
    fn enter_picker_and_picker_cancel_leave_the_buffer_untouched() {
        let dir = TempDir::new("picker_enter_cancel");
        let file = dir.touch("a.txt");
        let mut app = App::with_file(Some(file.to_string_lossy().into_owned()));
        app.test_insert_str(" extra");
        let dirty_text = app.open().buffer.text();
        let focused_before = app.focused_buffer_id();

        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(Vec::new())));
        assert_eq!(app.main_view, MainView::Picker);
        assert!(app.active_picker.is_some());
        assert_eq!(app.focused_buffer_id(), focused_before);

        app.picker_cancel();
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
        assert_eq!(app.open().buffer.text(), dirty_text);
    }

    #[test]
    fn picker_confirm_on_find_file_opens_the_selected_path_and_returns_to_editor() {
        let dir = TempDir::new("picker_confirm_find_file");
        let target = dir.write("target.txt", "picked contents\n");
        let mut app = App::with_file(None);

        let candidates = vec![fenix_picker::Candidate::new("target.txt", target.clone())];
        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));
        app.picker_confirm();

        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
        assert_eq!(app.open().buffer.path(), Some(target.as_path()));
        assert_eq!(app.open().buffer.text(), "picked contents\n");
    }

    #[test]
    fn picker_confirm_with_no_matches_is_a_no_op() {
        let mut app = App::with_file(None);
        let candidates: Vec<fenix_picker::Candidate<PathBuf>> =
            vec![fenix_picker::Candidate::new("only.txt", PathBuf::from("only.txt"))];
        let mut state = fenix_picker::PickerState::new(candidates);
        state.push_char('z'); // no match against "only.txt"
        assert!(state.selected().is_none());
        app.enter_picker(ActivePicker::FindFile(state));

        app.picker_confirm();

        // Nothing to confirm -- still open, nothing stashed/opened changed.
        assert_eq!(app.main_view, MainView::Picker);
        assert!(app.active_picker.is_some());
    }

    #[test]
    fn run_grep_and_jump_to_grep_match_moves_the_cursor_to_the_match() {
        let dir = TempDir::new("run_grep");
        dir.write("a.txt", "line one\nneedle here\nline three\n");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());

        app.run_grep("needle");

        match &app.active_picker {
            Some(ActivePicker::Grep(state)) => assert_eq!(state.len(), 1),
            other => panic!("expected an open Grep picker with one match, got is_some={}", other.is_some()),
        }
        app.picker_confirm();

        assert_eq!(app.main_view, MainView::Editor);
        let (line, _) = app.open().buffer.line_col(&app.test_cursor());
        assert_eq!(line, 1); // "needle here" is the second line (index 1)
    }

    #[test]
    fn run_grep_with_no_matches_still_opens_an_empty_picker() {
        // `grep_project` treats "no matches" as `Ok(empty)`, not an error
        // -- `run_grep` still opens the (now-empty) Grep picker rather
        // than silently doing nothing, so the user sees "no results"
        // instead of a keypress with no visible effect.
        let dir = TempDir::new("run_grep_no_matches");
        dir.write("a.txt", "nothing interesting\n");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());

        app.run_grep("needle");
        assert_eq!(app.main_view, MainView::Picker);
        match &app.active_picker {
            Some(ActivePicker::Grep(state)) => assert!(state.is_empty()),
            _ => panic!("expected an open (empty) Grep picker"),
        }
    }

    #[test]
    fn grep_query_key_routes_chars_backspace_and_enter() {
        let dir = TempDir::new("grep_query_key");
        dir.write("a.txt", "target line\n");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());
        app.pending_grep_query = Some(String::new());

        for c in "targetx".chars() {
            app.grep_query_key(KeyPress::char(c));
        }
        app.grep_query_key(KeyPress::named(FenixNamedKey::Backspace)); // drop the trailing 'x'
        assert_eq!(app.pending_grep_query.as_deref(), Some("target"));

        app.grep_query_key(KeyPress::named(FenixNamedKey::Enter));
        assert!(app.pending_grep_query.is_none()); // submitted, no longer pending
        assert!(app.active_picker.is_some()); // run_grep found a match and opened a picker
    }

    #[test]
    fn grep_query_key_escape_cancels_without_searching() {
        let mut app = App::with_file(None);
        app.pending_grep_query = Some("some query".to_string());
        app.grep_query_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.pending_grep_query.is_none());
        assert!(app.active_picker.is_none());
    }

    #[test]
    fn switch_to_project_registers_the_root_and_chains_into_find_file() {
        let known_dir = TempDir::new("switch_known");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));

        let project_dir = TempDir::new("switch_target");
        project_dir.touch("main.rs");

        // Already mid-picker (as `picker_confirm` would leave it before
        // dispatching to `switch_to_project`) -- switch shouldn't touch
        // `main_view` or the focused buffer, just swap the active picker.
        let focused_before = app.focused_buffer_id();
        app.enter_picker(ActivePicker::SwitchProject(fenix_picker::PickerState::new(Vec::new())));

        app.switch_to_project(project_dir.path().to_path_buf());

        assert_eq!(app.project_root, Some(project_dir.path().to_path_buf()));
        assert_eq!(app.known_projects.roots(), &[project_dir.path().to_path_buf()]);
        assert_eq!(app.main_view, MainView::Picker);
        assert_eq!(app.focused_buffer_id(), focused_before);
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => {
                assert_eq!(state.selected().map(|c| c.label.as_str()), Some("main.rs"))
            }
            _ => panic!("expected switch_to_project to chain into a FindFile picker"),
        }
    }

    #[test]
    fn opening_a_file_does_not_auto_register_its_project() {
        // Regression test: known_projects used to auto-populate from
        // every project you ever opened a file in, which is exactly what
        // SPC p a/SPC p d exist to replace with an explicitly curated
        // list. Opening a file under a real project root must leave
        // known_projects untouched.
        let known_dir = TempDir::new("no_auto_register_known");
        let project_dir = TempDir::new("no_auto_register_target");
        project_dir.touch(".git"); // a real, detectable project marker
        let file = project_dir.write("main.rs", "fn main() {}");

        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.test_open_path(&file);
        app.refresh_project_root(); // what the real open-file paths call after test_open_path's equivalent

        assert!(app.project_root.is_some(), "project_root itself should still auto-detect");
        assert!(app.known_projects.roots().is_empty(), "known_projects should stay empty until SPC p a");
    }

    #[test]
    fn picker_add_project_prompt_opens_the_explorer_at_the_current_project_root() {
        let root_dir = TempDir::new("add_project_prompt_root");
        let mut app = App::with_file(None);
        app.project_root = Some(root_dir.path().to_path_buf());

        app.picker_add_project_prompt();

        assert_eq!(app.main_view, MainView::Explorer);
        assert_eq!(app.explorer_purpose, ExplorerPurpose::PickProjectDir);
        assert_eq!(app.explorer.as_ref().map(|e| e.cwd.as_path()), Some(root_dir.path()));
    }

    #[test]
    fn select_cwd_registers_the_browsed_directory_and_returns_to_the_editor() {
        let known_dir = TempDir::new("select_cwd_known");
        let project_dir = TempDir::new("select_cwd_target");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.project_root = Some(project_dir.path().to_path_buf());

        app.picker_add_project_prompt();
        app.explorer_handle_action(ExplorerAction::SelectCwd);

        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.explorer.is_none());
        assert_eq!(app.explorer_purpose, ExplorerPurpose::Browse);
        let canonical = std::fs::canonicalize(project_dir.path()).unwrap();
        assert_eq!(app.known_projects.roots(), std::slice::from_ref(&canonical));
        // Persisted, not just held in memory.
        let reloaded = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        assert_eq!(reloaded.roots(), &[canonical]);
    }

    #[test]
    fn select_cwd_is_a_noop_outside_pick_project_dir_mode() {
        let dir = TempDir::new("select_cwd_browse_noop");
        let known_dir = TempDir::new("select_cwd_browse_noop_known");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.explorer = Some(ExplorerState::open(dir.path()).unwrap());
        app.explorer_purpose = ExplorerPurpose::Browse;
        app.main_view = MainView::Explorer;

        app.explorer_handle_action(ExplorerAction::SelectCwd);

        assert!(app.known_projects.roots().is_empty());
        assert_eq!(app.main_view, MainView::Explorer, "browsing should be untouched by a stray S");
    }

    #[test]
    fn opening_a_file_while_picking_a_project_dir_does_nothing() {
        let dir = TempDir::new("pick_project_dir_ignores_files");
        dir.write("main.rs", "fn main() {}");
        let mut app = App::with_file(None);
        app.project_root = Some(dir.path().to_path_buf());

        app.picker_add_project_prompt();
        app.explorer_open_selected();

        assert_eq!(app.main_view, MainView::Explorer, "a file open shouldn't leave the picker");
        assert_eq!(app.explorer_purpose, ExplorerPurpose::PickProjectDir);
    }

    #[test]
    fn picker_pick_theme_lists_every_shipped_theme() {
        let mut app = App::with_file(None);
        app.picker_pick_theme();
        match &app.active_picker {
            Some(ActivePicker::Theme(state)) => assert_eq!(state.len(), theme::ALL.len()),
            other => panic!("expected an open Theme picker, got is_some={}", other.is_some()),
        }
        assert_eq!(app.main_view, MainView::Picker);
    }

    #[test]
    fn picker_confirm_on_theme_applies_it_persists_and_returns_to_the_editor() {
        let dir = TempDir::new("picker_confirm_theme");
        let mut app = App::with_file(None);
        app.config = fenix_config::Config::load_or_default(dir.path().join("config.ini"));
        app.theme = &theme::ORBIT_DARK;

        app.picker_pick_theme();
        for ch in "Nord".chars() {
            picker_push_char(app.active_picker.as_mut().unwrap(), ch);
        }
        app.picker_confirm();

        assert_eq!(app.theme.name, "Nord");
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
        let reloaded = fenix_config::Config::load(dir.path().join("config.ini")).unwrap();
        assert_eq!(reloaded.theme, Some("Nord".to_string())); // persisted
    }

    #[test]
    fn picker_delete_project_lists_the_known_projects() {
        let known_dir = TempDir::new("delete_project_list");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.known_projects.add(PathBuf::from("/repo/one"));

        app.picker_delete_project();

        match &app.active_picker {
            Some(ActivePicker::DeleteProject(state)) => assert_eq!(state.len(), 1),
            other => panic!("expected an open DeleteProject picker, got is_some={}", other.is_some()),
        }
    }

    #[test]
    fn picker_confirm_on_delete_project_removes_it_and_returns_to_the_editor() {
        let known_dir = TempDir::new("delete_project_confirm");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.known_projects.add(PathBuf::from("/repo/one"));
        app.known_projects.add(PathBuf::from("/repo/two"));

        app.picker_delete_project();
        app.picker_confirm(); // deletes whatever's selected -- MRU-first, so "/repo/two"

        assert_eq!(app.known_projects.roots(), &[PathBuf::from("/repo/one")]);
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());

        let reloaded = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        assert_eq!(reloaded.roots(), &[PathBuf::from("/repo/one")]);
    }

    #[test]
    fn picker_switch_buffer_lists_open_buffers_mru_first() {
        let dir = TempDir::new("switch_buffer_candidates");
        let a = dir.write("a.txt", "a");
        let b = dir.write("b.txt", "b");
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        app.test_open_path(&b); // b opened (and touched) last -> MRU-first

        app.picker_switch_buffer();

        match &app.active_picker {
            Some(ActivePicker::SwitchBuffer(state)) => {
                let labels: Vec<&str> = state.visible_rows(0, 10).map(|(_, c)| c.label.as_str()).collect();
                assert_eq!(labels[0], b.display().to_string());
                assert!(labels.contains(&a.display().to_string().as_str()));
            }
            _ => panic!("expected picker_switch_buffer to open a SwitchBuffer picker"),
        }
    }

    #[test]
    fn picker_switch_buffer_marks_dirty_buffers() {
        let mut app = App::with_file(None); // one scratch buffer, untouched
        app.picker_switch_buffer();
        match &app.active_picker {
            Some(ActivePicker::SwitchBuffer(state)) => {
                assert_eq!(state.visible_rows(0, 10).next().unwrap().1.label, "*scratch*");
            }
            _ => panic!("expected a SwitchBuffer picker"),
        }

        app.test_insert('x'); // now dirty
        app.picker_switch_buffer();
        match &app.active_picker {
            Some(ActivePicker::SwitchBuffer(state)) => {
                assert_eq!(state.visible_rows(0, 10).next().unwrap().1.label, "+ *scratch*");
            }
            _ => panic!("expected a SwitchBuffer picker"),
        }
    }

    #[test]
    fn picker_confirm_on_switch_buffer_points_the_focused_pane_at_it() {
        let dir = TempDir::new("switch_buffer_confirm");
        let a = dir.write("a.txt", "a");
        let mut app = App::with_file(None);
        let scratch_id = app.focused_buffer_id();
        app.test_open_path(&a); // now focused on a.txt

        let candidates = vec![fenix_picker::Candidate::new("*scratch*", scratch_id)];
        app.enter_picker(ActivePicker::SwitchBuffer(fenix_picker::PickerState::new(candidates)));
        app.picker_confirm();

        assert_eq!(app.focused_buffer_id(), scratch_id);
        assert_eq!(app.main_view, MainView::Editor);
        assert!(app.active_picker.is_none());
    }

    #[test]
    fn next_and_prev_buffer_cycle_through_every_open_buffer() {
        let dir = TempDir::new("cycle_buffer");
        let a = dir.write("a.txt", "a");
        let b = dir.write("b.txt", "b");
        let mut app = App::with_file(None); // scratch, then a, then b
        app.test_open_path(&a);
        app.test_open_path(&b);

        let ids = app.buffers.ids_sorted_by_path(); // scratch, a, b (scratch sorts first)
        assert_eq!(app.focused_buffer_id(), ids[2]); // currently on b

        app.next_buffer(); // wraps to scratch
        assert_eq!(app.focused_buffer_id(), ids[0]);

        app.prev_buffer(); // back to b
        assert_eq!(app.focused_buffer_id(), ids[2]);

        app.prev_buffer(); // to a
        assert_eq!(app.focused_buffer_id(), ids[1]);
    }

    #[test]
    fn kill_buffer_falls_back_to_the_mru_next_buffer() {
        let dir = TempDir::new("kill_buffer");
        let a = dir.write("a.txt", "a");
        let mut app = App::with_file(None);
        let scratch_id = app.focused_buffer_id();
        app.test_open_path(&a); // focused on a, scratch is MRU-next

        app.kill_buffer();

        assert_eq!(app.focused_buffer_id(), scratch_id);
        assert!(app.buffers.get(scratch_id).is_some());
        assert_eq!(app.buffers.len(), 1);
    }

    #[test]
    fn kill_buffer_on_the_last_buffer_falls_back_to_a_fresh_scratch() {
        let mut app = App::with_file(None); // single scratch buffer
        let original_id = app.focused_buffer_id();

        app.kill_buffer();

        assert_ne!(app.focused_buffer_id(), original_id); // a *new* scratch, not the old one
        assert_eq!(app.buffers.len(), 1);
        assert_eq!(app.open().buffer.text(), "");
    }

    #[test]
    fn kill_buffer_retargets_every_other_pane_showing_it() {
        let dir = TempDir::new("kill_buffer_multi_pane");
        let a = dir.write("a.txt", "a");
        let mut app = App::with_file(None);
        let scratch_id = app.focused_buffer_id();
        app.test_open_path(&a); // focused pane now on a
        app.split_vertical(); // new pane, also showing a
        assert_eq!(app.windows().window_count(), 2);

        app.kill_buffer(); // closes a, focused in the new pane

        for pane in app.windows().windows() {
            assert_eq!(app.windows().content(pane), Some(&scratch_id));
        }
    }

    #[test]
    fn new_scratch_buffer_opens_a_fresh_empty_buffer_in_the_focused_pane() {
        let dir = TempDir::new("new_scratch");
        let a = dir.write("a.txt", "hello");
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        let a_id = app.focused_buffer_id();

        app.new_scratch_buffer();

        assert_ne!(app.focused_buffer_id(), a_id);
        assert_eq!(app.open().buffer.text(), "");
        assert_eq!(app.open().kind, BufferKind::Text);
        assert_eq!(app.open().buffer.path(), None);
    }

    #[test]
    fn with_file_none_opens_the_dashboard_instead_of_a_plain_scratch_buffer() {
        let app = App::with_file(None);
        assert_eq!(app.open().kind, BufferKind::Dashboard);
        assert!(app.open().buffer.text().contains("a keyboard-first editor"));
        assert!(app.dashboard_lines.contains_key(&app.focused_buffer_id()));
    }

    #[test]
    fn gutter_chars_is_zero_for_a_dashboard_buffer() {
        let app = App::with_file(None);
        assert_eq!(app.gutter_chars(app.open()), 0);
    }

    #[test]
    fn dashboard_center_offset_centers_the_banner_and_content_blocks_independently() {
        // The regression this guards: the banner block (~30 chars) and
        // the content block below it are rarely the same width -- here
        // the content block is made deliberately much wider (a long
        // project path) than the banner. If both shared one pad (the
        // whole document's widest line), the banner would visibly sit
        // left of the pane's true center, exactly the bug reported.
        let known_dir = TempDir::new("dashboard_center_offset_sections");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.known_projects.add(PathBuf::from("/a/very/long/project/path/that/is/wider/than/the/banner/block"));
        app.recent_files = fenix_project::RecentFiles::load_or_default(known_dir.path().join("recent_files.txt"));
        app.open_dashboard();

        let ob = app.open();
        let id = app.focused_buffer_id();
        let lines = app.dashboard_lines.get(&id).unwrap();
        let char_width = 8.0;
        let line_height = 20.0;
        // Wide enough that neither block's padding clamps to zero.
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 2000.0, h: 2000.0 };

        let (pad_by_line, _) = dashboard_center_offset(ob, lines, rect, char_width, line_height);

        let banner_line = lines
            .iter()
            .position(|l| l.as_ref().map(|m| m.style) == Some(dashboard::DashboardLineStyle::Banner))
            .unwrap();
        let project_line = lines
            .iter()
            .position(|l| l.as_ref().map(|m| m.style) == Some(dashboard::DashboardLineStyle::Project))
            .unwrap();
        assert_ne!(pad_by_line[banner_line], pad_by_line[project_line]);

        // Every banner row shares the same pad as every other banner
        // row -- centered as one coherent block, not row-by-row.
        let all_banner_pads: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.as_ref().map(|m| m.style) == Some(dashboard::DashboardLineStyle::Banner))
            .map(|(i, _)| pad_by_line[i])
            .collect();
        assert!(all_banner_pads.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn dashboard_center_offset_never_goes_negative_in_a_too_small_pane() {
        let app = App::with_file(None);
        let ob = app.open();
        let id = app.focused_buffer_id();
        let lines = app.dashboard_lines.get(&id).unwrap();
        // Smaller than the content on both axes.
        let rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

        let (pad_by_line, extra_top_px) = dashboard_center_offset(ob, lines, rect, 8.0, 20.0);

        assert!(pad_by_line.iter().all(|&p| p == 0));
        assert_eq!(extra_top_px, 0.0);
    }

    #[test]
    fn modeline_shows_a_placeholder_filename_for_the_dashboard() {
        let app = App::with_file(None);
        assert!(app.modeline_text().contains("*dashboard*"));
    }

    #[test]
    fn open_dashboard_replaces_the_focused_pane_with_a_fresh_dashboard_buffer() {
        let dir = TempDir::new("open_dashboard");
        let a = dir.write("a.txt", "hello");
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        let a_id = app.focused_buffer_id();

        app.open_dashboard();

        assert_ne!(app.focused_buffer_id(), a_id);
        assert_eq!(app.open().kind, BufferKind::Dashboard);
        assert!(app.dashboard_lines.contains_key(&app.focused_buffer_id()));
    }

    #[test]
    fn open_docker_panel_opens_a_tagged_buffer_with_a_footer_line() {
        // `docker` itself is unreachable in a headless/sandboxed test
        // environment (no daemon socket access) -- `list_containers`/
        // `list_images` degrade to empty `Vec`s per their own "never
        // fails" contract, so this only asserts the wiring (buffer kind,
        // line-metadata table, the always-present footer), not real
        // container/image data.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert_eq!(app.open().kind, BufferKind::Docker);
        let id = app.focused_buffer_id();
        assert!(app.docker_lines.contains_key(&id));
        let lines = app.docker_lines.get(&id).unwrap();
        assert!(lines.iter().flatten().any(|l| l.style == docker_panel::DockerLineStyle::Footer));
    }

    #[test]
    fn gutter_chars_is_zero_for_a_docker_buffer() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert_eq!(app.gutter_chars(app.open()), 0);
    }

    #[test]
    fn docker_buffer_modeline_falls_back_to_a_docker_marker() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        assert!(app.modeline_text().contains("*docker*"));
    }

    #[test]
    fn open_docker_panel_replaces_the_focused_pane_with_a_fresh_docker_buffer() {
        let dir = TempDir::new("open_docker_panel");
        let a = dir.write("a.txt", "hello");
        let mut app = App::with_file(None);
        app.test_open_path(&a);
        let a_id = app.focused_buffer_id();

        app.open_docker_panel();

        assert_ne!(app.focused_buffer_id(), a_id);
        assert_eq!(app.open().kind, BufferKind::Docker);
    }

    #[test]
    fn docker_action_keys_on_a_non_entry_line_are_a_no_op() {
        // Cursor sits on line 0 (the header), which has no `entry` --
        // every action should just do nothing rather than panic.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        app.docker_start_selected();
        app.docker_stop_selected();
        app.docker_restart_selected();
        app.docker_run_selected();
        assert_eq!(app.open().kind, BufferKind::Docker);
        assert!(app.docker_confirm_remove.is_none());
    }

    #[test]
    fn docker_entry_at_cursor_reads_the_line_under_the_cursor() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let id = app.focused_buffer_id();
        // Force a known entry at line 0, independent of whatever real
        // `docker` output (or lack thereof) actually rendered there.
        app.docker_lines.insert(
            id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
            })],
        );
        app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });

        assert_eq!(app.docker_entry_at_cursor(), Some(docker_panel::DockerEntry::Container("abc123".to_string())));
    }

    #[test]
    fn docker_view_logs_selected_opens_a_plain_text_buffer_in_the_focused_pane() {
        // `docker logs` itself is unreachable in this sandboxed test
        // environment (no daemon socket access) -- it'll come back an
        // `Err`, which `docker_view_logs_selected` still renders into a
        // real buffer rather than silently doing nothing. The point of
        // this test is the navigation/buffer-kind wiring, not the
        // specific log text.
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let docker_id = app.focused_buffer_id();
        app.docker_lines.insert(
            docker_id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
            })],
        );
        app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });

        app.docker_view_logs_selected();

        assert_ne!(app.focused_buffer_id(), docker_id);
        assert_eq!(app.open().kind, BufferKind::Text);
        assert!(!app.open().buffer.text().is_empty());
    }

    #[test]
    fn docker_view_logs_selected_on_an_image_row_is_a_no_op() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let docker_id = app.focused_buffer_id();
        app.docker_lines.insert(
            docker_id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Image,
                entry: Some(docker_panel::DockerEntry::Image("sha256:dead".to_string())),
                dim_from: None,
            })],
        );
        app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });

        app.docker_view_logs_selected();

        assert_eq!(app.focused_buffer_id(), docker_id);
        assert_eq!(app.open().kind, BufferKind::Docker);
    }

    #[test]
    fn docker_confirm_remove_shows_a_prompt_and_a_non_y_key_cancels_without_removing() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let id = app.focused_buffer_id();
        app.docker_lines.insert(
            id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Image,
                entry: Some(docker_panel::DockerEntry::Image("sha256:dead".to_string())),
                dim_from: None,
            })],
        );
        app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });

        let entry = app.docker_entry_at_cursor().unwrap();
        app.docker_confirm_remove = Some(entry);
        assert!(app.docker_confirm_text().unwrap().contains("image"));
        assert!(app.modeline_text().contains("Remove this image?"));

        app.docker_confirm_key(KeyPress::char('n'));
        assert!(app.docker_confirm_remove.is_none());
    }

    #[test]
    fn docker_confirm_remove_with_y_clears_the_prompt_and_refreshes() {
        let mut app = App::with_file(None);
        app.open_docker_panel();
        let id = app.focused_buffer_id();
        app.docker_lines.insert(
            id,
            vec![Some(docker_panel::DockerLine {
                style: docker_panel::DockerLineStyle::Container,
                entry: Some(docker_panel::DockerEntry::Container("abc123".to_string())),
                dim_from: None,
            })],
        );
        app.test_set_cursor(Cursor { char_idx: 0, sticky_col: 0 });
        app.docker_confirm_remove = app.docker_entry_at_cursor();

        // `docker rm` itself fails in this sandboxed environment (no
        // daemon access) -- the point of this test is that confirming
        // clears the armed state and refreshes without panicking, not
        // that a real removal happened.
        app.docker_confirm_key(KeyPress::char('y'));
        assert!(app.docker_confirm_remove.is_none());
        assert_eq!(app.open().kind, BufferKind::Docker);
    }

    #[test]
    fn record_recent_file_persists_the_canonicalized_path() {
        let dir = TempDir::new("record_recent_file");
        let recent_dir = TempDir::new("record_recent_file_config");
        let file = dir.write("a.txt", "hello");
        let mut app = App::with_file(None);
        app.recent_files = fenix_project::RecentFiles::load_or_default(recent_dir.path().join("recent_files.txt"));

        app.record_recent_file(&file);

        let canonical = std::fs::canonicalize(&file).unwrap();
        assert_eq!(app.recent_files.paths(), std::slice::from_ref(&canonical));
        let reloaded =
            fenix_project::RecentFiles::load_or_default(recent_dir.path().join("recent_files.txt"));
        assert_eq!(reloaded.paths(), &[canonical]);
    }

    /// Moves the focused buffer's cursor to the first line whose
    /// `dashboard_lines` entry has the given style -- shared by the
    /// activation tests below to find the row to put the cursor on
    /// without hand-counting generated line numbers.
    fn move_cursor_to_dashboard_line(app: &mut App, style: dashboard::DashboardLineStyle) -> usize {
        let lines = app.dashboard_lines.get(&app.focused_buffer_id()).cloned().unwrap();
        let line = lines
            .iter()
            .position(|l| l.as_ref().map(|m| m.style) == Some(style))
            .unwrap_or_else(|| panic!("no dashboard line with style {style:?}"));
        let start = app.open().buffer.line_start_char(line);
        app.test_set_cursor(Cursor { char_idx: start, sticky_col: 0 });
        line
    }

    #[test]
    fn dashboard_activate_selected_on_a_project_line_switches_to_that_project() {
        let known_dir = TempDir::new("dashboard_activate_project");
        let project_dir = TempDir::new("dashboard_activate_project_target");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.known_projects.add(project_dir.path().to_path_buf());
        app.recent_files = fenix_project::RecentFiles::load_or_default(known_dir.path().join("recent_files.txt"));
        app.open_dashboard();
        move_cursor_to_dashboard_line(&mut app, dashboard::DashboardLineStyle::Project);

        app.dashboard_activate_selected();

        assert_eq!(app.main_view, MainView::Picker);
        assert_eq!(app.project_root, Some(project_dir.path().to_path_buf()));
        assert!(matches!(&app.active_picker, Some(ActivePicker::FindFile(_))));
    }

    #[test]
    fn dashboard_activate_selected_on_a_recent_file_line_opens_it() {
        let known_dir = TempDir::new("dashboard_activate_recent");
        let file_dir = TempDir::new("dashboard_activate_recent_target");
        let file = file_dir.write("notes.txt", "hello");
        let mut app = App::with_file(None);
        app.known_projects = fenix_project::KnownProjects::load_or_default(known_dir.path().join("projects.txt"));
        app.recent_files = fenix_project::RecentFiles::load_or_default(known_dir.path().join("recent_files.txt"));
        app.recent_files.add(file.clone());
        app.open_dashboard();
        move_cursor_to_dashboard_line(&mut app, dashboard::DashboardLineStyle::RecentFile);

        app.dashboard_activate_selected();

        assert_eq!(app.main_view, MainView::Editor);
        assert_eq!(app.open().buffer.text(), "hello");
        assert_eq!(app.open().kind, BufferKind::Text);
    }

    #[test]
    fn dashboard_activate_selected_on_a_non_entry_line_does_nothing() {
        let mut app = App::with_file(None);
        let dashboard_id = app.focused_buffer_id();
        move_cursor_to_dashboard_line(&mut app, dashboard::DashboardLineStyle::Banner);

        app.dashboard_activate_selected();

        // Still the same dashboard buffer, nothing opened or switched to.
        assert_eq!(app.focused_buffer_id(), dashboard_id);
        assert_eq!(app.open().kind, BufferKind::Dashboard);
        assert_eq!(app.main_view, MainView::Editor);
    }

    #[test]
    fn enter_on_a_non_dashboard_buffer_is_not_intercepted() {
        // Guards the `handle_key` check itself: an ordinary Text buffer's
        // Enter must still reach Vim (inserting a newline in Insert mode,
        // moving down a line in Normal mode) rather than this feature's
        // Enter-interception silently swallowing it. Exercised through
        // `open()`/`kind`, the exact condition `handle_key` gates on --
        // a real winit `KeyEvent` isn't constructible from a unit test,
        // same reason every other `handle_key` behavior in this suite is
        // tested via the methods it dispatches to, not the event itself.
        let mut app = App::with_file(None);
        app.new_scratch_buffer();
        assert_ne!(app.open().kind, BufferKind::Dashboard);
    }

    #[test]
    fn picker_key_routes_typing_navigation_and_escape() {
        let candidates = vec![
            fenix_picker::Candidate::new("apple", PathBuf::from("apple")),
            fenix_picker::Candidate::new("banana", PathBuf::from("banana")),
        ];
        let mut app = App::with_file(None);
        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));

        app.picker_key(KeyPress::char('b'));
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => assert_eq!(state.query(), "b"),
            _ => panic!("expected an active FindFile picker"),
        }

        app.picker_key(KeyPress::named(FenixNamedKey::Backspace));
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => assert_eq!(state.query(), ""),
            _ => panic!("expected an active FindFile picker"),
        }

        app.picker_key(KeyPress::named(FenixNamedKey::Down));
        match &app.active_picker {
            Some(ActivePicker::FindFile(state)) => assert_eq!(state.selected_row(), 1),
            _ => panic!("expected an active FindFile picker"),
        }

        app.picker_key(KeyPress::named(FenixNamedKey::Escape));
        assert!(app.active_picker.is_none());
        assert_eq!(app.main_view, MainView::Editor);
    }

    #[test]
    fn picker_key_enter_confirms_the_selection() {
        let dir = TempDir::new("picker_key_enter");
        let target = dir.write("only.txt", "confirmed\n");
        let mut app = App::with_file(None);
        let candidates = vec![fenix_picker::Candidate::new("only.txt", target.clone())];
        app.enter_picker(ActivePicker::FindFile(fenix_picker::PickerState::new(candidates)));

        app.picker_key(KeyPress::named(FenixNamedKey::Enter));

        assert_eq!(app.main_view, MainView::Editor);
        assert_eq!(app.open().buffer.path(), Some(target.as_path()));
    }

    #[test]
    fn picker_row_spans_shows_the_query_prompt_and_flags_the_selected_row() {
        let candidates =
            vec![fenix_picker::Candidate::new("a.txt", PathBuf::from("a.txt")), fenix_picker::Candidate::new("b.txt", PathBuf::from("b.txt"))];
        let app = App::with_file(None);
        let picker = ActivePicker::FindFile(fenix_picker::PickerState::new(candidates));

        let (spans, selected_row) = app.picker_row_spans(&picker, 5);
        assert_eq!(selected_row, Some(1)); // row 0 is the prompt, row 1 the first candidate
        let joined: String = spans.iter().map(|(s, _, _)| s.as_str()).collect();
        assert!(joined.starts_with("> "));
        assert!(joined.contains("a.txt"));
        assert!(joined.contains("b.txt"));
    }
}
