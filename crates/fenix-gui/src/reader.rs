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

/// Turns a zoom into the pixel size to render a page at, given the page's
/// size in points, the pane's size in pixels and the window's scale
/// factor. `(0, 0)` while the page's size or the pane's isn't known yet,
/// which callers read as "nothing to render".
pub fn target_size(zoom: Zoom, page_pts: (f32, f32), pane_px: (u32, u32), scale: f32) -> (u32, u32) {
    if page_pts.0 <= 0.0 || page_pts.1 <= 0.0 || pane_px.0 == 0 || pane_px.1 == 0 {
        return (0, 0);
    }
    match zoom {
        Zoom::FitPage => fenix_pdf::coords::fit_page_size(page_pts.0, page_pts.1, pane_px.0, pane_px.1),
        Zoom::FitWidth => fenix_pdf::coords::fit_width_size(page_pts.0, page_pts.1, pane_px.0),
        Zoom::Percent(percent) => {
            let percent = ((percent as f32) * scale.max(0.1)).round().max(1.0) as u32;
            fenix_pdf::coords::percent_size(page_pts.0, page_pts.1, percent)
        }
    }
}

/// The percentage a render of `rendered_w` pixels is at, for a page
/// `page_w_pts` wide on a screen of scale factor `scale` -- where `+` and
/// `-` step from when the view is fitted. 100 when nothing's rendered.
pub fn effective_percent(rendered_w: u32, page_w_pts: f32, scale: f32) -> u32 {
    if rendered_w == 0 || page_w_pts <= 0.0 {
        return 100;
    }
    ((rendered_w as f32 / page_w_pts / scale.max(0.1)) * 100.0).round().max(1.0) as u32
}

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
    /// `n`/`N`: the next or previous page with a match of the last search.
    NextMatch,
    PrevMatch,
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
            KeyCode::Char(c @ ('g' | 'z')) => {
                self.prefix = Some(c);
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
        assert_eq!(keys.key(KeyPress::named(NamedKey::Escape)), Outcome::Dropped);
        assert_eq!(keys.key(KeyPress::named(NamedKey::Escape)), Outcome::Pass, "Esc with nothing pending isn't the reader's");
    }

    #[test]
    fn target_size_follows_the_zoom() {
        assert_eq!(target_size(Zoom::FitPage, (612.0, 792.0), (612, 792), 1.0), (612, 792));
        assert_eq!(target_size(Zoom::FitWidth, (612.0, 792.0), (306, 200), 1.0), (306, 396));
        assert_eq!(target_size(Zoom::Percent(100), (612.0, 792.0), (10, 10), 1.0), (612, 792), "percent ignores the pane");
        assert_eq!(target_size(Zoom::FitPage, (0.0, 0.0), (612, 792), 1.0), (0, 0), "no page size yet");
        assert_eq!(target_size(Zoom::FitPage, (612.0, 792.0), (0, 792), 1.0), (0, 0), "no pane yet");
    }

    #[test]
    fn a_percent_is_of_the_pages_real_size_on_a_scaled_screen() {
        assert_eq!(target_size(Zoom::Percent(100), (612.0, 792.0), (1, 1), 1.5), (918, 1188));
        assert_eq!(effective_percent(918, 612.0, 1.5), 100);
        assert_eq!(effective_percent(612, 612.0, 1.0), 100);
        assert_eq!(effective_percent(1224, 612.0, 1.0), 200);
        assert_eq!(effective_percent(0, 612.0, 1.0), 100, "nothing rendered yet");
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
}
