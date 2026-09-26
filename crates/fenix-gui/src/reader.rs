//! The PDF reader's pure half: how a zoom turns into a render size, the
//! zoom steps, and the reader's keys. No pdfium, GPU or `App` here -- the
//! host half (`app/reader.rs`) owns the documents, their views and the
//! worker, and asks this module what a key or a zoom means.

use fenix_keymap::{KeyCode, KeyPress, Mods, NamedKey};

/// How a view is zoomed. `FitPage`/`FitWidth` follow the pane's size, so
/// a resize re-renders; `Percent` doesn't, so a resize only moves what's
/// shown of the same render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    FitPage,
    FitWidth,
    /// Percent of the page's real size on this screen: 100 is one point
    /// per point, times the window's scale factor.
    Percent(u32),
}

/// The zoom a document opens at: the page's width fills the pane.
pub const DEFAULT_ZOOM: Zoom = Zoom::FitWidth;

/// `+` and `-` move between these, as browsers do. The largest is
/// bounded by the page being rendered into one texture: 400% of a large
/// page is already several thousand pixels on a side.
pub const ZOOM_STEPS: &[u32] = &[25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400];

/// The zoom step after `current` (`in_ = true`) or before it: the next
/// one strictly larger or smaller, so a fitted 97% goes to 100% and then
/// 110%. Stays at either end.
pub fn step_zoom(current: u32, in_: bool) -> u32 {
    let (first, last) = (ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
    if in_ {
        ZOOM_STEPS.iter().copied().find(|&s| s > current).unwrap_or(last)
    } else {
        ZOOM_STEPS.iter().rev().copied().find(|&s| s < current).unwrap_or(first)
    }
}

/// How a zoom reads in the modeline.
pub fn zoom_label(zoom: Zoom) -> String {
    match zoom {
        Zoom::FitPage => "Fit page".to_string(),
        Zoom::FitWidth => "Fit width".to_string(),
        Zoom::Percent(percent) => format!("{percent}%"),
    }
}

/// Pixels between pages, and around them, before the scale factor.
pub const PAGE_GAP: f32 = 12.0;

/// Where one page sits in the document's column, in pixels from the
/// column's top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl PageRect {
    /// The size to render it at.
    pub fn px(&self) -> (u32, u32) {
        (self.w.round().max(1.0) as u32, self.h.round().max(1.0) as u32)
    }
}

/// Every page of a document, one under the other, at one scale.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    /// Pixels per point.
    pub scale: f32,
    pub gap: f32,
    pub pages: Vec<PageRect>,
    /// The column's size: at least the pane's width, so a narrow page is
    /// centred in it.
    pub width: f32,
    pub height: f32,
}

/// Lays `pages` (sizes in points) out for `zoom` in a pane `pane` pixels
/// big, on a screen of scale factor `dpi`. Fitting fits the widest page
/// (and, for fit page, the tallest), so every page has one scale and a
/// narrower one sits centred.
pub fn layout(pages: &[(f32, f32)], zoom: Zoom, pane: (f32, f32), dpi: f32) -> Layout {
    let dpi = dpi.max(0.1);
    let gap = (PAGE_GAP * dpi).round();
    let max_w = pages.iter().map(|p| p.0).fold(0.0f32, f32::max).max(1.0);
    let max_h = pages.iter().map(|p| p.1).fold(0.0f32, f32::max).max(1.0);
    let room = ((pane.0 - gap * 2.0).max(1.0), (pane.1 - gap * 2.0).max(1.0));
    let scale = match zoom {
        Zoom::FitWidth => room.0 / max_w,
        Zoom::FitPage => (room.0 / max_w).min(room.1 / max_h),
        Zoom::Percent(p) => p as f32 / 100.0 * dpi,
    }
    .max(0.01);
    let width = pane.0.max((max_w * scale).round() + gap * 2.0);
    let mut y = gap;
    let pages: Vec<PageRect> = pages
        .iter()
        .map(|&(w, h)| {
            let (w, h) = ((w * scale).round().max(1.0), (h * scale).round().max(1.0));
            let rect = PageRect { x: ((width - w) / 2.0).round(), y, w, h };
            y += h + gap;
            rect
        })
        .collect();
    let height = if pages.is_empty() { 0.0 } else { y };
    Layout { scale, gap, pages, width, height }
}

impl Layout {
    /// The page at column height `y` -- a gap counts as the page above it.
    pub fn page_at(&self, y: f32) -> usize {
        if self.pages.is_empty() {
            return 0;
        }
        self.pages.partition_point(|p| p.y <= y).saturating_sub(1)
    }

    /// The pages any of `top..top + h` shows.
    pub fn visible(&self, top: f32, h: f32) -> std::ops::Range<usize> {
        if self.pages.is_empty() {
            return 0..0;
        }
        let first = self.page_at(top);
        let last = self.page_at(top + h.max(1.0) - 1.0);
        first..last + 1
    }

    pub fn max_scroll(&self, pane: (f32, f32)) -> (f32, f32) {
        ((self.width - pane.0).max(0.0), (self.height - pane.1).max(0.0))
    }

    /// The scroll that puts `page`'s top at the pane's top, less the gap.
    pub fn top_of(&self, page: usize) -> f32 {
        self.pages.get(page).map(|p| (p.y - self.gap).max(0.0)).unwrap_or(0.0)
    }

    /// The page being read: the one across the middle of the pane, or
    /// the last when scrolled to the end.
    pub fn current(&self, scroll_y: f32, pane: (f32, f32)) -> usize {
        if self.pages.is_empty() {
            return 0;
        }
        if scroll_y >= self.max_scroll(pane).1 - 0.5 && self.height > pane.1 {
            return self.pages.len() - 1;
        }
        self.page_at(scroll_y + pane.1 / 2.0)
    }

    /// Where the pane's top is, as a page and how far down it (0..1), so
    /// the same place can be found in another layout of the document.
    pub fn anchor(&self, scroll_y: f32) -> (usize, f32) {
        let page = self.page_at(scroll_y);
        let Some(p) = self.pages.get(page) else { return (0, 0.0) };
        (page, (scroll_y - p.y) / p.h.max(1.0))
    }

    pub fn scroll_for(&self, anchor: (usize, f32)) -> f32 {
        self.pages.get(anchor.0).map(|p| p.y + anchor.1 * p.h).unwrap_or(0.0).max(0.0)
    }
}

/// What a key in a PDF pane asks for. Counts are already folded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// `j`/`k`: scroll by this many steps (negative is up).
    Scroll(i32),
    /// `Ctrl-d`/`Ctrl-u`: this many half screens.
    HalfScreen(i32),
    /// `Ctrl-f`/`Ctrl-b`, `PageDown`/`PageUp`: this many screens.
    Screen(i32),
    /// `h`/`l`: pan sideways by this many steps.
    Pan(i32),
    /// `J`/`K`: this many pages on, each landing at the page's top.
    Page(i32),
    /// `gg`, `{n}G`, `{n}gg`: page n, counting from 1.
    GotoPage(u32),
    /// `G` with no count.
    LastPage,
    /// `gt`, `gT`, `gh`, `g<Tab>`: the pane's tabs, as in a text buffer.
    Tab(fenix_vim::TabMove),
    ZoomIn,
    ZoomOut,
    /// `=`: fit width and fit page, in turn.
    ToggleFit,
    FitWidth,
    FitPage,
    /// `z0`: 100%.
    ActualSize,
    /// `o`: the outline.
    Outline,
    /// `/`: search.
    Search,
    /// `n`/`N`: the next or previous match of the last search.
    NextMatch,
    PrevMatch,
    /// `m{a}`: a mark here.
    SetMark(char),
    /// `'{a}` or `` `{a} ``: back to a mark.
    GotoMark(char),
    /// `Esc` with nothing typed: hide the search's highlights, or close
    /// the sidebar.
    Escape,
}

/// What `Keys::key` made of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Taken, waiting for more (a count, or the second key of `g`/`z`).
    Pending,
    /// Taken and understood.
    Run(Cmd),
    /// Taken and dropped: a `g` or `z` followed by a key that means
    /// nothing after it, or `Esc` clearing a count.
    Dropped,
    /// Not the reader's: the leader, `:`, `Ctrl-w`, `Ctrl-o` and anything
    /// else go on to the editor.
    Pass,
}

/// The reader's keys, as Vim reads them: an optional count, then a key,
/// with `g` and `z` waiting for a second.
#[derive(Debug, Default, Clone)]
pub struct Keys {
    count: Option<u32>,
    prefix: Option<char>,
}

impl Keys {
    /// Whether a count or a `g`/`z` is waiting for its next key.
    pub fn is_pending(&self) -> bool {
        self.count.is_some() || self.prefix.is_some()
    }

    pub fn reset(&mut self) {
        *self = Keys::default();
    }

    /// The count typed so far, for the modeline.
    pub fn pending_text(&self) -> String {
        let mut text = self.count.map(|c| c.to_string()).unwrap_or_default();
        if let Some(p) = self.prefix {
            text.push(p);
        }
        text
    }

    pub fn key(&mut self, key: KeyPress) -> Outcome {
        let count = self.count;
        let n = count.unwrap_or(1).min(i32::MAX as u32) as i32;
        if let Some(prefix) = self.prefix.take() {
            self.count = None;
            if key.mods != Mods::default() {
                return Outcome::Dropped;
            }
            let cmd = match (prefix, key.code) {
                ('g', KeyCode::Char('g')) => Cmd::GotoPage(count.unwrap_or(1)),
                ('g', KeyCode::Char('t')) => Cmd::Tab(match count {
                    Some(n) => fenix_vim::TabMove::Nth(n),
                    None => fenix_vim::TabMove::Next,
                }),
                ('g', KeyCode::Char('T')) => Cmd::Tab(fenix_vim::TabMove::Prev(count.unwrap_or(1))),
                ('g', KeyCode::Char('h')) => Cmd::Tab(fenix_vim::TabMove::Home),
                ('g', KeyCode::Named(NamedKey::Tab)) => Cmd::Tab(fenix_vim::TabMove::Last),
                ('z', KeyCode::Char('w')) => Cmd::FitWidth,
                ('z', KeyCode::Char('p')) => Cmd::FitPage,
                ('z', KeyCode::Char('0')) => Cmd::ActualSize,
                ('m', KeyCode::Char(c)) if c.is_ascii_alphabetic() => Cmd::SetMark(c),
                ('\'', KeyCode::Char(c)) if c.is_ascii_alphabetic() => Cmd::GotoMark(c),
                _ => return Outcome::Dropped,
            };
            return Outcome::Run(cmd);
        }

        let ctrl = Mods { ctrl: true, ..Mods::default() };
        if key.mods == ctrl {
            let cmd = match key.code {
                KeyCode::Char('d') => Cmd::HalfScreen(n),
                KeyCode::Char('u') => Cmd::HalfScreen(-n),
                KeyCode::Char('f') => Cmd::Screen(n),
                KeyCode::Char('b') => Cmd::Screen(-n),
                _ => {
                    self.count = None;
                    return Outcome::Pass;
                }
            };
            self.count = None;
            return Outcome::Run(cmd);
        }
        if key.mods != Mods::default() {
            self.count = None;
            return Outcome::Pass;
        }

        let cmd = match key.code {
            KeyCode::Char(c @ '1'..='9') => return self.digit(c),
            KeyCode::Char('0') if count.is_some() => return self.digit('0'),
            KeyCode::Char(c @ ('g' | 'z' | 'm' | '\'')) => {
                self.prefix = Some(c);
                return Outcome::Pending;
            }
            KeyCode::Char('`') => {
                self.prefix = Some('\'');
                return Outcome::Pending;
            }
            KeyCode::Char('j') | KeyCode::Named(NamedKey::Down) => Cmd::Scroll(n),
            KeyCode::Char('k') | KeyCode::Named(NamedKey::Up) => Cmd::Scroll(-n),
            KeyCode::Char('h') | KeyCode::Named(NamedKey::Left) => Cmd::Pan(-n),
            KeyCode::Char('l') | KeyCode::Named(NamedKey::Right) => Cmd::Pan(n),
            KeyCode::Named(NamedKey::PageDown) => Cmd::Screen(n),
            KeyCode::Named(NamedKey::PageUp) => Cmd::Screen(-n),
            KeyCode::Char('J') => Cmd::Page(n),
            KeyCode::Char('K') => Cmd::Page(-n),
            KeyCode::Char('G') => count.map(Cmd::GotoPage).unwrap_or(Cmd::LastPage),
            KeyCode::Named(NamedKey::Home) => Cmd::GotoPage(1),
            KeyCode::Named(NamedKey::End) => Cmd::LastPage,
            KeyCode::Char('+') => Cmd::ZoomIn,
            KeyCode::Char('-') => Cmd::ZoomOut,
            KeyCode::Char('=') => Cmd::ToggleFit,
            KeyCode::Char('o') => Cmd::Outline,
            KeyCode::Char('/') => Cmd::Search,
            KeyCode::Char('n') => Cmd::NextMatch,
            KeyCode::Char('N') => Cmd::PrevMatch,
            KeyCode::Named(NamedKey::Escape) if self.is_pending() => {
                self.reset();
                return Outcome::Dropped;
            }
            KeyCode::Named(NamedKey::Escape) => Cmd::Escape,
            _ => {
                self.count = None;
                return Outcome::Pass;
            }
        };
        self.count = None;
        Outcome::Run(cmd)
    }

    fn digit(&mut self, c: char) -> Outcome {
        let d = c.to_digit(10).unwrap_or(0);
        self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(d).min(99_999));
        Outcome::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_vim::TabMove;

    fn feed(keys: &mut Keys, text: &str) -> Vec<Outcome> {
        text.chars().map(|c| keys.key(KeyPress::char(c))).collect()
    }

    fn last(text: &str) -> Outcome {
        *feed(&mut Keys::default(), text).last().unwrap()
    }

    #[test]
    fn counts_repeat_a_motion() {
        assert_eq!(last("j"), Outcome::Run(Cmd::Scroll(1)));
        assert_eq!(last("5j"), Outcome::Run(Cmd::Scroll(5)));
        assert_eq!(last("12k"), Outcome::Run(Cmd::Scroll(-12)));
        assert_eq!(last("3J"), Outcome::Run(Cmd::Page(3)));
        assert_eq!(last("K"), Outcome::Run(Cmd::Page(-1)));
    }

    #[test]
    fn g_and_capital_g_go_to_pages_as_in_vim() {
        assert_eq!(last("gg"), Outcome::Run(Cmd::GotoPage(1)));
        assert_eq!(last("G"), Outcome::Run(Cmd::LastPage));
        assert_eq!(last("38G"), Outcome::Run(Cmd::GotoPage(38)));
        assert_eq!(last("104G"), Outcome::Run(Cmd::GotoPage(104)), "0 continues a count");
        assert_eq!(last("7gg"), Outcome::Run(Cmd::GotoPage(7)));
    }

    #[test]
    fn g_waits_so_the_tab_keys_reach_the_tabs() {
        let mut keys = Keys::default();
        assert_eq!(keys.key(KeyPress::char('g')), Outcome::Pending);
        assert_eq!(keys.key(KeyPress::char('t')), Outcome::Run(Cmd::Tab(TabMove::Next)));
        assert_eq!(last("gT"), Outcome::Run(Cmd::Tab(TabMove::Prev(1))));
        assert_eq!(last("2gT"), Outcome::Run(Cmd::Tab(TabMove::Prev(2))));
        assert_eq!(last("3gt"), Outcome::Run(Cmd::Tab(TabMove::Nth(3))));
        assert_eq!(last("gh"), Outcome::Run(Cmd::Tab(TabMove::Home)));
        let mut keys = Keys::default();
        keys.key(KeyPress::char('g'));
        assert_eq!(keys.key(KeyPress::named(NamedKey::Tab)), Outcome::Run(Cmd::Tab(TabMove::Last)));
        assert_eq!(last("gx"), Outcome::Dropped);
        assert!(!keys.is_pending());
    }

    #[test]
    fn zoom_keys() {
        assert_eq!(last("+"), Outcome::Run(Cmd::ZoomIn));
        assert_eq!(last("-"), Outcome::Run(Cmd::ZoomOut));
        assert_eq!(last("="), Outcome::Run(Cmd::ToggleFit));
        assert_eq!(last("zw"), Outcome::Run(Cmd::FitWidth));
        assert_eq!(last("zp"), Outcome::Run(Cmd::FitPage));
        assert_eq!(last("z0"), Outcome::Run(Cmd::ActualSize));
    }

    #[test]
    fn marks() {
        assert_eq!(last("ma"), Outcome::Run(Cmd::SetMark('a')));
        assert_eq!(last("'a"), Outcome::Run(Cmd::GotoMark('a')));
        assert_eq!(last("`b"), Outcome::Run(Cmd::GotoMark('b')));
        assert_eq!(last("m1"), Outcome::Dropped);
    }

    #[test]
    fn the_old_single_keys_are_gone() {
        for key in ["p", "w", "0"] {
            assert_eq!(last(key), Outcome::Pass, "{key}");
        }
        assert_eq!(last("n"), Outcome::Run(Cmd::NextMatch), "n is the next match now, not the next page");
    }

    #[test]
    fn ctrl_keys_scroll_by_screens_and_the_rest_pass_on() {
        let mut keys = Keys::default();
        assert_eq!(keys.key(KeyPress::char('d').with_ctrl()), Outcome::Run(Cmd::HalfScreen(1)));
        assert_eq!(keys.key(KeyPress::char('b').with_ctrl()), Outcome::Run(Cmd::Screen(-1)));
        feed(&mut keys, "2");
        assert_eq!(keys.key(KeyPress::char('f').with_ctrl()), Outcome::Run(Cmd::Screen(2)));
        assert_eq!(keys.key(KeyPress::char('w').with_ctrl()), Outcome::Pass);
        assert_eq!(keys.key(KeyPress::char('o').with_ctrl()), Outcome::Pass);
        assert_eq!(keys.key(KeyPress::named(NamedKey::PageDown)), Outcome::Run(Cmd::Screen(1)));
    }

    #[test]
    fn keys_that_arent_the_readers_pass_on_and_clear_a_count() {
        let mut keys = Keys::default();
        feed(&mut keys, "4");
        assert_eq!(keys.key(KeyPress::char(':')), Outcome::Pass);
        assert!(!keys.is_pending());
        assert_eq!(keys.key(KeyPress::char(' ')), Outcome::Pass);
        feed(&mut keys, "4");
        assert_eq!(keys.key(KeyPress::named(NamedKey::Escape)), Outcome::Dropped, "Esc clears a count");
        assert_eq!(keys.key(KeyPress::named(NamedKey::Escape)), Outcome::Run(Cmd::Escape));
    }

    #[test]
    fn zoom_steps_go_to_the_next_step_and_stop_at_the_ends() {
        assert_eq!(step_zoom(100, true), 110);
        assert_eq!(step_zoom(97, true), 100, "a fitted 97% goes to the next real step");
        assert_eq!(step_zoom(97, false), 90);
        assert_eq!(step_zoom(400, true), 400);
        assert_eq!(step_zoom(25, false), 25);
        assert_eq!(step_zoom(1000, false), 400);
    }
    fn letter(n: usize) -> Vec<(f32, f32)> {
        vec![(612.0, 792.0); n]
    }

    #[test]
    fn fit_width_stacks_the_pages_one_under_the_other() {
        let l = layout(&letter(3), Zoom::FitWidth, (636.0, 400.0), 1.0);
        assert_eq!(l.scale, 1.0);
        assert_eq!(l.pages[0], PageRect { x: 12.0, y: 12.0, w: 612.0, h: 792.0 });
        assert_eq!(l.pages[1].y, 12.0 + 792.0 + 12.0);
        assert_eq!(l.height, 12.0 + 3.0 * (792.0 + 12.0));
        assert_eq!(l.width, 636.0);
    }

    #[test]
    fn a_narrower_page_is_centred_at_the_same_scale() {
        let l = layout(&[(612.0, 792.0), (306.0, 396.0)], Zoom::FitWidth, (636.0, 400.0), 1.0);
        assert_eq!((l.pages[1].w, l.pages[1].x), (306.0, 165.0));
    }

    #[test]
    fn fit_page_fits_the_tallest_page_and_a_percent_ignores_the_pane() {
        let l = layout(&letter(2), Zoom::FitPage, (1000.0, 420.0), 1.0);
        assert_eq!(l.pages[0].h, 396.0);
        let l = layout(&letter(2), Zoom::Percent(200), (100.0, 100.0), 1.5);
        assert_eq!(l.pages[0].w, 1836.0);
        assert!(l.width > 100.0, "wider than the pane, so it pans");
    }

    #[test]
    fn what_shows_and_what_is_being_read() {
        let l = layout(&letter(5), Zoom::FitWidth, (636.0, 400.0), 1.0);
        assert_eq!(l.visible(0.0, 400.0), 0..1);
        assert_eq!(l.visible(700.0, 400.0), 0..2);
        assert_eq!(l.current(0.0, (636.0, 400.0)), 0);
        assert_eq!(l.current(700.0, (636.0, 400.0)), 1, "page 2 crosses the middle");
        let end = l.max_scroll((636.0, 400.0)).1;
        assert_eq!(l.current(end, (636.0, 400.0)), 4, "the last page at the end");
        assert_eq!(l.top_of(2), l.pages[2].y - 12.0);
    }

    #[test]
    fn an_anchor_finds_the_same_place_at_another_zoom() {
        let small = layout(&letter(5), Zoom::FitWidth, (336.0, 400.0), 1.0);
        let big = layout(&letter(5), Zoom::FitWidth, (1236.0, 400.0), 1.0);
        let y = small.pages[3].y + small.pages[3].h / 4.0;
        let anchor = small.anchor(y);
        assert_eq!(anchor.0, 3);
        assert!((anchor.1 - 0.25).abs() < 0.01);
        let again = big.scroll_for(anchor);
        assert_eq!(big.page_at(again), 3);
    }
}
