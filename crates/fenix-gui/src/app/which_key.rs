//! The which-key drawer: what the keys pressed so far can go on to,
//! opening up out of the modeline. Its header is the sequence so far as
//! keycaps and the group's name; below, the keys in columns -- groups
//! first, each with a `+` and how many commands it holds, then commands,
//! or the group's own sections. It waits `which_key.delay_ms` before
//! opening, so a quick `SPC f f` never flashes it; Backspace goes up a
//! level and `Ctrl-n`/`Ctrl-p` page through a group too big to show at
//! once.

use super::*;
use fenix_keymap::Hint;

/// Space between two columns.
const GUTTER_CHARS: usize = 4;
/// The widest a column gets, key included.
const MAX_COLUMN_CHARS: usize = 30;
/// How much of the window the drawer may cover.
const MAX_HEIGHT_SHARE: f32 = 0.4;
/// Where the drawer's text starts inside it: left, and top.
pub(super) const INSET_X: f32 = 20.0;
pub(super) const INSET_Y: f32 = 9.0;

/// Where the menu is, and what it shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pending {
    /// The keys pressed so far, as keycaps.
    pub path: Vec<KeyPress>,
    pub title: String,
    pub hints: Vec<Hint>,
}

/// One laid-out drawer: its rect, its text as rows of spans, and what's
/// drawn around the text -- keycaps behind the header's keys and hairlines
/// after section names -- in text cells (row, column) from the drawer's
/// text origin.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Drawer {
    pub rect: fenix_window::Rect,
    pub spans: RowSpans,
    /// `(column, width in cells, the current key)` on the header row.
    pub caps: Vec<(usize, usize, bool)>,
    /// `(row, first column)` of each section's hairline.
    pub rules: Vec<(usize, usize)>,
    pub page: usize,
    pub pages: usize,
}

/// The order keys are listed in: letters, then capitals, then the rest,
/// then named keys -- so a key keeps its place whatever it's called.
fn key_rank(key: &KeyPress) -> (u8, u8, char) {
    let mods = u8::from(key.mods.ctrl) + u8::from(key.mods.alt) * 2 + u8::from(key.mods.super_) * 4;
    match key.code {
        KeyCode::Char(c) if c.is_ascii_lowercase() => (0, mods, c),
        KeyCode::Char(c) if c.is_ascii_uppercase() => (1, mods, c.to_ascii_lowercase()),
        KeyCode::Char(' ') => (3, mods, ' '),
        KeyCode::Char(c) => (2, mods, c),
        KeyCode::Named(_) => (4, mods, ' '),
    }
}

/// `hints` in sections, each `(name, hints)`, in the order they're drawn.
pub(super) fn sections(mut hints: Vec<Hint>, by_label: bool) -> Vec<(String, Vec<Hint>)> {
    if by_label {
        hints.sort_by(|a, b| a.label.cmp(b.label));
    } else {
        hints.sort_by_key(|h| key_rank(&h.key));
    }
    let named: Vec<&'static str> = {
        let mut names: Vec<&'static str> = hints.iter().filter_map(|h| h.section).collect();
        names.sort_by_key(|n| keymap::WHICH_KEY_SECTIONS.iter().position(|s| s == n).unwrap_or(usize::MAX));
        names.dedup();
        names
    };
    let mut out: Vec<(String, Vec<Hint>)> = Vec::new();
    for name in &named {
        out.push((name.to_string(), hints.iter().filter(|h| h.section == Some(name)).copied().collect()));
    }
    let groups: Vec<Hint> = hints.iter().filter(|h| h.section.is_none() && h.group).copied().collect();
    let commands: Vec<Hint> = hints.iter().filter(|h| h.section.is_none() && !h.group).copied().collect();
    let plain = named.is_empty() && groups.is_empty();
    if !groups.is_empty() {
        out.push(("Groups".to_string(), groups));
    }
    if !commands.is_empty() {
        out.push((if plain { String::new() } else if named.is_empty() { "Commands".to_string() } else { "More".to_string() }, commands));
    }
    out
}

impl App {
    /// What the menu would show now, whether or not it's open yet: the
    /// local leader's, the leader's, or Vim's own prefix's.
    pub(super) fn which_key_pending(&self) -> Option<Pending> {
        let spc = KeyPress::char(' ');
        let (path, title, hints) = if let Some(local) = &self.local_matcher {
            let mut path = vec![spc, KeyPress::char('m')];
            path.extend_from_slice(local.path());
            let title = local.label().map(str::to_string).unwrap_or_else(|| self.local_contexts().iter().map(|c| c.name()).collect::<Vec<_>>().join(" · "));
            (path, title, local.pending_hints())
        } else if self.leader_matcher.is_pending() {
            let title = self.leader_matcher.label().unwrap_or("leader").to_string();
            (self.leader_matcher.path().to_vec(), title, self.leader_matcher.pending_hints())
        } else {
            let path = self.vim.pending_path();
            let title = self.vim.pending_label().map(str::to_string).unwrap_or_else(|| {
                match path.first().map(|k| k.code) {
                    Some(KeyCode::Char('g')) => "go".to_string(),
                    Some(KeyCode::Char('z')) => "fold & view".to_string(),
                    Some(KeyCode::Char(']')) => "next".to_string(),
                    Some(KeyCode::Char('[')) => "previous".to_string(),
                    _ => "then".to_string(),
                }
            });
            (path, title, self.vim.pending_hints())
        };
        (!hints.is_empty()).then_some(Pending { path, title, hints })
    }

    /// Whether the menu is open this frame: once a sequence has waited
    /// `which_key.delay_ms`, and from then on until it ends.
    pub(super) fn which_key_open(&mut self, now: Instant) -> bool {
        let pending = self.which_key_pending();
        let Some(pending) = pending else {
            self.motion.which_key_since = None;
            self.motion.which_key_shown = false;
            self.motion.which_key_page = 0;
            self.motion.which_key_path.clear();
            return false;
        };
        if pending.path != self.motion.which_key_path {
            self.motion.which_key_path = pending.path.clone();
            self.motion.which_key_page = 0;
        }
        let since = *self.motion.which_key_since.get_or_insert(now);
        if !self.motion.which_key_shown && now.saturating_duration_since(since) >= self.which_key_delay() {
            self.motion.which_key_shown = true;
        }
        self.motion.which_key_shown
    }

    pub(super) fn which_key_delay(&self) -> Duration {
        Duration::from_millis(self.config.polish.which_key_delay_ms.unwrap_or(250))
    }

    /// When a waiting menu is due to open, for the event loop's timer.
    pub(super) fn which_key_wake(&self) -> Option<Instant> {
        (!self.motion.which_key_shown).then_some(())?;
        self.motion.which_key_since.map(|since| since + self.which_key_delay())
    }

    /// Backspace in an open leader menu: back up one level, or out.
    /// Says whether it took the key.
    pub(super) fn which_key_back(&mut self, key: KeyPress) -> bool {
        if key != KeyPress::named(FenixNamedKey::Backspace) {
            return false;
        }
        if let Some(local) = self.local_matcher.as_mut() {
            if !local.back() && local.path().is_empty() {
                // Back from the local menu's root: to the leader's.
                self.local_matcher = None;
                self.leader_matcher.feed(KeyPress::char(' '));
            }
            return true;
        }
        if self.leader_matcher.is_pending() {
            self.leader_matcher.back();
            return true;
        }
        false
    }

    /// `Ctrl-n`/`Ctrl-p` in an open menu too big for one page.
    pub(super) fn which_key_paging(&mut self, key: KeyPress) -> bool {
        let forward = key == KeyPress::char('n').with_ctrl();
        let back = key == KeyPress::char('p').with_ctrl();
        if !(forward || back) || !self.motion.which_key_shown || self.motion.which_key_pages < 2 {
            return false;
        }
        let pages = self.motion.which_key_pages;
        self.motion.which_key_page = if forward { (self.motion.which_key_page + 1) % pages } else { (self.motion.which_key_page + pages - 1) % pages };
        true
    }

    /// The drawer laid out for a `window_width` window whose modeline
    /// starts at `modeline_top`.
    pub(super) fn which_key_drawer(&self, pending: &Pending, window_width: f32, window_height: f32, modeline_top: f32, char_width: f32, line_height: f32) -> Drawer {
        let theme = self.theme;
        let by_label = self.config.polish.which_key_order.as_deref() == Some("label");
        let text_cols = (((window_width - INSET_X * 2.0) / char_width).floor() as usize).max(20);
        // Keys get just the room the longest of them needs, and one cell.
        let key_chars = pending.hints.iter().map(|h| keymap::describe_keypress(&h.key).chars().count()).max().unwrap_or(1) + 1;

        // The widest entry decides the columns.
        let entry_text = |h: &Hint| -> usize {
            let count = if h.group { format!(" {}", h.count).chars().count() + 2 } else { 0 };
            key_chars + h.label.chars().count() + count
        };
        let widest = pending.hints.iter().map(entry_text).max().unwrap_or(10).min(MAX_COLUMN_CHARS);
        let column = widest + GUTTER_CHARS;
        let columns = (text_cols / column).clamp(1, 6);

        // Rows: a header, then each section's name and its rows.
        let sections = sections(pending.hints.clone(), by_label);
        let mut body: Vec<Vec<(String, glyphon::Color)>> = Vec::new();
        let mut rule_rows: Vec<(usize, usize)> = Vec::new();
        for (name, hints) in &sections {
            if !name.is_empty() {
                let label = name.to_uppercase();
                rule_rows.push((body.len(), label.chars().count() + 1));
                body.push(vec![(label, theme.gutter_fg)]);
            }
            for chunk in hints.chunks(columns) {
                let mut row: Vec<(String, glyphon::Color)> = Vec::new();
                for (i, h) in chunk.iter().enumerate() {
                    let key = keymap::describe_keypress(&h.key);
                    let mut used = 0;
                    row.push((format!("{key:<key_chars$}"), theme.caret_text));
                    used += key_chars.max(key.chars().count());
                    let room = widest.saturating_sub(used);
                    if h.group {
                        let count = format!(" {}", h.count);
                        let label = truncate_tab_name(h.label, room.saturating_sub(count.chars().count() + 2));
                        used += label.chars().count() + 2 + count.chars().count();
                        row.push((format!("+ {label}"), rgba_to_glyphon(theme.mode_insert)));
                        row.push((count, theme.gutter_fg));
                    } else {
                        let label = truncate_tab_name(h.label, room);
                        used += label.chars().count();
                        row.push((label, theme.fg_modeline));
                    }
                    if i + 1 < chunk.len() {
                        row.push((" ".repeat(column.saturating_sub(used)), theme.fg_modeline));
                    }
                }
                body.push(row);
            }
        }

        // What fits: at most 40% of the window, in pages after that.
        let max_rows = (((window_height * MAX_HEIGHT_SHARE) - INSET_Y * 2.0) / line_height).floor().max(3.0) as usize;
        let per_page = max_rows.saturating_sub(1).max(1);
        let pages = body.len().div_ceil(per_page).max(1);
        let page = self.motion.which_key_page.min(pages - 1);
        let shown: Vec<_> = body.iter().skip(page * per_page).take(per_page).cloned().collect();
        let rules: Vec<(usize, usize)> =
            rule_rows.iter().filter(|(r, _)| (page * per_page..(page + 1) * per_page).contains(r)).map(|&(r, c)| (r - page * per_page + 1, c)).collect();

        // The header: keycaps, the name, and the keys that move around.
        let mut header: Vec<(String, glyphon::Color)> = Vec::new();
        let mut caps = Vec::new();
        let mut col = 0usize;
        for (i, key) in pending.path.iter().enumerate() {
            let text = format!(" {} ", keymap::describe_keypress(key));
            let width = text.chars().count();
            let current = i + 1 == pending.path.len();
            caps.push((col, width, current));
            header.push((text, if current { theme.mode_text_dark } else { rgba_to_glyphon(theme.mode_normal) }));
            col += width;
            if !current {
                header.push((" › ".to_string(), theme.gutter_fg));
                col += 3;
            }
        }
        let title = format!("  {}", pending.title);
        col += title.chars().count();
        header.push((title, theme.caret_text));
        let back = if pending.path.len() > 1 { "Bksp back" } else { "Bksp close" };
        let hint = if pages > 1 { format!("page {} of {}   C-n next   {back}   Esc close", page + 1, pages) } else { format!("{back}   Esc close") };
        let pad = text_cols.saturating_sub(col + hint.chars().count() + 1);
        header.push((" ".repeat(pad), theme.fg_modeline));
        header.push((hint, theme.gutter_fg));

        let mut spans: RowSpans = header.into_iter().map(|(s, c)| (s, c, false)).collect();
        for row in &shown {
            spans.push(("\n".to_string(), theme.fg_modeline, false));
            spans.extend(row.iter().map(|(s, c)| (s.clone(), *c, false)));
        }
        let rows = shown.len() + 1;
        let height = rows as f32 * line_height + INSET_Y * 2.0;
        let rect = fenix_window::Rect { x: 0.0, y: (modeline_top - height).max(0.0), w: window_width, h: height.min(modeline_top) };
        Drawer { rect, spans, caps, rules, page, pages }
    }
}

/// Draws the drawer's surface under its text: the modeline's colour,
/// a hairline and shadow along its top, the mode rail down its left
/// edge, keycaps behind the header's keys, and hairlines after section
/// names -- all at `alpha`, for fading.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_drawer(
    rects: &mut RectRenderer,
    gpu: &GpuState,
    drawer: &Drawer,
    rect: fenix_window::Rect,
    clip: f32,
    rail: [f32; 4],
    theme: &Theme,
    char_width: f32,
    line_height: f32,
    shadows: bool,
) {
    // Nothing of it below the modeline's top: it slides up from behind.
    let bottom = (rect.y + rect.h).min(clip);
    if bottom <= rect.y {
        return;
    }
    let clipped = |rects: &mut RectRenderer, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]| {
        let h = h.min(bottom - y);
        if h > 0.0 {
            rects.push_rect(gpu, x, y, w, h, color);
        }
    };
    if shadows {
        // Cast upwards only, onto the code; the drawer's own fill covers
        // the rest of it, so none reaches the modeline.
        rects.push_shadow(gpu, rect.x, rect.y, rect.w, 16.0, 0.0, 10.0, [0.0, 0.0, 0.0, 0.35]);
    }
    clipped(rects, rect.x, rect.y, rect.w, rect.h, theme.bg_modeline);
    clipped(rects, rect.x, rect.y, rect.w, 1.0, theme.divider);
    clipped(rects, rect.x, rect.y, 5.0, rect.h, rail);
    let text_x = rect.x + INSET_X;
    let text_y = rect.y + INSET_Y;
    for &(col, width, current) in &drawer.caps {
        let (x, y, h) = (text_x + col as f32 * char_width - 1.0, text_y + 1.0, line_height - 2.0);
        if y + h > bottom {
            continue;
        }
        let style = if current {
            crate::rect::BoxStyle { radius: 3.0, fill: rail, border: 0.0, border_color: [0.0; 4] }
        } else {
            crate::rect::BoxStyle { radius: 3.0, fill: [0.0; 4], border: 1.0, border_color: theme.mode_normal }
        };
        rects.push_box(gpu, x, y, width as f32 * char_width + 2.0, h, style);
    }
    let rule = glyphon_to_rgba(theme.gutter_fg);
    for &(row, col) in &drawer.rules {
        let x = text_x + col as f32 * char_width;
        let y = (text_y + row as f32 * line_height + line_height / 2.0).round();
        if y < bottom {
            rects.push_rect(gpu, x, y, (rect.x + rect.w - INSET_X - x).max(0.0), 1.0, [rule[0], rule[1], rule[2], 0.3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(key: char, label: &'static str, group: bool) -> Hint {
        Hint { key: KeyPress::char(key), label, group, count: if group { 3 } else { 0 }, section: None }
    }

    #[test]
    fn keys_are_ordered_by_key_letters_first() {
        let hints = vec![hint('g', "git", true), hint('a', "agenda", true), hint('G', "graph", false), hint(',', "settings", false), hint(' ', "find file", false)];
        let s = sections(hints, false);
        assert_eq!(s[0].0, "Groups");
        assert_eq!(s[0].1.iter().map(|h| h.label).collect::<Vec<_>>(), vec!["agenda", "git"]);
        assert_eq!(s[1].0, "Commands");
        assert_eq!(s[1].1.iter().map(|h| h.label).collect::<Vec<_>>(), vec!["graph", "settings", "find file"]);
    }

    #[test]
    fn named_sections_come_in_their_order_then_the_rest() {
        let mut hints = vec![hint('a', "stage hunk", false), hint('g', "status page", false), hint('q', "close", false)];
        hints[0].section = Some("This change");
        hints[1].section = Some("View");
        let s = sections(hints, false);
        let names: Vec<&str> = s.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["View", "This change", "More"]);
    }

    #[test]
    fn a_plain_list_has_no_section_names() {
        let s = sections(vec![hint('g', "top", false), hint('d', "definition", false)], false);
        assert_eq!(s.len(), 1);
        assert!(s[0].0.is_empty());
        assert_eq!(s[0].1[0].label, "definition");
    }

    #[test]
    fn by_label_when_asked() {
        let s = sections(vec![hint('a', "zebra", false), hint('z', "apple", false)], true);
        assert_eq!(s[0].1[0].label, "apple");
    }

    fn leader(app: &mut App, keys: &[char]) {
        for &k in keys {
            app.leader_matcher.feed(KeyPress::char(k));
        }
    }

    #[test]
    fn the_drawer_heads_with_the_keys_so_far_and_the_groups_name() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ', 'g']);
        let pending = app.which_key_pending().expect("SPC g is pending");
        assert_eq!(pending.title, "git");
        let drawer = app.which_key_drawer(&pending, 1600.0, 900.0, 870.0, 9.0, 20.0);
        let text: String = drawer.spans.iter().map(|s| s.0.as_str()).collect();
        let header = text.lines().next().unwrap();
        assert!(header.starts_with(" SPC  ›  g   git"), "{header}");
        assert_eq!(drawer.caps.len(), 2);
        assert!(drawer.caps[1].2 && !drawer.caps[0].2, "the last key is the bright one");
        assert!(text.contains("VIEW") && text.contains("BRANCH & REMOTE") && text.contains("THIS CHANGE"), "{text}");
        assert!(drawer.rect.y + drawer.rect.h <= 870.0 + 0.01, "sits on the modeline");
        assert_eq!((drawer.rect.x, drawer.rect.w), (0.0, 1600.0), "full width");
    }

    #[test]
    fn groups_show_a_plus_and_how_many_commands_they_hold() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ']);
        let pending = app.which_key_pending().unwrap();
        let drawer = app.which_key_drawer(&pending, 1600.0, 900.0, 870.0, 9.0, 20.0);
        let text: String = drawer.spans.iter().map(|s| s.0.as_str()).collect();
        assert!(text.contains("GROUPS") && text.contains("COMMANDS"));
        assert!(text.contains("+ git "), "{text}");
    }

    #[test]
    fn a_small_window_pages_instead_of_hiding() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ']);
        let pending = app.which_key_pending().unwrap();
        let drawer = app.which_key_drawer(&pending, 400.0, 200.0, 180.0, 9.0, 20.0);
        assert!(drawer.pages > 1);
        let text: String = drawer.spans.iter().map(|s| s.0.as_str()).collect();
        assert!(text.contains("page 1 of"));
    }

    #[test]
    fn the_menu_waits_before_opening() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ']);
        let now = Instant::now();
        assert!(!app.which_key_open(now), "not straight away");
        assert!(app.which_key_open(now + Duration::from_millis(300)));
        // Deeper levels open at once.
        app.leader_matcher.feed(KeyPress::char('g'));
        assert!(app.which_key_open(now + Duration::from_millis(301)));
        app.leader_matcher.cancel();
        assert!(!app.which_key_open(now + Duration::from_millis(302)));
        leader(&mut app, &[' ']);
        assert!(!app.which_key_open(now + Duration::from_millis(303)), "a new sequence waits again");
    }

    #[test]
    fn backspace_goes_up_a_level() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ', 'g']);
        assert!(app.which_key_back(KeyPress::named(FenixNamedKey::Backspace)));
        assert_eq!(app.leader_matcher.path(), &[KeyPress::char(' ')]);
        assert!(app.which_key_back(KeyPress::named(FenixNamedKey::Backspace)));
        assert!(!app.leader_matcher.is_pending());
    }
}
